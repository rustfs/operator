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

/// Extended pool details for list/detail views
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolDetails {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub total_volumes: i32,
    pub storage_class: Option<String>,
    pub volume_size: Option<String>,
    pub replicas: i32,
    pub ready_replicas: i32,
    pub updated_replicas: i32,
    pub current_revision: Option<String>,
    pub update_revision: Option<String>,
    pub state: String,
    pub lifecycle_state: Option<String>,
    pub workload_state: Option<String>,
    pub decommission_phase: Option<String>,
    pub decommission_objects_migrated: Option<i64>,
    pub decommission_bytes_migrated: Option<i64>,
    pub decommission_objects_failed: Option<i64>,
    pub decommission_bytes_failed: Option<i64>,
    pub decommission_cleanup_state: Option<String>,
    pub decommission_last_error: Option<String>,
    pub decommission_last_poll_time: Option<String>,
    pub created_at: Option<String>,
}

/// Response listing pools for a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolListResponse {
    pub pools: Vec<PoolDetails>,
}

/// Request body to add a pool to a tenant
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddPoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,

    // Optional scheduling overrides
    pub node_selector: Option<std::collections::BTreeMap<String, String>>,
    pub resources: Option<ResourceRequirements>,
}

/// CPU/memory requests and limits
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ResourceRequirements {
    pub requests: Option<ResourceList>,
    pub limits: Option<ResourceList>,
}

/// Named resource quantities (e.g. cpu, memory)
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ResourceList {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

/// Response after deleting a pool
#[derive(Debug, Serialize, ToSchema)]
pub struct DeletePoolResponse {
    pub success: bool,
    pub message: String,
    pub warning: Option<String>,
}

/// Response after adding a pool
#[derive(Debug, Serialize, ToSchema)]
pub struct AddPoolResponse {
    pub success: bool,
    pub message: String,
    pub pool: PoolDetails,
}

/// Request body to start decommissioning a pool.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartPoolDecommissionRequest {
    pub request_id: String,
    pub reason: Option<String>,
}

/// Request body to cancel pool decommissioning.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelPoolDecommissionRequest {
    pub request_id: String,
    pub reason: Option<String>,
}

/// Response after writing a pool decommission lifecycle request.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolDecommissionRequestResponse {
    pub success: bool,
    pub message: String,
    pub pool_name: String,
    pub request_id: String,
    pub action: String,
}
