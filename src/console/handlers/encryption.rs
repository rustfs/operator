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
    models::encryption::*,
    state::Claims,
};
use crate::types::v1alpha1::encryption::{
    EncryptionConfig, KmsBackendType, LocalKmsConfig, VaultKmsConfig,
};
use crate::types::v1alpha1::tenant::Tenant;
use axum::{Extension, Json, extract::Path};
use k8s_openapi::api::core::v1 as corev1;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client};

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

    let enc_resp =
        match tenant.spec.encryption {
            Some(ref enc) => EncryptionInfoResponse {
                enabled: enc.enabled,
                backend: enc.backend.to_string(),
                vault: enc.vault.as_ref().map(|v| VaultInfo {
                    endpoint: v.endpoint.clone(),
                }),
                local: enc.local.as_ref().map(|l| LocalInfo {
                    key_directory: l.key_directory.clone(),
                }),
                kms_secret_name: enc.kms_secret.as_ref().map(|s| s.name.clone()),
                default_key_id: enc.default_key_id.clone(),
                security_context: tenant.spec.security_context.as_ref().map(|sc| {
                    SecurityContextInfo {
                        run_as_user: sc.run_as_user,
                        run_as_group: sc.run_as_group,
                        fs_group: sc.fs_group,
                        run_as_non_root: sc.run_as_non_root,
                    }
                }),
            },
            None => EncryptionInfoResponse {
                enabled: false,
                backend: "local".to_string(),
                vault: None,
                local: None,
                kms_secret_name: None,
                default_key_id: None,
                security_context: tenant.spec.security_context.as_ref().map(|sc| {
                    SecurityContextInfo {
                        run_as_user: sc.run_as_user,
                        run_as_group: sc.run_as_group,
                        fs_group: sc.fs_group,
                        run_as_non_root: sc.run_as_non_root,
                    }
                }),
            },
        };

    Ok(Json(enc_resp))
}

/// PUT /namespaces/:namespace/tenants/:name/encryption
pub async fn update_encryption(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<UpdateEncryptionRequest>,
) -> Result<Json<EncryptionUpdateResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let _tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    let encryption = if body.enabled {
        let backend = match body.backend.as_deref() {
            Some("vault") => KmsBackendType::Vault,
            _ => KmsBackendType::Local,
        };

        if backend == KmsBackendType::Vault {
            let vault_ok = body
                .vault
                .as_ref()
                .map(|v| !v.endpoint.is_empty())
                .unwrap_or(false);
            if !vault_ok {
                return Err(Error::BadRequest {
                    message: "Vault backend requires vault.endpoint to be non-empty".to_string(),
                });
            }
            let secret_ok = body
                .kms_secret_name
                .as_ref()
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if !secret_ok {
                return Err(Error::BadRequest {
                    message: "Vault backend requires kmsSecretName".to_string(),
                });
            }
        }

        let vault = if backend == KmsBackendType::Vault {
            body.vault.map(|v| VaultKmsConfig {
                endpoint: v.endpoint,
            })
        } else {
            None
        };

        let local = if backend == KmsBackendType::Local {
            body.local.map(|l| LocalKmsConfig {
                key_directory: l.key_directory,
            })
        } else {
            None
        };

        let kms_secret = body
            .kms_secret_name
            .filter(|s| !s.is_empty())
            .map(|s| corev1::LocalObjectReference { name: s });

        Some(EncryptionConfig {
            enabled: true,
            backend,
            vault,
            local,
            kms_secret,
            default_key_id: body.default_key_id.filter(|s| !s.is_empty()),
        })
    } else {
        Some(EncryptionConfig {
            enabled: false,
            ..Default::default()
        })
    };

    let patch = serde_json::json!({ "spec": { "encryption": encryption } });

    api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(EncryptionUpdateResponse {
        success: true,
        message: if body.enabled {
            "Encryption configuration updated".to_string()
        } else {
            "Encryption disabled".to_string()
        },
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
