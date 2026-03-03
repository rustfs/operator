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

use axum::{Extension, Json, extract::Path};
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, ResourceExt, api::ListParams};

use crate::console::{
    error::{self, Error, Result},
    models::pool::*,
    state::Claims,
};
use crate::types::v1alpha1::{
    persistence::PersistenceConfig,
    pool::{Pool, SchedulingConfig},
    tenant::Tenant,
};

/// Kubernetes 资源名称校验（RFC 1123 子域名：小写字母数字、连字符，1-63 字符）
fn is_valid_k8s_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    for c in chars {
        if c != '-' && !c.is_ascii_alphanumeric() {
            return false;
        }
    }
    s.chars().last().map_or(false, |c| c != '-')
}

/// Kubernetes Quantity 格式校验（如 10Gi、100M、1）
fn is_valid_k8s_quantity(s: &str) -> bool {
    if s.is_empty() || s.len() > 32 {
        return false;
    }
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // 允许：纯数字、数字+小数、数字+后缀(E|P|T|G|M|K|Ei|Pi|Ti|Gi|Mi|Ki)
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b'.' || (bytes[i] as char).is_ascii_digit()) {
        i += 1;
    }
    if i == 0 {
        return false;
    }
    if i < bytes.len() {
        let suffix = std::str::from_utf8(&bytes[i..]).unwrap_or("");
        const VALID: &[&str] = &[
            "Ei", "Pi", "Ti", "Gi", "Mi", "Ki", // 二进制后缀优先
            "E", "P", "T", "G", "M", "K",       // 十进制后缀
        ];
        if !VALID.contains(&suffix) {
            return false;
        }
    }
    true
}

/// 校验 Pool 卷数（与 CRD 一致：2 server 至少 4 卷，3 server 至少 6 卷，其余至少 4 卷）
fn validate_pool_volumes(servers: i32, volumes_per_server: i32) -> Result<i32> {
    let total = servers * volumes_per_server;
    if servers <= 0 || volumes_per_server <= 0 {
        return Err(Error::BadRequest {
            message: "servers and volumes_per_server must be positive".to_string(),
        });
    }
    if servers == 2 && total < 4 {
        return Err(Error::BadRequest {
            message: "Pool with 2 servers must have at least 4 volumes in total".to_string(),
        });
    }
    if servers == 3 && total < 6 {
        return Err(Error::BadRequest {
            message: "Pool with 3 servers must have at least 6 volumes in total".to_string(),
        });
    }
    if total < 4 {
        return Err(Error::BadRequest {
            message: format!(
                "Pool must have at least 4 total volumes (got {} servers × {} volumes = {})",
                servers, volumes_per_server, total
            ),
        });
    }
    Ok(total)
}

