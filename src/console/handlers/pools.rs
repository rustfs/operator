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
use snafu::ResultExt;

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
        .context(error::KubeApiSnafu)?;

    // 获取所有 StatefulSets
    let ss_api: Api<appsv1::StatefulSet> = Api::namespaced(client, &namespace);
    let statefulsets = ss_api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", tenant_name)))
        .await
        .context(error::KubeApiSnafu)?;

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

/// 添加新的 Pool 到 Tenant
pub async fn add_pool(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AddPoolRequest>,
) -> Result<Json<AddPoolResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client, &namespace);

    // 获取当前 Tenant
    let mut tenant = tenant_api
        .get(&tenant_name)
        .await
        .context(error::KubeApiSnafu)?;

    // 验证 Pool 名称不重复
    if tenant.spec.pools.iter().any(|p| p.name == req.name) {
        return Err(Error::BadRequest {
            message: format!("Pool '{}' already exists", req.name),
        });
    }

    // 验证最小卷数要求 (servers * volumes_per_server >= 4)
    let total_volumes = req.servers * req.volumes_per_server;
    if total_volumes < 4 {
        return Err(Error::BadRequest {
            message: format!(
                "Pool must have at least 4 total volumes (got {} servers × {} volumes = {})",
                req.servers, req.volumes_per_server, total_volumes
            ),
        });
    }

    // 构建新的 Pool
    let new_pool = Pool {
        name: req.name.clone(),
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
                                req.storage_size.clone(),
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

    // 添加到 Tenant
    tenant.spec.pools.push(new_pool);

    // 更新 Tenant
    let updated_tenant = tenant_api
        .replace(&tenant_name, &Default::default(), &tenant)
        .await
        .context(error::KubeApiSnafu)?;

    Ok(Json(AddPoolResponse {
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
            created_at: updated_tenant
                .metadata
                .creation_timestamp
                .map(|ts| ts.0.to_rfc3339()),
        },
    }))
}

/// 删除 Pool
pub async fn delete_pool(
    Path((namespace, tenant_name, pool_name)): Path<(String, String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DeletePoolResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client, &namespace);

    // 获取当前 Tenant
    let mut tenant = tenant_api
        .get(&tenant_name)
        .await
        .context(error::KubeApiSnafu)?;

    // 检查是否为最后一个 Pool
    if tenant.spec.pools.len() == 1 {
        return Err(Error::BadRequest {
            message: "Cannot delete the last pool. Delete the entire Tenant instead.".to_string(),
        });
    }

    // 查找并移除 Pool
    let pool_index = tenant
        .spec
        .pools
        .iter()
        .position(|p| p.name == pool_name)
        .ok_or_else(|| Error::NotFound {
            resource: format!("Pool '{}'", pool_name),
        })?;

    tenant.spec.pools.remove(pool_index);

    // 更新 Tenant
    tenant_api
        .replace(&tenant_name, &Default::default(), &tenant)
        .await
        .context(error::KubeApiSnafu)?;

    Ok(Json(DeletePoolResponse {
        success: true,
        message: format!("Pool '{}' deleted successfully", pool_name),
        warning: Some(
            "The StatefulSet and PVCs will be deleted by the Operator. \
             Data may be lost if PVCs are not using a retain policy."
                .to_string(),
        ),
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
