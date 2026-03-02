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
use kube::{Api, Client, ResourceExt, api::ListParams};
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
            created_at: ns.metadata.creation_timestamp.map(|ts| ts.0.to_rfc3339()),
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

    // 累加所有节点的 capacity 与 allocatable（数值累加后格式化）
    let (total_cpu_millicores, total_memory_bytes, alloc_cpu_millicores, alloc_memory_bytes) =
        nodes.items.iter().fold(
            (0i64, 0i64, 0i64, 0i64),
            |(cap_cpu, cap_mem, alloc_cpu, alloc_mem), node| {
                let (dcap_cpu, dcap_mem, dalloc_cpu, dalloc_mem) = node
                    .status
                    .as_ref()
                    .map(|s| {
                        (
                            s.capacity
                                .as_ref()
                                .and_then(|c| c.get("cpu"))
                                .map(|q| parse_cpu_to_millicores(&q.0))
                                .unwrap_or(0),
                            s.capacity
                                .as_ref()
                                .and_then(|c| c.get("memory"))
                                .map(|q| parse_memory_to_bytes(&q.0))
                                .unwrap_or(0),
                            s.allocatable
                                .as_ref()
                                .and_then(|a| a.get("cpu"))
                                .map(|q| parse_cpu_to_millicores(&q.0))
                                .unwrap_or(0),
                            s.allocatable
                                .as_ref()
                                .and_then(|a| a.get("memory"))
                                .map(|q| parse_memory_to_bytes(&q.0))
                                .unwrap_or(0),
                        )
                    })
                    .unwrap_or((0, 0, 0, 0));
                (
                    cap_cpu + dcap_cpu,
                    cap_mem + dcap_mem,
                    alloc_cpu + dalloc_cpu,
                    alloc_mem + dalloc_mem,
                )
            },
        );

    let total_cpu = format_cpu_from_millicores(total_cpu_millicores);
    let total_memory = format_memory_from_bytes(total_memory_bytes);
    let allocatable_cpu = format_cpu_from_millicores(alloc_cpu_millicores);
    let allocatable_memory = format_memory_from_bytes(alloc_memory_bytes);

    Ok(Json(ClusterResourcesResponse {
        total_nodes,
        total_cpu,
        total_memory,
        allocatable_cpu,
        allocatable_memory,
    }))
}

/// 将 Kubernetes CPU Quantity 解析为毫核 (millicores)。
/// 支持 "1"（核）、"1000m"、"500m" 等格式。
fn parse_cpu_to_millicores(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    if let Some(rest) = s.strip_suffix('n') {
        if let Ok(n) = rest.trim().parse::<f64>() {
            return (n / 1_000_000.0) as i64;
        }
    }
    if let Some(rest) = s.strip_suffix('u') {
        if let Ok(n) = rest.trim().parse::<f64>() {
            return (n / 1000.0) as i64;
        }
    }
    if let Some(rest) = s.strip_suffix('m') {
        if let Ok(n) = rest.trim().parse::<f64>() {
            return n as i64;
        }
    }
    if let Ok(n) = s.parse::<f64>() {
        return (n * 1000.0) as i64;
    }
    0
}

/// 将毫核格式化为 CPU 字符串（如 "8" 或 "500m"）。
fn format_cpu_from_millicores(m: i64) -> String {
    if m == 0 {
        return "0".to_string();
    }
    if m % 1000 == 0 {
        (m / 1000).to_string()
    } else {
        format!("{}m", m)
    }
}

/// 将 Kubernetes Memory Quantity 解析为字节。
/// 支持 "1Gi"、"1G"、"1024Mi"、"1Ki" 等格式。
fn parse_memory_to_bytes(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let mut num_end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' {
            num_end = i + c.len_utf8();
        } else {
            break;
        }
    }
    let num_str = &s[..num_end];
    let Ok(n) = num_str.parse::<f64>() else {
        return 0;
    };
    let suffix = s[num_end..].trim();
    let multiplier: i64 = match suffix {
        "Ei" => 1_024_i64.pow(6),
        "Pi" => 1_024_i64.pow(5),
        "Ti" => 1_024_i64.pow(4),
        "Gi" => 1_024_i64.pow(3),
        "Mi" => 1_024_i64.pow(2),
        "Ki" => 1_024,
        "E" => 1_000_000_000_000_000_000,
        "P" => 1_000_000_000_000_000,
        "T" => 1_000_000_000_000,
        "G" => 1_000_000_000,
        "M" => 1_000_000,
        "k" => 1_000,
        _ => return (n as i64).max(0),
    };
    (n * multiplier as f64) as i64
}

/// 将字节格式化为可读内存字符串（优先 Gi）。
fn format_memory_from_bytes(b: i64) -> String {
    const GIB: i64 = 1024 * 1024 * 1024;
    const MIB: i64 = 1024 * 1024;
    const KIB: i64 = 1024;
    if b <= 0 {
        return "0".to_string();
    }
    if b >= GIB && b % GIB == 0 {
        format!("{}Gi", b / GIB)
    } else if b >= GIB {
        format!("{:.2}Gi", b as f64 / GIB as f64)
    } else if b >= MIB && b % MIB == 0 {
        format!("{}Mi", b / MIB)
    } else if b >= MIB {
        format!("{:.2}Mi", b as f64 / MIB as f64)
    } else if b >= KIB && b % KIB == 0 {
        format!("{}Ki", b / KIB)
    } else {
        format!("{}", b)
    }
}

/// 创建 Kubernetes 客户端
async fn create_client(claims: &Claims) -> Result<Client> {
    let mut config = kube::Config::infer()
        .await
        .map_err(|e| Error::InternalServer {
            message: format!("Failed to load kubeconfig: {}", e),
        })?;

    config.auth_info.token = Some(claims.k8s_token.clone().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}
