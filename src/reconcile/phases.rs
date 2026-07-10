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

use super::pool_lifecycle::{PoolLifecycleDecision, PoolLifecycleDecisions};
use super::provisioning::{ProvisioningOutcome, reconcile_provisioning};
use super::{
    Error, cleanup_stuck_terminating_pods_on_down_nodes, context, context_result,
    patch_status_and_record, patch_status_error, statefulset_owned_by_tenant, types_result,
};
use crate::context::Context;
use crate::status::{StatusBuilder, StatusError};
use crate::types;
use crate::types::v1alpha1::status::pool::PoolLifecycleState;
use crate::types::v1alpha1::status::{ConditionType, Reason};
use crate::types::v1alpha1::tenant::Tenant;
use crate::types::v1alpha1::tls::TlsPlan;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, PropagationPolicy};
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Default)]
pub(super) struct PoolReconcileSummary {
    pool_statuses: Vec<crate::types::v1alpha1::status::pool::Pool>,
    any_updating: bool,
    any_degraded: bool,
    any_lifecycle_reconciling: bool,
    any_removed_pool_cleanup_reconciling: bool,
    any_lifecycle_decommissioned: bool,
    any_lifecycle_failed: bool,
    any_lifecycle_canceled: bool,
    lifecycle_requeue_after: Option<Duration>,
    total_replicas: i32,
    ready_replicas: i32,
}

const REMOVED_POOL_CLEANUP_REQUEUE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct RemovedDecommissionedPoolCleanup {
    pub(super) allowed_removed_pool_names: HashSet<String>,
    pub(super) any_reconciling: bool,
    pub(super) requeue_after: Option<Duration>,
}

impl RemovedDecommissionedPoolCleanup {
    fn mark_reconciling(&mut self) {
        self.any_reconciling = true;
        self.requeue_after = Some(REMOVED_POOL_CLEANUP_REQUEUE_INTERVAL);
    }
}

