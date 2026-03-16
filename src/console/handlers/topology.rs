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

use crate::console::{
    error::{self, Error, Result},
    handlers::cluster::{
        format_cpu_from_millicores, format_memory_from_bytes, parse_cpu_to_millicores,
        parse_memory_to_bytes,
    },
    models::topology::*,
    state::Claims,
};
use crate::types::v1alpha1::{status::pool::PoolState, tenant::Tenant};
use axum::{Extension, Json};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, ResourceExt, api::ListParams};
use std::collections::BTreeMap;

/// 获取集群拓扑总览
pub async fn get_topology_overview(
    Extension(claims): Extension<Claims>,
) -> Result<Json<TopologyOverviewResponse>> {
    let client = create_client(&claims).await?;

    // 并行获取 nodes, tenants, pods
    let node_api: Api<corev1::Node> = Api::all(client.clone());
    let tenant_api: Api<Tenant> = Api::all(client.clone());
    let pod_api: Api<corev1::Pod> = Api::all(client.clone());

    let node_params = ListParams::default();
    let tenant_params = ListParams::default();
    let pod_params = ListParams::default().labels("rustfs.tenant");

    let (nodes_result, tenants_result, pods_result) = tokio::join!(
        node_api.list(&node_params),
        tenant_api.list(&tenant_params),
        pod_api.list(&pod_params),
    );

    let k8s_nodes = nodes_result.map_err(|e| error::map_kube_error(e, "Nodes"))?;
    let k8s_tenants = tenants_result.map_err(|e| error::map_kube_error(e, "Tenants"))?;
    let k8s_pods = pods_result.map_err(|e| error::map_kube_error(e, "Pods"))?;

    // 构建节点列表 + 集群资源汇总
    let mut total_cpu_m: i64 = 0;
    let mut total_mem_b: i64 = 0;
    let mut alloc_cpu_m: i64 = 0;
    let mut alloc_mem_b: i64 = 0;

    let nodes: Vec<TopologyNode> = k8s_nodes
        .items
        .iter()
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
                            k.strip_prefix("node-role.kubernetes.io/")
                                .map(|r| r.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();

            let (cpu_cap, mem_cap, cpu_alloc, mem_alloc) = node
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

            // 累加集群资源
            total_cpu_m += parse_cpu_to_millicores(&cpu_cap);
            total_mem_b += parse_memory_to_bytes(&mem_cap);
            alloc_cpu_m += parse_cpu_to_millicores(&cpu_alloc);
            alloc_mem_b += parse_memory_to_bytes(&mem_alloc);

            TopologyNode {
                name: node.name_any(),
                status,
                roles,
                cpu_capacity: cpu_cap,
                memory_capacity: mem_cap,
                cpu_allocatable: cpu_alloc,
                memory_allocatable: mem_alloc,
            }
        })
        .collect();

    // 按 (namespace, tenant_name) 索引 pods
    let mut pod_index: BTreeMap<(String, String), Vec<TopologyPod>> = BTreeMap::new();
    for pod in &k8s_pods.items {
        let labels = pod.metadata.labels.as_ref();
        let tenant_name = labels
            .and_then(|l| l.get("rustfs.tenant"))
            .cloned()
            .unwrap_or_default();
        let pool = labels
            .and_then(|l| l.get("rustfs.pool"))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let ns = pod.namespace().unwrap_or_default();

        let phase = pod
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        let (ready_count, total_count) = pod
            .status
            .as_ref()
            .and_then(|s| s.container_statuses.as_ref())
            .map(|cs| (cs.iter().filter(|c| c.ready).count(), cs.len()))
            .unwrap_or((0, 0));

        let node = pod.spec.as_ref().and_then(|s| s.node_name.clone());

        let key = (ns, tenant_name);
        pod_index.entry(key).or_default().push(TopologyPod {
            name: pod.name_any(),
            pool,
            phase,
            ready: format!("{}/{}", ready_count, total_count),
            node,
        });
    }

    // 按 namespace 分组 tenants
    let mut ns_map: BTreeMap<String, Vec<&Tenant>> = BTreeMap::new();
    for t in &k8s_tenants.items {
        let ns = t.namespace().unwrap_or_default();
        ns_map.entry(ns).or_default().push(t);
    }

    let mut total_unhealthy: usize = 0;

    let namespaces: Vec<TopologyNamespace> = ns_map
        .into_iter()
        .map(|(ns_name, tenants)| {
            let mut unhealthy_count: usize = 0;

            let tenant_items: Vec<TopologyTenant> = tenants
                .into_iter()
                .map(|t| {
                    let name = t.name_any();
                    let namespace = t.namespace().unwrap_or_default();
                    let state = t
                        .status
                        .as_ref()
                        .map(|s| s.current_state.clone())
                        .unwrap_or_else(|| "Unknown".to_string());

                    if !is_healthy_state(&state) {
                        unhealthy_count += 1;
                    }

                    let created_at = t
                        .metadata
                        .creation_timestamp
                        .as_ref()
                        .map(|ts| ts.0.to_rfc3339());

                    // Pool 信息
                    let pools: Vec<TopologyPool> = t
                        .spec
                        .pools
                        .iter()
                        .map(|spec_pool| {
                            let pool_status = t
                                .status
                                .as_ref()
                                .and_then(|s| {
                                    s.pools.iter().find(|sp| {
                                        sp.ss_name.contains(&spec_pool.name)
                                    })
                                });

                            let pool_state = pool_status
                                .map(|ps| map_pool_state(&ps.state))
                                .unwrap_or_else(|| "Unknown".to_string());

                            let replicas = pool_status
                                .and_then(|ps| ps.replicas)
                                .unwrap_or(spec_pool.servers);

                            let per_volume_bytes = get_per_volume_bytes(
                                &spec_pool.persistence,
                            );
                            let pool_capacity_bytes = (spec_pool.servers as i64)
                                * (spec_pool.persistence.volumes_per_server as i64)
                                * per_volume_bytes;

                            TopologyPool {
                                name: spec_pool.name.clone(),
                                state: pool_state,
                                servers: spec_pool.servers,
                                volumes_per_server: spec_pool.persistence.volumes_per_server,
                                replicas,
                                capacity: format_storage_bytes(pool_capacity_bytes),
                            }
                        })
                        .collect();

                    // Tenant 摘要
                    let pool_count = pools.len();
                    let total_replicas: i32 = pools.iter().map(|p| p.replicas).sum();
                    let total_capacity_bytes: i64 = t
                        .spec
                        .pools
                        .iter()
                        .map(|p| {
                            let per_vol = get_per_volume_bytes(&p.persistence);
                            (p.servers as i64) * (p.persistence.volumes_per_server as i64) * per_vol
                        })
                        .sum();

                    let endpoint = Some(format!(
                        "http://{}-io.{}.svc:9000",
                        name, namespace
                    ));
                    let console_endpoint = Some(format!(
                        "http://{}-console.{}.svc:9001",
                        name, namespace
                    ));

                    // 匹配 pods
                    let key = (namespace.clone(), name.clone());
                    let tenant_pods = pod_index.remove(&key);

                    TopologyTenant {
                        name,
                        namespace,
                        state,
                        created_at,
                        summary: TopologyTenantSummary {
                            pool_count,
                            replicas: total_replicas,
                            capacity: format_storage_bytes(total_capacity_bytes),
                            capacity_bytes: total_capacity_bytes,
                            endpoint,
                            console_endpoint,
                        },
                        pools: Some(pools),
                        pods: tenant_pods,
                    }
                })
                .collect();

            total_unhealthy += unhealthy_count;

            TopologyNamespace {
                name: ns_name,
                tenant_count: tenant_items.len(),
                unhealthy_tenant_count: unhealthy_count,
                tenants: tenant_items,
            }
        })
        .collect();

    // 集群信息
    let cluster = TopologyCluster {
        id: "rustfs-cluster".to_string(),
        name: std::env::var("CLUSTER_NAME")
            .unwrap_or_else(|_| "RustFS Cluster".to_string()),
        version: get_cluster_version(&client).await,
        summary: TopologyClusterSummary {
            nodes: nodes.len(),
            namespaces: namespaces.len(),
            tenants: k8s_tenants.items.len(),
            unhealthy_tenants: total_unhealthy,
            total_cpu: format_cpu_from_millicores(total_cpu_m),
            total_memory: format_memory_from_bytes(total_mem_b),
            allocatable_cpu: format_cpu_from_millicores(alloc_cpu_m),
            allocatable_memory: format_memory_from_bytes(alloc_mem_b),
        },
    };

    Ok(Json(TopologyOverviewResponse {
        cluster,
        namespaces,
        nodes,
    }))
}

