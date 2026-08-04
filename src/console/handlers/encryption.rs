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

use crate::console::{
    error::{self, Error, Result},
    json::ConsoleJson,
    models::encryption::*,
    state::Claims,
};
use crate::types::v1alpha1::encryption::{
    EncryptionConfig, KmsBackendType, LocalKmsConfig, LocalKmsMasterKeySecretRef, VaultKmsConfig,
};
use crate::types::v1alpha1::tenant::Tenant;
use axum::{Extension, Json, extract::Path};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client};

fn trim_to_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn encryption_config_from_request(body: &UpdateEncryptionRequest) -> Result<EncryptionConfig> {
    let encryption = if body.enabled {
        let backend = match body.backend {
            Some(UpdateEncryptionBackend::Vault) => KmsBackendType::Vault,
            Some(UpdateEncryptionBackend::Local) | None => KmsBackendType::Local,
        };
        let vault_endpoint = body
            .vault
            .as_ref()
            .and_then(|vault| trim_to_non_empty(Some(vault.endpoint.clone())));
        let kms_secret_name = trim_to_non_empty(body.kms_secret_name.clone());
        let default_key_id = trim_to_non_empty(body.default_key_id.clone());

        if backend == KmsBackendType::Vault {
            if vault_endpoint.is_none() {
                return Err(Error::BadRequest {
                    message: "Vault backend requires vault.endpoint to be non-empty".to_string(),
                });
            }
            if kms_secret_name.is_none() {
                return Err(Error::BadRequest {
                    message: "Vault backend requires kmsSecretName".to_string(),
                });
            }
        } else {
            let local = body.local.as_ref();
            let allow_insecure_dev_defaults = local
                .and_then(|local| local.allow_insecure_dev_defaults)
                .unwrap_or(false);
            let master_key_ref_ok = local
                .and_then(|local| local.master_key_secret_ref.as_ref())
                .is_some_and(|secret| {
                    !secret.name.trim().is_empty() && !secret.key.trim().is_empty()
                });
            if !allow_insecure_dev_defaults && !master_key_ref_ok {
                return Err(Error::BadRequest {
                    message: "Local backend requires local.masterKeySecretRef.name/key unless local.allowInsecureDevDefaults is true".to_string(),
                });
            }
        }

        let vault = if backend == KmsBackendType::Vault {
            vault_endpoint.map(|endpoint| VaultKmsConfig { endpoint })
        } else {
            None
        };
        let local = if backend == KmsBackendType::Local {
            body.local.as_ref().map(|local| LocalKmsConfig {
                key_directory: trim_to_non_empty(local.key_directory.clone()),
                master_key_secret_ref: local.master_key_secret_ref.as_ref().map(|secret| {
                    LocalKmsMasterKeySecretRef {
                        name: secret.name.trim().to_string(),
                        key: secret.key.trim().to_string(),
                    }
                }),
                allow_insecure_dev_defaults: local.allow_insecure_dev_defaults.unwrap_or(false),
            })
        } else {
            None
        };
        let kms_secret = if backend == KmsBackendType::Vault {
            kms_secret_name.map(|name| corev1::LocalObjectReference { name })
        } else {
            None
        };

        EncryptionConfig {
            enabled: true,
            backend,
            vault,
            local,
            kms_secret,
            default_key_id,
        }
    } else {
        EncryptionConfig {
            enabled: false,
            ..Default::default()
        }
    };

    Ok(encryption)
}

fn apply_encryption_update(tenant: &mut Tenant, body: &UpdateEncryptionRequest) -> Result<()> {
    // Replace the complete encryption configuration in memory. The caller persists the Tenant with
    // resourceVersion optimistic concurrency so fields omitted from the request are truly removed.
    tenant.spec.encryption = Some(encryption_config_from_request(body)?);
    Ok(())
}

