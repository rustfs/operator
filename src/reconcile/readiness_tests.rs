// Copyright 2026 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::types::v1alpha1::tenant::ReadinessProbeConfig;
use futures::{FutureExt, StreamExt, TryStreamExt};
use http::{Method, Request, Response, StatusCode};
use k8s_openapi::api::core::v1::{Container, Namespace, Probe};
use kube::api::{DeleteParams, Patch, PatchParams, PostParams, WatchEvent, WatchParams};
use kube::config::KubeConfigOptions;
use kube::{Api, Client, Config, client::Body};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::service_fn;

#[derive(Debug)]
struct RecordedRequest {
    method: Method,
    path: String,
    query: String,
    body: Value,
}

fn response(status: StatusCode, body: &Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(
            serde_json::to_vec(body).expect("response should serialize"),
        ))
        .expect("response should build")
}

fn mock_context(
    responses: Vec<(StatusCode, Value)>,
) -> (Context, Arc<Mutex<Vec<RecordedRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let service = service_fn({
        let requests = Arc::clone(&requests);
        let responses = Arc::new(responses);
        move |request: Request<Body>| {
            let requests = Arc::clone(&requests);
            let responses = Arc::clone(&responses);
            async move {
                let method = request.method().clone();
                let path = request.uri().path().to_string();
                let query = request.uri().query().unwrap_or_default().to_string();
                let body = serde_json::from_slice(
                    &request
                        .into_body()
                        .collect_bytes()
                        .await
                        .expect("request body should read"),
                )
                .expect("request body should be JSON");
                let mut requests = requests.lock().expect("request lock should be available");
                let index = requests.len();
                requests.push(RecordedRequest {
                    method,
                    path,
                    query,
                    body,
                });
                let (status, body) = responses.get(index).expect("unexpected API request");
                Ok::<_, Infallible>(response(*status, body))
            }
        }
    });
    (Context::new(Client::new(service, "default")), requests)
}

fn fixture() -> (Tenant, StatefulSet, StatefulSet) {
    let mut tenant = crate::tests::create_test_tenant(None, None);
    let mut existing = tenant
        .new_statefulset(&tenant.spec.pools[0])
        .expect("StatefulSet should render");
    existing.metadata.uid = Some("statefulset-uid".to_string());
    existing.metadata.resource_version = Some("41".to_string());
    tenant.spec.readiness_probe = Some(ReadinessProbeConfig {
        initial_delay_seconds: Some(2),
        timeout_seconds: Some(8),
        period_seconds: Some(7),
        failure_threshold: Some(4),
        success_threshold: Some(2),
    });
    let desired = tenant
        .new_statefulset(&tenant.spec.pools[0])
        .expect("StatefulSet should render");
    (tenant, existing, desired)
}

fn rustfs_container(statefulset: &StatefulSet) -> &Container {
    statefulset
        .spec
        .as_ref()
        .expect("StatefulSet should have spec")
        .template
        .spec
        .as_ref()
        .expect("Pod template should have spec")
        .containers
        .iter()
        .find(|container| container.name == "rustfs")
        .expect("RustFS container should exist")
}

fn rustfs_container_mut(statefulset: &mut StatefulSet) -> &mut Container {
    statefulset
        .spec
        .as_mut()
        .expect("StatefulSet should have spec")
        .template
        .spec
        .as_mut()
        .expect("Pod template should have spec")
        .containers
        .iter_mut()
        .find(|container| container.name == "rustfs")
        .expect("RustFS container should exist")
}

fn readiness_probe(statefulset: &StatefulSet) -> &Probe {
    rustfs_container(statefulset)
        .readiness_probe
        .as_ref()
        .expect("readiness probe should exist")
}

fn successful_response(
    mut statefulset: StatefulSet,
    resource_version: &str,
) -> (StatusCode, Value) {
    statefulset.metadata.resource_version = Some(resource_version.to_string());
    (
        StatusCode::OK,
        serde_json::to_value(statefulset).expect("StatefulSet should serialize"),
    )
}

