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

use axum::{Extension, Json};
use k8s_openapi::api::core::v1 as corev1;
use kube::{api::ListParams, Api, Client, ResourceExt};
use snafu::ResultExt;

use crate::console::{
    error::{self, Error, Result},
    models::cluster::*,
    state::Claims,
};

/// 列出所有节点
pub async fn list_nodes(Extension(claims): Extension<Claims>) -> Result<Json<NodeListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Node> = Api::all(client);

    let nodes = api
        .list(&ListParams::default())
        .await
        .context(error::KubeApiSnafu)?;

    let items: Vec<NodeInfo> = nodes
        .items
        .into_iter()
        .map(|node| {
            let status = node
                .status
                .as_ref()
                .and_then(|s| {
                    s.conditions.as_ref().and_then(|conds| {
                        conds.iter().find(|c| c.type_ == "Ready").map(|c| {
                            if c.status == "True" {
                                "Ready"
                            } else {
                                "NotReady"
                            }
                        })
                    })
                })
                .unwrap_or("Unknown")
                .to_string();

            let roles: Vec<String> = node
                .metadata
                .labels
                .as_ref()
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|(k, _)| {
                            if k.starts_with("node-role.kubernetes.io/") {
                                Some(k.trim_start_matches("node-role.kubernetes.io/").to_string())
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let (cpu_capacity, memory_capacity, cpu_allocatable, memory_allocatable) = node
                .status
                .as_ref()
                .map(|s| {
                    (
                        s.capacity
                            .as_ref()
                            .and_then(|c| c.get("cpu"))
                            .map(|q| q.0.clone())
                            .unwrap_or_default(),
                        s.capacity
                            .as_ref()
                            .and_then(|c| c.get("memory"))
                            .map(|q| q.0.clone())
                            .unwrap_or_default(),
                        s.allocatable
                            .as_ref()
                            .and_then(|a| a.get("cpu"))
                            .map(|q| q.0.clone())
                            .unwrap_or_default(),
                        s.allocatable
                            .as_ref()
                            .and_then(|a| a.get("memory"))
                            .map(|q| q.0.clone())
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or_default();

            NodeInfo {
                name: node.name_any(),
                status,
                roles,
                cpu_capacity,
                memory_capacity,
                cpu_allocatable,
                memory_allocatable,
            }
        })
        .collect();

    Ok(Json(NodeListResponse { nodes: items }))
}

/// 列出所有 Namespaces
pub async fn list_namespaces(
    Extension(claims): Extension<Claims>,
) -> Result<Json<NamespaceListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Namespace> = Api::all(client);

    let namespaces = api
        .list(&ListParams::default())
        .await
        .context(error::KubeApiSnafu)?;

    let items: Vec<NamespaceItem> = namespaces
        .items
        .into_iter()
        .map(|ns| NamespaceItem {
            name: ns.name_any(),
            status: ns
                .status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            created_at: ns
                .metadata
                .creation_timestamp
                .map(|ts| ts.0.to_rfc3339()),
        })
        .collect();

    Ok(Json(NamespaceListResponse { namespaces: items }))
}

/// 创建 Namespace
pub async fn create_namespace(
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateNamespaceRequest>,
) -> Result<Json<NamespaceItem>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Namespace> = Api::all(client);

    let ns = corev1::Namespace {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(req.name.clone()),
            ..Default::default()
        },
        ..Default::default()
    };

    let created = api
        .create(&Default::default(), &ns)
        .await
        .context(error::KubeApiSnafu)?;

    Ok(Json(NamespaceItem {
        name: created.name_any(),
        status: created
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Active".to_string()),
        created_at: created
            .metadata
            .creation_timestamp
            .map(|ts| ts.0.to_rfc3339()),
    }))
}

/// 获取集群资源摘要
pub async fn get_cluster_resources(
    Extension(claims): Extension<Claims>,
) -> Result<Json<ClusterResourcesResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Node> = Api::all(client);

    let nodes = api
        .list(&ListParams::default())
        .await
        .context(error::KubeApiSnafu)?;

    let total_nodes = nodes.items.len();

    // 简化统计 (实际生产中需要更精确的计算)
    let (total_cpu, total_memory, allocatable_cpu, allocatable_memory) = nodes
        .items
        .iter()
        .fold(
            (String::new(), String::new(), String::new(), String::new()),
            |acc, node| {
                // 这里简化处理,实际需要累加 Quantity
                if let Some(status) = &node.status {
                    if let Some(capacity) = &status.capacity {
                        // 实际应该累加,这里仅作演示
                        let cpu = capacity.get("cpu").map(|q| q.0.clone()).unwrap_or_default();
                        let mem = capacity.get("memory").map(|q| q.0.clone()).unwrap_or_default();
                        return (cpu, mem, acc.2, acc.3);
                    }
                }
                acc
            },
        );

    Ok(Json(ClusterResourcesResponse {
        total_nodes,
        total_cpu,
        total_memory,
        allocatable_cpu,
        allocatable_memory,
    }))
}

/// 创建 Kubernetes 客户端
async fn create_client(claims: &Claims) -> Result<Client> {
    let mut config = kube::Config::infer().await.map_err(|e| Error::InternalServer {
        message: format!("Failed to load kubeconfig: {}", e),
    })?;

    config.auth_info.token = Some(claims.k8s_token.clone().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}
