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

/// Tenant 列表项
#[derive(Debug, Serialize)]
pub struct TenantListItem {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub created_at: Option<String>,
}

/// Pool 信息
#[derive(Debug, Serialize)]
pub struct PoolInfo {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
}

/// Tenant 列表响应
#[derive(Debug, Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantListItem>,
}

/// Tenant 详情响应
#[derive(Debug, Serialize)]
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

/// Service 信息
#[derive(Debug, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
}

/// Service 端口信息
#[derive(Debug, Serialize)]
pub struct ServicePort {
    pub name: String,
    pub port: i32,
    pub target_port: String,
}

/// 创建 Tenant 请求
#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<CreatePoolRequest>,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub creds_secret: Option<String>,
}

/// 创建 Pool 请求
#[derive(Debug, Deserialize)]
pub struct CreatePoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,
}

/// 删除 Tenant 响应
#[derive(Debug, Serialize)]
pub struct DeleteTenantResponse {
    pub success: bool,
    pub message: String,
}