fn api_failure(code: u16) -> (StatusCode, Value) {
    (
        StatusCode::from_u16(code).expect("status code should be valid"),
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Failure",
            "reason": if code == 409 { "Conflict" } else { "InternalError" },
            "message": "injected apply failure",
            "code": code,
        }),
    )
}

fn field_manager_conflict(field: &str, manager: &str) -> Value {
    json!({
        "reason": "FieldManagerConflict",
        "message": format!("conflict with \"{manager}\""),
        "field": field,
    })
}

fn conflict_response(causes: Vec<Value>) -> (StatusCode, Value) {
    let (status, mut body) = api_failure(409);
    body["details"] = json!({"causes": causes});
    (status, body)
}

fn timing_conflict(field: &str) -> Value {
    field_manager_conflict(
        &format!(".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.{field}"),
        "external-readiness-manager",
    )
}

fn expected_apply(existing: &StatefulSet, desired: &StatefulSet) -> Value {
    let mut expected = desired.clone();
    expected.metadata.uid = existing.metadata.uid.clone();
    expected.metadata.resource_version = existing.metadata.resource_version.clone();
    serde_json::to_value(expected).expect("desired StatefulSet should serialize")
}

#[tokio::test]
async fn ordinary_apply_submits_the_complete_template_once_without_force() {
    let (tenant, existing, mut desired) = fixture();
    let container = rustfs_container_mut(&mut desired);
    container.image = Some("example.invalid/rustfs:updated".to_string());
    container
        .env
        .get_or_insert_with(Vec::new)
        .push(k8s_openapi::api::core::v1::EnvVar {
            name: "READINESS_TEST_ENV".to_string(),
            value: Some("updated".to_string()),
            ..Default::default()
        });
    let expected = expected_apply(&existing, &desired);
    let (ctx, requests) = mock_context(vec![successful_response(desired.clone(), "42")]);

    let result = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
        .await
        .expect("ordinary apply should succeed");
    assert_eq!(result.metadata.resource_version.as_deref(), Some("42"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::PATCH);
    assert_eq!(
        requests[0].path,
        "/apis/apps/v1/namespaces/default/statefulsets/test-tenant-pool-0"
    );
    assert!(
        requests[0]
            .query
            .split('&')
            .any(|parameter| parameter == "fieldManager=rustfs-operator")
    );
    assert!(!requests[0].query.contains("force=true"));
    assert_eq!(requests[0].body, expected);
}

#[tokio::test]
async fn timing_conflicts_retry_the_identical_complete_template_with_force() {
    let fields = [
        "initialDelaySeconds",
        "timeoutSeconds",
        "periodSeconds",
        "failureThreshold",
        "successThreshold",
    ];
    let mut conflicts: Vec<Vec<Value>> = fields
        .iter()
        .map(|field| vec![timing_conflict(field)])
        .collect();
    conflicts.push(
        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                field_manager_conflict(
                    &format!(
                        ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.{field}"
                    ),
                    &format!("external-manager-{index}"),
                )
            })
            .collect(),
    );
    for causes in conflicts {
        let (tenant, existing, mut desired) = fixture();
        rustfs_container_mut(&mut desired).image =
            Some("example.invalid/rustfs:updated".to_string());
        let expected = expected_apply(&existing, &desired);
        let (ctx, requests) = mock_context(vec![
            conflict_response(causes),
            successful_response(desired.clone(), "42"),
        ]);
        apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect("timing-only conflicts should permit one force retry");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(request.method, Method::PATCH);
            assert!(
                request
                    .query
                    .split('&')
                    .any(|parameter| parameter == "fieldManager=rustfs-operator")
            );
            assert_eq!(request.body, expected);
            assert_eq!(request.body["metadata"]["uid"], "statefulset-uid");
            assert_eq!(request.body["metadata"]["resourceVersion"], "41");
        }
        assert_eq!(requests[0].path, requests[1].path);
        assert!(!requests[0].query.contains("force=true"));
        assert!(requests[1].query.contains("force=true"));
    }
}

