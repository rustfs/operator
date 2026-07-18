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

use super::Error;
use crate::context::{self, Context};
use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, ResourceExt};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{debug, info};

pub(super) const MISSING_REFERENCE_REQUEUE_INTERVAL: Duration = Duration::from_secs(60);

pub(super) async fn reconcile_policy_config_map_labels(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<bool, Error> {
    let tenant_name = tenant.name();
    let desired_names = tenant
        .spec
        .policies
        .iter()
        .map(|policy| policy.document.config_map_key_ref.name.clone())
        .collect::<BTreeSet<_>>();
    if desired_names.is_empty() {
        return Ok(false);
    }

    let api = Api::<ConfigMap>::namespaced(ctx.client.clone(), namespace);
    let params = ListParams::default().labels(&format!("{RUSTFS_TENANT_LABEL}={tenant_name}"));
    let labeled_names = api
        .list(&params)
        .await
        .map_err(|source| context::Error::Kube { source })?
        .iter()
        .map(ResourceExt::name_any)
        .collect::<BTreeSet<_>>();

    let mut has_missing_resources = false;
    for name in desired_names.difference(&labeled_names) {
        let config_map = match api.get(name).await {
            Ok(config_map) => config_map,
            Err(kube::Error::Api(response)) if response.code == 404 => {
                has_missing_resources = true;
                debug!(
                    namespace,
                    tenant = tenant_name,
                    config_map = name,
                    "policy ConfigMap is not available for Tenant watch labeling"
                );
                continue;
            }
            Err(source) => return Err(context::Error::Kube { source }.into()),
        };

        let mut metadata = json!({
            "labels": {
                (RUSTFS_TENANT_LABEL): tenant_name,
            },
        });
        if let Some(resource_version) = &config_map.metadata.resource_version {
            metadata["resourceVersion"] = json!(resource_version);
        }
        let patch = json!({ "metadata": metadata });
        api.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .map_err(|source| context::Error::Kube { source })?;
        info!(
            namespace,
            tenant = tenant_name,
            config_map = name,
            "labeled policy ConfigMap for Tenant watch"
        );
    }

    Ok(has_missing_resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::provisioning::{
        ConfigMapKeyReference, PolicyDocumentSource, ProvisioningPolicy,
    };
    use crate::types::v1alpha1::tenant::TenantSpec;
    use http::{Method, Request, Response, StatusCode};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::{Client, client::Body};
    use serde_json::Value;
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};
    use tower::service_fn;

    #[tokio::test]
    async fn operator_labels_policy_config_map_with_resource_version() {
        let captured = Arc::new(Mutex::new(Vec::<(Method, String, Option<Value>)>::new()));
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
                    .push((method.clone(), path.clone(), body));

                let response = match (method.as_str(), path.as_str()) {
                    ("GET", "/api/v1/namespaces/storage/configmaps") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "ConfigMapList",
                            "metadata": { "resourceVersion": "50" },
                            "items": [],
                        }),
                    ),
                    ("GET", "/api/v1/namespaces/storage/configmaps/app-policy") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "ConfigMap",
                            "metadata": {
                                "name": "app-policy",
                                "namespace": "storage",
                                "resourceVersion": "7"
                            }
                        }),
                    ),
                    ("PATCH", "/api/v1/namespaces/storage/configmaps/app-policy") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "ConfigMap",
                            "metadata": {
                                "name": "app-policy",
                                "namespace": "storage",
                                "resourceVersion": "8",
                                "labels": { "rustfs.tenant": "tenant-a" }
                            }
                        }),
                    ),
                    _ => panic!("unexpected Kubernetes request: {method} {path}"),
                };
                Ok::<_, Infallible>(response)
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        let missing = reconcile_policy_config_map_labels(&ctx, &tenant(), "storage")
            .await
            .expect("ConfigMap label should reconcile");

        assert!(!missing);
        let captured = captured
            .lock()
            .expect("request capture lock should be available");
        let patch = captured
            .iter()
            .find(|(method, _, _)| method == Method::PATCH)
            .expect("policy ConfigMap should be patched");
        assert_eq!(
            patch.2,
            Some(json!({
                "metadata": {
                    "resourceVersion": "7",
                    "labels": { "rustfs.tenant": "tenant-a" }
                }
            }))
        );
    }

    fn tenant() -> Tenant {
        Tenant {
            metadata: ObjectMeta {
                name: Some("tenant-a".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            spec: TenantSpec {
                policies: vec![ProvisioningPolicy {
                    name: "readwrite".to_string(),
                    document: PolicyDocumentSource {
                        config_map_key_ref: ConfigMapKeyReference {
                            name: "app-policy".to_string(),
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

    fn json_response(status: StatusCode, value: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::from(
                serde_json::to_vec(&value).expect("response should serialize"),
            ))
            .expect("response should build")
    }
}