/// GET /namespaces/:namespace/tenants/:name/encryption
pub async fn get_encryption(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<EncryptionInfoResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;
    let security_context = tenant.spec.security_context.as_ref().map(|_| {
        SecurityContextInfo::from_contexts(
            tenant.spec.security_context.as_ref(),
            tenant.spec.container_security_context.as_ref(),
        )
    });

    let enc_resp = match tenant.spec.encryption {
        Some(ref enc) => EncryptionInfoResponse {
            enabled: enc.enabled,
            backend: enc.backend.to_string(),
            vault: enc.vault.as_ref().map(|v| VaultInfo {
                endpoint: v.endpoint.clone(),
            }),
            local: enc.local.as_ref().map(|l| LocalInfo {
                key_directory: l.key_directory.clone(),
                master_key_secret_ref: l.master_key_secret_ref.as_ref().map(|s| {
                    LocalMasterKeySecretRefInfo {
                        name: s.name.clone(),
                        key: s.key.clone(),
                    }
                }),
                allow_insecure_dev_defaults: l.allow_insecure_dev_defaults,
            }),
            kms_secret_name: (enc.backend == KmsBackendType::Vault)
                .then(|| enc.kms_secret.as_ref().map(|s| s.name.clone()))
                .flatten(),
            default_key_id: enc.default_key_id.clone(),
            security_context,
        },
        None => EncryptionInfoResponse {
            enabled: false,
            backend: "local".to_string(),
            vault: None,
            local: None,
            kms_secret_name: None,
            default_key_id: None,
            security_context,
        },
    };

    Ok(Json(enc_resp))
}

/// PUT /namespaces/:namespace/tenants/:name/encryption
pub async fn update_encryption(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    ConsoleJson(body): ConsoleJson<UpdateEncryptionRequest>,
) -> Result<Json<EncryptionUpdateResponse>> {
    let client = create_client(&claims).await?;
    let response = update_encryption_with_client(&client, &namespace, &name, &body).await?;
    Ok(Json(response))
}

async fn update_encryption_with_client(
    client: &Client,
    namespace: &str,
    name: &str,
    body: &UpdateEncryptionRequest,
) -> Result<EncryptionUpdateResponse> {
    let api: Api<Tenant> = Api::namespaced(client.clone(), namespace);

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_conflict = None;
    for _ in 0..MAX_ATTEMPTS {
        let mut tenant = api
            .get(name)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Tenant '{name}'")))?;
        apply_encryption_update(&mut tenant, body)?;

        match api.replace(name, &Default::default(), &tenant).await {
            Ok(_) => {
                return Ok(EncryptionUpdateResponse {
                    success: true,
                    message: if body.enabled {
                        "Encryption configuration updated".to_string()
                    } else {
                        "Encryption disabled".to_string()
                    },
                });
            }
            Err(error) => {
                let mapped = error::map_kube_error(error, format!("Tenant '{name}'"));
                if !matches!(&mapped, Error::Conflict { .. }) {
                    return Err(mapped);
                }
                last_conflict = Some(mapped);
            }
        }
    }

    Err(last_conflict.unwrap_or_else(|| Error::Conflict {
        message: "Resource was modified by another request, please retry".to_string(),
    }))
}