#[tokio::test]
async fn conflicts_outside_readiness_timing_never_force_or_partially_update() {
    let unrelated_fields = [
        ".spec.template.spec.containers[name=\"rustfs\"].image",
        ".spec.template.spec.containers[name=\"rustfs\"].env[name=\"TEST\"].value",
        ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.httpGet.path",
        ".spec.template.spec.containers[name=\"sidecar\"].readinessProbe.timeoutSeconds",
        ".spec.template.spec.containers[name=\"rustfs\"].livenessProbe.timeoutSeconds",
        ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.timeoutSeconds.extra",
        "spec.template.spec.containers[name=\"rustfs\"].readinessProbe.timeoutSeconds",
    ];
    for field in unrelated_fields {
        let (tenant, existing, desired) = fixture();
        let (ctx, requests) = mock_context(vec![conflict_response(vec![
            timing_conflict("timeoutSeconds"),
            field_manager_conflict(field, "external-owner"),
        ])]);
        apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("a mixed conflict must preserve all existing fields");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "unexpected retry for {field}");
        assert!(!requests[0].query.contains("force=true"));
    }
}

#[tokio::test]
async fn missing_or_invalid_conflict_details_never_enable_force() {
    let valid_cause = timing_conflict("timeoutSeconds");
    let mut responses = vec![api_failure(409), api_failure(500)];
    for details in [
        Value::Null,
        json!({}),
        json!({"causes": []}),
        json!({"causes": null}),
        json!({"causes": "invalid"}),
        json!({"causes": [null]}),
        json!({"causes": [{"reason": "FieldManagerConflict"}]}),
        json!({"causes": [{"field": valid_cause["field"]}]}),
        json!({"causes": [{"reason": "FieldValueInvalid", "field": valid_cause["field"]}]}),
        json!({"causes": [{"reason": "FieldManagerConflict", "field": 42}]}),
    ] {
        let (status, mut body) = api_failure(409);
        body["details"] = details;
        responses.push((status, body));
    }
    let (status, mut wrong_code) = conflict_response(vec![valid_cause.clone()]);
    wrong_code["code"] = json!(500);
    responses.push((status, wrong_code));
    let (status, mut wrong_reason) = conflict_response(vec![valid_cause.clone()]);
    wrong_reason["reason"] = json!("Invalid");
    responses.push((status, wrong_reason));
    let (_, valid_body) = conflict_response(vec![valid_cause]);
    responses.push((StatusCode::INTERNAL_SERVER_ERROR, valid_body));

    for failure in responses {
        let (tenant, existing, desired) = fixture();
        let (ctx, requests) = mock_context(vec![failure]);
        apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("only complete structured timing conflicts may enable force");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].query.contains("force=true"));
    }
}

#[tokio::test]
async fn invalid_ownership_or_identity_prevents_all_apply_requests() {
    for invalid in ["owner", "tenant_uid", "uid", "resource_version"] {
        let (mut tenant, mut existing, desired) = fixture();
        match invalid {
            "owner" => {
                existing.metadata.owner_references.as_mut().unwrap()[0].uid =
                    "another-tenant".to_string()
            }
            "tenant_uid" => tenant.metadata.uid = None,
            "uid" => existing.metadata.uid = None,
            "resource_version" => existing.metadata.resource_version = None,
            _ => unreachable!(),
        }
        let (ctx, requests) = mock_context(Vec::new());
        let error = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("invalid resource identity must stop the update");
        assert!(
            matches!(error, Error::Types { .. }),
            "unexpected error for {invalid}: {error}"
        );
        assert!(
            requests.lock().unwrap().is_empty(),
            "unexpected API write for {invalid}"
        );
    }
}

