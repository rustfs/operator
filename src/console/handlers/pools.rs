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
    pool::{Pool, SchedulingConfig, validate_pool_total_volumes},
    tenant::Tenant,
};

/// Validate a Kubernetes resource name (RFC 1123 subdomain: lowercase alphanumeric + hyphen, 1–63).
fn is_valid_k8s_name(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    for c in chars {
        if c != '-' && !c.is_ascii_alphanumeric() {
            return false;
        }
    }
    s.chars().last().is_some_and(|c| c != '-')
}

/// Loose validation for a Kubernetes resource quantity (e.g. `10Gi`, `100M`, `1`).
fn is_valid_k8s_quantity(s: &str) -> bool {
    if s.is_empty() || s.len() > 32 {
        return false;
    }
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // Allow: integer, decimal, or decimal + SI/binary suffix
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
            "Ei", "Pi", "Ti", "Gi", "Mi", "Ki", // binary SI
            "E", "P", "T", "G", "M", "K", // decimal SI
        ];
        if !VALID.contains(&suffix) {
            return false;
        }
    }
    true
}

/// Validate pool volume count (same rules as CRD CEL on [`Pool`] and [`validate_pool_total_volumes`]).
fn validate_pool_volumes(servers: i32, volumes_per_server: i32) -> Result<i32> {
    validate_pool_total_volumes(servers, volumes_per_server)
        .map_err(|message| Error::BadRequest { message })
}

/// List pools for a tenant (from spec + StatefulSet status).
pub async fn list_pools(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PoolListResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);

    // Load Tenant
    let tenant = tenant_api
        .get(&tenant_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant_name)))?;

    // List StatefulSets in namespace
    let ss_api: Api<appsv1::StatefulSet> = Api::namespaced(client, &namespace);
    let statefulsets = ss_api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", tenant_name)))
        .await
        .map_err(|e| {
            error::map_kube_error(e, format!("StatefulSets for tenant '{}'", tenant_name))
        })?;

    let mut pools_details = Vec::new();

    for pool in &tenant.spec.pools {
        let ss_name = format!("{}-{}", tenant_name, pool.name);

        // Match StatefulSet for this pool name
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

        // PVC template / storage size
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

/// Append a pool to `Tenant.spec.pools` with optimistic-lock retries.
pub async fn add_pool(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AddPoolRequest>,
) -> Result<Json<AddPoolResponse>> {
    let client = create_client(&claims).await?;
    let tenant_api: Api<Tenant> = Api::namespaced(client, &namespace);

    // Validate pool name and quantities
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

    // Build Pool spec
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

    // Optimistic loop: get -> validate -> push -> replace; retry on 409
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
                        created_at: t.metadata.creation_timestamp.map(|ts| ts.0.to_rfc3339()),
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

/// Remove a pool from the tenant with optimistic-lock retries.
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
                message: "Cannot delete the last pool. Delete the entire Tenant instead."
                    .to_string(),
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

/// Build a client using the session bearer token.
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