/// 判断 Tenant 状态是否健康
fn is_healthy_state(state: &str) -> bool {
    matches!(state, "Ready" | "Initialized")
}

/// 将 PoolState 映射到前端状态字符串
fn map_pool_state(state: &PoolState) -> String {
    match state {
        PoolState::Created | PoolState::Initialized | PoolState::RolloutComplete => {
            "Ready".to_string()
        }
        PoolState::Updating => "Updating".to_string(),
        PoolState::Degraded | PoolState::RolloutFailed => "Degraded".to_string(),
        PoolState::NotCreated => "NotReady".to_string(),
    }
}

/// 从 PersistenceConfig 获取每个 volume 的字节数
fn get_per_volume_bytes(
    persistence: &crate::types::v1alpha1::persistence::PersistenceConfig,
) -> i64 {
    const DEFAULT_BYTES: i64 = 10 * 1024 * 1024 * 1024; // 10Gi

    persistence
        .volume_claim_template
        .as_ref()
        .and_then(|vct| vct.resources.as_ref())
        .and_then(|res| res.requests.as_ref())
        .and_then(|req| req.get("storage"))
        .map(|q| parse_memory_to_bytes(&q.0))
        .unwrap_or(DEFAULT_BYTES)
}

/// 将字节数格式化为可读存储字符串（优先 TiB, GiB）
fn format_storage_bytes(b: i64) -> String {
    const TIB: i64 = 1024 * 1024 * 1024 * 1024;
    const GIB: i64 = 1024 * 1024 * 1024;
    const MIB: i64 = 1024 * 1024;

    if b <= 0 {
        return "0".to_string();
    }
    if b >= TIB && b % TIB == 0 {
        format!("{} TiB", b / TIB)
    } else if b >= TIB {
        format!("{:.1} TiB", b as f64 / TIB as f64)
    } else if b >= GIB && b % GIB == 0 {
        format!("{} GiB", b / GIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b as f64 / GIB as f64)
    } else if b >= MIB && b % MIB == 0 {
        format!("{} MiB", b / MIB)
    } else {
        format!("{} B", b)
    }
}

/// 获取 Kubernetes 集群版本
async fn get_cluster_version(client: &Client) -> String {
    match client.apiserver_version().await {
        Ok(info) => format!("v{}.{}", info.major, info.minor),
        Err(_) => "unknown".to_string(),
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