#[tokio::test]
async fn force_retry_errors_are_propagated_without_further_requests() {
    for code in [409, 500] {
        let (tenant, existing, desired) = fixture();
        let (ctx, requests) = mock_context(vec![
            conflict_response(vec![timing_conflict("timeoutSeconds")]),
            api_failure(code),
        ]);
        let error = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("retry errors must propagate to reconciliation");
        assert!(
            matches!(error, Error::Kube { source: kube::Error::Api(ref response) } if response.code == code)
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, requests[1].body);
        assert_eq!(requests[1].body["metadata"]["resourceVersion"], "41");
    }
}

#[tokio::test]
async fn missing_existing_probe_is_restored_with_handler_and_timing_in_one_apply() {
    let (tenant, mut existing, desired) = fixture();
    rustfs_container_mut(&mut existing).readiness_probe = None;
    let (ctx, requests) = mock_context(vec![successful_response(desired.clone(), "42")]);
    apply_statefulset(&ctx, &tenant, "default", &existing, desired)
        .await
        .expect("ordinary apply should restore the complete probe");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .query
            .split('&')
            .any(|parameter| parameter == "fieldManager=rustfs-operator")
    );
    assert!(!requests[0].query.contains("force=true"));
    assert_eq!(requests[0].body["metadata"]["uid"], "statefulset-uid");
    assert_eq!(requests[0].body["metadata"]["resourceVersion"], "41");
    let probe = &requests[0].body["spec"]["template"]["spec"]["containers"][0]["readinessProbe"];
    assert_eq!(probe["httpGet"]["path"], "/health/ready");
    assert_eq!(probe["timeoutSeconds"], 8);
}

fn live_desired(tenant: &Tenant) -> StatefulSet {
    let mut desired = tenant
        .new_statefulset(&tenant.spec.pools[0])
        .expect("StatefulSet should render");
    desired.spec.as_mut().unwrap().replicas = Some(0);
    desired
}

fn managed_container_fields<'a>(statefulset: &'a StatefulSet, manager: &str) -> Option<&'a Value> {
    statefulset
        .metadata
        .managed_fields
        .as_ref()?
        .iter()
        .find(|entry| entry.manager.as_deref() == Some(manager))?
        .fields_v1
        .as_ref()?
        .0
        .get("f:spec")?
        .get("f:template")?
        .get("f:spec")?
        .get("f:containers")?
        .get("k:{\"name\":\"rustfs\"}")
}

async fn live_apply_with_status_retries(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    api: &Api<StatefulSet>,
    existing: &StatefulSet,
) -> Result<StatefulSet, Error> {
    let mut current = existing.clone();
    let mut retries = 0;
    loop {
        match apply_statefulset(ctx, tenant, namespace, &current, live_desired(tenant)).await {
            Ok(updated) => return Ok(updated),
            Err(Error::Kube {
                source: kube::Error::Api(response),
            }) if response.code == 409
                && response.message.contains("the object has been modified")
                && retries < 5 =>
            {
                // The StatefulSet controller can update status between applies.
                // Reconciliation retries from a fresh observation in this case.
                retries += 1;
                current = api
                    .get(&existing.name_any())
                    .await
                    .expect("StatefulSet should refresh after a concurrent status update");
            }
            Err(error) => return Err(error),
        }
    }
}

