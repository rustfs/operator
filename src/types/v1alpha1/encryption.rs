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

/// Secret key selector for the Local KMS master key.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalKmsMasterKeySecretRef {
    /// Secret name in the Tenant namespace.
    #[schemars(length(min = 1))]
    pub name: String,

    /// Secret data key containing the local master key string.
    #[schemars(length(min = 1))]
    pub key: String,
}

/// Local file-based KMS: key material directory inside the container.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalKmsConfig {
    /// Absolute directory for KMS key files.
    ///
    /// Must be in a subdirectory under a RustFS data PVC mount. Defaults to the first data PVC mount
    /// under `persistence.path`, for example `/data/rustfs0/.kms-keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_directory: Option<String>,

    /// Secret key selector for `RUSTFS_KMS_LOCAL_MASTER_KEY`.
    ///
    /// Required for Local KMS unless `allowInsecureDevDefaults` is explicitly set to `true`.
    /// The referenced Secret key should contain the local master key string used to encrypt
    /// Local KMS key files at rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_key_secret_ref: Option<LocalKmsMasterKeySecretRef>,

    /// Explicitly allow RustFS development-only insecure KMS defaults.
    ///
    /// When true, the operator sets `RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS=true`, allowing
    /// Local KMS to start without a master key and store key material as plaintext-dev-only.
    /// Do not use this in production.
    #[serde(default)]
    pub allow_insecure_dev_defaults: bool,
}

/// Encryption / KMS configuration for a Tenant.
///
/// Injected env vars match the RustFS server (`rustfs/src/config/cli.rs`, `init_kms_system`):
/// `RUSTFS_KMS_ENABLE`, `RUSTFS_KMS_BACKEND`, `RUSTFS_KMS_KEY_DIR`,
/// `RUSTFS_KMS_LOCAL_MASTER_KEY`, `RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS`,
/// `RUSTFS_KMS_DEFAULT_KEY_ID`, `RUSTFS_KMS_VAULT_ADDRESS`, `RUSTFS_KMS_VAULT_TOKEN`.
///
/// **Vault Secret:** key `vault-token` (required).
///
/// **Local:** use `local.masterKeySecretRef` unless explicitly enabling development-only
/// insecure defaults; use a single-server tenant (operator validates replica count).
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