pub(super) async fn validate_tenant_prerequisites(
    ctx: &Context,
    tenant: &Tenant,
) -> Result<(), Error> {
    // Validate tenant name is DNS-1035 compliant (required for derived Service names).
    if let Err(e) = tenant.validate_name() {
        let status_error = StatusError::from_types_error(&e);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    if let Err(e) = tenant.validate_pools() {
        let status_error = StatusError::from_types_error(&e);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    // Validate credential Secret if configured.
    // This only validates the Secret exists and has required keys.
    // Actual credential injection happens via secretKeyRef in the StatefulSet.
    if let Some(ref cfg) = tenant.spec.creds_secret
        && !cfg.name.is_empty()
        && let Err(e) = ctx.validate_credential_secret(tenant).await
    {
        let status_error = StatusError::from_context_error(&e);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    // Validate encryption / KMS and reject raw RUSTFS_KMS_* env overrides even when
    // spec.encryption is omitted or disabled.
    if let Err(e) = ctx.validate_kms_secret(tenant).await {
        let status_error = StatusError::from_context_error(&e);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    // Warn if Local backend has a kmsSecret configured (not used for Local).
    if let Some(ref enc) = tenant.spec.encryption
        && enc.enabled
        && enc.backend == crate::types::v1alpha1::encryption::KmsBackendType::Local
        && enc.kms_secret.as_ref().is_some_and(|s| !s.name.is_empty())
    {
        let _ = ctx
            .record(
                tenant,
                EventType::Warning,
                "KmsConfigWarning",
                "Local KMS backend ignores kmsSecret; use spec.encryption.local.masterKeySecretRef for the local master key",
            )
            .await;
    }

    Ok(())
}

pub(super) async fn maybe_cleanup_terminating_pods(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), Error> {
    // Optional: unblock StatefulSet pods stuck terminating when their node is down.
    // This is inspired by Longhorn's "Pod Deletion Policy When Node is Down".
    if let Some(policy) = tenant.spec.pod_deletion_policy_when_node_is_down.clone()
        && policy != crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing
    {
        cleanup_stuck_terminating_pods_on_down_nodes(tenant, namespace, ctx, policy).await?;
    }
    Ok(())
}

pub(super) fn should_create_rbac(tenant: &Tenant) -> bool {
    let custom_sa = tenant.spec.service_account_name.is_some();
    let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
    !custom_sa || create_rbac
}

pub(super) async fn reconcile_rbac_resources(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), Error> {
    if !should_create_rbac(tenant) {
        return Ok(());
    }

    let role = context_result(ctx.apply(&tenant.new_role(), namespace).await, ctx, tenant).await?;

    if tenant.spec.service_account_name.is_some() {
        let sa_name = tenant.service_account_name();
        context_result(
            ctx.apply(&tenant.new_role_binding(&sa_name, &role), namespace)
                .await,
            ctx,
            tenant,
        )
        .await?;
    } else {
        let service_account = context_result(
            ctx.apply(&tenant.new_service_account(), namespace).await,
            ctx,
            tenant,
        )
        .await?;
        context_result(
            ctx.apply(
                &tenant.new_role_binding(&service_account.name_any(), &role),
                namespace,
            )
            .await,
            ctx,
            tenant,
        )
        .await?;
    }

    Ok(())
}

pub(super) async fn reconcile_services(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    tls_plan: &TlsPlan,
) -> Result<(), Error> {
    context_result(
        ctx.apply(&tenant.new_io_service_with_tls_plan(tls_plan), namespace)
            .await,
        ctx,
        tenant,
    )
    .await?;
    context_result(
        ctx.apply(&tenant.new_console_service(), namespace).await,
        ctx,
        tenant,
    )
    .await?;
    context_result(
        ctx.apply(
            &tenant.new_headless_service_with_tls_plan(tls_plan),
            namespace,
        )
        .await,
        ctx,
        tenant,
    )
    .await?;

    Ok(())
}

pub(super) async fn cleanup_removed_decommissioned_pool_statefulsets(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<RemovedDecommissionedPoolCleanup, Error> {
    let owned_statefulsets = context_result(
        ctx.list_with_params::<k8s_openapi::api::apps::v1::StatefulSet>(
            namespace,
            &ListParams::default().labels(&format!("rustfs.tenant={}", tenant.name())),
        )
        .await,
        ctx,
        tenant,
    )
    .await?;

    let current_pool_names: HashSet<_> =
        tenant.spec.pools.iter().map(|p| p.name.as_str()).collect();
    let tenant_prefix = format!("{}-", tenant.name());
    let mut cleanup = RemovedDecommissionedPoolCleanup::default();

    for ss in owned_statefulsets
        .iter()
        .filter(|ss| statefulset_owned_by_tenant(ss, tenant))
    {
        let Some(ss_name) = ss.metadata.name.as_deref() else {
            continue;
        };
        let Some(pool_name) = ss_name.strip_prefix(&tenant_prefix) else {
            continue;
        };
        if current_pool_names.contains(pool_name) {
            continue;
        }
        if !removed_pool_is_decommissioned(tenant, pool_name, ss_name) {
            continue;
        }

        cleanup
            .allowed_removed_pool_names
            .insert(pool_name.to_string());
        if ss.metadata.deletion_timestamp.is_some() {
            cleanup.mark_reconciling();
            continue;
        }

        let delete_params = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..DeleteParams::default()
        };
        debug!(
            tenant = %tenant.name(),
            namespace = %namespace,
            pool = %pool_name,
            statefulset = %ss_name,
            "deleting StatefulSet for removed decommissioned pool"
        );
        let delete_requested = match ctx
            .delete_with_params::<k8s_openapi::api::apps::v1::StatefulSet>(
                ss_name,
                namespace,
                &delete_params,
            )
            .await
        {
            Ok(()) => true,
            Err(error) if is_not_found_context_error(&error) => false,
            Err(error) => {
                let status_error = StatusError::from_context_error(&error);
                patch_status_error(ctx, tenant, &status_error).await;
                return Err(error.into());
            }
        };

        if delete_requested {
            cleanup.mark_reconciling();
            let _ = ctx
                .record(
                    tenant,
                    EventType::Normal,
                    "PoolRemoved",
                    &format!(
                        "Deleting StatefulSet '{}' after decommissioned pool was removed from spec",
                        ss_name
                    ),
                )
                .await;
        }
    }

    Ok(cleanup)
}

fn removed_pool_is_decommissioned(tenant: &Tenant, pool_name: &str, ss_name: &str) -> bool {
    tenant.status.as_ref().is_some_and(|status| {
        status.pools.iter().any(|pool_status| {
            (pool_status.name.as_deref() == Some(pool_name) || pool_status.ss_name == ss_name)
                && matches!(
                    pool_status.lifecycle_state,
                    Some(PoolLifecycleState::Decommissioned)
                )
        })
    })
}

pub(super) async fn validate_no_pool_rename(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    allowed_removed_pool_names: &HashSet<String>,
) -> Result<(), Error> {
    let owned_statefulsets = context_result(
        ctx.list_with_params::<k8s_openapi::api::apps::v1::StatefulSet>(
            namespace,
            &ListParams::default().labels(&format!("rustfs.tenant={}", tenant.name())),
        )
        .await,
        ctx,
        tenant,
    )
    .await?;

    let current_pool_names: std::collections::HashSet<_> =
        tenant.spec.pools.iter().map(|p| p.name.as_str()).collect();

    let tenant_prefix = format!("{}-", tenant.name());
    let existing_pool_names: std::collections::HashSet<String> = owned_statefulsets
        .iter()
        .filter(|ss| ss.metadata.deletion_timestamp.is_none())
        .filter(|ss| statefulset_owned_by_tenant(ss, tenant))
        .filter_map(|ss| {
            ss.metadata
                .name
                .as_deref()
                .and_then(|name| name.strip_prefix(&tenant_prefix))
                .map(ToOwned::to_owned)
        })
        .collect();

    let mut removed_pool_names: Vec<_> = existing_pool_names
        .iter()
        .filter(|pool_name| !current_pool_names.contains(pool_name.as_str()))
        .filter(|pool_name| !allowed_removed_pool_names.contains(*pool_name))
        .cloned()
        .collect();
    removed_pool_names.sort_unstable();
    let mut added_pool_names: Vec<_> = current_pool_names
        .iter()
        .filter(|pool_name| !existing_pool_names.contains::<str>(*pool_name))
        .cloned()
        .collect();
    added_pool_names.sort_unstable();

    if removed_pool_names.is_empty() {
        return Ok(());
    }

    warn!(
        tenant = %tenant.name(),
        namespace = %namespace,
        removed_pools = ?removed_pool_names,
        added_pools = ?added_pool_names,
        "detected pool removal or rename while owned StatefulSets still exist"
    );
    let err = if added_pool_names.is_empty() {
        types::error::Error::PoolDeleteBlocked {
            name: tenant.name(),
            message: format!(
                "Pool(s) '{}' were removed from spec while owned StatefulSets still exist. Restore the pool spec before starting a controlled decommission.",
                removed_pool_names.join(",")
            ),
        }
    } else {
        types::error::Error::ImmutableFieldModified {
            name: tenant.name(),
            field: "spec.pools[].name".to_string(),
            message: format!(
                "Pool name cannot be changed. Removed pool(s) '{}' and added pool(s) '{}' in the same spec change.",
                removed_pool_names.join(","),
                added_pool_names.join(",")
            ),
        }
    };
    let status_error = StatusError::from_types_error(&err);
    patch_status_error(ctx, tenant, &status_error).await;

    Err(err.into())
}

pub(super) async fn reconcile_pool_statefulsets(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    tls_plan: &TlsPlan,
    lifecycle_decisions: &PoolLifecycleDecisions,
    removed_pool_cleanup: &RemovedDecommissionedPoolCleanup,
) -> Result<PoolReconcileSummary, Error> {
    let mut summary = PoolReconcileSummary {
        any_lifecycle_reconciling: lifecycle_decisions.any_reconciling,
        any_removed_pool_cleanup_reconciling: removed_pool_cleanup.any_reconciling,
        any_lifecycle_failed: lifecycle_decisions.any_failed,
        any_lifecycle_canceled: lifecycle_decisions.any_canceled,
        lifecycle_requeue_after: earliest_requeue_after(
            lifecycle_decisions.requeue_after,
            removed_pool_cleanup.requeue_after,
        ),
        ..Default::default()
    };

    let mut existing_pool_statefulsets = Vec::new();
    let mut created_missing_pool = false;

    for pool in &tenant.spec.pools {
        let ss_name = format!("{}-{}", tenant.name(), pool.name);
        let lifecycle_decision = lifecycle_decisions.decision_for(&pool.name);
        if lifecycle_decision.is_some_and(|decision| decision.skip_workload_reconcile) {
            reconcile_lifecycle_gated_pool_statefulset(
                ctx,
                tenant,
                namespace,
                pool,
                &ss_name,
                lifecycle_decision,
                &mut summary,
            )
            .await?;
            continue;
        }

        match ctx
            .get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, namespace)
            .await
        {
            Ok(existing_ss) => {
                existing_pool_statefulsets.push((pool, existing_ss));
            }
            Err(e) if is_not_found_context_error(&e) => {
                reconcile_missing_pool_statefulset(
                    ctx,
                    tenant,
                    namespace,
                    pool,
                    &ss_name,
                    tls_plan,
                    &mut summary,
                )
                .await?;
                created_missing_pool = true;
            }
            Err(e) => {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    statefulset = %ss_name,
                    error = %e,
                    "failed to get pool StatefulSet"
                );
                let status_error = StatusError::from_context_error(&e);
                patch_status_error(ctx, tenant, &status_error).await;
                return Err(e.into());
            }
        }
    }

    if created_missing_pool {
        for (pool, existing_ss) in existing_pool_statefulsets {
            let pool_status = tenant.build_pool_status(&pool.name, &existing_ss);
            update_pool_summary(&mut summary, pool_status);
        }
        return Ok(summary);
    }

    for (pool, existing_ss) in existing_pool_statefulsets {
        reconcile_existing_pool_statefulset(
            ctx,
            tenant,
            namespace,
            pool,
            existing_ss,
            tls_plan,
            &mut summary,
        )
        .await?;
    }

    Ok(summary)
}

fn earliest_requeue_after(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn is_not_found_context_error(error: &context::Error) -> bool {
    matches!(
        error,
        context::Error::Kube {
            source: kube::Error::Api(api_error)
        } if api_error.code == 404
    )
}

async fn reconcile_lifecycle_gated_pool_statefulset(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &crate::types::v1alpha1::pool::Pool,
    ss_name: &str,
    lifecycle_decision: Option<&PoolLifecycleDecision>,
    summary: &mut PoolReconcileSummary,
) -> Result<(), Error> {
    debug!(
        tenant = %tenant.name(),
        namespace = %namespace,
        pool = %pool.name,
        "skipping normal StatefulSet reconcile because pool lifecycle gate is active"
    );

    let mut pool_status = match ctx
        .get::<k8s_openapi::api::apps::v1::StatefulSet>(ss_name, namespace)
        .await
    {
        Ok(ss) => tenant.build_pool_status(&pool.name, &ss),
        Err(error) if is_not_found_context_error(&error) => missing_pool_status(tenant, &pool.name),
        Err(error) => {
            let status_error = StatusError::from_context_error(&error);
            patch_status_error(ctx, tenant, &status_error).await;
            return Err(error.into());
        }
    };

    if let Some(decision) = lifecycle_decision {
        apply_lifecycle_decision(&mut pool_status, decision);
    }

    update_pool_summary(summary, pool_status);

    Ok(())
}

fn missing_pool_status(
    tenant: &Tenant,
    pool_name: &str,
) -> crate::types::v1alpha1::status::pool::Pool {
    crate::types::v1alpha1::status::pool::Pool {
        name: Some(pool_name.to_string()),
        ss_name: format!("{}-{}", tenant.name(), pool_name),
        state: crate::types::v1alpha1::status::pool::PoolState::NotCreated,
        lifecycle_state: Some(PoolLifecycleState::Active),
        workload_state: Some(crate::types::v1alpha1::status::pool::PoolState::NotCreated),
        decommission: None,
        replicas: None,
        ready_replicas: None,
        current_replicas: None,
        updated_replicas: None,
        current_revision: None,
        update_revision: None,
        last_update_time: Some(
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ),
    }
}

fn apply_lifecycle_decision(
    pool_status: &mut crate::types::v1alpha1::status::pool::Pool,
    decision: &PoolLifecycleDecision,
) {
    pool_status.lifecycle_state = Some(decision.state.clone());
    pool_status.workload_state = Some(pool_status.state.clone());
    pool_status.decommission = decision.decommission.clone();
}

async fn reconcile_existing_pool_statefulset(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &crate::types::v1alpha1::pool::Pool,
    existing_ss: k8s_openapi::api::apps::v1::StatefulSet,
    tls_plan: &TlsPlan,
    summary: &mut PoolReconcileSummary,
) -> Result<(), Error> {
    let ss_name = existing_ss.name_any();
    debug!(
        tenant = %tenant.name(),
        namespace = %namespace,
        pool = %pool.name,
        statefulset = %ss_name,
        "checking existing pool StatefulSet"
    );

    if let Err(e) = tenant.validate_statefulset_update_with_tls_plan(&existing_ss, pool, tls_plan) {
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            pool = %pool.name,
            statefulset = %ss_name,
            error = %e,
            "StatefulSet update validation failed"
        );

        let status_error = if matches!(e, types::error::Error::KmsMigrationBlocked { .. }) {
            StatusError::from_types_error(&e)
        } else {
            StatusError::statefulset_update_validation_failed(&ss_name)
        };
        patch_status_error(ctx, tenant, &status_error).await;
        let _ = ctx
            .record(
                tenant,
                EventType::Warning,
                "StatefulSetUpdateBlocked",
                &format!("StatefulSet '{}' update blocked: {}", ss_name, e),
            )
            .await;
        return Err(e.into());
    }

    if types_result(
        tenant.statefulset_needs_update_with_tls_plan(&existing_ss, pool, tls_plan),
        ctx,
        tenant,
    )
    .await?
    {
        info!(
            tenant = %tenant.name(),
            namespace = %namespace,
            pool = %pool.name,
            statefulset = %ss_name,
            "applying StatefulSet update"
        );

        let _ = ctx
            .record(
                tenant,
                EventType::Normal,
                "StatefulSetUpdateStarted",
                &format!("Updating StatefulSet {}", ss_name),
            )
            .await;

        let desired = types_result(
            tenant.new_statefulset_with_tls_plan(pool, tls_plan),
            ctx,
            tenant,
        )
        .await?;
        if let Err(e) = ctx.apply(&desired, namespace).await {
            let status_error = StatusError::statefulset_apply_failed(&ss_name);
            patch_status_error(ctx, tenant, &status_error).await;
            return Err(e.into());
        }

        info!(
            tenant = %tenant.name(),
            namespace = %namespace,
            pool = %pool.name,
            statefulset = %ss_name,
            "StatefulSet updated successfully"
        );
    } else {
        debug!(
            tenant = %tenant.name(),
            namespace = %namespace,
            pool = %pool.name,
            statefulset = %ss_name,
            "StatefulSet is up to date"
        );
    }

    let ss = context_result(
        ctx.get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, namespace)
            .await,
        ctx,
        tenant,
    )
    .await?;
    let pool_status = tenant.build_pool_status(&pool.name, &ss);
    update_pool_summary(summary, pool_status);

    Ok(())
}

async fn reconcile_missing_pool_statefulset(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &crate::types::v1alpha1::pool::Pool,
    ss_name: &str,
    tls_plan: &TlsPlan,
    summary: &mut PoolReconcileSummary,
) -> Result<(), Error> {
    info!(
        tenant = %tenant.name(),
        namespace = %namespace,
        pool = %pool.name,
        statefulset = %ss_name,
        "creating missing StatefulSet"
    );

    let _ = ctx
        .record(
            tenant,
            EventType::Normal,
            "StatefulSetCreated",
            &format!("Creating StatefulSet {}", ss_name),
        )
        .await;

    let desired = types_result(
        tenant.new_statefulset_with_tls_plan(pool, tls_plan),
        ctx,
        tenant,
    )
    .await?;
    if let Err(e) = ctx.apply(&desired, namespace).await {
        let status_error = StatusError::statefulset_apply_failed(ss_name);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    info!(
        tenant = %tenant.name(),
        namespace = %namespace,
        pool = %pool.name,
        statefulset = %ss_name,
        "StatefulSet created successfully"
    );

    let ss = context_result(
        ctx.get::<k8s_openapi::api::apps::v1::StatefulSet>(ss_name, namespace)
            .await,
        ctx,
        tenant,
    )
    .await?;
    let pool_status = tenant.build_pool_status(&pool.name, &ss);
    summary.any_updating = true; // New StatefulSet is always updating initially.
    update_pool_summary(summary, pool_status);

    Ok(())
}

fn update_pool_summary(
    summary: &mut PoolReconcileSummary,
    pool_status: crate::types::v1alpha1::status::pool::Pool,
) {
    use crate::types::v1alpha1::status::pool::PoolState;

    match pool_status.state {
        PoolState::Updating => summary.any_updating = true,
        PoolState::Degraded | PoolState::RolloutFailed => summary.any_degraded = true,
        _ => {}
    }

    match pool_status.lifecycle_state {
        Some(PoolLifecycleState::Decommissioning) => summary.any_lifecycle_reconciling = true,
        Some(PoolLifecycleState::Decommissioned) => summary.any_lifecycle_decommissioned = true,
        Some(PoolLifecycleState::DecommissionFailed) => summary.any_lifecycle_failed = true,
        Some(PoolLifecycleState::DecommissionCanceled) => summary.any_lifecycle_canceled = true,
        _ => {}
    }

    if matches!(
        pool_status.lifecycle_state,
        Some(PoolLifecycleState::Decommissioned)
    ) {
        summary.pool_statuses.push(pool_status);
        return;
    }

    if let Some(replicas) = pool_status.replicas {
        summary.total_replicas += replicas;
    }
    if let Some(ready) = pool_status.ready_replicas {
        summary.ready_replicas += ready;
    }

    summary.pool_statuses.push(pool_status);
}

pub(super) async fn finalize_tenant_status(
    ctx: &Context,
    tenant: &Tenant,
    summary: PoolReconcileSummary,
    tls_plan: TlsPlan,
) -> Result<Action, Error> {
    let mut builder = StatusBuilder::from_tenant(tenant);
    let pool_count = summary.pool_statuses.len();
    builder.set_pool_statuses(summary.pool_statuses);
    if let Some(tls_status) = tls_plan.status {
        builder.set_tls_status(tls_status);
    }

    let (event_condition, event_reason, event_type, event_message) = if summary.any_lifecycle_failed
    {
        builder.finish_degraded(
            Reason::PoolDecommissionFailed,
            ConditionType::PoolsReady,
            "One or more pool decommission operations failed".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDecommissionFailed,
            EventType::Warning,
            "One or more pool decommission operations failed".to_string(),
        )
    } else if summary.any_lifecycle_canceled {
        builder.finish_degraded(
            Reason::PoolDecommissionCanceled,
            ConditionType::PoolsReady,
            "One or more pool decommission operations were canceled".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDecommissionCanceled,
            EventType::Warning,
            "One or more pool decommission operations were canceled".to_string(),
        )
    } else if summary.any_removed_pool_cleanup_reconciling {
        builder.finish_reconciling(
            Reason::PoolDecommissioning,
            "Decommissioned pool cleanup is in progress".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDecommissioning,
            EventType::Normal,
            "Decommissioned pool cleanup is in progress".to_string(),
        )
    } else if summary.any_lifecycle_reconciling {
        builder.finish_reconciling(
            Reason::PoolDecommissioning,
            "Pool decommission is in progress".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDecommissioning,
            EventType::Normal,
            "Pool decommission is in progress".to_string(),
        )
    } else if summary.any_lifecycle_decommissioned {
        builder.finish_reconciling(
            Reason::PoolDecommissioned,
            "Pool decommission completed; remove the pool from spec to finish cleanup".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDecommissioned,
            EventType::Normal,
            "Pool decommission completed; remove the pool from spec to finish cleanup".to_string(),
        )
    } else if summary.any_degraded {
        builder.finish_degraded(
            Reason::PoolDegraded,
            ConditionType::PoolsReady,
            "One or more pools are degraded".to_string(),
        );
        (
            ConditionType::PoolsReady,
            Reason::PoolDegraded,
            EventType::Warning,
            "One or more pools are degraded".to_string(),
        )
    } else if summary.any_updating {
        builder.finish_reconciling(
            Reason::RolloutInProgress,
            "StatefulSet rollout in progress".to_string(),
        );
        (
            ConditionType::WorkloadsReady,
            Reason::RolloutInProgress,
            EventType::Normal,
            "StatefulSet rollout in progress".to_string(),
        )
    } else if summary.ready_replicas == summary.total_replicas && summary.total_replicas > 0 {
        let namespace = tenant.namespace()?;
        let provisioning = reconcile_provisioning(ctx, tenant, &namespace).await;
        builder.set_provisioning_status(provisioning.status);
        match provisioning.outcome {
            ProvisioningOutcome::Ready => {
                builder.finish_provisioning_ready();
                (
                    ConditionType::Ready,
                    Reason::ReconcileSucceeded,
                    EventType::Normal,
                    format!(
                        "{}/{} pods ready",
                        summary.ready_replicas, summary.total_replicas
                    ),
                )
            }
            ProvisioningOutcome::Pending { message } => {
                builder.finish_provisioning_pending(message.clone());
                (
                    ConditionType::ProvisioningReady,
                    Reason::ProvisioningPending,
                    EventType::Normal,
                    message,
                )
            }
            ProvisioningOutcome::Failed { reason, message } => {
                builder.finish_provisioning_failed(reason, message.clone());
                (
                    ConditionType::ProvisioningReady,
                    reason,
                    EventType::Warning,
                    message,
                )
            }
        }
    } else {
        builder.finish_reconciling(
            Reason::PodsNotReady,
            format!(
                "{}/{} pods ready",
                summary.ready_replicas, summary.total_replicas
            ),
        );
        (
            ConditionType::WorkloadsReady,
            Reason::PodsNotReady,
            EventType::Normal,
            format!(
                "{}/{} pods ready",
                summary.ready_replicas, summary.total_replicas
            ),
        )
    };

    let status = builder.build();
    debug!(
        tenant = %tenant.name(),
        namespace = ?tenant.namespace(),
        current_state = %status.current_state,
        observed_generation = ?status.observed_generation,
        pool_count,
        condition_count = status.conditions.len(),
        reason = event_reason.as_str(),
        condition = event_condition.as_str(),
        ready_replicas = summary.ready_replicas,
        total_replicas = summary.total_replicas,
        "patching Tenant status if changed"
    );
    patch_status_and_record(
        ctx,
        tenant,
        status,
        event_condition,
        event_reason,
        event_type,
        &event_message,
    )
    .await?;

    if let Some(requeue_after) = summary.lifecycle_requeue_after {
        debug!(
            tenant = %tenant.name(),
            namespace = ?tenant.namespace(),
            seconds = requeue_after.as_secs(),
            "Pool lifecycle is active, requeuing"
        );
        Ok(Action::requeue(requeue_after))
    } else if summary.any_updating {
        debug!(
            tenant = %tenant.name(),
            namespace = ?tenant.namespace(),
            seconds = 10,
            "Pools are updating, requeuing"
        );
        Ok(Action::requeue(Duration::from_secs(10)))
    } else {
        Ok(Action::await_change())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_pool_cleanup_marks_reconciling_and_requeues() {
        let mut cleanup = RemovedDecommissionedPoolCleanup::default();

        cleanup.mark_reconciling();

        assert!(cleanup.any_reconciling);
        assert_eq!(
            cleanup.requeue_after,
            Some(REMOVED_POOL_CLEANUP_REQUEUE_INTERVAL)
        );
    }

    #[test]
    fn earliest_requeue_after_prefers_shorter_duration() {
        assert_eq!(
            earliest_requeue_after(Some(Duration::from_secs(30)), Some(Duration::from_secs(10))),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            earliest_requeue_after(None, Some(Duration::from_secs(10))),
            Some(Duration::from_secs(10))
        );
    }
}