async fn live_apply_and_check_events(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    api: &Api<StatefulSet>,
    existing: &StatefulSet,
    expected_timeout: i32,
) -> StatefulSet {
    let resource_version = existing.metadata.resource_version.as_deref().unwrap();
    let mut events = api
        .watch(
            &WatchParams::default()
                .fields(&format!("metadata.name={}", existing.name_any()))
                .timeout(10),
            resource_version,
        )
        .await
        .expect("StatefulSet watch should start")
        .boxed();
    let updated = live_apply_with_status_retries(ctx, tenant, namespace, api, existing)
        .await
        .expect("readiness timing update should succeed");
    let expected_version = updated.metadata.resource_version.as_deref().unwrap();
    let before_strategy =
        serde_json::to_value(&existing.spec.as_ref().unwrap().update_strategy).unwrap();
    let before_template = serde_json::to_value(&existing.spec.as_ref().unwrap().template).unwrap();
    let expected_template = serde_json::to_value(&updated.spec.as_ref().unwrap().template).unwrap();
    if expected_version != resource_version {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut target_observed = false;
            while let Some(event) = events.try_next().await.expect("watch event should read") {
                if let WatchEvent::Added(observed) | WatchEvent::Modified(observed) = event {
                    let template =
                        serde_json::to_value(&observed.spec.as_ref().unwrap().template).unwrap();
                    target_observed |= template == expected_template;
                    assert!(
                        template == expected_template
                            || (!target_observed && template == before_template),
                        "stored revisions must contain the complete old or requested template"
                    );
                    assert_eq!(
                        serde_json::to_value(&observed.spec.as_ref().unwrap().update_strategy)
                            .unwrap(),
                        before_strategy
                    );
                    if observed.metadata.resource_version.as_deref() == Some(expected_version) {
                        return;
                    }
                }
            }
            panic!("watch ended before the final apply revision");
        })
        .await
        .expect("the complete applied template should be observed");
    }
    assert_eq!(
        readiness_probe(&updated).timeout_seconds,
        Some(expected_timeout)
    );
    let timing_fields = managed_container_fields(&updated, "rustfs-operator")
        .and_then(|fields| fields.get("f:readinessProbe"))
        .expect("the Operator should own readiness timing");
    for field in [
        "f:initialDelaySeconds",
        "f:timeoutSeconds",
        "f:periodSeconds",
        "f:failureThreshold",
        "f:successThreshold",
    ] {
        assert!(
            timing_fields.get(field).is_some(),
            "the Operator must own {field}"
        );
    }
    assert!(timing_fields.get("f:httpGet").is_some());
    let http = readiness_probe(&updated).http_get.as_ref().unwrap();
    assert_eq!(http.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(
        http.http_headers.as_ref().unwrap()[0].name,
        "X-Probe-Source"
    );
    assert_eq!(
        http.http_headers.as_ref().unwrap()[0].value,
        "external-manager"
    );
    let mut observed = tenant
        .new_statefulset(&tenant.spec.pools[0])
        .expect("StatefulSet should render for comparison");
    // Keep unrelated API defaults out of the readiness convergence check.
    rustfs_container_mut(&mut observed).readiness_probe =
        rustfs_container(&updated).readiness_probe.clone();
    assert!(
        !tenant
            .statefulset_needs_update(&observed, &tenant.spec.pools[0])
            .unwrap(),
        "external probe headers and host must not cause repeated reconciliation writes"
    );
    updated
}

