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

use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, Client, Resource};
use serde_json::json;
use tracing::{info, warn};

/// Remove the legacy single-Tenant routing label from external Secrets.
///
/// Tenant-owned Secrets retain the label because it still describes ownership.
/// External Secret events are routed through references in every Tenant spec.
pub(crate) async fn cleanup_external_secret_labels(client: &Client) {
    let secrets = match Api::<Secret>::all(client.clone())
        .list(&ListParams::default().labels(RUSTFS_TENANT_LABEL))
        .await
    {
        Ok(secrets) => secrets,
        Err(error) => {
            warn!(%error, "failed to list legacy-labeled Secrets");
            return;
        }
    };

    let mut cleaned = 0usize;
    for secret in secrets {
        if has_tenant_controller_owner(&secret.metadata) {
            continue;
        }

        let (Some(namespace), Some(name), Some(resource_version)) = (
            secret.metadata.namespace.as_deref(),
            secret.metadata.name.as_deref(),
            secret.metadata.resource_version.as_deref(),
        ) else {
            warn!(
                secret = ?secret.metadata.name,
                namespace = ?secret.metadata.namespace,
                "skipping legacy Secret label cleanup because metadata is incomplete"
            );
            continue;
        };

        let patch = json!({
            "metadata": {
                "resourceVersion": resource_version,
                "labels": {
                    (RUSTFS_TENANT_LABEL): null,
                },
            },
        });
        let api = Api::<Secret>::namespaced(client.clone(), namespace);
        match api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
        {
            Ok(_) => cleaned += 1,
            Err(error) => {
                warn!(
                    %error,
                    namespace,
                    secret = name,
                    "failed to remove legacy Tenant routing label from Secret"
                );
            }
        }
    }

    if cleaned > 0 {
        info!(
            cleaned,
            "removed legacy Tenant routing labels from external Secrets"
        );
    }
}

fn has_tenant_controller_owner(metadata: &ObjectMeta) -> bool {
    metadata.owner_references.as_ref().is_some_and(|owners| {
        owners.iter().any(|owner| {
            owner.controller == Some(true)
                && owner.api_version == Tenant::api_version(&())
                && owner.kind == Tenant::kind(&())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, Request, Response, StatusCode};
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
    async fn cleanup_patches_only_external_secrets_with_resource_version() {
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
                    ("GET", "/api/v1/secrets") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "SecretList",
                            "metadata": { "resourceVersion": "50" },
                            "items": [
                                labeled_secret("external", "42", None),
                                labeled_secret("owned", "43", Some(tenant_owner_reference())),
                            ],
                        }),
                    ),
                    ("PATCH", "/api/v1/namespaces/storage/secrets/external") => json_response(
                        StatusCode::OK,
                        json!({
                            "apiVersion": "v1",
                            "kind": "Secret",
                            "metadata": {
                                "name": "external",
                                "namespace": "storage",
                                "resourceVersion": "43"
                            }
                        }),
                    ),
                    _ => panic!("unexpected Kubernetes request: {method} {path}"),
                };
                Ok::<_, Infallible>(response)
            }
        });

        cleanup_external_secret_labels(&Client::new(service, "default")).await;

        let captured = captured
            .lock()
            .expect("request capture lock should be available");
        assert_eq!(
            captured
                .iter()
                .filter(|request| request.method == Method::PATCH)
                .count(),
            1
        );
        let patch = captured
            .iter()
            .find(|request| request.method == Method::PATCH)
            .expect("external Secret should be patched");
        assert_eq!(patch.path, "/api/v1/namespaces/storage/secrets/external");
        assert_eq!(
            patch.body,
            Some(json!({
                "metadata": {
                    "resourceVersion": "42",
                    "labels": { "rustfs.tenant": null }
                }
            }))
        );
    }

    fn labeled_secret(
        name: &str,
        resource_version: &str,
        owner: Option<serde_json::Value>,
    ) -> Value {
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

    fn tenant_owner_reference() -> Value {
        json!({
            "apiVersion": "rustfs.com/v1alpha1",
            "kind": "Tenant",
            "name": "tenant-a",
            "uid": "tenant-uid",
            "controller": true,
            "blockOwnerDeletion": true
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
