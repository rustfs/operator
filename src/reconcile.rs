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
use crate::status::{StatusBuilder, StatusError};
use crate::types::v1alpha1::status::{ConditionType, Reason, Status};
use crate::types::v1alpha1::tenant::Tenant;
use crate::{context, types};
use k8s_openapi::api::core::v1 as corev1;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, PropagationPolicy};
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use snafu::Snafu;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

mod phases;
mod tls;

use phases::{
    finalize_tenant_status, maybe_cleanup_terminating_pods, reconcile_pool_statefulsets,
    reconcile_rbac_resources, reconcile_services, validate_no_pool_rename,
    validate_tenant_prerequisites,
};

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(transparent)]
    Context { source: context::Error },

    #[snafu(transparent)]
    Types { source: types::error::Error },

    #[snafu(display("TLS reconciliation blocked ({reason}): {message}"))]
    TlsBlocked { reason: String, message: String },

    #[snafu(display("TLS reconciliation pending ({reason}): {message}"))]
    TlsPending { reason: String, message: String },
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

    if should_mark_reconcile_started(&latest_tenant) {
        patch_reconcile_started(&ctx, &latest_tenant).await;
    }

    validate_tenant_prerequisites(&ctx, &latest_tenant).await?;
    let tls_plan = tls::reconcile_tls(&ctx, &latest_tenant, &ns).await?;

    maybe_cleanup_terminating_pods(&ctx, &latest_tenant, &ns).await?;

    reconcile_rbac_resources(&ctx, &latest_tenant, &ns).await?;

    reconcile_services(&ctx, &latest_tenant, &ns, &tls_plan).await?;

    validate_no_pool_rename(&ctx, &latest_tenant, &ns).await?;

    let summary = reconcile_pool_statefulsets(&ctx, &latest_tenant, &ns, &tls_plan).await?;
    finalize_tenant_status(&ctx, &latest_tenant, summary, tls_plan).await
}

#[cfg(test)]
fn should_create_rbac(tenant: &Tenant) -> bool {
    phases::should_create_rbac(tenant)
}

async fn context_result<T>(
    result: Result<T, context::Error>,
    ctx: &Context,
    tenant: &Tenant,
) -> Result<T, Error> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let status_error = StatusError::from_context_error(&error);
            patch_status_error(ctx, tenant, &status_error).await;
            Err(error.into())
        }
    }
}

async fn types_result<T>(
    result: Result<T, types::error::Error>,
    ctx: &Context,
    tenant: &Tenant,
) -> Result<T, Error> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let status_error = StatusError::from_types_error(&error);
            patch_status_error(ctx, tenant, &status_error).await;
            Err(error.into())
        }
    }
}

async fn patch_status_error(ctx: &Context, tenant: &Tenant, status_error: &StatusError) {
    let mut builder = StatusBuilder::from_tenant(tenant);
    builder.mark_error(status_error);
    let status = builder.build();
    let should_record =
        condition_marker_changed(tenant.status.as_ref(), &status, status_error.condition_type);

    if should_record {
        let _ = ctx
            .record(
                tenant,
                status_error.event_type,
                status_error.reason.as_str(),
                &status_error.safe_message,
            )
            .await;
    }

    match ctx.patch_status_if_changed(tenant, status).await {
        Ok(Some(_)) => {
            info!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                reason = status_error.reason.as_str(),
                condition = status_error.condition_type.as_str(),
                "patched Tenant status for reconcile error"
            );
        }
        Ok(None) => {
            debug!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                reason = status_error.reason.as_str(),
                "skipped Tenant status patch because error status is unchanged"
            );
        }
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                reason = status_error.reason.as_str(),
                %error,
                "failed to patch Tenant status for reconcile error"
            );
            if should_record {
                let _ = ctx
                    .record(
                        tenant,
                        status_error.event_type,
                        status_error.reason.as_str(),
                        &status_error.safe_message,
                    )
                    .await;
            }
            let status_patch_error = StatusError::status_patch_failed(status_error.reason);
            let _ = ctx
                .record(
                    tenant,
                    status_patch_error.event_type,
                    status_patch_error.reason.as_str(),
                    &status_patch_error.safe_message,
                )
                .await;
        }
    }
}

