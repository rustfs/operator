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
use serde_json::Value;
use std::convert::Infallible;
use std::panic::AssertUnwindSafe;
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

#[tokio::test]
async fn timing_apply_is_scoped_and_full_apply_releases_timing_ownership() {
    let (tenant, mut existing, desired) = fixture();
    existing
        .spec
        .as_mut()
        .unwrap()
        .template
        .spec
        .as_mut()
        .unwrap()
        .containers
        .insert(
            0,
            Container {
                name: "sidecar".to_string(),
                image: Some("example.invalid/sidecar:1".to_string()),
                ..Default::default()
            },
        );
    let (ctx, requests) = mock_context(vec![
        successful_response(desired.clone(), "42"),
        successful_response(desired.clone(), "43"),
    ]);

    let result = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
        .await
        .expect("both apply requests should succeed");
    assert_eq!(result.metadata.resource_version.as_deref(), Some("43"));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let narrow = &requests[0];
    assert_eq!(narrow.method, Method::PATCH);
    assert_eq!(
        narrow.path,
        "/apis/apps/v1/namespaces/default/statefulsets/test-tenant-pool-0"
    );
    assert!(
        narrow
            .query
            .contains("fieldManager=rustfs-operator-readiness")
    );
    assert!(narrow.query.contains("force=true"));
    assert_eq!(
        narrow.body,
        json!({
            "apiVersion": "apps/v1",
            "kind": "StatefulSet",
            "metadata": {
                "name": "test-tenant-pool-0",
                "namespace": "default",
                "uid": "statefulset-uid",
                "resourceVersion": "41",
            },
            "spec": { "template": { "spec": { "containers": [{
                "name": "rustfs",
                "readinessProbe": {
                    "initialDelaySeconds": 2,
                    "timeoutSeconds": 8,
                    "periodSeconds": 7,
                    "failureThreshold": 4,
                    "successThreshold": 2,
                },
            }] } } },
        })
    );

    let ordinary = &requests[1];
    assert_eq!(ordinary.method, Method::PATCH);
    assert_eq!(ordinary.path, narrow.path);
    assert!(ordinary.query.contains("fieldManager=rustfs-operator"));
    assert!(!ordinary.query.contains("force=true"));
    assert_eq!(ordinary.body["metadata"]["uid"], "statefulset-uid");
    assert_eq!(ordinary.body["metadata"]["resourceVersion"], "42");
    assert_eq!(
        ordinary.body["spec"]["template"]["spec"]["containers"][0]["readinessProbe"],
        json!({"httpGet": {"path": "/health/ready", "port": 9000, "scheme": "HTTP"}})
    );
}

#[tokio::test]
async fn invalid_ownership_or_identity_prevents_all_apply_requests() {
    for invalid in [
        "owner",
        "tenant_uid",
        "uid",
        "resource_version",
        "desired_probe",
    ] {
        let (mut tenant, mut existing, mut desired) = fixture();
        match invalid {
            "owner" => {
                existing.metadata.owner_references.as_mut().unwrap()[0].uid =
                    "another-tenant".to_string()
            }
            "tenant_uid" => tenant.metadata.uid = None,
            "uid" => existing.metadata.uid = None,
            "resource_version" => existing.metadata.resource_version = None,
            "desired_probe" => rustfs_container_mut(&mut desired).readiness_probe = None,
            _ => unreachable!(),
        }
        let (ctx, requests) = mock_context(Vec::new());
        let error = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("invalid resource identity or generated probe must stop the update");
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
async fn apply_errors_are_propagated_without_retrying_or_continuing() {
    for (failing_request, code) in [(0, 409), (0, 500), (1, 409), (1, 500)] {
        let (tenant, existing, desired) = fixture();
        let mut responses = Vec::new();
        if failing_request == 1 {
            responses.push(successful_response(desired.clone(), "42"));
        }
        responses.push(api_failure(code));
        let (ctx, requests) = mock_context(responses);
        let error = apply_statefulset(&ctx, &tenant, "default", &existing, desired)
            .await
            .expect_err("API failure must propagate to reconciliation");
        assert!(
            matches!(error, Error::Kube { source: kube::Error::Api(ref response) } if response.code == code)
        );
        assert_eq!(requests.lock().unwrap().len(), failing_request + 1);
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
    assert!(requests[0].query.contains("fieldManager=rustfs-operator"));
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
    let before_container = rustfs_container(existing);
    let previous_timeout = readiness_probe(existing).timeout_seconds;
    if expected_version != resource_version {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut target_observed = false;
            while let Some(event) = events.try_next().await.expect("watch event should read") {
                if let WatchEvent::Added(observed) | WatchEvent::Modified(observed) = event {
                    let probe = readiness_probe(&observed);
                    target_observed |= probe.timeout_seconds == Some(expected_timeout);
                    assert!(
                        probe.timeout_seconds == Some(expected_timeout)
                            || (!target_observed && probe.timeout_seconds == previous_timeout),
                        "stored revisions must move to the requested timeout without resetting it: {:?}",
                        probe.timeout_seconds
                    );
                    let container = rustfs_container(&observed);
                    assert_eq!(container.image, before_container.image);
                    assert_eq!(
                        serde_json::to_value(&container.liveness_probe).unwrap(),
                        serde_json::to_value(&before_container.liveness_probe).unwrap()
                    );
                    assert_eq!(
                        serde_json::to_value(&container.startup_probe).unwrap(),
                        serde_json::to_value(&before_container.startup_probe).unwrap()
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
        .expect("both apply revisions should be observed");
    }
    assert_eq!(
        readiness_probe(&updated).timeout_seconds,
        Some(expected_timeout)
    );
    let timing_fields = managed_container_fields(&updated, "rustfs-operator-readiness")
        .and_then(|fields| fields.get("f:readinessProbe"))
        .expect("dedicated manager should own readiness timing");
    for field in [
        "f:initialDelaySeconds",
        "f:timeoutSeconds",
        "f:periodSeconds",
        "f:failureThreshold",
        "f:successThreshold",
    ] {
        assert!(
            timing_fields.get(field).is_some(),
            "timing manager must own {field}"
        );
    }
    assert!(timing_fields.get("f:httpGet").is_none());
    let ordinary_probe_fields = managed_container_fields(&updated, "rustfs-operator")
        .and_then(|fields| fields.get("f:readinessProbe"));
    assert!(ordinary_probe_fields.is_none_or(|fields| fields.get("f:timeoutSeconds").is_none()));
    updated
}

async fn run_live_scenario(client: Client, namespace: &str) {
    let tenants: Api<Tenant> = Api::namespaced(client.clone(), namespace);
    let statefulsets: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let ctx = Context::new(client);
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
        "name": "rustfs", "readinessProbe": {"timeoutSeconds": 5},
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
    let updated =
        live_apply_and_check_events(&ctx, &tenant, namespace, &statefulsets, &existing, 5).await;
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
        timeout_seconds: Some(8),
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
    assert!(
        managed_container_fields(&preserved, "rustfs-operator-readiness")
            .unwrap()
            .get("f:image")
            .is_none()
    );
    assert_eq!(readiness_probe(&preserved).timeout_seconds, Some(8));
    assert_eq!(preserved.spec.as_ref().unwrap().replicas, Some(0));
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
