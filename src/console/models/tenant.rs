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

/// Tenant 列表项
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListItem {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub created_at: Option<String>,
}

/// Pool 信息
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolInfo {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
}

/// Tenant 列表响应
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantListItem>,
}

/// Tenant 列表查询参数
#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct TenantListQuery {
    /// 按状态过滤（大小写不敏感）
    pub state: Option<String>,
}

/// Tenant 状态统计响应
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantStateCountsResponse {
    /// Tenant 总数
    pub total: u32,
    /// 各状态对应的数量，例如 Ready/Updating/Degraded/NotReady/Unknown
    pub counts: std::collections::BTreeMap<String, u32>,
}

/// Tenant 详情响应
#[derive(Debug, Serialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceInfo {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
}

/// Service 端口信息
#[derive(Debug, Serialize, ToSchema)]
pub struct ServicePort {
    pub name: String,
    pub port: i32,
    pub target_port: String,
}

/// SecurityContext for create/update (Pod runAsUser, runAsGroup, fsGroup, runAsNonRoot).
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSecurityContextRequest {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
}

/// 创建 Tenant 请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTenantRequest {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<CreatePoolRequest>,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub creds_secret: Option<String>,
    /// Optional Pod SecurityContext override (runAsUser, runAsGroup, fsGroup, runAsNonRoot).
    pub security_context: Option<CreateSecurityContextRequest>,
}

/// 创建 Pool 请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,
}

/// 删除 Tenant 响应
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteTenantResponse {
    pub success: bool,
    pub message: String,
}

/// 更新 Tenant 请求
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTenantRequest {
    /// 更新镜像版本
    pub image: Option<String>,

    /// 更新挂载路径
    pub mount_path: Option<String>,

    /// 更新环境变量
    pub env: Option<Vec<EnvVar>>,

    /// 更新凭证 Secret
    pub creds_secret: Option<String>,

    /// 更新 Pod 管理策略
    pub pod_management_policy: Option<String>,

    /// 更新镜像拉取策略
    pub image_pull_policy: Option<String>,

    /// 更新日志配置
    pub logging: Option<LoggingConfig>,
}

/// 环境变量
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct EnvVar {
    pub name: String,
    pub value: Option<String>,
}

/// 日志配置
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    pub log_type: String, // "stdout" | "emptyDir" | "persistent"
    pub volume_size: Option<String>,
    pub storage_class: Option<String>,
}

/// 更新 Tenant 响应
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateTenantResponse {
    pub success: bool,
    pub message: String,
    pub tenant: TenantListItem,
}

/// Tenant YAML 请求/响应
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TenantYAML {
    pub yaml: String,
}
