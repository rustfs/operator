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

use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::schemars::JsonSchema;
use kube::KubeSchema;
use serde::{Deserialize, Serialize};

/// KMS backend type for server-side encryption.
///
/// RustFS `init_kms_system` reads `RUSTFS_KMS_BACKEND` (`local` or `vault`).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum KmsBackendType {
    #[default]
    Local,
    Vault,
}

impl std::fmt::Display for KmsBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KmsBackendType::Local => write!(f, "local"),
            KmsBackendType::Vault => write!(f, "vault"),
        }
    }
}

/// Vault endpoint for KMS. Token is supplied via `kmsSecret` (`vault-token` key).
///
/// RustFS currently fixes Transit mount, KV mount, and key prefix inside `build_vault_kms_config`;
/// only address and token are configurable at startup.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct VaultKmsConfig {
    /// Vault server URL (e.g. `https://vault.example.com:8200`).
    pub endpoint: String,
}

/// Local file-based KMS: key material directory inside the container.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalKmsConfig {
    /// Absolute directory for KMS key files (default: `/data/kms-keys`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_directory: Option<String>,
}

/// Encryption / KMS configuration for a Tenant.
///
/// Injected env vars match the RustFS server (`rustfs/src/config/cli.rs`, `init_kms_system`):
/// `RUSTFS_KMS_ENABLE`, `RUSTFS_KMS_BACKEND`, `RUSTFS_KMS_KEY_DIR`, `RUSTFS_KMS_LOCAL_KEY_DIR`,
/// `RUSTFS_KMS_DEFAULT_KEY_ID`, `RUSTFS_KMS_VAULT_ADDRESS`, `RUSTFS_KMS_VAULT_TOKEN`.
///
/// **Vault Secret:** key `vault-token` (required).
///
/// **Local:** no Secret; use a single-server tenant (operator validates replica count).
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionConfig {
    /// Enable server-side encryption. When `false`, all other fields are ignored.
    #[serde(default)]
    pub enabled: bool,

    /// KMS backend: `local` or `vault`.
    #[serde(default)]
    pub backend: KmsBackendType,

    /// Vault: HTTP(S) endpoint (required when `backend: vault`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultKmsConfig>,

    /// Local: optional key directory override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalKmsConfig>,

    /// Secret holding `vault-token` when using Vault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_secret: Option<corev1::LocalObjectReference>,

    /// Optional default SSE key id (`RUSTFS_KMS_DEFAULT_KEY_ID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_key_id: Option<String>,
}

/// Pod SecurityContext overrides for all RustFS pods in this Tenant.
///
/// Overrides the default Pod SecurityContext (`runAsUser` / `runAsGroup` / `fsGroup` = 10001).
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct PodSecurityContextOverride {
    /// UID to run the container process as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_user: Option<i64>,

    /// GID to run the container process as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_group: Option<i64>,

    /// GID applied to all volumes mounted in the Pod (`fsGroup`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group: Option<i64>,

    /// Enforce non-root execution (default in the operator: `true` when set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_non_root: Option<bool>,
}
