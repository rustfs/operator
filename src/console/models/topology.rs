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

/// Topology overview: cluster, namespaces, nodes
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyOverviewResponse {
    pub cluster: TopologyCluster,
    pub namespaces: Vec<TopologyNamespace>,
    pub nodes: Vec<TopologyNode>,
}

/// Cluster identity and version
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyCluster {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: TopologyClusterSummary,
}

/// Rolled-up capacity and tenant health counts
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyClusterSummary {
    pub nodes: usize,
    pub namespaces: usize,
    pub tenants: usize,
    pub unhealthy_tenants: usize,
    pub total_cpu: String,
    pub total_memory: String,
    pub allocatable_cpu: String,
    pub allocatable_memory: String,
}

/// Namespace with nested tenants
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyNamespace {
    pub name: String,
    pub tenant_count: usize,
    pub unhealthy_tenant_count: usize,
    pub tenants: Vec<TopologyTenant>,
}

/// Tenant node in the topology tree
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyTenant {
    pub name: String,
    pub namespace: String,
    pub state: String,
    pub created_at: Option<String>,
    pub summary: TopologyTenantSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pools: Option<Vec<TopologyPool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pods: Option<Vec<TopologyPod>>,
}

/// Short tenant stats for topology cards
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyTenantSummary {
    pub pool_count: usize,
    pub replicas: i32,
    pub capacity: String,
    pub capacity_bytes: i64,
    pub endpoint: Option<String>,
    pub console_endpoint: Option<String>,
}

/// Pool row under a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyPool {
    pub name: String,
    pub state: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub replicas: i32,
    pub capacity: String,
}

/// Pod row under a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyPod {
    pub name: String,
    pub pool: String,
    pub phase: String,
    pub ready: String,
    pub node: Option<String>,
}

/// Node row for topology sidebar
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyNode {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub cpu_capacity: String,
    pub memory_capacity: String,
    pub cpu_allocatable: String,
    pub memory_allocatable: String,
}
