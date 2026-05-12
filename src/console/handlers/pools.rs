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

use axum::{Extension, Json, extract::Path, http::StatusCode};
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, ResourceExt, api::ListParams};

use crate::console::{
    error::{self, Error, Result},
    models::{common::ConsoleErrorDetails, pool::*},
    state::Claims,
};
use crate::types::v1alpha1::{
    persistence::PersistenceConfig,
    pool::{Pool, SchedulingConfig, validate_pool_total_volumes},
    status::next_actions_for_reason,
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

#[derive(Debug, PartialEq, Eq)]
enum PoolDeleteDecision {
    RemoveFromSpec,
    RequiresDecommission { reason: &'static str },
}

const REASON_DECOMMISSION_REQUIRED: &str = "DecommissionRequired";
const REASON_OBSERVATION_STALE: &str = "ObservedGenerationStale";

fn action_strings(reason: &str) -> Vec<String> {
    next_actions_for_reason(reason)
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn ensure_pool_delete_does_not_remove_last_pool(total_pools: usize) -> Result<()> {
    if total_pools == 1 {
        return Err(Error::BadRequest {
            message: "Cannot delete the last pool. Delete the entire Tenant instead.".to_string(),
        });
    }

    Ok(())
}

fn classify_pool_delete(
    total_pools: usize,
    managed_statefulset_exists: bool,
    recorded_pool_status_exists: bool,
) -> Result<PoolDeleteDecision> {
    ensure_pool_delete_does_not_remove_last_pool(total_pools)?;

    if managed_statefulset_exists || recorded_pool_status_exists {
        return Ok(PoolDeleteDecision::RequiresDecommission {
            reason: REASON_DECOMMISSION_REQUIRED,
        });
    }

    Ok(PoolDeleteDecision::RemoveFromSpec)
}

fn is_pool_observation_current(tenant: &Tenant) -> bool {
    tenant
        .status
        .as_ref()
        .and_then(|status| status.observed_generation)
        .zip(tenant.metadata.generation)
        .is_some_and(|(observed_generation, generation)| observed_generation >= generation)
}

fn is_managed_pool_statefulset(
    tenant: &Tenant,
    statefulset: &appsv1::StatefulSet,
    pool_name: &str,
) -> bool {
    let tenant_name = tenant.name_any();
    let expected_name = format!("{}-{}", tenant_name, pool_name);
    if statefulset.name_any() != expected_name {
        return false;
    }

    let labels_match = statefulset.metadata.labels.as_ref().is_some_and(|labels| {
        labels
            .get("rustfs.tenant")
            .is_some_and(|value| value == &tenant_name)
            && labels
                .get("rustfs.pool")
                .is_some_and(|value| value == pool_name)
    });
    if !labels_match {
        return false;
    }

    let Some(owner_references) = statefulset.metadata.owner_references.as_ref() else {
        return false;
    };
    owner_references.iter().any(|owner| {
        owner.kind == "Tenant"
            && owner.name == tenant_name
            && match tenant.metadata.uid.as_deref() {
                Some(uid) => owner.uid == uid,
                None => true,
            }
    })
}

fn pool_delete_requires_decommission_error(
    namespace: &str,
    tenant_name: &str,
    pool_name: &str,
    reason: &'static str,
) -> Error {
    Error::ActionRequired {
        status: StatusCode::CONFLICT,
        code: "PoolDeleteRequiresDecommission".to_string(),
        reason: reason.to_string(),
        message: format!(
            "Pool '{}' already has managed resources and must be decommissioned before removal.",
            pool_name
        ),
        next_actions: action_strings(reason),
        details: Some(Box::new(ConsoleErrorDetails {
            namespace: Some(namespace.to_string()),
            tenant: Some(tenant_name.to_string()),
            resource: Some(pool_name.to_string()),
        })),
    }
}

fn pool_delete_observation_pending_error(
    namespace: &str,
    tenant_name: &str,
    pool_name: &str,
) -> Error {
    Error::ActionRequired {
        status: StatusCode::CONFLICT,
        code: "PoolDeleteObservationPending".to_string(),
        reason: REASON_OBSERVATION_STALE.to_string(),
        message: format!(
            "Pool '{}' cannot be removed until the operator observes the current Tenant generation.",
            pool_name
        ),
        next_actions: action_strings(REASON_OBSERVATION_STALE),
        details: Some(Box::new(ConsoleErrorDetails {
            namespace: Some(namespace.to_string()),
            tenant: Some(tenant_name.to_string()),
            resource: Some(pool_name.to_string()),
        })),
    }
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
    let tenant_api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);
    let ss_api: Api<appsv1::StatefulSet> = Api::namespaced(client, &namespace);

    const MAX_RETRIES: u32 = 3;
    let mut last_conflict = None;
    for _ in 0..MAX_RETRIES {
        let mut tenant = tenant_api
            .get(&tenant_name)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant_name)))?;

        let pool_index = tenant
            .spec
            .pools
            .iter()
            .position(|p| p.name == pool_name)
            .ok_or_else(|| Error::NotFound {
                resource: format!("Pool '{}'", pool_name),
            })?;

        ensure_pool_delete_does_not_remove_last_pool(tenant.spec.pools.len())?;

        if !is_pool_observation_current(&tenant) {
            return Err(pool_delete_observation_pending_error(
                &namespace,
                &tenant_name,
                &pool_name,
            ));
        }

        let ss_name = format!("{}-{}", tenant_name, pool_name);
        let managed_statefulset_exists = match ss_api.get(&ss_name).await {
            Ok(statefulset) => is_managed_pool_statefulset(&tenant, &statefulset, &pool_name),
            Err(kube::Error::Api(api_error)) if api_error.code == 404 => false,
            Err(e) => {
                return Err(error::map_kube_error(
                    e,
                    format!("StatefulSet '{}'", ss_name),
                ));
            }
        };

        let recorded_pool_status_exists = tenant.status.as_ref().is_some_and(|status| {
            status
                .pools
                .iter()
                .any(|pool_status| pool_status.ss_name == ss_name)
        });

        match classify_pool_delete(
            tenant.spec.pools.len(),
            managed_statefulset_exists,
            recorded_pool_status_exists,
        )? {
            PoolDeleteDecision::RemoveFromSpec => {}
            PoolDeleteDecision::RequiresDecommission { reason } => {
                return Err(pool_delete_requires_decommission_error(
                    &namespace,
                    &tenant_name,
                    &pool_name,
                    reason,
                ));
            }
        }

        tenant.spec.pools.remove(pool_index);

        match tenant_api
            .replace(&tenant_name, &Default::default(), &tenant)
            .await
        {
            Ok(_) => {
                return Ok(Json(DeletePoolResponse {
                    success: true,
                    message: format!(
                        "Pool '{}' was removed from Tenant spec before managed resources were created",
                        pool_name
                    ),
                    warning: None,
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

#[cfg(test)]
mod tests {
    use super::{
        PoolDeleteDecision, classify_pool_delete, is_managed_pool_statefulset,
        is_pool_observation_current, pool_delete_observation_pending_error,
        pool_delete_requires_decommission_error,
    };
    use crate::console::error::Error;
    use crate::types::v1alpha1::{status::Status, tenant::TenantSpec};
    use axum::http::StatusCode;
    use std::collections::BTreeMap;

    fn tenant_with_generations(
        generation: i64,
        observed_generation: Option<i64>,
    ) -> crate::types::v1alpha1::tenant::Tenant {
        crate::types::v1alpha1::tenant::Tenant {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("logs".to_string()),
                namespace: Some("rustfs-system".to_string()),
                generation: Some(generation),
                ..Default::default()
            },
            spec: TenantSpec::default(),
            status: Some(Status {
                observed_generation,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn pool_delete_requires_decommission_error_contract() {
        let error = pool_delete_requires_decommission_error(
            "rustfs-system",
            "logs",
            "pool-a",
            "DecommissionRequired",
        );

        match error {
            Error::ActionRequired {
                status,
                code,
                reason,
                message,
                next_actions,
                details,
            } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(code, "PoolDeleteRequiresDecommission");
                assert_eq!(reason, "DecommissionRequired");
                assert_eq!(
                    message,
                    "Pool 'pool-a' already has managed resources and must be decommissioned before removal."
                );
                assert_eq!(
                    next_actions,
                    vec![
                        "startDecommission".to_string(),
                        "inspectPoolStatus".to_string()
                    ]
                );

                let details = details.unwrap_or_else(|| panic!("expected error details"));
                assert_eq!(details.namespace.as_deref(), Some("rustfs-system"));
                assert_eq!(details.tenant.as_deref(), Some("logs"));
                assert_eq!(details.resource.as_deref(), Some("pool-a"));
            }
            other => panic!("expected action-required error, got {other:?}"),
        }
    }

    #[test]
    fn classify_pool_delete_blocks_last_pool() {
        let result = classify_pool_delete(1, false, false);

        match result {
            Err(Error::BadRequest { message }) => assert_eq!(
                message,
                "Cannot delete the last pool. Delete the entire Tenant instead."
            ),
            other => panic!("expected bad request, got {other:?}"),
        }
    }

    #[test]
    fn classify_pool_delete_requires_decommission_for_statefulset() {
        let result = classify_pool_delete(2, true, false);

        match result {
            Ok(PoolDeleteDecision::RequiresDecommission { reason }) => {
                assert_eq!(reason, "DecommissionRequired");
            }
            other => panic!("expected decommission requirement, got {other:?}"),
        }
    }

    #[test]
    fn classify_pool_delete_requires_decommission_for_recorded_status() {
        let result = classify_pool_delete(2, false, true);

        match result {
            Ok(PoolDeleteDecision::RequiresDecommission { reason }) => {
                assert_eq!(reason, "DecommissionRequired");
            }
            other => panic!("expected decommission requirement, got {other:?}"),
        }
    }

    #[test]
    fn classify_pool_delete_allows_uncreated_pool() {
        let result = classify_pool_delete(2, false, false);

        match result {
            Ok(PoolDeleteDecision::RemoveFromSpec) => {}
            other => panic!("expected remove-from-spec decision, got {other:?}"),
        }
    }

    #[test]
    fn pool_observation_requires_current_generation() {
        assert!(!is_pool_observation_current(&tenant_with_generations(
            2,
            Some(1)
        )));
        assert!(is_pool_observation_current(&tenant_with_generations(
            2,
            Some(2)
        )));
        assert!(is_pool_observation_current(&tenant_with_generations(
            2,
            Some(3)
        )));
    }

    #[test]
    fn pool_delete_observation_pending_error_contract() {
        let error = pool_delete_observation_pending_error("rustfs-system", "logs", "pool-a");

        match error {
            Error::ActionRequired {
                status,
                code,
                reason,
                next_actions,
                details,
                ..
            } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(code, "PoolDeleteObservationPending");
                assert_eq!(reason, "ObservedGenerationStale");
                assert_eq!(next_actions, vec!["waitForReconcile".to_string()]);

                let details = details.unwrap_or_else(|| panic!("expected error details"));
                assert_eq!(details.namespace.as_deref(), Some("rustfs-system"));
                assert_eq!(details.tenant.as_deref(), Some("logs"));
                assert_eq!(details.resource.as_deref(), Some("pool-a"));
            }
            other => panic!("expected action-required error, got {other:?}"),
        }
    }

    #[test]
    fn managed_pool_statefulset_requires_owner_and_labels() {
        let mut tenant = tenant_with_generations(2, Some(2));
        tenant.metadata.uid = Some("tenant-uid".to_string());

        let labels = BTreeMap::from([
            ("rustfs.tenant".to_string(), "logs".to_string()),
            ("rustfs.pool".to_string(), "pool-a".to_string()),
        ]);
        let owner_reference = k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
            api_version: "rustfs.com/v1alpha1".to_string(),
            kind: "Tenant".to_string(),
            name: "logs".to_string(),
            uid: "tenant-uid".to_string(),
            ..Default::default()
        };
        let statefulset = k8s_openapi::api::apps::v1::StatefulSet {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some("logs-pool-a".to_string()),
                labels: Some(labels),
                owner_references: Some(vec![owner_reference]),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(is_managed_pool_statefulset(&tenant, &statefulset, "pool-a"));

        let mut unowned = statefulset.clone();
        unowned.metadata.owner_references = None;
        assert!(!is_managed_pool_statefulset(&tenant, &unowned, "pool-a"));
    }
}