async fn run_live_scenario(client: Client, namespace: &str) {
    let tenants: Api<Tenant> = Api::namespaced(client.clone(), namespace);
    let statefulsets: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let ctx = Context::new(client.clone());
    let mut tenant =
        crate::tests::create_test_tenant(Some("external-workload-identity".to_string()), None);
    tenant.metadata.namespace = Some(namespace.to_string());
    tenant.metadata.uid = None;
    tenant = tenants
        .create(&PostParams::default(), &tenant)
        .await
        .expect("test Tenant should create");

    let mut legacy = live_desired(&tenant);
    let legacy_probe = rustfs_container_mut(&mut legacy)
        .readiness_probe
        .as_mut()
        .unwrap();
    legacy_probe.initial_delay_seconds = None;
    legacy_probe.timeout_seconds = None;
    legacy_probe.period_seconds = None;
    legacy_probe.failure_threshold = None;
    legacy_probe.success_threshold = None;
    let name = legacy.name_any();
    let mut existing = statefulsets
        .patch(
            &name,
            &PatchParams::apply("rustfs-operator"),
            &Patch::Apply(&legacy),
        )
        .await
        .expect("legacy StatefulSet should create through SSA");
    assert_eq!(readiness_probe(&existing).timeout_seconds, Some(1));
    let manual = json!({"spec": {"template": {"spec": {"containers": [{
        "name": "rustfs", "readinessProbe": {
            "timeoutSeconds": 5,
            "httpGet": {
                "host": "127.0.0.1",
                "httpHeaders": [{"name": "X-Probe-Source", "value": "external-manager"}],
            },
        },
    }]}}}});
    existing = statefulsets
        .patch(
            &name,
            &PatchParams {
                field_manager: Some("kubectl-patch".to_string()),
                ..Default::default()
            },
            &Patch::Strategic(&manual),
        )
        .await
        .expect("manual timeout workaround should apply");
    assert_eq!(readiness_probe(&existing).timeout_seconds, Some(5));

    let on_delete = json!({"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name, "namespace": namespace},
        "spec": {"updateStrategy": {"type": "OnDelete", "rollingUpdate": null}}});
    existing = statefulsets
        .patch(
            &name,
            &PatchParams {
                field_manager: Some("external-rollout-policy".to_string()),
                ..Default::default()
            },
            &Patch::Merge(&on_delete),
        )
        .await
        .expect("external OnDelete strategy should apply");

    for timeout in [Some(5), Some(8), None] {
        let config = timeout.map(|value| json!({"timeoutSeconds": value}));
        tenant = tenants
            .patch(
                &tenant.name(),
                &PatchParams::default(),
                &Patch::Merge(json!({"spec": {"readinessProbe": config}})),
            )
            .await
            .expect("Tenant readiness configuration should persist");
        assert_eq!(
            tenant
                .spec
                .readiness_probe
                .as_ref()
                .and_then(|probe| probe.timeout_seconds),
            timeout
        );
        existing = live_apply_and_check_events(
            &ctx,
            &tenant,
            namespace,
            &statefulsets,
            &existing,
            timeout.unwrap_or(5),
        )
        .await;
        if timeout == Some(5) {
            let shared = json!({"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name, "namespace": namespace},
                "spec": {"template": {"spec": {"containers": [{"name": "rustfs", "readinessProbe": {"timeoutSeconds": 5}}]}}}});
            existing = statefulsets
                .patch(
                    &name,
                    &PatchParams::apply("external-shared-readiness"),
                    &Patch::Apply(&shared),
                )
                .await
                .expect("equal readiness values should permit shared ownership");
            assert!(managed_container_fields(&existing, "external-shared-readiness").unwrap()["f:readinessProbe"].get("f:timeoutSeconds").is_some());
        }
    }

    let partitioned = json!({"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name, "namespace": namespace},
        "spec": {"updateStrategy": {"type": "RollingUpdate", "rollingUpdate": {"partition": 3}}}});
    existing = statefulsets
        .patch(
            &name,
            &PatchParams::apply("external-rollout-policy").force(),
            &Patch::Apply(&partitioned),
        )
        .await
        .expect("external rolling partition should apply");
    existing =
        live_apply_and_check_events(&ctx, &tenant, namespace, &statefulsets, &existing, 5).await;

    tenant.spec.env.push(k8s_openapi::api::core::v1::EnvVar {
        name: "READINESS_TEST_ENV".to_string(),
        value: Some("updated".to_string()),
        ..Default::default()
    });
    tenant.spec.readiness_probe = Some(ReadinessProbeConfig {
        timeout_seconds: Some(8),
        ..Default::default()
    });
    let updated =
        live_apply_and_check_events(&ctx, &tenant, namespace, &statefulsets, &existing, 8).await;
    assert!(
        rustfs_container(&updated)
            .env
            .as_ref()
            .unwrap()
            .iter()
            .any(|variable| variable.name == "READINESS_TEST_ENV"
                && variable.value.as_deref() == Some("updated"))
    );

    let external_image = "example.invalid/external-image:1";
    let image_patch = json!({"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name, "namespace": namespace},
        "spec": {"template": {"spec": {"containers": [{"name": "rustfs", "image": external_image}]}}}});
    existing = statefulsets
        .patch(
            &name,
            &PatchParams::apply("external-image-owner").force(),
            &Patch::Apply(&image_patch),
        )
        .await
        .expect("external manager should own the image");
    tenant.spec.readiness_probe = Some(ReadinessProbeConfig {
        timeout_seconds: Some(9),
        ..Default::default()
    });
    let error = live_apply_with_status_retries(&ctx, &tenant, namespace, &statefulsets, &existing)
        .await
        .expect_err("ordinary apply must preserve conflicts on unrelated fields");
    assert!(
        matches!(error, Error::Kube { source: kube::Error::Api(ref response) }
            if response.code == 409 && response.message.contains("external-image-owner")
                && response.message.contains(".image")),
        "expected an image ownership conflict, got {error}"
    );
    let preserved = statefulsets
        .get(&name)
        .await
        .expect("StatefulSet should remain readable");
    assert_eq!(
        rustfs_container(&preserved).image.as_deref(),
        Some(external_image)
    );
    assert!(
        managed_container_fields(&preserved, "external-image-owner")
            .unwrap()
            .get("f:image")
            .is_some()
    );
    assert_eq!(
        serde_json::to_value(&preserved.spec).unwrap(),
        serde_json::to_value(&existing.spec).unwrap(),
        "an unrelated ownership conflict must leave the entire StatefulSet spec unchanged"
    );
    assert_eq!(readiness_probe(&preserved).timeout_seconds, Some(8));
    assert_eq!(preserved.spec.as_ref().unwrap().replicas, Some(0));

    for replace_existing in [false, true] {
        live_force_retry_preserves_concurrent_change(
            client.clone(),
            &tenant,
            namespace,
            replace_existing,
        )
        .await;
    }
}

