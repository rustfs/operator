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

use serde::Serialize;

/// 节点信息
#[derive(Debug, Serialize)]
pub struct NodeInfo {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: String,
    pub memory_capacity: String,
    pub cpu_allocatable: String,
    pub memory_allocatable: String,
}

/// 节点列表响应
#[derive(Debug, Serialize)]
pub struct NodeListResponse {
    pub nodes: Vec<NodeInfo>,
}

/// Namespace 列表项
#[derive(Debug, Serialize)]
pub struct NamespaceItem {
    pub name: String,
    pub status: String,
    pub created_at: Option<String>,
}

/// Namespace 列表响应
#[derive(Debug, Serialize)]
pub struct NamespaceListResponse {
    pub namespaces: Vec<NamespaceItem>,
}

/// 创建 Namespace 请求
#[derive(Debug, serde::Deserialize)]
pub struct CreateNamespaceRequest {
    pub name: String,
}

/// 集群资源响应
#[derive(Debug, Serialize)]
pub struct ClusterResourcesResponse {
    pub total_nodes: usize,
    pub total_cpu: String,
    pub total_memory: String,
    pub allocatable_cpu: String,
    pub allocatable_memory: String,
}
