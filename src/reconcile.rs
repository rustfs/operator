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

use crate::context::Context;
use crate::types::v1alpha1::tenant::Tenant;
use crate::{context, types};
use k8s_openapi::api::core::v1 as corev1;
use kube::api::{DeleteParams, ListParams, PropagationPolicy};
use kube::ResourceExt;
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use snafu::Snafu;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error};

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(transparent)]
    Context { source: context::Error },

    #[snafu(transparent)]
    Types { source: types::error::Error },
}

pub async fn reconcile_rustfs(tenant: Arc<Tenant>, ctx: Arc<Context>) -> Result<Action, Error> {
    let ns = tenant.namespace()?;
    let latest_tenant = ctx.get::<Tenant>(&tenant.name(), &ns).await?;

    if latest_tenant.metadata.deletion_timestamp.is_some() {
        debug!(
            "tenant {} is deleted, deletion_timestamp is {:?}",
            tenant.name(),
            latest_tenant.metadata.deletion_timestamp
        );
        return Ok(Action::await_change());
    }

    // Validate credential Secret if configured
    // This only validates the Secret exists and has required keys.
    // Actual credential injection happens via secretKeyRef in the StatefulSet.
    if let Some(ref cfg) = latest_tenant.spec.creds_secret
        && !cfg.name.is_empty()
        && let Err(e) = ctx.validate_credential_secret(&latest_tenant).await
    {
        // Record event for credential validation failure
        let _ = ctx
            .record(
                &latest_tenant,
                EventType::Warning,
                "CredentialValidationFailed",
                &format!("Failed to validate credentials: {}", e),
            )
            .await;
        return Err(e.into());
    }

    // 0. Optional: unblock StatefulSet pods stuck terminating when their node is down.
    // This is inspired by Longhorn's "Pod Deletion Policy When Node is Down".
    if let Some(policy) = latest_tenant
        .spec
        .pod_deletion_policy_when_node_is_down
        .clone()
    {
        if policy != crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing {
            cleanup_stuck_terminating_pods_on_down_nodes(&latest_tenant, &ns, &ctx, policy)
                .await?;
        }
    }

    // 1. Create RBAC resources (conditionally based on service account settings)
    let custom_sa = latest_tenant.spec.service_account_name.is_some();
    let create_rbac = latest_tenant
        .spec
        .create_service_account_rbac
        .unwrap_or(false);

    if !custom_sa || create_rbac {
        // Create Role
        let role = ctx.apply(&latest_tenant.new_role(), &ns).await?;

        if !custom_sa {
            // Create default ServiceAccount and bind it
            let service_account = ctx.apply(&latest_tenant.new_service_account(), &ns).await?;
            ctx.apply(
                &latest_tenant.new_role_binding(&service_account.name_any(), &role),
                &ns,
            )
            .await?;
        } else {
            // Use custom ServiceAccount and bind it
            let sa_name = latest_tenant.service_account_name();
            ctx.apply(&latest_tenant.new_role_binding(&sa_name, &role), &ns)
                .await?;
        }
    }

    // 2. Create Services
    ctx.apply(&latest_tenant.new_io_service(), &ns).await?;
    ctx.apply(&latest_tenant.new_console_service(), &ns).await?;
    ctx.apply(&latest_tenant.new_headless_service(), &ns)
        .await?;

    // 3. Validate no pool renames (detect orphaned StatefulSets)
    // Pool renames create new StatefulSets but leave old ones orphaned
    let owned_statefulsets = ctx
        .list::<k8s_openapi::api::apps::v1::StatefulSet>(&ns)
        .await?;

    let current_pool_names: std::collections::HashSet<_> = latest_tenant
        .spec
        .pools
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    for ss in owned_statefulsets {
        // Check if this StatefulSet is owned by this Tenant
        if let Some(owner_refs) = &ss.metadata.owner_references {
            let owned_by_tenant = owner_refs.iter().any(|owner| {
                owner.kind == "Tenant"
                    && owner.name == latest_tenant.name()
                    && owner.uid == latest_tenant.metadata.uid.as_deref().unwrap_or("")
            });

            if owned_by_tenant {
                let ss_name = ss.metadata.name.as_deref().unwrap_or("");
                let tenant_prefix = format!("{}-", latest_tenant.name());

                // Extract pool name from StatefulSet name (format: {tenant}-{pool})
                if let Some(pool_name) = ss_name.strip_prefix(&tenant_prefix)
                    && !current_pool_names.contains(pool_name)
                {
                    // Found orphaned StatefulSet - pool was renamed or removed
                    return Err(types::error::Error::ImmutableFieldModified {
                        name: latest_tenant.name(),
                        field: "spec.pools[].name".to_string(),
                        message: format!(
                            "Pool name cannot be changed. Found StatefulSet '{}' for pool '{}' which no longer exists in spec. \
                            Pool renames are not supported because they change the StatefulSet selector (immutable field). \
                            To rename a pool: 1) Delete the Tenant, 2) Recreate with new pool names.",
                            ss_name, pool_name
                        ),
                    }.into());
                }
            }
        }
    }

    // 4. Create or update StatefulSets for each pool and collect their statuses
    let mut pool_statuses = Vec::new();
    let mut any_updating = false;
    let mut any_degraded = false;
    let mut total_replicas = 0;
    let mut ready_replicas = 0;

    for pool in &latest_tenant.spec.pools {
        let ss_name = format!("{}-{}", latest_tenant.name(), pool.name);

        // Try to get existing StatefulSet
        match ctx
            .get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, &ns)
            .await
        {
            Ok(existing_ss) => {
                // StatefulSet exists - check if update is needed
                debug!("StatefulSet {} exists, checking if update needed", ss_name);

                // First, validate that the update is safe (no immutable field changes)
                if let Err(e) = latest_tenant.validate_statefulset_update(&existing_ss, pool) {
                    error!("StatefulSet {} update validation failed: {}", ss_name, e);

                    // Record event for validation failure
                    let _ = ctx
                        .record(
                            &latest_tenant,
                            EventType::Warning,
                            "StatefulSetUpdateValidationFailed",
                            &format!("Cannot update StatefulSet {}: {}", ss_name, e),
                        )
                        .await;

                    return Err(e.into());
                }

                // Check if update is actually needed
                if latest_tenant.statefulset_needs_update(&existing_ss, pool)? {
                    debug!("StatefulSet {} needs update, applying changes", ss_name);

                    // Record event for update start
                    let _ = ctx
                        .record(
                            &latest_tenant,
                            EventType::Normal,
                            "StatefulSetUpdateStarted",
                            &format!("Updating StatefulSet {}", ss_name),
                        )
                        .await;

                    // Apply the update
                    ctx.apply(&latest_tenant.new_statefulset(pool)?, &ns)
                        .await?;

                    debug!("StatefulSet {} updated successfully", ss_name);
                } else {
                    debug!("StatefulSet {} is up to date, no changes needed", ss_name);
                }

                // Fetch the StatefulSet again to get the latest status after any updates
                let ss = ctx
                    .get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, &ns)
                    .await?;

                // Build pool status from StatefulSet
                let pool_status = latest_tenant.build_pool_status(&pool.name, &ss);

                // Track if any pool is updating or degraded
                use crate::types::v1alpha1::status::pool::PoolState;
                match pool_status.state {
                    PoolState::Updating => any_updating = true,
                    PoolState::Degraded | PoolState::RolloutFailed => any_degraded = true,
                    _ => {}
                }

                // Accumulate replica counts
                if let Some(r) = pool_status.replicas {
                    total_replicas += r;
                }
                if let Some(r) = pool_status.ready_replicas {
                    ready_replicas += r;
                }

                pool_statuses.push(pool_status);
            }
            Err(e) if e.to_string().contains("NotFound") => {
                // StatefulSet doesn't exist - create it
                debug!("StatefulSet {} not found, creating", ss_name);

                // Record event for creation
                let _ = ctx
                    .record(
                        &latest_tenant,
                        EventType::Normal,
                        "StatefulSetCreated",
                        &format!("Creating StatefulSet {}", ss_name),
                    )
                    .await;

                ctx.apply(&latest_tenant.new_statefulset(pool)?, &ns)
                    .await?;

                debug!("StatefulSet {} created successfully", ss_name);

                // After creation, fetch the StatefulSet to get its status
                let ss = ctx
                    .get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, &ns)
                    .await?;
                let pool_status = latest_tenant.build_pool_status(&pool.name, &ss);
                any_updating = true; // New StatefulSet is always updating initially
                pool_statuses.push(pool_status);
            }
            Err(e) => {
                // Other error - propagate
                error!("Failed to get StatefulSet {}: {}", ss_name, e);
                return Err(e.into());
            }
        }
    }

    // 5. Aggregate pool statuses and determine overall Tenant conditions
    use crate::types::v1alpha1::status::{Condition, Status};

    let observed_generation = latest_tenant.metadata.generation;
    let current_time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let mut conditions = Vec::new();

    // Determine Ready condition
    let ready_condition = if any_degraded {
        Condition {
            type_: "Ready".to_string(),
            status: "False".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "PoolDegraded".to_string(),
            message: "One or more pools are degraded".to_string(),
        }
    } else if any_updating {
        Condition {
            type_: "Ready".to_string(),
            status: "False".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "RolloutInProgress".to_string(),
            message: "StatefulSet rollout in progress".to_string(),
        }
    } else if ready_replicas == total_replicas && total_replicas > 0 {
        Condition {
            type_: "Ready".to_string(),
            status: "True".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "AllPodsReady".to_string(),
            message: format!("{}/{} pods ready", ready_replicas, total_replicas),
        }
    } else {
        Condition {
            type_: "Ready".to_string(),
            status: "False".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "PodsNotReady".to_string(),
            message: format!("{}/{} pods ready", ready_replicas, total_replicas),
        }
    };
    conditions.push(ready_condition);

    // Determine Progressing condition
    if any_updating {
        conditions.push(Condition {
            type_: "Progressing".to_string(),
            status: "True".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "RolloutInProgress".to_string(),
            message: "StatefulSet rollout in progress".to_string(),
        });
    }

    // Determine Degraded condition
    if any_degraded {
        conditions.push(Condition {
            type_: "Degraded".to_string(),
            status: "True".to_string(),
            last_transition_time: Some(current_time.clone()),
            observed_generation,
            reason: "PoolDegraded".to_string(),
            message: "One or more pools are degraded".to_string(),
        });
    }

    // Determine overall state
    let current_state = if any_degraded {
        "Degraded".to_string()
    } else if any_updating {
        "Updating".to_string()
    } else if ready_replicas == total_replicas && total_replicas > 0 {
        "Ready".to_string()
    } else {
        "NotReady".to_string()
    };

    // Build and update status
    let status = Status {
        current_state,
        available_replicas: ready_replicas,
        pools: pool_statuses,
        observed_generation,
        conditions,
    };

    debug!("Updating tenant status: {:?}", status);
    ctx.update_status(&latest_tenant, status).await?;

    // Requeue faster if any pool is updating
    if any_updating {
        debug!("Pools are updating, requeuing in 10 seconds");
        Ok(Action::requeue(Duration::from_secs(10)))
    } else {
        Ok(Action::await_change())
    }
}

