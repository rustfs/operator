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

use super::{
    Error, cleanup_stuck_terminating_pods_on_down_nodes, context, context_result,
    patch_status_and_record, patch_status_error, statefulset_owned_by_tenant, types_result,
};
use crate::context::Context;
use crate::status::{StatusBuilder, StatusError};
use crate::types;
use crate::types::v1alpha1::status::{ConditionType, Reason};
use crate::types::v1alpha1::tenant::Tenant;
use kube::ResourceExt;
use kube::api::ListParams;
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use std::time::Duration;
use tracing::{debug, error, warn};

#[derive(Default)]
pub(super) struct PoolReconcileSummary {
    pool_statuses: Vec<crate::types::v1alpha1::status::pool::Pool>,
    any_updating: bool,
    any_degraded: bool,
    total_replicas: i32,
    ready_replicas: i32,
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

    // Validate encryption / KMS: Vault requires endpoint + kmsSecret (and correct keys);
    // must run whenever encryption is enabled — not only when kmsSecret is set, or Vault
    // without a Secret reference would skip validation entirely.
    if let Some(ref enc) = tenant.spec.encryption
        && enc.enabled
        && let Err(e) = ctx.validate_kms_secret(tenant).await
    {
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
                "Local KMS backend does not use kmsSecret; the Secret reference will be ignored",
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
) -> Result<(), Error> {
    context_result(
        ctx.apply(&tenant.new_io_service(), namespace).await,
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
        ctx.apply(&tenant.new_headless_service(), namespace).await,
        ctx,
        tenant,
    )
    .await?;

    Ok(())
}

pub(super) async fn validate_no_pool_rename(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
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
) -> Result<PoolReconcileSummary, Error> {
    let mut summary = PoolReconcileSummary::default();

    for pool in &tenant.spec.pools {
        let ss_name = format!("{}-{}", tenant.name(), pool.name);
        match ctx
            .get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, namespace)
            .await
        {
            Ok(existing_ss) => {
                reconcile_existing_pool_statefulset(
                    ctx,
                    tenant,
                    namespace,
                    pool,
                    &ss_name,
                    existing_ss,
                    &mut summary,
                )
                .await?;
            }
            Err(e) if is_not_found_context_error(&e) => {
                reconcile_missing_pool_statefulset(
                    ctx,
                    tenant,
                    namespace,
                    pool,
                    &ss_name,
                    &mut summary,
                )
                .await?;
            }
            Err(e) => {
                error!("Failed to get StatefulSet {}: {}", ss_name, e);
                let status_error = StatusError::from_context_error(&e);
                patch_status_error(ctx, tenant, &status_error).await;
                return Err(e.into());
            }
        }
    }

    Ok(summary)
}

fn is_not_found_context_error(error: &context::Error) -> bool {
    matches!(
        error,
        context::Error::Kube {
            source: kube::Error::Api(api_error)
        } if api_error.code == 404
    )
}

async fn reconcile_existing_pool_statefulset(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &crate::types::v1alpha1::pool::Pool,
    ss_name: &str,
    existing_ss: k8s_openapi::api::apps::v1::StatefulSet,
    summary: &mut PoolReconcileSummary,
) -> Result<(), Error> {
    debug!("StatefulSet {} exists, checking if update needed", ss_name);

    if let Err(e) = tenant.validate_statefulset_update(&existing_ss, pool) {
        error!("StatefulSet {} update validation failed: {}", ss_name, e);

        let status_error = StatusError::statefulset_update_validation_failed(ss_name);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    if types_result(
        tenant.statefulset_needs_update(&existing_ss, pool),
        ctx,
        tenant,
    )
    .await?
    {
        debug!("StatefulSet {} needs update, applying changes", ss_name);

        let _ = ctx
            .record(
                tenant,
                EventType::Normal,
                "StatefulSetUpdateStarted",
                &format!("Updating StatefulSet {}", ss_name),
            )
            .await;

        let desired = types_result(tenant.new_statefulset(pool), ctx, tenant).await?;
        if let Err(e) = ctx.apply(&desired, namespace).await {
            let status_error = StatusError::statefulset_apply_failed(ss_name);
            patch_status_error(ctx, tenant, &status_error).await;
            return Err(e.into());
        }

        debug!("StatefulSet {} updated successfully", ss_name);
    } else {
        debug!("StatefulSet {} is up to date, no changes needed", ss_name);
    }

    let ss = context_result(
        ctx.get::<k8s_openapi::api::apps::v1::StatefulSet>(ss_name, namespace)
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
    summary: &mut PoolReconcileSummary,
) -> Result<(), Error> {
    debug!("StatefulSet {} not found, creating", ss_name);

    let _ = ctx
        .record(
            tenant,
            EventType::Normal,
            "StatefulSetCreated",
            &format!("Creating StatefulSet {}", ss_name),
        )
        .await;

    let desired = types_result(tenant.new_statefulset(pool), ctx, tenant).await?;
    if let Err(e) = ctx.apply(&desired, namespace).await {
        let status_error = StatusError::statefulset_apply_failed(ss_name);
        patch_status_error(ctx, tenant, &status_error).await;
        return Err(e.into());
    }

    debug!("StatefulSet {} created successfully", ss_name);

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
) -> Result<Action, Error> {
    let mut builder = StatusBuilder::from_tenant(tenant);
    builder.set_pool_statuses(summary.pool_statuses);

    let (event_condition, event_reason, event_type, event_message) = if summary.any_degraded {
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
        builder.finish_success();
        (
            ConditionType::Ready,
            Reason::ReconcileSucceeded,
            EventType::Normal,
            format!(
                "{}/{} pods ready",
                summary.ready_replicas, summary.total_replicas
            ),
        )
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
    debug!("Patching tenant status if changed: {:?}", status);
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

    if summary.any_updating {
        debug!("Pools are updating, requeuing in 10 seconds");
        Ok(Action::requeue(Duration::from_secs(10)))
    } else {
        Ok(Action::await_change())
    }
}