/// 列出 Tenant 的所有 Pools
pub async fn list_pools(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PoolListResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);

    // 获取 Tenant
    let tenant = tenant_api
        .get(&tenant_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant_name)))?;

    // 获取所有 StatefulSets
    let ss_api: Api<appsv1::StatefulSet> = Api::namespaced(client, &namespace);
    let statefulsets = ss_api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", tenant_name)))
        .await
        .map_err(|e| error::map_kube_error(e, format!("StatefulSets for tenant '{}'", tenant_name)))?;

    let mut pools_details = Vec::new();

    for pool in &tenant.spec.pools {
        let ss_name = format!("{}-{}", tenant_name, pool.name);

        // 查找对应的 StatefulSet
        let ss = statefulsets
            .items
            .iter()
            .find(|ss| ss.name_any() == ss_name);

        let (replicas, ready_replicas, updated_replicas, current_revision, update_revision, state) =
            if let Some(ss) = ss {
                let status = ss.status.as_ref();
                let replicas = status.map(|s| s.replicas).unwrap_or(0);
                let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
                let updated = status.and_then(|s| s.updated_replicas).unwrap_or(0);
                let current_rev = status.and_then(|s| s.current_revision.clone());
                let update_rev = status.and_then(|s| s.update_revision.clone());

                let state = if ready == replicas && updated == replicas && replicas > 0 {
                    "Ready"
                } else if updated < replicas {
                    "Updating"
                } else if ready < replicas {
                    "Degraded"
                } else {
                    "NotReady"
                };

                (
                    replicas,
                    ready,
                    updated,
                    current_rev,
                    update_rev,
                    state.to_string(),
                )
            } else {
                (0, 0, 0, None, None, "NotCreated".to_string())
            };

        // 获取存储配置
        let storage_class = pool
            .persistence
            .volume_claim_template
            .as_ref()
            .and_then(|t| t.storage_class_name.clone());

        let volume_size = pool
            .persistence
            .volume_claim_template
            .as_ref()
            .and_then(|t| {
                t.resources.as_ref().and_then(|r| {
                    r.requests
                        .as_ref()
                        .and_then(|req| req.get("storage").map(|q| q.0.clone()))
                })
            });

        pools_details.push(PoolDetails {
            name: pool.name.clone(),
            servers: pool.servers,
            volumes_per_server: pool.persistence.volumes_per_server,
            total_volumes: pool.servers * pool.persistence.volumes_per_server,
            storage_class,
            volume_size,
            replicas,
            ready_replicas,
            updated_replicas,
            current_revision,
            update_revision,
            state,
            created_at: ss.and_then(|s| {
                s.metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|ts| ts.0.to_rfc3339())
            }),
        });
    }

    Ok(Json(PoolListResponse {
        pools: pools_details,
    }))
}

/// 添加新的 Pool 到 Tenant（乐观锁重试）
pub async fn add_pool(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AddPoolRequest>,
) -> Result<Json<AddPoolResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client, &namespace);

    // 输入校验（pool name 需符合 K8s 资源命名）
    let pool_name = req.name.trim();
    if !is_valid_k8s_name(pool_name) {
        return Err(Error::BadRequest {
            message: format!(
                "Invalid pool name '{}': must be 1-63 chars, lowercase alphanumeric and hyphens (RFC 1123)",
                req.name
            ),
        });
    }
    if !is_valid_k8s_quantity(req.storage_size.trim()) {
        return Err(Error::BadRequest {
            message: format!(
                "Invalid storage size '{}': must be a valid Kubernetes quantity (e.g. 10Gi, 100M)",
                req.storage_size
            ),
        });
    }
    let total_volumes = validate_pool_volumes(req.servers, req.volumes_per_server)?;

    // 构建新的 Pool
    let new_pool = Pool {
        name: pool_name.to_string(),
        servers: req.servers,
        persistence: PersistenceConfig {
            volumes_per_server: req.volumes_per_server,
            volume_claim_template: Some(corev1::PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                resources: Some(corev1::VolumeResourceRequirements {
                    requests: Some(
                        vec![(
                            "storage".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                req.storage_size.trim().to_string(),
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    ..Default::default()
                }),
                storage_class_name: req.storage_class.clone(),
                ..Default::default()
            }),
            path: None,
            labels: None,
            annotations: None,
        },
        scheduling: SchedulingConfig {
            node_selector: req.node_selector,
            resources: req.resources.map(|r| corev1::ResourceRequirements {
                requests: r.requests.map(|req| {
                    let mut map = std::collections::BTreeMap::new();
                    if let Some(cpu) = req.cpu {
                        map.insert(
                            "cpu".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(cpu),
                        );
                    }
                    if let Some(memory) = req.memory {
                        map.insert(
                            "memory".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(memory),
                        );
                    }
                    map
                }),
                limits: r.limits.map(|lim| {
                    let mut map = std::collections::BTreeMap::new();
                    if let Some(cpu) = lim.cpu {
                        map.insert(
                            "cpu".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(cpu),
                        );
                    }
                    if let Some(memory) = lim.memory {
                        map.insert(
                            "memory".to_string(),
                            k8s_openapi::apimachinery::pkg::api::resource::Quantity(memory),
                        );
                    }
                    map
                }),
                ..Default::default()
            }),
            affinity: None,
            tolerations: None,
            topology_spread_constraints: None,
            priority_class_name: None,
        },
    };

    // 乐观锁重试：get -> 校验 -> push -> replace，409 时重试
    const MAX_RETRIES: u32 = 3;
    let mut last_conflict = None;
    for _ in 0..MAX_RETRIES {
        let mut tenant = tenant_api
            .get(&tenant_name)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant_name)))?;

        if tenant.spec.pools.iter().any(|p| p.name == pool_name) {
            return Err(Error::BadRequest {
                message: format!("Pool '{}' already exists", req.name),
            });
        }

        tenant.spec.pools.push(new_pool.clone());

        match tenant_api
            .replace(&tenant_name, &Default::default(), &tenant)
            .await
        {
            Ok(t) => {
                return Ok(Json(AddPoolResponse {
                    success: true,
                    message: format!("Pool '{}' added successfully", req.name),
                    pool: PoolDetails {
                        name: req.name.clone(),
                        servers: req.servers,
                        volumes_per_server: req.volumes_per_server,
                        total_volumes,
                        storage_class: req.storage_class,
                        volume_size: Some(req.storage_size),
                        replicas: 0,
                        ready_replicas: 0,
                        updated_replicas: 0,
                        current_revision: None,
                        update_revision: None,
                        state: "Creating".to_string(),
                        created_at: t
                            .metadata
                            .creation_timestamp
                            .map(|ts| ts.0.to_rfc3339()),
                    },
                }));
            }
            Err(e) => {
                let mapped = error::map_kube_error(e, String::new());
                if !matches!(&mapped, Error::Conflict { .. }) {
                    return Err(mapped);
                }
                last_conflict = Some(mapped);
            }
        }
    }
    Err(last_conflict.unwrap_or_else(|| Error::Conflict {
        message: "Resource was modified by another request, please retry".to_string(),
    }))
}