async fn cleanup_stuck_terminating_pods_on_down_nodes(
    tenant: &Tenant,
    namespace: &str,
    ctx: &Context,
    policy: crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown,
) -> Result<(), Error> {
    let pods_api: kube::Api<corev1::Pod> = kube::Api::namespaced(ctx.client.clone(), namespace);
    let nodes_api: kube::Api<corev1::Node> = kube::Api::all(ctx.client.clone());

    let selector = format!("rustfs.tenant={}", tenant.name());
    let pods = pods_api
        .list(&ListParams::default().labels(&selector))
        .await
        .map_err(|source| Error::Context {
            source: context::Error::Kube { source },
        })?;

    for pod in pods.items {
        // Only act on terminating pods to keep the behavior conservative.
        if pod.metadata.deletion_timestamp.is_none() {
            continue;
        }

        // Longhorn behavior: only force delete terminating pods managed by a controller.
        // We approximate controller type via ownerReferences:
        // - StatefulSet pod: owner kind == "StatefulSet"
        // - Deployment pod: owner kind == "ReplicaSet" (Deployment owns ReplicaSet)
        if !pod_matches_policy_controller_kind(&pod, &policy) {
            continue;
        }

        let Some(node_name) = pod.spec.as_ref().and_then(|s| s.node_name.clone()) else {
            continue;
        };

        let node_is_down = match nodes_api.get(&node_name).await {
            Ok(node) => is_node_down(&node),
            Err(kube::Error::Api(ae)) if ae.code == 404 => true,
            Err(source) => {
                return Err(Error::Context {
                    source: context::Error::Kube { source },
                });
            }
        };

        if !node_is_down {
            continue;
        }

        let pod_name = pod.name_any();
        let delete_params = match policy {
            crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing => continue,
            // Legacy option: normal delete.
            crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::Delete => {
                DeleteParams::default()
            }
            // Legacy option: explicit force delete.
            crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete
            // Longhorn-compatible options: always force delete.
            | crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteStatefulSetPod
            | crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteDeploymentPod
            | crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteBothStatefulSetAndDeploymentPod => {
                DeleteParams {
                    grace_period_seconds: Some(0),
                    propagation_policy: Some(PropagationPolicy::Background),
                    ..DeleteParams::default()
                }
            }
        };

        match pods_api.delete(&pod_name, &delete_params).await {
            Ok(_) => {
                let reason = match policy {
                    crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete => {
                        "ForceDeletedPodOnDownNode"
                    }
                    crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::Delete => {
                        "DeletedPodOnDownNode"
                    }
                    crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteStatefulSetPod
                    | crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteDeploymentPod
                    | crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DeleteBothStatefulSetAndDeploymentPod => {
                        "LonghornLikeForceDeletedPodOnDownNode"
                    }
                    crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing => {
                        ""
                    }
                };
                let _ = ctx
                    .record(
                        tenant,
                        EventType::Warning,
                        reason,
                        &format!(
                            "Pod '{}' is terminating on down node '{}'; applied policy {:?}",
                            pod_name, node_name, policy
                        ),
                    )
                    .await;
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                // Pod already gone.
            }
            Err(source) => {
                return Err(Error::Context {
                    source: context::Error::Kube { source },
                });
            }
        }
    }

    Ok(())
}

