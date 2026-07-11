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
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::api::{DeleteParams, ListParams, Preconditions, PropagationPolicy};
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use kube::{Resource, ResourceExt};
use snafu::Snafu;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

mod phases;
mod pool_lifecycle;
mod provisioning;
mod tls;

const OUT_OF_SERVICE_TAINT_KEY: &str = "node.kubernetes.io/out-of-service";

use phases::{
    cleanup_removed_decommissioned_pool_statefulsets, finalize_tenant_status,
    maybe_cleanup_terminating_pods, reconcile_pool_statefulsets, reconcile_rbac_resources,
    reconcile_services, validate_no_pool_rename, validate_tenant_prerequisites,
};
use pool_lifecycle::reconcile_pool_lifecycle;

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
            tenant = %tenant.name(),
            namespace = %ns,
            deletion_timestamp = ?latest_tenant.metadata.deletion_timestamp,
            "tenant is deleting; skipping reconcile"
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

    let removed_pool_cleanup =
        cleanup_removed_decommissioned_pool_statefulsets(&ctx, &latest_tenant, &ns).await?;

    validate_no_pool_rename(
        &ctx,
        &latest_tenant,
        &ns,
        &removed_pool_cleanup.allowed_removed_pool_names,
    )
    .await?;

    let lifecycle_decisions = reconcile_pool_lifecycle(&ctx, &latest_tenant, &ns).await?;

    let summary = reconcile_pool_statefulsets(
        &ctx,
        &latest_tenant,
        &ns,
        &tls_plan,
        &lifecycle_decisions,
        &removed_pool_cleanup,
    )
    .await?;
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

fn object_owned_by_tenant(metadata: &metav1::ObjectMeta, tenant: &Tenant) -> bool {
    let Some(tenant_uid) = tenant.metadata.uid.as_deref().filter(|uid| !uid.is_empty()) else {
        return false;
    };

    metadata.owner_references.as_ref().is_some_and(|refs| {
        refs.iter().any(|owner| {
            owner.api_version == Tenant::api_version(&())
                && owner.kind == Tenant::kind(&())
                && owner.name == tenant.name()
                && owner.uid == tenant_uid
                && owner.controller == Some(true)
        })
    })
}

fn statefulset_owned_by_tenant(ss: &appsv1::StatefulSet, tenant: &Tenant) -> bool {
    object_owned_by_tenant(&ss.metadata, tenant)
}

fn statefulset_matches_pod_controller_and_tenant(
    pod: &corev1::Pod,
    statefulset: &appsv1::StatefulSet,
    tenant: &Tenant,
) -> bool {
    let Some((_, statefulset_uid)) = pod_controller_owner_name_and_uid(pod, "StatefulSet") else {
        return false;
    };

    statefulset_owned_by_tenant(statefulset, tenant)
        && statefulset.metadata.uid.as_deref() == Some(statefulset_uid.as_str())
}

fn replicaset_matches_pod_controller_and_tenant(
    pod: &corev1::Pod,
    replicaset: &appsv1::ReplicaSet,
    owning_deployment: Option<&appsv1::Deployment>,
    tenant: &Tenant,
    require_deployment_owner: bool,
) -> bool {
    let Some((_, replicaset_uid)) = pod_controller_owner_name_and_uid(pod, "ReplicaSet") else {
        return false;
    };
    if replicaset.metadata.uid.as_deref() != Some(replicaset_uid.as_str()) {
        return false;
    }
    if !require_deployment_owner && object_owned_by_tenant(&replicaset.metadata, tenant) {
        return true;
    }

    let Some((_, deployment_uid)) =
        controller_owner_name_and_uid(replicaset.metadata.owner_references.as_ref(), "Deployment")
    else {
        return false;
    };
    let Some(deployment) = owning_deployment else {
        return false;
    };

    object_owned_by_tenant(&deployment.metadata, tenant)
        && deployment.metadata.uid.as_deref() == Some(deployment_uid.as_str())
}

