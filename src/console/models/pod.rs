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

/// Pod 列表项
#[derive(Debug, Serialize)]
pub struct PodListItem {
    pub name: String,
    pub pool: String,
    pub status: String,
    pub phase: String,
    pub node: Option<String>,
    pub ready: String,  // e.g., "1/1"
    pub restarts: i32,
    pub age: String,
    pub created_at: Option<String>,
}

/// Pod 列表响应
#[derive(Debug, Serialize)]
pub struct PodListResponse {
    pub pods: Vec<PodListItem>,
}

/// Pod 详情
#[derive(Debug, Serialize)]
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

/// Pod 状态
#[derive(Debug, Serialize)]
pub struct PodStatus {
    pub phase: String,
    pub conditions: Vec<PodCondition>,
    pub host_ip: Option<String>,
    pub pod_ip: Option<String>,
    pub start_time: Option<String>,
}

/// Pod 条件
#[derive(Debug, Serialize)]
pub struct PodCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition_time: Option<String>,
}

/// 容器信息
#[derive(Debug, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restart_count: i32,
    pub state: ContainerState,
}

/// 容器状态
#[derive(Debug, Serialize)]
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

/// Volume 信息
#[derive(Debug, Serialize)]
pub struct VolumeInfo {
    pub name: String,
    pub volume_type: String,
    pub claim_name: Option<String>,
}

/// 删除 Pod 响应
#[derive(Debug, Serialize)]
pub struct DeletePodResponse {
    pub success: bool,
    pub message: String,
}

/// 重启 Pod 请求
#[derive(Debug, Deserialize)]
pub struct RestartPodRequest {
    #[serde(default)]
    pub force: bool,
}

/// Pod 日志请求参数
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// 容器名称
    pub container: Option<String>,
    /// 尾部行数
    #[serde(default = "default_tail_lines")]
    pub tail_lines: i64,
    /// 是否跟随
    #[serde(default)]
    pub follow: bool,
    /// 显示时间戳
    #[serde(default)]
    pub timestamps: bool,
    /// 从指定时间开始（RFC3339 格式）
    pub since_time: Option<String>,
}

fn default_tail_lines() -> i64 {
    100
}