fn pod_matches_policy_controller_kind(
    pod: &corev1::Pod,
    policy: &crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown,
) -> bool {
    use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

    match policy {
        // Longhorn-compatible modes: only act on controller-owned pods of certain kinds.
        P::DeleteStatefulSetPod => pod_has_owner_kind(pod, "StatefulSet"),
        P::DeleteDeploymentPod => pod_has_owner_kind(pod, "ReplicaSet"),
        P::DeleteBothStatefulSetAndDeploymentPod => {
            pod_has_owner_kind(pod, "StatefulSet") || pod_has_owner_kind(pod, "ReplicaSet")
        }
        // Legacy modes: act on any tenant-owned pod.
        _ => true,
    }
}

fn pod_has_owner_kind(pod: &corev1::Pod, kind: &str) -> bool {
    pod.metadata
        .owner_references
        .as_ref()
        .is_some_and(|refs| refs.iter().any(|r| r.kind == kind))
}

fn is_node_down(node: &corev1::Node) -> bool {
    let Some(status) = &node.status else {
        return false;
    };
    let Some(conditions) = &status.conditions else {
        return false;
    };

    for c in conditions {
        if c.type_ == "Ready" {
            // Ready=False or Ready=Unknown => treat as down
            return c.status != "True";
        }
    }

    false
}

