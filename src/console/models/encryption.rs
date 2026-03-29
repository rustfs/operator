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

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// GET response – current encryption configuration for a Tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionInfoResponse {
    pub enabled: bool,
    pub backend: String,
    pub vault: Option<VaultInfo>,
    pub local: Option<LocalInfo>,
    pub kms_secret_name: Option<String>,
    pub ping_seconds: Option<i32>,
    pub security_context: Option<SecurityContextInfo>,
}

/// Vault configuration (non-sensitive fields only).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub endpoint: String,
    pub engine: Option<String>,
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub auth_type: Option<String>,
    pub app_role: Option<AppRoleInfo>,
}

/// AppRole non-sensitive fields.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppRoleInfo {
    pub engine: Option<String>,
    pub retry_seconds: Option<i32>,
}

/// Local KMS configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInfo {
    pub key_directory: Option<String>,
    pub master_key_id: Option<String>,
}

/// SecurityContext information (lives at TenantSpec level, shown alongside encryption).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityContextInfo {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
}

/// PUT request – update encryption configuration.
/// SecurityContext is managed separately via the Security tab (PUT .../security-context).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEncryptionRequest {
    pub enabled: bool,
    pub backend: Option<String>,
    pub vault: Option<UpdateVaultRequest>,
    pub local: Option<UpdateLocalRequest>,
    pub kms_secret_name: Option<String>,
    pub ping_seconds: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVaultRequest {
    pub endpoint: String,
    pub engine: Option<String>,
    pub namespace: Option<String>,
    pub prefix: Option<String>,
    pub auth_type: Option<String>,
    pub app_role: Option<UpdateAppRoleRequest>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAppRoleRequest {
    pub engine: Option<String>,
    pub retry_seconds: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLocalRequest {
    pub key_directory: Option<String>,
    pub master_key_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSecurityContextRequest {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
}

/// Generic success response.
#[derive(Debug, Serialize, ToSchema)]
pub struct EncryptionUpdateResponse {
    pub success: bool,
    pub message: String,
}
