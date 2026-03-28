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
/// RustFS supports two backends:
/// - `Local`: File-based key storage on disk (development / single-node)
/// - `Vault`: HashiCorp Vault KV2 engine (production)
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

/// Vault authentication method.
///
/// `Token` is the default and fully implemented in rustfs-kms.
/// `Approle` type exists in rustfs-kms but the backend is not yet functional.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum VaultAuthType {
    #[default]
    Token,
    Approle,
}

impl std::fmt::Display for VaultAuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultAuthType::Token => write!(f, "token"),
            VaultAuthType::Approle => write!(f, "approle"),
        }
    }
}

/// Vault-specific KMS configuration.
///
/// Maps to `VaultConfig` in the `rustfs-kms` crate.
/// Sensitive fields (Vault token or AppRole credentials) are stored in the Secret referenced
/// by `EncryptionConfig::kms_secret`.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct VaultKmsConfig {
    /// Vault server endpoint (e.g. `https://vault.example.com:8200`).
    pub endpoint: String,

    /// KV secrets engine mount path (maps to `RUSTFS_KMS_VAULT_KV_MOUNT` in rustfs-kms; e.g. `secret`, `kv`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,

    /// Vault namespace (Enterprise feature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Key prefix inside the KV engine (maps to `RUSTFS_KMS_VAULT_KEY_PREFIX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// Authentication method. Defaults to `token` when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<VaultAuthType>,

    /// AppRole authentication settings. Only used when `authType: approle`.
    /// The actual `role_id` and `secret_id` values live in the KMS Secret
    /// under keys `vault-approle-id` and `vault-approle-secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_role: Option<VaultAppRoleConfig>,
}

/// Vault AppRole authentication settings.
///
/// Sensitive credentials (`role_id`, `secret_id`) are NOT stored here.
/// They must be placed in the KMS Secret referenced by `EncryptionConfig::kms_secret`
/// under keys `vault-approle-id` and `vault-approle-secret`.
///
/// NOTE: The rustfs-kms `VaultAuthMethod::AppRole` type exists, but the
/// Vault backend does **not** implement it yet. These fields are provided
/// so the CRD/UI is ready when the backend adds support.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct VaultAppRoleConfig {
    /// Engine mount path for AppRole auth (e.g. `approle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,

    /// Retry interval in seconds for AppRole login attempts (default: 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_seconds: Option<i32>,
}

/// Local file-based KMS configuration.
///
/// Maps to `LocalConfig` in the `rustfs-kms` crate.
/// Keys are stored as JSON files in the specified directory.
///
/// **RustFS binary alignment**: `key_directory` is injected as `RUSTFS_KMS_KEY_DIR` (required by
/// `rustfs` server startup). `master_key_id` maps to `RUSTFS_KMS_DEFAULT_KEY_ID` (default SSE key id),
/// not to a "master encryption passphrase" (`RUSTFS_KMS_LOCAL_MASTER_KEY` is separate in rustfs-kms).
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalKmsConfig {
    /// Absolute directory for KMS key files inside the container (default: `/data/kms-keys`).
    /// Must be absolute; RustFS validates this for the local backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_directory: Option<String>,

    /// Default KMS key id for SSE (maps to `RUSTFS_KMS_DEFAULT_KEY_ID` in the RustFS binary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_key_id: Option<String>,
}

/// Encryption / KMS configuration for a Tenant.
///
/// When enabled, the operator injects environment variables matching the **RustFS server**
/// (`rustfs` CLI / `init_kms_system`) and `rustfs_kms::KmsConfig::from_env()` where applicable.
/// See `Tenant::configure_kms` in `tenant/workloads.rs` for the exact variable names.
///
/// Example YAML:
/// ```yaml
/// spec:
///   encryption:
///     enabled: true
///     backend: vault
///     vault:
///       endpoint: "https://vault.example.com:8200"
///       engine: "kv"
///       namespace: "tenant1"
///       prefix: "rustfs"
///     kmsSecret:
///       name: "my-tenant-kms-secret"
/// ```
///
/// The referenced Secret must contain backend-specific keys:
///
/// **Vault backend (Token auth):**
/// - `vault-token` (required): Vault authentication token
///
/// **Vault backend (AppRole auth):**
/// - `vault-approle-id` (required): AppRole role ID
/// - `vault-approle-secret` (required): AppRole secret ID
///
/// **Local backend:**
/// No secret keys required (keys are stored on disk).
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionConfig {
    /// Enable server-side encryption. When `false`, all other fields are ignored.
    #[serde(default)]
    pub enabled: bool,

    /// KMS backend type: `local` or `vault`.
    #[serde(default)]
    pub backend: KmsBackendType,

    /// Vault-specific settings (required when `backend: vault`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultKmsConfig>,

    /// Local file-based settings (optional when `backend: local`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalKmsConfig>,

    /// Reference to a Secret containing sensitive KMS credentials
    /// (Vault token or AppRole credentials).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_secret: Option<corev1::LocalObjectReference>,

    /// Reserved for future KMS health-check tuning. Not injected into pods: the current RustFS
    /// release does not read `RUSTFS_KMS_PING_SECONDS` in the server startup path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_seconds: Option<i32>,
}

/// Pod SecurityContext overrides.
///
/// Since RustFS KMS runs in-process (no separate sidecar like MinIO KES),
/// these values override the default Pod SecurityContext
/// (runAsUser/runAsGroup/fsGroup = 10001) for all RustFS pods in the Tenant.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct PodSecurityContextOverride {
    /// UID to run the container process as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_user: Option<i64>,

    /// GID to run the container process as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_group: Option<i64>,

    /// GID applied to all volumes mounted in the Pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group: Option<i64>,

    /// Enforce non-root execution (default: true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_non_root: Option<bool>,
}