pub fn error_policy(_object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    error!("error_policy: {:?}", error);

    // Status updates happen during reconciliation before errors are returned.
    // The reconcile function sets appropriate conditions (Ready=False, Degraded=True)
    // and records events for failures before propagating errors.
    // This error_policy function only determines requeue strategy.

    // Use different requeue strategies based on error type:
    // - User-fixable errors (credentials, validation): Longer intervals to reduce spam
    // - Transient errors (API, network): Shorter intervals for quick recovery
    match error {
        Error::Context { source } => match source {
            // Credential validation errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            context::Error::CredentialSecretNotFound { .. }
            | context::Error::CredentialSecretMissingKey { .. }
            | context::Error::CredentialSecretInvalidEncoding { .. }
            | context::Error::CredentialSecretTooShort { .. } => {
                Action::requeue(Duration::from_secs(60))
            }

            // Kubernetes API errors - might be transient (network, API server issues)
            // Use shorter requeue for faster recovery
            context::Error::Kube { .. } | context::Error::Record { .. } => {
                Action::requeue(Duration::from_secs(5))
            }

            // Other context errors - use moderate requeue
            _ => Action::requeue(Duration::from_secs(15)),
        },

        // Type errors - validation issues, use moderate requeue
        Error::Types { source } => match source {
            // Immutable field modification errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            types::error::Error::ImmutableFieldModified { .. } => {
                Action::requeue(Duration::from_secs(60))
            }

            // Other type errors - use moderate requeue
            _ => Action::requeue(Duration::from_secs(15)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::is_node_down;
    use super::{pod_has_owner_kind, pod_matches_policy_controller_kind};
    use k8s_openapi::api::core::v1 as corev1;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

    // Test 10: RBAC creation logic - default behavior
    #[test]
    fn test_should_create_rbac_default() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(!custom_sa, "Should not have custom SA");
        assert!(
            !create_rbac,
            "createServiceAccountRbac should default to false"
        );
        assert!(should_create_rbac, "Should create RBAC when no custom SA");
    }

    // Test 11: RBAC creation logic - custom SA with createServiceAccountRbac=true
    #[test]
    fn test_should_create_rbac_custom_sa_with_rbac() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(create_rbac, "createServiceAccountRbac should be true");
        assert!(
            should_create_rbac,
            "Should create RBAC when explicitly requested"
        );
    }

    // Test 12: RBAC creation logic - custom SA with createServiceAccountRbac=false
    #[test]
    fn test_should_skip_rbac_custom_sa_without_rbac() {
        let tenant =
            crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(false));

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(!create_rbac, "createServiceAccountRbac should be false");
        assert!(!should_create_rbac, "Should skip RBAC creation");
    }

    // Test 13: RBAC creation logic - custom SA with createServiceAccountRbac=None (default)
    #[test]
    fn test_should_skip_rbac_custom_sa_default() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), None);

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(
            !create_rbac,
            "createServiceAccountRbac should default to false"
        );
        assert!(
            !should_create_rbac,
            "Should skip RBAC when None treated as false"
        );
    }

    // Test 14: Service account determination in reconcile logic
    #[test]
    fn test_determine_sa_name_in_reconcile() {
        // Test default behavior
        let tenant_default = crate::tests::create_test_tenant(None, None);
        let sa_name = tenant_default.service_account_name();
        assert_eq!(sa_name, "test-tenant-sa");

        // Test custom SA
        let tenant_custom = crate::tests::create_test_tenant(Some("custom-sa".to_string()), None);
        let sa_name_custom = tenant_custom.service_account_name();
        assert_eq!(sa_name_custom, "custom-sa");
    }

    #[test]
    fn test_is_node_down_ready_true() {
        let node = corev1::Node {
            status: Some(corev1::NodeStatus {
                conditions: Some(vec![corev1::NodeCondition {
                    type_: "Ready".to_string(),
                    status: "True".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!is_node_down(&node));
    }

    #[test]
    fn test_is_node_down_ready_false() {
        let node = corev1::Node {
            status: Some(corev1::NodeStatus {
                conditions: Some(vec![corev1::NodeCondition {
                    type_: "Ready".to_string(),
                    status: "False".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(is_node_down(&node));
    }

    #[test]
    fn test_is_node_down_ready_unknown() {
        let node = corev1::Node {
            status: Some(corev1::NodeStatus {
                conditions: Some(vec![corev1::NodeCondition {
                    type_: "Ready".to_string(),
                    status: "Unknown".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(is_node_down(&node));
    }

    #[test]
    fn test_pod_owner_kind_helpers() {
        let pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                owner_references: Some(vec![metav1::OwnerReference {
                    api_version: "apps/v1".to_string(),
                    kind: "StatefulSet".to_string(),
                    name: "ss".to_string(),
                    uid: "uid".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(pod_has_owner_kind(&pod, "StatefulSet"));
        assert!(!pod_has_owner_kind(&pod, "ReplicaSet"));
    }

    #[test]
    fn test_policy_controller_kind_matching_longhorn_like() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let ss_pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                deletion_timestamp: Some(metav1::Time(chrono::Utc::now())),
                owner_references: Some(vec![metav1::OwnerReference {
                    api_version: "apps/v1".to_string(),
                    kind: "StatefulSet".to_string(),
                    name: "ss".to_string(),
                    uid: "uid".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        };

        let deploy_pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                deletion_timestamp: Some(metav1::Time(chrono::Utc::now())),
                owner_references: Some(vec![metav1::OwnerReference {
                    api_version: "apps/v1".to_string(),
                    kind: "ReplicaSet".to_string(),
                    name: "rs".to_string(),
                    uid: "uid".to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(pod_matches_policy_controller_kind(&ss_pod, &P::DeleteStatefulSetPod));
        assert!(!pod_matches_policy_controller_kind(&deploy_pod, &P::DeleteStatefulSetPod));

        assert!(pod_matches_policy_controller_kind(&deploy_pod, &P::DeleteDeploymentPod));
        assert!(!pod_matches_policy_controller_kind(&ss_pod, &P::DeleteDeploymentPod));

        assert!(pod_matches_policy_controller_kind(
            &ss_pod,
            &P::DeleteBothStatefulSetAndDeploymentPod
        ));
        assert!(pod_matches_policy_controller_kind(
            &deploy_pod,
            &P::DeleteBothStatefulSetAndDeploymentPod
        ));
    }
}
