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
    pub total_volumes: i64,
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

impl PoolDetails {
    /// Return the exact volume count derived from the CRD's `i32` pool dimensions.
    ///
    /// Widening both operands before multiplication is lossless because every `i32 * i32`
    /// product fits in an `i64`. This preserves the real count instead of rejecting or clamping a
    /// valid CRD value.
    pub(crate) fn total_volumes(servers: i32, volumes_per_server: i32) -> i64 {
        i64::from(servers) * i64::from(volumes_per_server)
    }
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

#[cfg(test)]
mod tests {
    use super::PoolDetails;

    #[test]
    fn total_volumes_widens_before_multiplication() {
        assert_eq!(PoolDetails::total_volumes(i32::MAX, 2), 4_294_967_294);
        assert_eq!(
            PoolDetails::total_volumes(i32::MAX, i32::MAX),
            4_611_686_014_132_420_609
        );
    }
}