async fn live_force_retry_preserves_concurrent_change(
    client: Client,
    tenant: &Tenant,
    namespace: &str,
    replace_existing: bool,
) {
    let api: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let mut desired = live_desired(tenant);
    let suffix = if replace_existing {
        "replacement"
    } else {
        "update"
    };
    let name = format!("{}-{suffix}", desired.name_any());
    desired.metadata.name = Some(name.clone());
    api.patch(
        &name,
        &PatchParams::apply("rustfs-operator"),
        &Patch::Apply(&desired),
    )
    .await
    .expect("concurrency test StatefulSet should create");
    let timing = json!({
        "apiVersion": "apps/v1", "kind": "StatefulSet",
        "metadata": {"name": name, "namespace": namespace},
        "spec": {"template": {"spec": {"containers": [{
            "name": "rustfs", "readinessProbe": {"timeoutSeconds": 1}
        }]}}}
    });
    api.patch(
        &name,
        &PatchParams::apply("external-readiness").force(),
        &Patch::Apply(&timing),
    )
    .await
    .expect("external timing ownership should create a real SSA conflict");
    let existing = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = api
                .get(&name)
                .await
                .expect("StatefulSet should remain readable");
            if current
                .status
                .as_ref()
                .and_then(|status| status.observed_generation)
                == current.metadata.generation
            {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("zero-replica StatefulSet status should converge before the race");

    let request_count = Arc::new(AtomicUsize::new(0));
    let changed = Arc::new(Mutex::new(None::<StatefulSet>));
    let mut replacement = desired.clone();
    rustfs_container_mut(&mut replacement).image =
        Some("example.invalid/concurrent-image:1".to_string());
    rustfs_container_mut(&mut replacement)
        .readiness_probe
        .as_mut()
        .unwrap()
        .timeout_seconds = Some(1);
    let service = service_fn({
        let request_count = Arc::clone(&request_count);
        let changed = Arc::clone(&changed);
        let name = name.clone();
        let api = api.clone();
        move |request: Request<Body>| {
            let request_count = Arc::clone(&request_count);
            let changed = Arc::clone(&changed);
            let name = name.clone();
            let api = api.clone();
            let client = client.clone();
            let replacement = replacement.clone();
            async move {
                let index = request_count.fetch_add(1, Ordering::SeqCst);
                assert!(
                    index < 2,
                    "only the ordinary apply and one force retry are allowed"
                );
                let force = request
                    .uri()
                    .query()
                    .unwrap_or_default()
                    .split('&')
                    .any(|parameter| parameter == "force=true");
                assert_eq!(force, index == 1);
                let response = client.send(request).await?;
                if index == 0 {
                    assert_eq!(response.status(), StatusCode::CONFLICT);
                    let updated = if replace_existing {
                        api.delete(&name, &DeleteParams::default()).await?;
                        tokio::time::timeout(Duration::from_secs(15), async {
                            loop {
                                match api.get(&name).await {
                                    Err(kube::Error::Api(error)) if error.code == 404 => break,
                                    Ok(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                                    Err(error) => panic!("replacement wait failed: {error}"),
                                }
                            }
                        })
                        .await
                        .expect("zero-replica StatefulSet should delete");
                        api.create(&PostParams::default(), &replacement).await?
                    } else {
                        let patch = json!({
                            "metadata": {"annotations": {"concurrent-update": "retained"}},
                            "spec": {"template": {"spec": {"containers": [{
                                "name": "rustfs", "image": "example.invalid/concurrent-image:1"
                            }]}}}
                        });
                        api.patch(&name, &PatchParams::default(), &Patch::Strategic(&patch))
                            .await?
                    };
                    *changed
                        .lock()
                        .expect("concurrent result should be available") = Some(updated);
                }
                Ok::<_, kube::Error>(response)
            }
        }
    });
    let ctx = Context::new(Client::new(service, namespace));
    let error = apply_statefulset(&ctx, tenant, namespace, &existing, desired)
        .await
        .expect_err("force retry must reject a concurrent update or replacement");
    assert!(
        matches!(error, Error::Kube { source: kube::Error::Api(ref response) }
            if matches!(response.code, 409 | 422)),
        "expected a resource identity or version conflict, got {error}"
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 2);
    let changed = changed
        .lock()
        .unwrap()
        .take()
        .expect("the race should have run");
    let preserved = api
        .get(&name)
        .await
        .expect("concurrent resource should remain readable");
    assert_eq!(
        serde_json::to_value(&preserved.spec).unwrap(),
        serde_json::to_value(&changed.spec).unwrap(),
        "the rejected retry must preserve the entire concurrent spec"
    );
    assert_eq!(preserved.metadata.annotations, changed.metadata.annotations);
    assert_eq!(preserved.metadata.uid, changed.metadata.uid);
    assert_eq!(
        preserved.metadata.uid != existing.metadata.uid,
        replace_existing
    );
}

#[tokio::test]
#[ignore = "requires RUSTFS_READINESS_TEST_CONTEXT and the Tenant CRD; run explicitly"]
async fn readiness_ssa_live_preserves_values_ownership_and_external_fields() {
    crate::install_rustls_crypto_provider();
    let context = std::env::var("RUSTFS_READINESS_TEST_CONTEXT")
        .expect("set RUSTFS_READINESS_TEST_CONTEXT to a disposable Kubernetes context");
    let mut config = Config::from_kubeconfig(&KubeConfigOptions {
        context: Some(context),
        ..Default::default()
    })
    .await
    .expect("dedicated readiness test context should load");
    config.proxy_url = None;
    let client = Client::try_from(config).expect("Kubernetes client should initialize");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let namespace = namespaces
        .create(
            &PostParams::default(),
            &Namespace {
                metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                    generate_name: Some("readiness-ssa-".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("isolated test namespace should create")
        .name_any();
    let result = AssertUnwindSafe(run_live_scenario(client, &namespace))
        .catch_unwind()
        .await;
    let cleanup = namespaces
        .delete(&namespace, &DeleteParams::default())
        .await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    cleanup.expect("isolated test namespace should delete");
}