async fn patch_reconcile_started(ctx: &Context, tenant: &Tenant) {
    if !should_mark_reconcile_started(tenant) {
        debug!(
            tenant = %tenant.name(),
            namespace = ?tenant.namespace(),
            generation = ?tenant.metadata.generation,
            observed_generation = ?tenant.status.as_ref().and_then(|status| status.observed_generation),
            "skipping ReconcileStarted status patch because observed generation is current"
        );
        return;
    }

    let mut builder = StatusBuilder::from_tenant(tenant);
    builder.mark_started();
    let status = builder.build();

    info!(
        tenant = %tenant.name(),
        namespace = ?tenant.namespace(),
        generation = ?tenant.metadata.generation,
        observed_generation = ?tenant.status.as_ref().and_then(|status| status.observed_generation),
        "marking Tenant reconcile started for stale or missing status"
    );

    match ctx.patch_status_if_changed(tenant, status).await {
        Ok(Some(_)) => {
            info!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                "patched Tenant ReconcileStarted status"
            );
        }
        Ok(None) => {
            debug!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                "ReconcileStarted status patch was a no-op"
            );
        }
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                %error,
                "failed to patch Tenant ReconcileStarted status"
            );
            let status_patch_error = StatusError::status_patch_failed(Reason::ReconcileStarted);
            let _ = ctx
                .record(
                    tenant,
                    status_patch_error.event_type,
                    status_patch_error.reason.as_str(),
                    &status_patch_error.safe_message,
                )
                .await;
        }
    }
}

fn should_mark_reconcile_started(tenant: &Tenant) -> bool {
    match (
        tenant
            .status
            .as_ref()
            .and_then(|status| status.observed_generation),
        tenant.metadata.generation,
    ) {
        (Some(observed), Some(generation)) => observed < generation,
        (None, Some(_)) => true,
        (None, None) => tenant.status.is_none(),
        (Some(_), None) => false,
    }
}

async fn patch_status_and_record(
    ctx: &Context,
    tenant: &Tenant,
    status: Status,
    condition_type: ConditionType,
    reason: Reason,
    event_type: EventType,
    message: &str,
) -> Result<(), Error> {
    let should_record = condition_marker_changed(tenant.status.as_ref(), &status, condition_type);
    let patched = ctx.patch_status_if_changed(tenant, status).await?;
    match patched {
        Some(_) => {
            info!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                reason = reason.as_str(),
                condition = condition_type.as_str(),
                "patched Tenant status after reconciliation"
            );
            if should_record {
                let _ = ctx
                    .record(tenant, event_type, reason.as_str(), message)
                    .await;
            }
        }
        None => {
            debug!(
                tenant = %tenant.name(),
                namespace = ?tenant.namespace(),
                reason = reason.as_str(),
                "skipped Tenant status patch because reconciled status is unchanged"
            );
        }
    }
    Ok(())
}

fn condition_marker_changed(
    previous_status: Option<&Status>,
    next_status: &Status,
    condition_type: ConditionType,
) -> bool {
    condition_marker(previous_status, condition_type)
        != condition_marker(Some(next_status), condition_type)
}

fn condition_marker(
    status: Option<&Status>,
    condition_type: ConditionType,
) -> Option<(String, String)> {
    status
        .and_then(|status| status.condition(condition_type))
        .map(|condition| (condition.status.clone(), condition.reason.clone()))
}