fn policy_requires_deployment_owner_for_replicaset(
    policy: &crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown,
) -> bool {
    use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

    matches!(
        policy,
        P::DeleteDeploymentPod | P::DeleteBothStatefulSetAndDeploymentPod
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodePodDeletionSafety {
    Ready,
    DownUnfenced,
    Fenced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PodDeletionPlan {
    force: bool,
    precondition_uid: Option<String>,
    event_reason: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PodCleanupDecision {
    Skip,
    SkipForceDeleteNeedsFencing,
    Delete(PodDeletionPlan),
}

impl PodDeletionPlan {
    fn delete_params(&self) -> DeleteParams {
        let preconditions = self.precondition_uid.clone().map(|uid| Preconditions {
            uid: Some(uid),
            resource_version: None,
        });

        if self.force {
            DeleteParams {
                grace_period_seconds: Some(0),
                propagation_policy: Some(PropagationPolicy::Background),
                preconditions,
                ..DeleteParams::default()
            }
        } else {
            DeleteParams {
                preconditions,
                ..DeleteParams::default()
            }
        }
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
    let statefulsets_api: kube::Api<appsv1::StatefulSet> =
        kube::Api::namespaced(ctx.client.clone(), namespace);
    let replicasets_api: kube::Api<appsv1::ReplicaSet> =
        kube::Api::namespaced(ctx.client.clone(), namespace);
    let deployments_api: kube::Api<appsv1::Deployment> =
        kube::Api::namespaced(ctx.client.clone(), namespace);

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

        let mut verified_tenant_owner = object_owned_by_tenant(&pod.metadata, tenant);
        if let Some((statefulset_name, _statefulset_uid)) =
            pod_controller_owner_name_and_uid(&pod, "StatefulSet")
        {
            let owned_by_tenant = match statefulsets_api.get(&statefulset_name).await {
                Ok(statefulset) => {
                    statefulset_matches_pod_controller_and_tenant(&pod, &statefulset, tenant)
                }
                Err(kube::Error::Api(ae)) if ae.code == 404 => false,
                Err(source) => {
                    return Err(Error::Context {
                        source: context::Error::Kube { source },
                    });
                }
            };

            if !owned_by_tenant {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pod = %pod.name_any(),
                    statefulset = %statefulset_name,
                    "skipping terminating StatefulSet pod because the owner StatefulSet is not owned by this tenant"
                );
                continue;
            }
            verified_tenant_owner = true;
        }
        if let Some((replicaset_name, _replicaset_uid)) =
            pod_controller_owner_name_and_uid(&pod, "ReplicaSet")
        {
            let owned_by_tenant = match replicasets_api.get(&replicaset_name).await {
                Ok(replicaset) => {
                    let deployment = if let Some((deployment_name, _deployment_uid)) =
                        controller_owner_name_and_uid(
                            replicaset.metadata.owner_references.as_ref(),
                            "Deployment",
                        ) {
                        match deployments_api.get(&deployment_name).await {
                            Ok(deployment) => Some(deployment),
                            Err(kube::Error::Api(ae)) if ae.code == 404 => None,
                            Err(source) => {
                                return Err(Error::Context {
                                    source: context::Error::Kube { source },
                                });
                            }
                        }
                    } else {
                        None
                    };

                    replicaset_matches_pod_controller_and_tenant(
                        &pod,
                        &replicaset,
                        deployment.as_ref(),
                        tenant,
                        policy_requires_deployment_owner_for_replicaset(&policy),
                    )
                }
                Err(kube::Error::Api(ae)) if ae.code == 404 => false,
                Err(source) => {
                    return Err(Error::Context {
                        source: context::Error::Kube { source },
                    });
                }
            };

            if !owned_by_tenant {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pod = %pod.name_any(),
                    replicaset = %replicaset_name,
                    "skipping terminating ReplicaSet pod because the owner chain is not owned by this tenant"
                );
                continue;
            }
            verified_tenant_owner = true;
        }

        if !verified_tenant_owner {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                pod = %pod.name_any(),
                "skipping terminating pod because it does not have a verified Tenant owner chain"
            );
            continue;
        }

        let Some(node_name) = pod.spec.as_ref().and_then(|s| s.node_name.clone()) else {
            continue;
        };

        let node_deletion_safety = match nodes_api.get(&node_name).await {
            Ok(node) => node_pod_deletion_safety(&node, &pod),
            Err(kube::Error::Api(ae)) if ae.code == 404 => NodePodDeletionSafety::Fenced,
            Err(source) => {
                return Err(Error::Context {
                    source: context::Error::Kube { source },
                });
            }
        };

        if node_deletion_safety == NodePodDeletionSafety::Ready {
            continue;
        }

        let deletion_plan = match cleanup_decision_for_pod(&pod, &policy, node_deletion_safety) {
            PodCleanupDecision::Skip => continue,
            PodCleanupDecision::SkipForceDeleteNeedsFencing => {
                let pod_name = pod.name_any();
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    node = %node_name,
                    pod = %pod_name,
                    policy = ?policy,
                    "skipping pod force deletion because the node is not fenced"
                );
                let _ = ctx
                    .record(
                        tenant,
                        EventType::Warning,
                        "PodForceDeleteSkippedNodeNotFenced",
                        &format!(
                            "Pod '{}' is terminating on down node '{}', but policy {:?} requires the Node to be deleted or marked with an effective node.kubernetes.io/out-of-service taint before force deletion",
                            pod_name, node_name, policy
                        ),
                    )
                    .await;
                continue;
            }
            PodCleanupDecision::Delete(plan) => plan,
        };

        let pod_name = pod.name_any();
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            node = %node_name,
            pod = %pod_name,
            policy = ?policy,
            "terminating pod is scheduled on a down node"
        );
        let delete_params = deletion_plan.delete_params();

        match pods_api.delete(&pod_name, &delete_params).await {
            Ok(_) => {
                let _ = ctx
                    .record(
                        tenant,
                        EventType::Warning,
                        deletion_plan.event_reason,
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
            Err(kube::Error::Api(ae)) if ae.code == 409 => {
                // UID precondition failed; a new object may already exist with the same pod name.
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pod = %pod_name,
                    "skipping pod deletion because the pod changed before delete"
                );
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
    pod_controller_owner_name_and_uid(pod, kind).is_some()
}

fn pod_controller_owner_name_and_uid(pod: &corev1::Pod, kind: &str) -> Option<(String, String)> {
    controller_owner_name_and_uid(pod.metadata.owner_references.as_ref(), kind)
}

fn controller_owner_name_and_uid(
    owner_references: Option<&Vec<metav1::OwnerReference>>,
    kind: &str,
) -> Option<(String, String)> {
    owner_references.and_then(|refs| {
        refs.iter()
            .find(|r| r.kind == kind && r.api_version == "apps/v1" && r.controller == Some(true))
            .map(|r| (r.name.clone(), r.uid.clone()))
    })
}

fn force_delete_requires_fencing(
    policy: &crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown,
) -> bool {
    use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

    matches!(
        policy,
        P::ForceDelete
            | P::DeleteStatefulSetPod
            | P::DeleteDeploymentPod
            | P::DeleteBothStatefulSetAndDeploymentPod
    )
}

fn cleanup_decision_for_pod(
    pod: &corev1::Pod,
    policy: &crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown,
    node_deletion_safety: NodePodDeletionSafety,
) -> PodCleanupDecision {
    use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

    if !pod_matches_policy_controller_kind(pod, policy)
        || node_deletion_safety == NodePodDeletionSafety::Ready
    {
        return PodCleanupDecision::Skip;
    }

    if force_delete_requires_fencing(policy)
        && node_deletion_safety != NodePodDeletionSafety::Fenced
    {
        return PodCleanupDecision::SkipForceDeleteNeedsFencing;
    }

    let precondition_uid = pod.metadata.uid.clone();
    match policy {
        P::DoNothing => PodCleanupDecision::Skip,
        P::Delete => PodCleanupDecision::Delete(PodDeletionPlan {
            force: false,
            precondition_uid,
            event_reason: "RequestedPodDeleteOnDownNode",
        }),
        P::ForceDelete => PodCleanupDecision::Delete(PodDeletionPlan {
            force: true,
            precondition_uid,
            event_reason: "ForceDeletedPodOnDownNode",
        }),
        P::DeleteStatefulSetPod
        | P::DeleteDeploymentPod
        | P::DeleteBothStatefulSetAndDeploymentPod => PodCleanupDecision::Delete(PodDeletionPlan {
            force: true,
            precondition_uid,
            event_reason: "LonghornLikeForceDeletedPodOnDownNode",
        }),
    }
}

fn node_pod_deletion_safety(node: &corev1::Node, pod: &corev1::Pod) -> NodePodDeletionSafety {
    if !is_node_down(node) {
        return NodePodDeletionSafety::Ready;
    }

    if node_has_effective_out_of_service_taint(node, pod) {
        NodePodDeletionSafety::Fenced
    } else {
        NodePodDeletionSafety::DownUnfenced
    }
}

fn node_has_effective_out_of_service_taint(node: &corev1::Node, pod: &corev1::Pod) -> bool {
    node.spec
        .as_ref()
        .and_then(|spec| spec.taints.as_ref())
        .is_some_and(|taints| {
            taints.iter().any(|taint| {
                taint.key == OUT_OF_SERVICE_TAINT_KEY
                    && !pod_tolerates_taint(pod, taint)
                    && (taint.effect == "NoExecute" || taint.effect == "NoSchedule")
            })
        })
}

fn pod_tolerates_taint(pod: &corev1::Pod, taint: &corev1::Taint) -> bool {
    pod.spec
        .as_ref()
        .and_then(|spec| spec.tolerations.as_ref())
        .is_some_and(|tolerations| {
            tolerations
                .iter()
                .any(|toleration| toleration_matches_taint(toleration, taint))
        })
}

fn toleration_matches_taint(toleration: &corev1::Toleration, taint: &corev1::Taint) -> bool {
    let effect_matches = toleration
        .effect
        .as_deref()
        .is_none_or(|effect| effect == taint.effect.as_str());
    if !effect_matches {
        return false;
    }

    let key_value_matches = match toleration.operator.as_deref().unwrap_or("Equal") {
        "Exists" => toleration
            .key
            .as_deref()
            .is_none_or(|key| key.is_empty() || key == taint.key),
        _ => {
            toleration.key.as_deref() == Some(taint.key.as_str())
                && toleration.value.as_deref() == taint.value.as_deref()
        }
    };
    if !key_value_matches {
        return false;
    }

    if taint.effect == "NoExecute"
        && let Some(seconds) = toleration.toleration_seconds
    {
        if seconds <= 0 {
            return false;
        }

        return taint.time_added.as_ref().is_none_or(|time_added| {
            chrono::Utc::now() < time_added.0 + chrono::Duration::seconds(seconds)
        });
    }

    true
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

fn requeue_after(duration: Duration) -> Action {
    crate::metrics::record_reconcile_requeue(duration);
    Action::requeue(duration)
}

pub fn error_policy(object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    // Status updates happen during reconciliation before errors are returned.
    // The reconcile function sets appropriate conditions (Ready=False, Degraded=True)
    // and records events for failures before propagating errors.
    // This error_policy function only determines requeue strategy.

    // Use different requeue strategies based on error type:
    // - User-fixable errors (credentials, validation): Longer intervals to reduce spam
    // - Transient errors (API, network): Shorter intervals for quick recovery
    let requeue = match error {
        Error::Context { source } => match source {
            // Credential / KMS validation errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            context::Error::CredentialSecretNotFound { .. }
            | context::Error::CredentialSecretMissingKey { .. }
            | context::Error::CredentialSecretInvalidEncoding { .. }
            | context::Error::CredentialSecretTooShort { .. }
            | context::Error::KmsSecretNotFound { .. }
            | context::Error::KmsSecretMissingKey { .. }
            | context::Error::KmsConfigInvalid { .. } => Duration::from_secs(60),

            // Kubernetes API errors - might be transient (network, API server issues)
            // Use shorter requeue for faster recovery
            context::Error::Kube { .. } | context::Error::Record { .. } => Duration::from_secs(5),

            // Other context errors - use moderate requeue
            _ => Duration::from_secs(15),
        },

        // Type errors - validation issues, use moderate requeue
        Error::Types { source } => match source {
            // Immutable field / invalid name errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            types::error::Error::ImmutableFieldModified { .. }
            | types::error::Error::InvalidTenantName { .. }
            | types::error::Error::KmsMigrationBlocked { .. }
            | types::error::Error::PoolDeleteBlocked { .. } => Duration::from_secs(60),

            // Other type errors - use moderate requeue
            _ => Duration::from_secs(15),
        },

        Error::TlsBlocked { .. } => Duration::from_secs(60),
        Error::TlsPending { .. } => Duration::from_secs(20),
    };

    warn!(
        tenant = %object.name(),
        namespace = ?object.namespace(),
        reason = reconcile_error_reason(error),
        requeue_seconds = requeue.as_secs(),
        %error,
        "reconcile failed; scheduling retry"
    );

    requeue_after(requeue)
}

fn reconcile_error_reason(error: &Error) -> &'static str {
    match error {
        Error::Context { source } => match source {
            context::Error::CredentialSecretNotFound { .. } => "CredentialSecretNotFound",
            context::Error::CredentialSecretMissingKey { .. } => "CredentialSecretMissingKey",
            context::Error::CredentialSecretInvalidEncoding { .. } => {
                "CredentialSecretInvalidEncoding"
            }
            context::Error::CredentialSecretTooShort { .. } => "CredentialSecretTooShort",
            context::Error::KmsSecretNotFound { .. } => "KmsSecretNotFound",
            context::Error::KmsSecretMissingKey { .. } => "KmsSecretMissingKey",
            context::Error::KmsConfigInvalid { .. } => "KmsConfigInvalid",
            context::Error::Kube { .. } => "KubernetesApiError",
            context::Error::Record { .. } => "KubernetesEventRecordError",
            context::Error::Types { .. } => "TypeError",
            context::Error::Serde { .. } => "SerdeError",
        },
        Error::Types { source } => match source {
            types::error::Error::InvalidTenantName { .. } => "InvalidTenantName",
            types::error::Error::InvalidPoolSpec { .. } => "InvalidPoolSpec",
            types::error::Error::ImmutableFieldModified { .. } => "ImmutableFieldModified",
            types::error::Error::PoolDeleteBlocked { .. } => "PoolDeleteBlocked",
            types::error::Error::KmsMigrationBlocked { .. } => "KmsMigrationBlocked",
            types::error::Error::NoNamespace => "NoNamespace",
            types::error::Error::InternalError { .. } => "InternalError",
            types::error::Error::SerdeJson { .. } => "SerdeJsonError",
        },
        Error::TlsBlocked { .. } => "TlsBlocked",
        Error::TlsPending { .. } => "TlsPending",
    }
}

#[cfg(test)]
mod tests {
    use super::is_node_down;
    use super::{
        NodePodDeletionSafety, PodCleanupDecision, cleanup_decision_for_pod,
        force_delete_requires_fencing, node_pod_deletion_safety, object_owned_by_tenant,
        pod_controller_owner_name_and_uid, pod_has_owner_kind, pod_matches_policy_controller_kind,
        replicaset_matches_pod_controller_and_tenant, should_create_rbac,
        should_mark_reconcile_started, statefulset_matches_pod_controller_and_tenant,
    };
    use crate::types::v1alpha1::status::Status;
    use k8s_openapi::api::apps::v1 as appsv1;
    use k8s_openapi::api::core::v1 as corev1;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

    fn node_with_ready_status(status: &str) -> corev1::Node {
        corev1::Node {
            status: Some(corev1::NodeStatus {
                conditions: Some(vec![corev1::NodeCondition {
                    type_: "Ready".to_string(),
                    status: status.to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn node_with_out_of_service_taint() -> corev1::Node {
        let mut node = node_with_ready_status("Unknown");
        node.spec = Some(corev1::NodeSpec {
            taints: Some(vec![corev1::Taint {
                key: super::OUT_OF_SERVICE_TAINT_KEY.to_string(),
                value: Some("nodeshutdown".to_string()),
                effect: "NoExecute".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        });
        node
    }

    fn owner_reference(kind: &str) -> metav1::OwnerReference {
        metav1::OwnerReference {
            api_version: "apps/v1".to_string(),
            kind: kind.to_string(),
            name: "owner".to_string(),
            uid: "uid".to_string(),
            controller: Some(true),
            ..Default::default()
        }
    }

    fn pod_with_owner(kind: &str) -> corev1::Pod {
        corev1::Pod {
            metadata: metav1::ObjectMeta {
                uid: Some("pod-uid".to_string()),
                deletion_timestamp: Some(metav1::Time(chrono::Utc::now())),
                owner_references: Some(vec![owner_reference(kind)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn statefulset_owned_by_test_tenant(uid: &str, tenant_uid: &str) -> appsv1::StatefulSet {
        appsv1::StatefulSet {
            metadata: metav1::ObjectMeta {
                name: Some("owner".to_string()),
                uid: Some(uid.to_string()),
                owner_references: Some(vec![tenant_owner_reference(tenant_uid)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn tenant_owner_reference(uid: &str) -> metav1::OwnerReference {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.uid = Some(uid.to_string());
        tenant.new_owner_ref()
    }

    fn deployment_owner_reference(uid: &str) -> metav1::OwnerReference {
        metav1::OwnerReference {
            api_version: "apps/v1".to_string(),
            kind: "Deployment".to_string(),
            name: "owner-deployment".to_string(),
            uid: uid.to_string(),
            controller: Some(true),
            ..Default::default()
        }
    }

    fn replicaset_owned_by_test_tenant(uid: &str, tenant_uid: &str) -> appsv1::ReplicaSet {
        appsv1::ReplicaSet {
            metadata: metav1::ObjectMeta {
                name: Some("owner".to_string()),
                uid: Some(uid.to_string()),
                owner_references: Some(vec![tenant_owner_reference(tenant_uid)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn replicaset_owned_by_deployment(uid: &str, deployment_uid: &str) -> appsv1::ReplicaSet {
        appsv1::ReplicaSet {
            metadata: metav1::ObjectMeta {
                name: Some("owner".to_string()),
                uid: Some(uid.to_string()),
                owner_references: Some(vec![deployment_owner_reference(deployment_uid)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn deployment_owned_by_test_tenant(uid: &str, tenant_uid: &str) -> appsv1::Deployment {
        appsv1::Deployment {
            metadata: metav1::ObjectMeta {
                name: Some("owner-deployment".to_string()),
                uid: Some(uid.to_string()),
                owner_references: Some(vec![tenant_owner_reference(tenant_uid)]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn object_meta_with_owner_reference(owner: metav1::OwnerReference) -> metav1::ObjectMeta {
        metav1::ObjectMeta {
            owner_references: Some(vec![owner]),
            ..Default::default()
        }
    }

    #[test]
    fn test_object_owner_match_requires_tenant_api_version() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut owner_ref = tenant_owner_reference("test-uid-123");

        assert!(object_owned_by_tenant(
            &object_meta_with_owner_reference(owner_ref.clone()),
            &tenant
        ));

        owner_ref.api_version = "rustfs.com/v1beta1".to_string();
        assert!(!object_owned_by_tenant(
            &object_meta_with_owner_reference(owner_ref),
            &tenant
        ));
    }

    #[test]
    fn test_object_owner_match_requires_controller_tenant_ref() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut non_controller_owner = tenant_owner_reference("test-uid-123");
        non_controller_owner.controller = Some(false);
        let mut missing_controller_owner = tenant_owner_reference("test-uid-123");
        missing_controller_owner.controller = None;

        assert!(!object_owned_by_tenant(
            &object_meta_with_owner_reference(non_controller_owner),
            &tenant
        ));
        assert!(!object_owned_by_tenant(
            &object_meta_with_owner_reference(missing_controller_owner),
            &tenant
        ));
    }

    #[test]
    fn test_object_owner_match_requires_tenant_uid() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.uid = None;

        assert!(!object_owned_by_tenant(
            &object_meta_with_owner_reference(tenant_owner_reference("")),
            &tenant
        ));
    }

    #[test]
    fn test_legacy_delete_policy_does_not_make_bare_labeled_pod_owned() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let tenant = crate::tests::create_test_tenant(None, None);
        let pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                name: Some("bare-labeled-pod".to_string()),
                labels: Some(std::collections::BTreeMap::from([(
                    "rustfs.tenant".to_string(),
                    "test-tenant".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(pod_matches_policy_controller_kind(&pod, &P::ForceDelete));
        assert!(!object_owned_by_tenant(&pod.metadata, &tenant));
    }

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
        let node = node_with_ready_status("True");
        assert!(!is_node_down(&node));
    }

    #[test]
    fn test_is_node_down_ready_false() {
        let node = node_with_ready_status("False");
        assert!(is_node_down(&node));
    }

    #[test]
    fn test_is_node_down_ready_unknown() {
        let node = node_with_ready_status("Unknown");
        assert!(is_node_down(&node));
    }

    #[test]
    fn test_node_deletion_safety_ready_unknown_without_taint_is_unfenced() {
        let node = node_with_ready_status("Unknown");
        let pod = pod_with_owner("StatefulSet");

        assert_eq!(
            node_pod_deletion_safety(&node, &pod),
            NodePodDeletionSafety::DownUnfenced
        );
    }

    #[test]
    fn test_node_deletion_safety_out_of_service_taint_is_fenced() {
        let node = node_with_out_of_service_taint();
        let pod = pod_with_owner("StatefulSet");

        assert_eq!(
            node_pod_deletion_safety(&node, &pod),
            NodePodDeletionSafety::Fenced
        );
    }

    #[test]
    fn test_node_deletion_safety_ready_true_with_out_of_service_taint_is_ready() {
        let mut node = node_with_out_of_service_taint();
        node.status = Some(corev1::NodeStatus {
            conditions: Some(vec![corev1::NodeCondition {
                type_: "Ready".to_string(),
                status: "True".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        });
        let pod = pod_with_owner("StatefulSet");

        assert_eq!(
            node_pod_deletion_safety(&node, &pod),
            NodePodDeletionSafety::Ready
        );
    }

    #[test]
    fn test_node_deletion_safety_tolerated_out_of_service_taint_is_unfenced() {
        let node = node_with_out_of_service_taint();
        let mut pod = pod_with_owner("StatefulSet");
        pod.spec = Some(corev1::PodSpec {
            tolerations: Some(vec![corev1::Toleration {
                key: Some(super::OUT_OF_SERVICE_TAINT_KEY.to_string()),
                operator: Some("Exists".to_string()),
                effect: Some("NoExecute".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        });

        assert_eq!(
            node_pod_deletion_safety(&node, &pod),
            NodePodDeletionSafety::DownUnfenced
        );
    }

    #[test]
    fn test_node_deletion_safety_expired_toleration_no_longer_blocks_fencing() {
        let mut node = node_with_out_of_service_taint();
        node.spec
            .as_mut()
            .and_then(|spec| spec.taints.as_mut())
            .and_then(|taints| taints.first_mut())
            .expect("out-of-service taint should exist")
            .time_added = Some(metav1::Time(
            chrono::Utc::now() - chrono::Duration::seconds(60),
        ));
        let mut pod = pod_with_owner("StatefulSet");
        pod.spec = Some(corev1::PodSpec {
            tolerations: Some(vec![corev1::Toleration {
                key: Some(super::OUT_OF_SERVICE_TAINT_KEY.to_string()),
                operator: Some("Exists".to_string()),
                effect: Some("NoExecute".to_string()),
                toleration_seconds: Some(1),
                ..Default::default()
            }]),
            ..Default::default()
        });

        assert_eq!(
            node_pod_deletion_safety(&node, &pod),
            NodePodDeletionSafety::Fenced
        );
    }

    #[test]
    fn test_pod_owner_kind_helpers() {
        let pod = pod_with_owner("StatefulSet");

        assert!(pod_has_owner_kind(&pod, "StatefulSet"));
        assert!(!pod_has_owner_kind(&pod, "ReplicaSet"));
        assert_eq!(
            pod_controller_owner_name_and_uid(&pod, "StatefulSet"),
            Some(("owner".to_string(), "uid".to_string()))
        );
    }

    #[test]
    fn test_pod_owner_kind_requires_controller_owner() {
        let pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                owner_references: Some(vec![metav1::OwnerReference {
                    api_version: "apps/v1".to_string(),
                    kind: "StatefulSet".to_string(),
                    name: "ss".to_string(),
                    uid: "uid".to_string(),
                    controller: Some(false),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!pod_has_owner_kind(&pod, "StatefulSet"));
    }

    #[test]
    fn test_statefulset_owner_match_requires_tenant_and_statefulset_uid() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pod = pod_with_owner("StatefulSet");
        let matching_statefulset = statefulset_owned_by_test_tenant("uid", "test-uid-123");
        let wrong_statefulset_uid = statefulset_owned_by_test_tenant("other-uid", "test-uid-123");
        let wrong_tenant_uid = statefulset_owned_by_test_tenant("uid", "other-tenant-uid");

        assert!(statefulset_matches_pod_controller_and_tenant(
            &pod,
            &matching_statefulset,
            &tenant
        ));
        assert!(!statefulset_matches_pod_controller_and_tenant(
            &pod,
            &wrong_statefulset_uid,
            &tenant
        ));
        assert!(!statefulset_matches_pod_controller_and_tenant(
            &pod,
            &wrong_tenant_uid,
            &tenant
        ));
    }

    #[test]
    fn test_replicaset_owner_match_requires_tenant_owner_chain_and_uid() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pod = pod_with_owner("ReplicaSet");
        let matching_replicaset = replicaset_owned_by_test_tenant("uid", "test-uid-123");
        let wrong_replicaset_uid = replicaset_owned_by_test_tenant("other-uid", "test-uid-123");
        let deployment_owned_replicaset = replicaset_owned_by_deployment("uid", "deployment-uid");
        let matching_deployment = deployment_owned_by_test_tenant("deployment-uid", "test-uid-123");
        let wrong_deployment_tenant =
            deployment_owned_by_test_tenant("deployment-uid", "other-tenant-uid");

        assert!(replicaset_matches_pod_controller_and_tenant(
            &pod,
            &matching_replicaset,
            None,
            &tenant,
            false
        ));
        assert!(!replicaset_matches_pod_controller_and_tenant(
            &pod,
            &wrong_replicaset_uid,
            None,
            &tenant,
            false
        ));
        assert!(replicaset_matches_pod_controller_and_tenant(
            &pod,
            &deployment_owned_replicaset,
            Some(&matching_deployment),
            &tenant,
            false
        ));
        assert!(!replicaset_matches_pod_controller_and_tenant(
            &pod,
            &deployment_owned_replicaset,
            Some(&wrong_deployment_tenant),
            &tenant,
            false
        ));
        assert!(!replicaset_matches_pod_controller_and_tenant(
            &pod,
            &deployment_owned_replicaset,
            None,
            &tenant,
            false
        ));
    }

    #[test]
    fn test_deployment_policy_requires_deployment_owned_replicaset() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pod = pod_with_owner("ReplicaSet");
        let direct_tenant_replicaset = replicaset_owned_by_test_tenant("uid", "test-uid-123");
        let deployment_owned_replicaset = replicaset_owned_by_deployment("uid", "deployment-uid");
        let matching_deployment = deployment_owned_by_test_tenant("deployment-uid", "test-uid-123");

        assert!(!replicaset_matches_pod_controller_and_tenant(
            &pod,
            &direct_tenant_replicaset,
            None,
            &tenant,
            true
        ));
        assert!(replicaset_matches_pod_controller_and_tenant(
            &pod,
            &deployment_owned_replicaset,
            Some(&matching_deployment),
            &tenant,
            true
        ));
    }

    #[test]
    fn test_force_delete_policies_require_fencing() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        assert!(force_delete_requires_fencing(&P::ForceDelete));
        assert!(force_delete_requires_fencing(&P::DeleteStatefulSetPod));
        assert!(force_delete_requires_fencing(&P::DeleteDeploymentPod));
        assert!(force_delete_requires_fencing(
            &P::DeleteBothStatefulSetAndDeploymentPod
        ));
        assert!(!force_delete_requires_fencing(&P::Delete));
        assert!(!force_delete_requires_fencing(&P::DoNothing));
    }

    #[test]
    fn test_cleanup_decision_force_delete_uses_uid_precondition() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let pod = pod_with_owner("StatefulSet");
        let decision =
            cleanup_decision_for_pod(&pod, &P::ForceDelete, NodePodDeletionSafety::Fenced);

        let PodCleanupDecision::Delete(plan) = decision else {
            panic!("expected force-delete plan");
        };
        assert!(plan.force);
        assert_eq!(plan.precondition_uid.as_deref(), Some("pod-uid"));
        assert_eq!(plan.event_reason, "ForceDeletedPodOnDownNode");

        let delete_params = plan.delete_params();
        assert_eq!(delete_params.grace_period_seconds, Some(0));
        assert_eq!(
            delete_params
                .preconditions
                .as_ref()
                .and_then(|preconditions| preconditions.uid.as_deref()),
            Some("pod-uid")
        );
    }

    #[test]
    fn test_cleanup_decision_normal_delete_keeps_uid_precondition_without_force() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let pod = pod_with_owner("StatefulSet");
        let decision =
            cleanup_decision_for_pod(&pod, &P::Delete, NodePodDeletionSafety::DownUnfenced);

        let PodCleanupDecision::Delete(plan) = decision else {
            panic!("expected normal delete plan");
        };
        assert!(!plan.force);
        assert_eq!(plan.precondition_uid.as_deref(), Some("pod-uid"));
        assert_eq!(plan.event_reason, "RequestedPodDeleteOnDownNode");

        let delete_params = plan.delete_params();
        assert_eq!(delete_params.grace_period_seconds, None);
        assert_eq!(
            delete_params
                .preconditions
                .as_ref()
                .and_then(|preconditions| preconditions.uid.as_deref()),
            Some("pod-uid")
        );
    }

    #[test]
    fn test_cleanup_decision_skips_unfenced_force_delete() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let statefulset_pod = pod_with_owner("StatefulSet");
        let deployment_pod = pod_with_owner("ReplicaSet");

        assert_eq!(
            cleanup_decision_for_pod(
                &statefulset_pod,
                &P::ForceDelete,
                NodePodDeletionSafety::DownUnfenced
            ),
            PodCleanupDecision::SkipForceDeleteNeedsFencing
        );
        assert_eq!(
            cleanup_decision_for_pod(
                &statefulset_pod,
                &P::DeleteStatefulSetPod,
                NodePodDeletionSafety::DownUnfenced
            ),
            PodCleanupDecision::SkipForceDeleteNeedsFencing
        );
        assert_eq!(
            cleanup_decision_for_pod(
                &deployment_pod,
                &P::DeleteDeploymentPod,
                NodePodDeletionSafety::DownUnfenced
            ),
            PodCleanupDecision::SkipForceDeleteNeedsFencing
        );
    }

    #[test]
    fn test_cleanup_decision_skips_ready_node_and_controller_mismatch() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let ss_pod = pod_with_owner("StatefulSet");
        let deploy_pod = pod_with_owner("ReplicaSet");

        assert_eq!(
            cleanup_decision_for_pod(&ss_pod, &P::Delete, NodePodDeletionSafety::Ready),
            PodCleanupDecision::Skip
        );
        assert_eq!(
            cleanup_decision_for_pod(
                &deploy_pod,
                &P::DeleteStatefulSetPod,
                NodePodDeletionSafety::Fenced
            ),
            PodCleanupDecision::Skip
        );
    }

    #[test]
    fn test_policy_controller_kind_matching_longhorn_like() {
        use crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown as P;

        let ss_pod = pod_with_owner("StatefulSet");
        let deploy_pod = pod_with_owner("ReplicaSet");

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
