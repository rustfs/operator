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

use kube::KubeSchema;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use strum::Display;

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    /// Pool name from Tenant spec. Optional for backward compatibility with older status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Name of the StatefulSet for this pool
    pub ss_name: String,

    /// Current state of the pool
    pub state: PoolState,

    /// Lifecycle state of the pool, separate from StatefulSet rollout state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<PoolLifecycleState>,

    /// Workload rollout state of this pool. Mirrors `state` for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_state: Option<PoolState>,

    /// Decommission progress and cleanup status for this pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decommission: Option<PoolDecommissionStatus>,

    /// Total number of non-terminated pods targeted by this pool's StatefulSet
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,

    /// Number of pods with Ready condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_replicas: Option<i32>,

    /// Number of pods with current revision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_replicas: Option<i32>,

    /// Number of pods with updated revision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_replicas: Option<i32>,

    /// Current revision hash of the StatefulSet
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,

    /// Update revision hash of the StatefulSet (different from current during rollout)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_revision: Option<String>,

    /// Last time the pool status was updated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_time: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display, PartialEq, Eq)]
pub enum PoolState {
    #[strum(to_string = "PoolCreated")]
    Created,

    #[strum(to_string = "PoolNotCreated")]
    NotCreated,

    #[strum(to_string = "PoolInitialized")]
    Initialized,

    #[strum(to_string = "PoolUpdating")]
    Updating,

    #[strum(to_string = "PoolRolloutComplete")]
    RolloutComplete,

    #[strum(to_string = "PoolRolloutFailed")]
    RolloutFailed,

    #[strum(to_string = "PoolDegraded")]
    Degraded,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PoolLifecycleState {
    #[strum(to_string = "Active")]
    Active,

    #[strum(to_string = "Decommissioning")]
    Decommissioning,

    #[strum(to_string = "Decommissioned")]
    Decommissioned,

    #[strum(to_string = "DecommissionCanceled")]
    DecommissionCanceled,

    #[strum(to_string = "DecommissionFailed")]
    DecommissionFailed,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolDecommissionStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustfs_pool_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_set_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PoolDecommissionPhase>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_poll_time: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<PoolDecommissionProgress>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<PoolDecommissionCleanupStatus>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<PoolDecommissionLastError>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PoolDecommissionPhase {
    #[strum(to_string = "Pending")]
    Pending,

    #[strum(to_string = "Running")]
    Running,

    #[strum(to_string = "Complete")]
    Complete,

    #[strum(to_string = "Canceled")]
    Canceled,

    #[strum(to_string = "Failed")]
    Failed,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolDecommissionProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects_migrated: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_migrated: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects_failed: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_failed: Option<i64>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolDecommissionCleanupStatus {
    pub state: PoolDecommissionCleanupState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stateful_set_deleted_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvc_retention_policy: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PoolDecommissionCleanupState {
    #[strum(to_string = "Pending")]
    Pending,

    #[strum(to_string = "StatefulSetDeleting")]
    StatefulSetDeleting,

    #[strum(to_string = "PvcRetained")]
    PvcRetained,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolDecommissionLastError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl JsonSchema for PoolState {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("State")
    }
    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed(concat!(module_path!(), "::", "State"))
    }
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema! {
            {"type": "string"}
        }
    }
}
