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
use utoipa::ToSchema;

/// Node summary for the cluster API
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeInfo {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: String,
    pub memory_capacity: String,
    pub cpu_allocatable: String,
    pub memory_allocatable: String,
}

/// Response listing cluster nodes
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeListResponse {
    pub nodes: Vec<NodeInfo>,
}

/// Single namespace row in a list
#[derive(Debug, Serialize, ToSchema)]
pub struct NamespaceItem {
    pub name: String,
    pub status: String,
    pub created_at: Option<String>,
}

/// Response listing namespaces
#[derive(Debug, Serialize, ToSchema)]
pub struct NamespaceListResponse {
    pub namespaces: Vec<NamespaceItem>,
}

/// Request body to create a namespace
#[derive(Debug, serde::Deserialize, ToSchema)]
pub struct CreateNamespaceRequest {
    pub name: String,
}

/// Aggregated cluster capacity / allocatable resources
#[derive(Debug, Serialize, ToSchema)]
pub struct ClusterResourcesResponse {
    pub total_nodes: usize,
    pub total_cpu: String,
    pub total_memory: String,
    pub allocatable_cpu: String,
    pub allocatable_memory: String,
}
