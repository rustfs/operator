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

/// Pool 信息（扩展版）
#[derive(Debug, Serialize)]
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
    pub created_at: Option<String>,
}

/// Pool 列表响应
#[derive(Debug, Serialize)]
pub struct PoolListResponse {
    pub pools: Vec<PoolDetails>,
}

/// 添加 Pool 请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,

    // 可选的调度配置
    pub node_selector: Option<std::collections::BTreeMap<String, String>>,
    pub resources: Option<ResourceRequirements>,
}

/// 资源需求
#[derive(Debug, Deserialize, Serialize)]
pub struct ResourceRequirements {
    pub requests: Option<ResourceList>,
    pub limits: Option<ResourceList>,
}

/// 资源列表
#[derive(Debug, Deserialize, Serialize)]
pub struct ResourceList {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

/// 删除 Pool 响应
#[derive(Debug, Serialize)]
pub struct DeletePoolResponse {
    pub success: bool,
    pub message: String,
    pub warning: Option<String>,
}

/// Pool 添加响应
#[derive(Debug, Serialize)]
pub struct AddPoolResponse {
    pub success: bool,
    pub message: String,
    pub pool: PoolDetails,
}
