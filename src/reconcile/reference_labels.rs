// Copyright 2025 RustFS Team
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

use super::{Error, object_owned_by_tenant};
use crate::context::{self, Context};
use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::time::Duration;
use tracing::{debug, info};

pub(super) const MISSING_REFERENCE_REQUEUE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct ReferenceLabelReconcileResult {
    pub(super) has_missing_resources: bool,
}

enum EnsureLabelOutcome {
    Present,
    Labeled,
    Missing,
}

pub(super) async fn reconcile_provisioning_reference_labels(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<ReferenceLabelReconcileResult, Error> {
    let tenant_name = tenant.name();
    let desired_user_secrets = tenant
        .spec
        .users
        .iter()
        .map(|user| user.credentials_secret_name().to_string())
        .collect::<BTreeSet<_>>();
    let referenced_secrets = tenant.spec.referenced_secret_names();
    let desired_policy_config_maps = tenant
        .spec
        .policies
        .iter()
        .map(|policy| policy.document.config_map_key_ref.name.clone())
        .collect::<BTreeSet<_>>();

    let secret_api = Api::<Secret>::namespaced(ctx.client.clone(), namespace);
    let labeled_secrets = list_labeled_resources(&secret_api, &tenant_name).await?;
    let mut result = ReferenceLabelReconcileResult {
        has_missing_resources: ensure_resource_labels(
            &secret_api,
            "Secret",
            namespace,
            &tenant_name,
            &desired_user_secrets,
            &labeled_resource_names(&labeled_secrets),
        )
        .await?,
    };
    remove_stale_secret_labels(
        &secret_api,
        tenant,
        namespace,
        &referenced_secrets,
        labeled_secrets,
    )
    .await?;

    if !desired_policy_config_maps.is_empty() {
        let config_map_api = Api::<ConfigMap>::namespaced(ctx.client.clone(), namespace);
        let labeled_config_maps = list_labeled_resources(&config_map_api, &tenant_name).await?;
        result.has_missing_resources |= ensure_resource_labels(
            &config_map_api,
            "ConfigMap",
            namespace,
            &tenant_name,
            &desired_policy_config_maps,
            &labeled_resource_names(&labeled_config_maps),
        )
        .await?;
    }

    Ok(result)
}

async fn list_labeled_resources<T>(api: &Api<T>, tenant_name: &str) -> Result<Vec<T>, Error>
where
    T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
    <T as Resource>::DynamicType: Default,
{
    let params = ListParams::default().labels(&format!("{RUSTFS_TENANT_LABEL}={tenant_name}"));
    api.list(&params)
        .await
        .map(|resources| resources.items)
        .map_err(|source| context::Error::Kube { source }.into())
}

fn labeled_resource_names<T>(resources: &[T]) -> BTreeSet<String>
where
    T: ResourceExt,
{
    resources.iter().map(ResourceExt::name_any).collect()
}

async fn ensure_resource_labels<T>(
    api: &Api<T>,
    resource_kind: &str,
    namespace: &str,
    tenant_name: &str,
    desired_names: &BTreeSet<String>,
    labeled_names: &BTreeSet<String>,
) -> Result<bool, Error>
where
    T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
    <T as Resource>::DynamicType: Default,
{
    let mut has_missing_resources = false;
    for name in desired_names.difference(labeled_names) {
        match ensure_resource_label(api, name, tenant_name).await? {
            EnsureLabelOutcome::Present => {}
            EnsureLabelOutcome::Labeled => {
                info!(
                    namespace,
                    tenant = tenant_name,
                    resource_kind,
                    resource = name,
                    "labeled provisioning reference for Tenant watch"
                );
            }
            EnsureLabelOutcome::Missing => {
                has_missing_resources = true;
                debug!(
                    namespace,
                    tenant = tenant_name,
                    resource_kind,
                    resource = name,
                    "provisioning reference is not available for Tenant watch labeling"
                );
            }
        }
    }
    Ok(has_missing_resources)
}

async fn ensure_resource_label<T>(
    api: &Api<T>,
    resource_name: &str,
    tenant_name: &str,
) -> Result<EnsureLabelOutcome, Error>
where
    T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
    <T as Resource>::DynamicType: Default,
{
    let resource = match api.get(resource_name).await {
        Ok(resource) => resource,
        Err(kube::Error::Api(response)) if response.code == 404 => {
            return Ok(EnsureLabelOutcome::Missing);
        }
        Err(source) => return Err(context::Error::Kube { source }.into()),
    };
    if resource
        .meta()
        .labels
        .as_ref()
        .and_then(|labels| labels.get(RUSTFS_TENANT_LABEL))
        .is_some_and(|owner| owner == tenant_name)
    {
        return Ok(EnsureLabelOutcome::Present);
    }

    let mut metadata = json!({
        "labels": {
            (RUSTFS_TENANT_LABEL): tenant_name,
        },
    });
    if let Some(resource_version) = &resource.meta().resource_version {
        metadata["resourceVersion"] = json!(resource_version);
    }
    let patch = json!({ "metadata": metadata });
    api.patch(
        resource_name,
        &PatchParams::default(),
        &Patch::Merge(&patch),
    )
    .await
    .map_err(|source| context::Error::Kube { source })?;
    Ok(EnsureLabelOutcome::Labeled)
}

async fn remove_stale_secret_labels(
    secret_api: &Api<Secret>,
    tenant: &Tenant,
    namespace: &str,
    referenced_secrets: &BTreeSet<String>,
    labeled_secrets: Vec<Secret>,
) -> Result<(), Error> {
    for secret in labeled_secrets {
        let name = secret.name_any();
        if referenced_secrets.contains(&name) || object_owned_by_tenant(&secret.metadata, tenant) {
            continue;
        }

        let mut metadata = json!({
            "labels": {
                (RUSTFS_TENANT_LABEL): null,
            },
        });
        if let Some(resource_version) = &secret.metadata.resource_version {
            metadata["resourceVersion"] = json!(resource_version);
        }
        let patch = json!({ "metadata": metadata });
        secret_api
            .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .map_err(|source| context::Error::Kube { source })?;
        info!(
            namespace,
            tenant = %tenant.name(),
            secret = name,
            "removed stale Tenant watch label from provisioning Secret"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::provisioning::{
        ConfigMapKeyReference, PolicyDocumentSource, ProvisioningPolicy, ProvisioningUser,
        UserCredentialsSecretRef,
    };
    use crate::types::v1alpha1::tenant::{RpcSecretRef, TenantSpec};
    use http::{Method, Request, Response, StatusCode};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
    use kube::{Client, client::Body};
    use serde_json::Value;
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};
    use tower::service_fn;

    #[derive(Debug)]
    struct CapturedRequest {
        method: Method,
        path: String,
        body: Option<Value>,
    }

    #[tokio::test]
    async fn operator_labels_current_references_and_cleans_only_stale_external_secrets() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let service_capture = captured.clone();
        let service = service_fn(move |request: Request<Body>| {
            let service_capture = service_capture.clone();
            async move {
                let method = request.method().clone();
                let path = request.uri().path().to_string();
                let body = request
                    .into_body()
                    .collect_bytes()
                    .await
                    .expect("request body should be readable");
                let body = (!body.is_empty()).then(|| {
                    serde_json::from_slice(&body).expect("request body should be valid JSON")
                });
                service_capture
                    .lock()
                    .expect("request capture lock should be available")
                    .push(CapturedRequest {
                        method: method.clone(),
                        path: path.clone(),
                        body,
                    });

                let response = match (method.as_str(), path.as_str()) {
                    ("GET", "/api/v1/namespaces/storage/secrets") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "SecretList",
                            "metadata": { "resourceVersion": "50" },
                            "items": [
                                labeled_secret("old-user-secret", "42", None),
                                labeled_secret("shared-secret", "43", None),
                                labeled_secret(
                                    "owned-secret",
                                    "44",
                                    Some(tenant_owner_reference()),
                                ),
                            ],
                        }),
                    ),
                    ("GET", "/api/v1/namespaces/storage/secrets/new-user-secret") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "Secret",
                            "metadata": {
                                "name": "new-user-secret",
                                "namespace": "storage",
                                "resourceVersion": "7"
                            }
                        }),
                    ),
                    ("PATCH", "/api/v1/namespaces/storage/secrets/new-user-secret") => {
                        json_response(StatusCode::OK, labeled_secret("new-user-secret", "8", None))
                    }
                    ("PATCH", "/api/v1/namespaces/storage/secrets/old-user-secret") => {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "apiVersion": "v1",
                                "kind": "Secret",
                                "metadata": {
                                    "name": "old-user-secret",
                                    "namespace": "storage",
                                    "resourceVersion": "43"
                                }
                            }),
                        )
                    }
                    ("GET", "/api/v1/namespaces/storage/configmaps") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "ConfigMapList",
                            "metadata": { "resourceVersion": "51" },
                            "items": [],
                        }),
                    ),
                    ("GET", "/api/v1/namespaces/storage/configmaps/policy-config") => {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "apiVersion": "v1",
                                "kind": "ConfigMap",
                                "metadata": {
                                    "name": "policy-config",
                                    "namespace": "storage",
                                    "resourceVersion": "9"
                                }
                            }),
                        )
                    }
                    ("PATCH", "/api/v1/namespaces/storage/configmaps/policy-config") => {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "apiVersion": "v1",
                                "kind": "ConfigMap",
                                "metadata": {
                                    "name": "policy-config",
                                    "namespace": "storage",
                                    "resourceVersion": "10",
                                    "labels": { "rustfs.tenant": "tenant-a" }
                                }
                            }),
                        )
                    }
                    _ => panic!("unexpected Kubernetes request: {method} {path}"),
                };
                Ok::<_, Infallible>(response)
            }
        });
        let ctx = Context::new(Client::new(service, "default"));
        let tenant = tenant_with_user_and_shared_rpc_secret();

        let result = reconcile_provisioning_reference_labels(&ctx, &tenant, "storage")
            .await
            .expect("reference labels should reconcile");

        assert!(!result.has_missing_resources);
        let captured = captured
            .lock()
            .expect("request capture lock should be available");
        let new_secret_patch = captured
            .iter()
            .find(|request| {
                request.method == Method::PATCH && request.path.ends_with("/new-user-secret")
            })
            .expect("new user Secret should be labeled");
        assert_eq!(
            new_secret_patch.body,
            Some(json!({
                "metadata": {
                    "resourceVersion": "7",
                    "labels": { "rustfs.tenant": "tenant-a" }
                }
            }))
        );
        let stale_secret_patch = captured
            .iter()
            .find(|request| {
                request.method == Method::PATCH && request.path.ends_with("/old-user-secret")
            })
            .expect("stale user Secret label should be removed");
        assert_eq!(
            stale_secret_patch.body,
            Some(json!({
                "metadata": {
                    "resourceVersion": "42",
                    "labels": { "rustfs.tenant": null }
                }
            }))
        );
        assert_eq!(
            captured
                .iter()
                .filter(|request| request.method == Method::PATCH)
                .count(),
            3,
            "shared and Tenant-owned Secrets must retain their labels"
        );
        let policy_config_map_patch = captured
            .iter()
            .find(|request| {
                request.method == Method::PATCH && request.path.ends_with("/policy-config")
            })
            .expect("policy ConfigMap should be labeled");
        assert_eq!(
            policy_config_map_patch.body,
            Some(json!({
                "metadata": {
                    "resourceVersion": "9",
                    "labels": { "rustfs.tenant": "tenant-a" }
                }
            }))
        );
    }

    #[tokio::test]
    async fn missing_reference_requests_a_follow_up_without_failing_reconcile() {
        let service = service_fn(move |request: Request<Body>| async move {
            let method = request.method().clone();
            let path = request.uri().path().to_string();
            let response = match (method.as_str(), path.as_str()) {
                ("GET", "/api/v1/namespaces/storage/secrets") => json_response(
                    StatusCode::OK,
                    json!({
                        "apiVersion": "v1",
                        "kind": "SecretList",
                        "metadata": { "resourceVersion": "1" },
                        "items": [],
                    }),
                ),
                ("GET", "/api/v1/namespaces/storage/secrets/new-user-secret") => json_response(
                    StatusCode::NOT_FOUND,
                    json!({
                        "apiVersion": "v1",
                        "kind": "Status",
                        "status": "Failure",
                        "reason": "NotFound",
                        "code": 404
                    }),
                ),
                _ => panic!("unexpected Kubernetes request: {method} {path}"),
            };
            Ok::<_, Infallible>(response)
        });
        let ctx = Context::new(Client::new(service, "default"));
        let mut tenant = tenant_with_user_and_shared_rpc_secret();
        tenant.spec.rpc_secret = None;
        tenant.spec.policies.clear();

        let result = reconcile_provisioning_reference_labels(&ctx, &tenant, "storage")
            .await
            .expect("a missing provisioning reference is a pending condition");

        assert!(result.has_missing_resources);
    }

    #[tokio::test]
    async fn stale_label_conflict_is_returned_for_controller_retry() {
        let service = service_fn(move |_request: Request<Body>| async move {
            Ok::<_, Infallible>(json_response(
                StatusCode::CONFLICT,
                json!({
                    "apiVersion": "v1",
                    "kind": "Status",
                    "status": "Failure",
                    "reason": "Conflict",
                    "code": 409
                }),
            ))
        });
        let secret_api = Api::<Secret>::namespaced(Client::new(service, "default"), "storage");
        let tenant = tenant_with_user_and_shared_rpc_secret();
        let stale_secret: Secret =
            serde_json::from_value(labeled_secret("old-user-secret", "42", None))
                .expect("Secret should deserialize");

        let error = remove_stale_secret_labels(
            &secret_api,
            &tenant,
            "storage",
            &BTreeSet::new(),
            vec![stale_secret],
        )
        .await
        .expect_err("resourceVersion conflicts must be retried by the controller");

        assert!(matches!(
            error,
            Error::Context {
                source: context::Error::Kube {
                    source: kube::Error::Api(response),
                },
            } if response.code == 409
        ));
    }

    fn tenant_with_user_and_shared_rpc_secret() -> Tenant {
        Tenant {
            metadata: ObjectMeta {
                name: Some("tenant-a".to_string()),
                namespace: Some("storage".to_string()),
                uid: Some("tenant-uid".to_string()),
                ..Default::default()
            },
            spec: TenantSpec {
                users: vec![ProvisioningUser {
                    name: "app-user".to_string(),
                    creds_secret: Some(UserCredentialsSecretRef {
                        name: "new-user-secret".to_string(),
                    }),
                    ..Default::default()
                }],
                rpc_secret: Some(RpcSecretRef {
                    name: "shared-secret".to_string(),
                    key: "rpc-secret".to_string(),
                }),
                policies: vec![ProvisioningPolicy {
                    name: "readwrite".to_string(),
                    document: PolicyDocumentSource {
                        config_map_key_ref: ConfigMapKeyReference {
                            name: "policy-config".to_string(),
                            key: "policy.json".to_string(),
                        },
                    },
                    deletion_policy: Default::default(),
                }],
                ..Default::default()
            },
            status: None,
        }
    }

    fn tenant_owner_reference() -> OwnerReference {
        OwnerReference {
            api_version: "rustfs.com/v1alpha1".to_string(),
            kind: "Tenant".to_string(),
            name: "tenant-a".to_string(),
            uid: "tenant-uid".to_string(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }
    }

    fn labeled_secret(name: &str, resource_version: &str, owner: Option<OwnerReference>) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": name,
                "namespace": "storage",
                "resourceVersion": resource_version,
                "labels": { "rustfs.tenant": "tenant-a" },
                "ownerReferences": owner.into_iter().collect::<Vec<_>>(),
            }
        })
    }

    fn json_response(status: StatusCode, value: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::from(
                serde_json::to_vec(&value).expect("response should serialize"),
            ))
            .expect("response should build")
    }
}