fn statefulset_owned_by_tenant(
    ss: &k8s_openapi::api::apps::v1::StatefulSet,
    tenant: &Tenant,
) -> bool {
    ss.metadata.owner_references.as_ref().is_some_and(|refs| {
        refs.iter().any(|owner| {
            owner.kind == "Tenant"
                && owner.name == tenant.name()
                && owner.uid == tenant.metadata.uid.as_deref().unwrap_or("")
        })
    })
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
        warn!(
            "Node {} is detected down. Pod {} is terminating on it.",
            node_name, pod_name
        );
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
            // Credential / KMS validation errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            context::Error::CredentialSecretNotFound { .. }
            | context::Error::CredentialSecretMissingKey { .. }
            | context::Error::CredentialSecretInvalidEncoding { .. }
            | context::Error::CredentialSecretTooShort { .. }
            | context::Error::KmsSecretNotFound { .. }
            | context::Error::KmsSecretMissingKey { .. }
            | context::Error::KmsConfigInvalid { .. } => Action::requeue(Duration::from_secs(60)),

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
            // Immutable field / invalid name errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            types::error::Error::ImmutableFieldModified { .. }
            | types::error::Error::InvalidTenantName { .. }
            | types::error::Error::PoolDeleteBlocked { .. } => {
                Action::requeue(Duration::from_secs(60))
            }

            // Other type errors - use moderate requeue
            _ => Action::requeue(Duration::from_secs(15)),
        },

        Error::TlsBlocked { .. } => Action::requeue(Duration::from_secs(60)),
        Error::TlsPending { .. } => Action::requeue(Duration::from_secs(20)),
    }
}

#[cfg(test)]
mod tests {
    use super::is_node_down;
    use super::{
        pod_has_owner_kind, pod_matches_policy_controller_kind, should_create_rbac,
        should_mark_reconcile_started,
    };
    use crate::types::v1alpha1::status::Status;
    use k8s_openapi::api::core::v1 as corev1;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

    #[test]
    fn should_not_mark_reconcile_started_when_generation_is_current() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(3);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            observed_generation: Some(3),
            ..Default::default()
        });

        assert!(!should_mark_reconcile_started(&tenant));
    }

    #[test]
    fn should_mark_reconcile_started_for_missing_or_stale_status() {
        let mut missing = crate::tests::create_test_tenant(None, None);
        missing.metadata.generation = Some(3);
        missing.status = None;
        assert!(should_mark_reconcile_started(&missing));

        let mut stale = crate::tests::create_test_tenant(None, None);
        stale.metadata.generation = Some(3);
        stale.status = Some(Status {
            current_state: "Ready".to_string(),
            observed_generation: Some(2),
            ..Default::default()
        });
        assert!(should_mark_reconcile_started(&stale));
    }

    #[test]
    fn test_should_create_rbac_default() {
        let tenant = crate::tests::create_test_tenant(None, None);

        assert!(should_create_rbac(&tenant));
    }

    // Test 11: RBAC creation logic - custom SA with createServiceAccountRbac=true
    #[test]
    fn test_should_create_rbac_custom_sa_with_rbac() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));

        assert!(should_create_rbac(&tenant));
    }

    // Test 12: RBAC creation logic - custom SA with createServiceAccountRbac=false
    #[test]
    fn test_should_skip_rbac_custom_sa_without_rbac() {
        let tenant =
            crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(false));

        assert!(!should_create_rbac(&tenant));
    }

    // Test 13: RBAC creation logic - custom SA with createServiceAccountRbac=None (default)
    #[test]
    fn test_should_skip_rbac_custom_sa_default() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), None);

        assert!(!should_create_rbac(&tenant));
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

        assert!(pod_matches_policy_controller_kind(
            &ss_pod,
            &P::DeleteStatefulSetPod
        ));
        assert!(!pod_matches_policy_controller_kind(
            &deploy_pod,
            &P::DeleteStatefulSetPod
        ));

        assert!(pod_matches_policy_controller_kind(
            &deploy_pod,
            &P::DeleteDeploymentPod
        ));
        assert!(!pod_matches_policy_controller_kind(
            &ss_pod,
            &P::DeleteDeploymentPod
        ));

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
