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

/// Single pod row in a tenant pod list
#[derive(Debug, Serialize, ToSchema)]
pub struct PodListItem {
    pub name: String,
    pub pool: String,
    pub status: String,
    pub phase: String,
    pub node: Option<String>,
    pub ready: String, // e.g., "1/1"
    pub restarts: i32,
    pub age: String,
    pub created_at: Option<String>,
}

/// Response listing pods
#[derive(Debug, Serialize, ToSchema)]
pub struct PodListResponse {
    pub pods: Vec<PodListItem>,
}

/// Full pod detail for the UI
#[derive(Debug, Serialize, ToSchema)]
pub struct PodDetails {
    pub name: String,
    pub namespace: String,
    pub pool: String,
    pub status: PodStatus,
    pub containers: Vec<ContainerInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub node: Option<String>,
    pub ip: Option<String>,
    pub labels: std::collections::BTreeMap<String, String>,
    pub annotations: std::collections::BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Phase, conditions, and networking summary
#[derive(Debug, Serialize, ToSchema)]
pub struct PodStatus {
    pub phase: String,
    pub conditions: Vec<PodCondition>,
    pub host_ip: Option<String>,
    pub pod_ip: Option<String>,
    pub start_time: Option<String>,
}

/// One Kubernetes pod condition
#[derive(Debug, Serialize, ToSchema)]
pub struct PodCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition_time: Option<String>,
}

/// Container status summary
#[derive(Debug, Serialize, ToSchema)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restart_count: i32,
    pub state: ContainerState,
}

/// Container lifecycle state
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "status")]
pub enum ContainerState {
    Running {
        started_at: Option<String>,
    },
    Waiting {
        reason: Option<String>,
        message: Option<String>,
    },
    Terminated {
        reason: Option<String>,
        exit_code: i32,
        finished_at: Option<String>,
    },
}

/// Volume mount / PVC reference
#[derive(Debug, Serialize, ToSchema)]
pub struct VolumeInfo {
    pub name: String,
    pub volume_type: String,
    pub claim_name: Option<String>,
}

/// Response after deleting a pod
#[derive(Debug, Serialize, ToSchema)]
pub struct DeletePodResponse {
    pub success: bool,
    pub message: String,
}

/// Optional flags when restarting a pod (delete/recreate)
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestartPodRequest {
    #[serde(default)]
    pub force: bool,
}

/// Query parameters for pod log streaming
#[derive(Debug, Deserialize, ToSchema)]
pub struct LogsQuery {
    /// Container name (if multi-container)
    pub container: Option<String>,
    /// Number of lines from the end of the log
    #[serde(default = "default_tail_lines")]
    pub tail_lines: i64,
    /// Stream new lines (follow)
    #[serde(default)]
    pub follow: bool,
    /// Prefix each line with a timestamp
    #[serde(default)]
    pub timestamps: bool,
    /// Only log lines after this instant (RFC3339)
    pub since_time: Option<String>,
}

fn default_tail_lines() -> i64 {
    100
}