/// 删除 Pool（乐观锁重试）
pub async fn delete_pool(
    Path((namespace, tenant_name, pool_name)): Path<(String, String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DeletePoolResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client, &namespace);

    const MAX_RETRIES: u32 = 3;
    let mut last_conflict = None;
    for _ in 0..MAX_RETRIES {
        let mut tenant = tenant_api
            .get(&tenant_name)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant_name)))?;

        if tenant.spec.pools.len() == 1 {
            return Err(Error::BadRequest {
                message: "Cannot delete the last pool. Delete the entire Tenant instead.".to_string(),
            });
        }

        let pool_index = tenant
            .spec
            .pools
            .iter()
            .position(|p| p.name == pool_name)
            .ok_or_else(|| Error::NotFound {
                resource: format!("Pool '{}'", pool_name),
            })?;

        tenant.spec.pools.remove(pool_index);

        match tenant_api
            .replace(&tenant_name, &Default::default(), &tenant)
            .await
        {
            Ok(_) => {
                return Ok(Json(DeletePoolResponse {
                    success: true,
                    message: format!("Pool '{}' deleted successfully", pool_name),
                    warning: Some(
                        "The StatefulSet and PVCs will be deleted by the Operator. \
                         Data may be lost if PVCs are not using a retain policy."
                            .to_string(),
                    ),
                }));
            }
            Err(e) => {
                let mapped = error::map_kube_error(e, String::new());
                if !matches!(&mapped, Error::Conflict { .. }) {
                    return Err(mapped);
                }
                last_conflict = Some(mapped);
            }
        }
    }

    Err(last_conflict.unwrap_or_else(|| Error::Conflict {
        message: "Resource was modified by another request, please retry".to_string(),
    }))
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
