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

/// Single tenant row in a list view
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListItem {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub created_at: Option<String>,
}

/// Pool summary embedded in tenant list/detail
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolInfo {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
}

/// Response listing tenants
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantListItem>,
}

/// Query parameters for listing tenants
#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct TenantListQuery {
    /// Filter by tenant state (case-insensitive)
    pub state: Option<String>,
}

/// Per-state tenant counts
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantStateCountsResponse {
    /// Total number of tenants
    pub total: u32,
    /// Counts keyed by state, e.g. Ready/Updating/Degraded/NotReady/Unknown
    pub counts: std::collections::BTreeMap<String, u32>,
}

/// Full tenant detail for the UI
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantDetailsResponse {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub created_at: Option<String>,
    pub services: Vec<ServiceInfo>,
}

/// Exposed Service summary
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceInfo {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
}

/// Port mapping for a Service
#[derive(Debug, Serialize, ToSchema)]
pub struct ServicePort {
    pub name: String,
    pub port: i32,
    pub target_port: String,
}

/// SecurityContext for create/update (Pod runAsUser, runAsGroup, fsGroup, runAsNonRoot).
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSecurityContextRequest {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
}

/// Request body to create a tenant
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTenantRequest {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<CreatePoolRequest>,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub creds_secret: Option<String>,
    /// Optional Pod SecurityContext override (runAsUser, runAsGroup, fsGroup, runAsNonRoot).
    pub security_context: Option<CreateSecurityContextRequest>,
}

/// Pool spec embedded in create-tenant request
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,
}

/// Response after deleting a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteTenantResponse {
    pub success: bool,
    pub message: String,
}

/// Partial update payload for a tenant
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTenantRequest {
    /// New container image
    pub image: Option<String>,

    /// New volume mount path
    pub mount_path: Option<String>,

    /// Replace env vars
    pub env: Option<Vec<EnvVar>>,

    /// Reference to credentials Secret
    pub creds_secret: Option<String>,

    /// Pod management policy
    pub pod_management_policy: Option<String>,

    /// Image pull policy
    pub image_pull_policy: Option<String>,

    /// Logging sidecar / volume settings
    pub logging: Option<LoggingConfig>,
}

/// Key/value environment variable
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct EnvVar {
    pub name: String,
    pub value: Option<String>,
}

/// Tenant logging configuration
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    pub log_type: String, // "stdout" | "emptyDir" | "persistent"
    pub volume_size: Option<String>,
    pub storage_class: Option<String>,
}

/// Response after updating a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateTenantResponse {
    pub success: bool,
    pub message: String,
    pub tenant: TenantListItem,
}

/// Raw Tenant manifest get/update payload
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TenantYAML {
    pub yaml: String,
}