async fn create_client(claims: &Claims) -> Result<Client> {
    let mut config = kube::Config::infer()
        .await
        .map_err(|e| Error::InternalServer {
            message: format!("Failed to load kubeconfig: {}", e),
        })?;

    config.auth_info.token = Some(claims.k8s_token.clone().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}

#[cfg(test)]
mod tests {
    use super::{apply_encryption_update, trim_to_non_empty, update_encryption_with_client};
    use crate::console::error::Error;
    use crate::console::models::encryption::{
        LocalMasterKeySecretRefInfo, UpdateEncryptionBackend, UpdateEncryptionRequest,
        UpdateLocalRequest, UpdateVaultRequest,
    };
    use crate::types::v1alpha1::encryption::{
        EncryptionConfig, KmsBackendType, LocalKmsConfig, LocalKmsMasterKeySecretRef,
        VaultKmsConfig,
    };
    use http::{Request, Response, StatusCode};
    use k8s_openapi::api::core::v1::LocalObjectReference;
    use kube::{Client, client::Body};
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::service_fn;

    fn tenant_response(
        resource_version: &str,
        image: &str,
        annotation: Option<(&str, &str)>,
    ) -> Value {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.name = Some("tenant-a".to_string());
        tenant.metadata.namespace = Some("storage".to_string());
        tenant.metadata.resource_version = Some(resource_version.to_string());
        tenant.metadata.annotations = annotation.map(|(key, value)| {
            std::collections::BTreeMap::from([(key.to_string(), value.to_string())])
        });
        tenant.spec.image = Some(image.to_string());
        serde_json::to_value(tenant).expect("Tenant response should serialize")
    }

    fn json_response(status: StatusCode, body: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::from(
                serde_json::to_vec(&body).expect("response body should serialize"),
            ))
            .expect("response should build")
    }

    fn disabled_encryption_request() -> UpdateEncryptionRequest {
        UpdateEncryptionRequest {
            enabled: false,
            backend: None,
            vault: None,
            local: None,
            kms_secret_name: None,
            default_key_id: None,
        }
    }

    #[test]
    fn trim_to_non_empty_drops_blank_strings() {
        assert_eq!(trim_to_non_empty(None), None);
        assert_eq!(trim_to_non_empty(Some("   ".to_string())), None);
        assert_eq!(
            trim_to_non_empty(Some("  /data/rustfs0/.kms-keys  ".to_string())),
            Some("/data/rustfs0/.kms-keys".to_string())
        );
    }

    #[test]
    fn disabling_encryption_removes_all_previous_backend_fields() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Vault,
            vault: Some(VaultKmsConfig {
                endpoint: "https://vault.example.com".to_string(),
            }),
            local: Some(LocalKmsConfig {
                key_directory: Some("/data/old-keys".to_string()),
                master_key_secret_ref: Some(LocalKmsMasterKeySecretRef {
                    name: "old-local-key".to_string(),
                    key: "master-key".to_string(),
                }),
                allow_insecure_dev_defaults: false,
            }),
            kms_secret: Some(LocalObjectReference {
                name: "old-vault-token".to_string(),
            }),
            default_key_id: Some("old-default-key".to_string()),
        });
        let request = UpdateEncryptionRequest {
            enabled: false,
            backend: None,
            vault: None,
            local: None,
            kms_secret_name: None,
            default_key_id: None,
        };

        apply_encryption_update(&mut tenant, &request).expect("disable should be valid");

        let encryption = tenant.spec.encryption.expect("disabled configuration");
        assert!(!encryption.enabled);
        assert_eq!(encryption.backend, KmsBackendType::Local);
        assert!(encryption.vault.is_none());
        assert!(encryption.local.is_none());
        assert!(encryption.kms_secret.is_none());
        assert!(encryption.default_key_id.is_none());
    }

    #[test]
    fn switching_vault_to_local_removes_vault_fields_and_clears_omitted_values() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Vault,
            vault: Some(VaultKmsConfig {
                endpoint: "https://vault.example.com".to_string(),
            }),
            kms_secret: Some(LocalObjectReference {
                name: "vault-token".to_string(),
            }),
            default_key_id: Some("old-default-key".to_string()),
            ..Default::default()
        });
        let request = UpdateEncryptionRequest {
            enabled: true,
            backend: Some(UpdateEncryptionBackend::Local),
            vault: None,
            local: Some(UpdateLocalRequest {
                key_directory: None,
                master_key_secret_ref: Some(LocalMasterKeySecretRefInfo {
                    name: " local-master ".to_string(),
                    key: " master-key ".to_string(),
                }),
                allow_insecure_dev_defaults: Some(false),
            }),
            kms_secret_name: None,
            default_key_id: None,
        };

        apply_encryption_update(&mut tenant, &request).expect("local update should be valid");

        let encryption = tenant.spec.encryption.expect("local configuration");
        assert_eq!(encryption.backend, KmsBackendType::Local);
        assert!(encryption.vault.is_none());
        assert!(encryption.kms_secret.is_none());
        assert!(encryption.default_key_id.is_none());
        let local = encryption.local.expect("local settings");
        assert!(local.key_directory.is_none());
        let secret = local.master_key_secret_ref.expect("master key reference");
        assert_eq!(secret.name, "local-master");
        assert_eq!(secret.key, "master-key");
    }

    #[test]
    fn switching_local_to_vault_removes_local_fields() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            local: Some(LocalKmsConfig {
                key_directory: Some("/data/old-keys".to_string()),
                master_key_secret_ref: Some(LocalKmsMasterKeySecretRef {
                    name: "old-local-key".to_string(),
                    key: "master-key".to_string(),
                }),
                allow_insecure_dev_defaults: false,
            }),
            ..Default::default()
        });
        let request = UpdateEncryptionRequest {
            enabled: true,
            backend: Some(UpdateEncryptionBackend::Vault),
            vault: Some(UpdateVaultRequest {
                endpoint: " https://vault.example.com ".to_string(),
            }),
            local: None,
            kms_secret_name: Some(" vault-token ".to_string()),
            default_key_id: Some(" tenant-key ".to_string()),
        };

        apply_encryption_update(&mut tenant, &request).expect("vault update should be valid");

        let encryption = tenant.spec.encryption.expect("vault configuration");
        assert_eq!(encryption.backend, KmsBackendType::Vault);
        assert!(encryption.local.is_none());
        assert_eq!(
            encryption.vault.expect("vault settings").endpoint,
            "https://vault.example.com"
        );
        assert_eq!(
            encryption.kms_secret.expect("vault secret").name,
            "vault-token"
        );
        assert_eq!(encryption.default_key_id.as_deref(), Some("tenant-key"));
    }

    #[tokio::test]
    async fn encryption_update_retries_conflict_against_latest_tenant() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let initial = tenant_response("1", "rustfs:initial", None);
        let concurrent = tenant_response(
            "2",
            "rustfs:concurrent",
            Some(("example.com/concurrent", "preserved")),
        );
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                let initial = initial.clone();
                let concurrent = concurrent.clone();
                async move {
                    let method = request.method().clone();
                    let path = request.uri().path().to_string();
                    let request_body = request
                        .into_body()
                        .collect_bytes()
                        .await
                        .expect("request body should be readable");
                    assert_eq!(
                        path,
                        "/apis/rustfs.com/v1alpha1/namespaces/storage/tenants/tenant-a"
                    );

                    let response = match request_number {
                        0 => {
                            assert_eq!(method, http::Method::GET);
                            json_response(StatusCode::OK, initial)
                        }
                        1 => {
                            assert_eq!(method, http::Method::PUT);
                            let body: Value = serde_json::from_slice(&request_body)
                                .expect("replacement should be JSON");
                            assert_eq!(
                                body.pointer("/metadata/resourceVersion"),
                                Some(&json!("1"))
                            );
                            assert_eq!(body.pointer("/spec/image"), Some(&json!("rustfs:initial")));
                            assert_eq!(
                                body.pointer("/spec/encryption/enabled"),
                                Some(&json!(false))
                            );
                            json_response(
                                StatusCode::CONFLICT,
                                json!({
                                    "status": "Failure",
                                    "message": "tenant changed concurrently",
                                    "reason": "Conflict",
                                    "code": 409
                                }),
                            )
                        }
                        2 => {
                            assert_eq!(method, http::Method::GET);
                            json_response(StatusCode::OK, concurrent)
                        }
                        3 => {
                            assert_eq!(method, http::Method::PUT);
                            let body: Value = serde_json::from_slice(&request_body)
                                .expect("replacement should be JSON");
                            assert_eq!(
                                body.pointer("/metadata/resourceVersion"),
                                Some(&json!("2"))
                            );
                            assert_eq!(
                                body.pointer("/metadata/annotations/example.com~1concurrent"),
                                Some(&json!("preserved"))
                            );
                            assert_eq!(
                                body.pointer("/spec/image"),
                                Some(&json!("rustfs:concurrent"))
                            );
                            assert_eq!(
                                body.pointer("/spec/encryption/enabled"),
                                Some(&json!(false))
                            );
                            json_response(StatusCode::OK, body)
                        }
                        other => panic!("unexpected request number {other}"),
                    };

                    Ok::<_, std::convert::Infallible>(response)
                }
            }
        });
        let client = Client::new(service, "default");

        let response = update_encryption_with_client(
            &client,
            "storage",
            "tenant-a",
            &disabled_encryption_request(),
        )
        .await
        .expect("the retry should succeed");

        assert!(response.success);
        assert_eq!(request_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn encryption_update_returns_last_conflict_after_attempts_are_exhausted() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    let attempt = request_number / 2 + 1;
                    let response = if request_number % 2 == 0 {
                        assert_eq!(request.method(), http::Method::GET);
                        json_response(
                            StatusCode::OK,
                            tenant_response(&attempt.to_string(), "rustfs:latest", None),
                        )
                    } else {
                        assert_eq!(request.method(), http::Method::PUT);
                        json_response(
                            StatusCode::CONFLICT,
                            json!({
                                "status": "Failure",
                                "message": format!("conflict attempt {attempt}"),
                                "reason": "Conflict",
                                "code": 409
                            }),
                        )
                    };
                    Ok::<_, std::convert::Infallible>(response)
                }
            }
        });
        let client = Client::new(service, "default");

        let error = update_encryption_with_client(
            &client,
            "storage",
            "tenant-a",
            &disabled_encryption_request(),
        )
        .await
        .expect_err("three conflicts should exhaust the update attempts");

        assert!(matches!(
            error,
            Error::Conflict { message } if message == "conflict attempt 3"
        ));
        assert_eq!(request_count.load(Ordering::SeqCst), 6);
    }
}
