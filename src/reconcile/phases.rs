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
    Error, PodCleanupOutcome, cleanup_stuck_terminating_pods_on_down_nodes, context,
    context_result, patch_status_and_record, patch_status_error, statefulset_owned_by_tenant,
    types_result,
};
use crate::context::Context;
use crate::status::{StatusBuilder, StatusError};
use crate::types;
use crate::types::v1alpha1::status::pool::PoolLifecycleState;
use crate::types::v1alpha1::status::{ConditionType, Reason};
use crate::types::v1alpha1::tenant::{
    RUSTFS_TENANT_LABEL, Tenant, uses_unpartitioned_rolling_update,
};
use crate::types::v1alpha1::tls::TlsPlan;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::ServiceAccount;
use k8s_openapi::api::rbac::v1::{Role, RoleBinding};
use kube::api::{DeleteParams, ListParams, Preconditions, PropagationPolicy};
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use kube::{Resource, ResourceExt};
use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::fmt::Debug;
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
const POD_DELETION_POLICY_REQUEUE_INTERVAL: Duration = Duration::from_secs(30);

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

    // Block known incompatible RustFS images before creating or rolling any StatefulSet.
    if let Err(e) = tenant.validate_workload_security_compatibility() {
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

    // Validate dedicated internode RPC authentication before applying workloads. An invalid
    // Secret would otherwise only fail when Kubernetes starts a Pod, after rollout has begun.
    if let Err(e) = ctx.validate_rpc_secret(tenant).await {
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
) -> Result<PodCleanupOutcome, Error> {
    // Optional: unblock StatefulSet pods stuck terminating when their node is down.
    // This is inspired by Longhorn's "Pod Deletion Policy When Node is Down".
    if let Some(policy) = tenant.spec.pod_deletion_policy_when_node_is_down.clone()
        && policy != crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing
    {
        return cleanup_stuck_terminating_pods_on_down_nodes(tenant, namespace, ctx, policy).await;
    }
    Ok(PodCleanupOutcome::Complete)
}

pub(super) async fn cleanup_legacy_tenant_rbac(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), context::Error> {
    // Attempt both deletions before returning an error. Deleting the Role first revokes every
    // binding to the legacy policy; deleting the RoleBinding also revokes access if Role deletion
    // fails transiently.
    let role_name = tenant.legacy_role_name();
    let role_binding_name = tenant.legacy_role_binding_name();
    let role_result =
        delete_owned_legacy_rbac_resource::<Role>(ctx, tenant, namespace, &role_name, "Role").await;
    let role_binding_result = delete_owned_legacy_rbac_resource::<RoleBinding>(
        ctx,
        tenant,
        namespace,
        &role_binding_name,
        "RoleBinding",
    )
    .await;

    role_result?;
    role_binding_result?;
    Ok(())
}

async fn delete_owned_legacy_rbac_resource<T>(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    name: &str,
    kind: &str,
) -> Result<(), context::Error>
where
    T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
    <T as Resource>::DynamicType: Default,
{
    let resource = match ctx.get::<T>(name, namespace).await {
        Ok(resource) => resource,
        Err(error) if context::is_kube_not_found(&error) => return Ok(()),
        Err(error) => return Err(error),
    };

    if !operator_resource_owned_by_tenant_or_predecessor(resource.meta(), tenant) {
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            resource_kind = kind,
            resource = name,
            "skipping legacy RBAC cleanup because the resource is not owned by this Tenant"
        );
        return Ok(());
    }

    let Some(uid) = resource.meta().uid.clone() else {
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            resource_kind = kind,
            resource = name,
            "skipping legacy RBAC cleanup because the resource UID is missing"
        );
        return Ok(());
    };
    let delete_params = DeleteParams {
        preconditions: Some(Preconditions {
            uid: Some(uid),
            resource_version: resource.meta().resource_version.clone(),
        }),
        ..DeleteParams::default()
    };

    match ctx
        .delete_with_params::<T>(name, namespace, &delete_params)
        .await
    {
        Ok(()) => {
            info!(
                tenant = %tenant.name(),
                namespace = %namespace,
                resource_kind = kind,
                resource = name,
                "deleted legacy Tenant workload RBAC"
            );
            Ok(())
        }
        Err(error) if context::is_kube_not_found(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn operator_resource_owned_by_tenant_or_predecessor(
    metadata: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    tenant: &Tenant,
) -> bool {
    let labels_match = metadata.labels.as_ref().is_some_and(|labels| {
        tenant
            .common_labels()
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value))
    });
    let owner_matches_tenant =
        |owner: &k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference| {
            owner.api_version == Tenant::api_version(&())
                && owner.kind == Tenant::kind(&())
                && owner.name == tenant.name()
                && owner.controller == Some(true)
        };
    let owner_scope_matches = metadata.owner_references.as_ref().is_none_or(|owners| {
        owners.is_empty()
            || (owners.iter().any(owner_matches_tenant)
                && !owners
                    .iter()
                    .any(|owner| owner.controller == Some(true) && !owner_matches_tenant(owner)))
    });
    let legacy_manager_matches = metadata.managed_fields.as_ref().is_some_and(|fields| {
        fields
            .iter()
            .any(|field| field.manager.as_deref() == Some("rustfs-operator"))
    });

    labels_match && legacy_manager_matches && owner_scope_matches
}

fn security_patch_metadata(
    metadata: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    name: &str,
    namespace: &str,
) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "name": name,
        "namespace": namespace,
        "uid": metadata.uid.as_ref()?,
        "resourceVersion": metadata.resource_version.as_ref()?
    }))
}

fn statefulset_uses_unpartitioned_rolling_update(statefulset: &StatefulSet) -> bool {
    uses_unpartitioned_rolling_update(
        statefulset
            .spec
            .as_ref()
            .and_then(|spec| spec.update_strategy.as_ref()),
    )
}

fn statefulset_disables_service_account_token(statefulset: &StatefulSet) -> bool {
    statefulset
        .spec
        .as_ref()
        .and_then(|spec| spec.template.spec.as_ref())
        .is_some_and(|spec| spec.automount_service_account_token == Some(false))
}

fn security_manager_owns_fields(
    metadata: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
) -> bool {
    metadata.managed_fields.as_ref().is_some_and(|fields| {
        fields
            .iter()
            .any(|field| field.manager.as_deref() == Some("rustfs-operator-security"))
    })
}

async fn harden_existing_default_service_account(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), context::Error> {
    if tenant.spec.service_account_name.is_some() {
        return Ok(());
    }

    let name = tenant.service_account_name();
    let service_account = match ctx.get::<ServiceAccount>(&name, namespace).await {
        Ok(service_account) => service_account,
        Err(error) if context::is_kube_not_found(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    if service_account.automount_service_account_token == Some(false)
        || !operator_resource_owned_by_tenant_or_predecessor(service_account.meta(), tenant)
    {
        return Ok(());
    }
    let Some(metadata) = security_patch_metadata(service_account.meta(), &name, namespace) else {
        warn!(tenant = %tenant.name(), namespace = %namespace, service_account = %name, "skipping ServiceAccount token hardening because resource identity is incomplete");
        return Ok(());
    };
    let patch = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": metadata,
        "automountServiceAccountToken": false
    });
    let _: ServiceAccount = ctx
        .force_apply_security_fields(&name, namespace, &patch)
        .await?;
    Ok(())
}

async fn harden_existing_statefulset_tokens(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), context::Error> {
    let selector = format!("{RUSTFS_TENANT_LABEL}={}", tenant.name());
    let statefulsets = ctx
        .list_with_params::<StatefulSet>(namespace, &ListParams::default().labels(&selector))
        .await?;
    let default_service_account = tenant.spec.service_account_name.is_none();
    let mut first_error = None;

    for statefulset in statefulsets.items {
        if !operator_resource_owned_by_tenant_or_predecessor(statefulset.meta(), tenant) {
            continue;
        }
        let needs_hardening = default_service_account
            && (!statefulset_disables_service_account_token(&statefulset)
                || !statefulset_uses_unpartitioned_rolling_update(&statefulset));
        let needs_release =
            !default_service_account && security_manager_owns_fields(statefulset.meta());
        if !needs_hardening && !needs_release {
            continue;
        }

        let name = statefulset.name_any();
        let Some(metadata) = security_patch_metadata(statefulset.meta(), &name, namespace) else {
            warn!(tenant = %tenant.name(), namespace = %namespace, statefulset = %name, "skipping StatefulSet token hardening because resource identity is incomplete");
            continue;
        };
        let patch = if needs_hardening {
            serde_json::json!({
                "apiVersion": "apps/v1",
                "kind": "StatefulSet",
                "metadata": metadata,
                "spec": {
                    "updateStrategy": {
                        "type": "RollingUpdate",
                        "rollingUpdate": { "partition": 0 }
                    },
                    "template": {
                        "spec": { "automountServiceAccountToken": false }
                    }
                }
            })
        } else {
            serde_json::json!({
                "apiVersion": "apps/v1",
                "kind": "StatefulSet",
                "metadata": metadata
            })
        };
        if let Err(error) = ctx
            .force_apply_security_fields::<StatefulSet, _>(&name, namespace, &patch)
            .await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    first_error.map_or(Ok(()), Err)
}

pub(super) async fn harden_existing_tenant_workload_identity(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), context::Error> {
    let service_account_result =
        harden_existing_default_service_account(ctx, tenant, namespace).await;
    let statefulset_result = harden_existing_statefulset_tokens(ctx, tenant, namespace).await;

    service_account_result?;
    statefulset_result
}

pub(super) async fn reconcile_service_account(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<(), Error> {
    if tenant.spec.service_account_name.is_none() {
        let desired = tenant.new_service_account();
        let name = desired.name_any();
        let existing = match ctx.get::<ServiceAccount>(&name, namespace).await {
            Ok(existing) => Some(existing),
            Err(error) if context::is_kube_not_found(&error) => None,
            Err(error) => return context_result(Err(error), ctx, tenant).await,
        };

        match existing {
            None => {
                context_result(ctx.apply(&desired, namespace).await, ctx, tenant).await?;
            }
            Some(existing)
                if operator_resource_owned_by_tenant_or_predecessor(existing.meta(), tenant) =>
            {
                context_result(ctx.apply(&desired, namespace).await, ctx, tenant).await?;
            }
            Some(_) => {
                warn!(tenant = %tenant.name(), namespace = %namespace, service_account = %name, "preserving same-name ServiceAccount because it is not operator-managed");
            }
        }
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

fn pod_deletion_policy_is_enabled(tenant: &Tenant) -> bool {
    tenant
        .spec
        .pod_deletion_policy_when_node_is_down
        .as_ref()
        .is_some_and(|policy| {
            policy != &crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::DoNothing
        })
}

fn pod_deletion_policy_requeue_after(
    tenant: &Tenant,
    summary: &PoolReconcileSummary,
) -> Option<Duration> {
    if pod_deletion_policy_is_enabled(tenant)
        && summary.total_replicas > 0
        && summary.ready_replicas < summary.total_replicas
    {
        Some(POD_DELETION_POLICY_REQUEUE_INTERVAL)
    } else {
        None
    }
}

fn reconcile_requeue_after(
    tenant: &Tenant,
    summary: &PoolReconcileSummary,
    pod_cleanup_outcome: PodCleanupOutcome,
) -> Option<Duration> {
    let lifecycle_requeue = summary.lifecycle_requeue_after;
    let updating_requeue = summary.any_updating.then_some(Duration::from_secs(10));
    let pod_cleanup_requeue = if pod_cleanup_outcome == PodCleanupOutcome::RetryNeeded {
        Some(POD_DELETION_POLICY_REQUEUE_INTERVAL)
    } else {
        pod_deletion_policy_requeue_after(tenant, summary)
    };

    earliest_requeue_after(
        earliest_requeue_after(lifecycle_requeue, updating_requeue),
        pod_cleanup_requeue,
    )
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

    if let Err(e) = tenant.validate_statefulset_update_with_tls_plan_and_cluster_domain(
        &existing_ss,
        pool,
        tls_plan,
        ctx.cluster_domain(),
    ) {
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
        tenant.statefulset_needs_update_with_tls_plan_and_cluster_domain(
            &existing_ss,
            pool,
            tls_plan,
            ctx.cluster_domain(),
        ),
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
            tenant.new_statefulset_with_tls_plan_and_cluster_domain(
                pool,
                tls_plan,
                ctx.cluster_domain(),
            ),
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
        tenant.new_statefulset_with_tls_plan_and_cluster_domain(
            pool,
            tls_plan,
            ctx.cluster_domain(),
        ),
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
    pod_cleanup_outcome: PodCleanupOutcome,
) -> Result<Action, Error> {
    let mut builder = StatusBuilder::from_tenant(tenant);
    let pool_count = summary.pool_statuses.len();
    let requeue_after = reconcile_requeue_after(tenant, &summary, pod_cleanup_outcome);
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

    if let Some(requeue_after) = requeue_after {
        debug!(
            tenant = %tenant.name(),
            namespace = ?tenant.namespace(),
            seconds = requeue_after.as_secs(),
            "tenant reconcile has active follow-up work, requeuing"
        );
        Ok(Action::requeue(requeue_after))
    } else {
        Ok(Action::await_change())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, Request, Response, StatusCode};
    use k8s_openapi::api::rbac::v1::{Role, RoleBinding};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta};
    use kube::{Client, client::Body};
    use serde_json::{Value, json};
    use std::convert::Infallible;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::service_fn;

    fn kube_response(status: StatusCode, body: Value) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Body::from(
                serde_json::to_vec(&body).expect("response should serialize"),
            ))
            .expect("response should build")
    }

    fn operator_managed_fields() -> Vec<ManagedFieldsEntry> {
        vec![ManagedFieldsEntry {
            manager: Some("rustfs-operator".to_string()),
            ..Default::default()
        }]
    }

    fn legacy_rbac_metadata(tenant: &Tenant, name: &str, uid: &str, owned: bool) -> ObjectMeta {
        ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("default".to_string()),
            uid: Some(uid.to_string()),
            resource_version: Some("7".to_string()),
            labels: owned.then(|| tenant.common_labels()),
            owner_references: owned.then(|| vec![tenant.new_owner_ref()]),
            managed_fields: owned.then(operator_managed_fields),
            ..Default::default()
        }
    }

    fn legacy_role(tenant: &Tenant, owned: bool) -> Role {
        Role {
            metadata: legacy_rbac_metadata(
                tenant,
                &tenant.legacy_role_name(),
                "legacy-role-uid",
                owned,
            ),
            ..Default::default()
        }
    }

    fn legacy_role_binding(tenant: &Tenant, owned: bool) -> RoleBinding {
        RoleBinding {
            metadata: legacy_rbac_metadata(
                tenant,
                &tenant.legacy_role_binding_name(),
                "legacy-role-binding-uid",
                owned,
            ),
            ..Default::default()
        }
    }

    #[test]
    fn operator_ownership_recognizes_recreated_and_orphaned_tenants() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let current_owned = ObjectMeta {
            labels: Some(tenant.common_labels()),
            owner_references: Some(vec![tenant.new_owner_ref()]),
            managed_fields: Some(operator_managed_fields()),
            ..Default::default()
        };
        assert!(operator_resource_owned_by_tenant_or_predecessor(
            &current_owned,
            &tenant
        ));

        let mut stale_owner = tenant.new_owner_ref();
        stale_owner.uid = "previous-tenant-uid".to_string();
        let stale_owned = ObjectMeta {
            labels: Some(tenant.common_labels()),
            owner_references: Some(vec![stale_owner]),
            managed_fields: Some(operator_managed_fields()),
            ..Default::default()
        };
        assert!(operator_resource_owned_by_tenant_or_predecessor(
            &stale_owned,
            &tenant
        ));

        let orphaned = ObjectMeta {
            labels: Some(tenant.common_labels()),
            managed_fields: Some(operator_managed_fields()),
            ..Default::default()
        };
        assert!(operator_resource_owned_by_tenant_or_predecessor(
            &orphaned, &tenant
        ));

        let user_managed = ObjectMeta {
            labels: Some(tenant.common_labels()),
            ..Default::default()
        };
        assert!(!operator_resource_owned_by_tenant_or_predecessor(
            &user_managed,
            &tenant
        ));

        let security_manager_only = ObjectMeta {
            labels: Some(tenant.common_labels()),
            owner_references: Some(vec![tenant.new_owner_ref()]),
            managed_fields: Some(vec![ManagedFieldsEntry {
                manager: Some("rustfs-operator-security".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert!(!operator_resource_owned_by_tenant_or_predecessor(
            &security_manager_only,
            &tenant
        ));

        let missing_labels = ObjectMeta {
            owner_references: Some(vec![tenant.new_owner_ref()]),
            managed_fields: Some(operator_managed_fields()),
            ..Default::default()
        };
        assert!(!operator_resource_owned_by_tenant_or_predecessor(
            &missing_labels,
            &tenant
        ));

        let mut foreign_owner = tenant.new_owner_ref();
        foreign_owner.api_version = "v1".to_string();
        foreign_owner.kind = "ConfigMap".to_string();
        foreign_owner.name = "foreign-controller".to_string();
        let foreign_owned = ObjectMeta {
            labels: Some(tenant.common_labels()),
            owner_references: Some(vec![foreign_owner]),
            managed_fields: Some(operator_managed_fields()),
            ..Default::default()
        };
        assert!(!operator_resource_owned_by_tenant_or_predecessor(
            &foreign_owned,
            &tenant
        ));
    }

    #[test]
    fn operator_ownership_rejects_user_managed_resource_owned_by_current_tenant() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let user_managed = ObjectMeta {
            labels: Some(tenant.common_labels()),
            owner_references: Some(vec![tenant.new_owner_ref()]),
            ..Default::default()
        };

        assert!(!operator_resource_owned_by_tenant_or_predecessor(
            &user_managed,
            &tenant
        ));
    }

    #[tokio::test]
    async fn legacy_rbac_cleanup_deletes_owned_resources_with_preconditions() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let tenant = tenant.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let tenant = tenant.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    let expected = [
                        (
                            Method::GET,
                            "/apis/rbac.authorization.k8s.io/v1/namespaces/default/roles/test-tenant-role",
                        ),
                        (
                            Method::DELETE,
                            "/apis/rbac.authorization.k8s.io/v1/namespaces/default/roles/test-tenant-role",
                        ),
                        (
                            Method::GET,
                            "/apis/rbac.authorization.k8s.io/v1/namespaces/default/rolebindings/test-tenant-role-binding",
                        ),
                        (
                            Method::DELETE,
                            "/apis/rbac.authorization.k8s.io/v1/namespaces/default/rolebindings/test-tenant-role-binding",
                        ),
                    ];
                    assert_eq!(
                        (request.method().clone(), request.uri().path()),
                        (
                            expected[request_number].0.clone(),
                            expected[request_number].1
                        )
                    );

                    let response = match request_number {
                        0 => kube_response(
                            StatusCode::OK,
                            serde_json::to_value(legacy_role(&tenant, true))
                                .expect("Role should serialize"),
                        ),
                        1 => {
                            let body: Value = serde_json::from_slice(
                                &request
                                    .into_body()
                                    .collect_bytes()
                                    .await
                                    .expect("delete body should read"),
                            )
                            .expect("delete body should be JSON");
                            assert_eq!(body["preconditions"]["uid"], "legacy-role-uid");
                            assert_eq!(body["preconditions"]["resourceVersion"], "7");
                            kube_response(
                                StatusCode::OK,
                                json!({"apiVersion":"v1","kind":"Status","status":"Success"}),
                            )
                        }
                        2 => kube_response(
                            StatusCode::OK,
                            serde_json::to_value(legacy_role_binding(&tenant, true))
                                .expect("RoleBinding should serialize"),
                        ),
                        3 => {
                            let body: Value = serde_json::from_slice(
                                &request
                                    .into_body()
                                    .collect_bytes()
                                    .await
                                    .expect("delete body should read"),
                            )
                            .expect("delete body should be JSON");
                            assert_eq!(body["preconditions"]["uid"], "legacy-role-binding-uid");
                            assert_eq!(body["preconditions"]["resourceVersion"], "7");
                            kube_response(
                                StatusCode::OK,
                                json!({"apiVersion":"v1","kind":"Status","status":"Success"}),
                            )
                        }
                        _ => unreachable!(),
                    };
                    Ok::<_, Infallible>(response)
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        cleanup_legacy_tenant_rbac(&ctx, &tenant, "default")
            .await
            .expect("owned legacy RBAC should be deleted");

        assert_eq!(request_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn legacy_rbac_cleanup_is_idempotent_when_resources_are_absent() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(request.method(), Method::GET);
                    Ok::<_, Infallible>(kube_response(
                        StatusCode::NOT_FOUND,
                        json!({
                            "apiVersion":"v1",
                            "kind":"Status",
                            "status":"Failure",
                            "reason":"NotFound",
                            "code":404
                        }),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        cleanup_legacy_tenant_rbac(&ctx, &tenant, "default")
            .await
            .expect("missing legacy RBAC should be ignored");

        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn legacy_rbac_cleanup_ignores_delete_not_found_race() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let tenant = tenant.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let tenant = tenant.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    let response = match request_number {
                        0 => {
                            assert_eq!(request.method(), Method::GET);
                            kube_response(
                                StatusCode::OK,
                                serde_json::to_value(legacy_role(&tenant, true))
                                    .expect("Role should serialize"),
                            )
                        }
                        1 => {
                            assert_eq!(request.method(), Method::DELETE);
                            kube_response(
                                StatusCode::NOT_FOUND,
                                json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}),
                            )
                        }
                        2 => {
                            assert_eq!(request.method(), Method::GET);
                            kube_response(
                                StatusCode::NOT_FOUND,
                                json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}),
                            )
                        }
                        _ => unreachable!(),
                    };
                    Ok::<_, Infallible>(response)
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        cleanup_legacy_tenant_rbac(&ctx, &tenant, "default")
            .await
            .expect("a delete race should be idempotent");

        assert_eq!(request_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn legacy_rbac_cleanup_preserves_user_managed_resources_owned_by_tenant() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let tenant = tenant.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let tenant = tenant.clone();
                request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(request.method(), Method::GET);
                    let body = if request.uri().path().contains("rolebindings") {
                        let mut role_binding = legacy_role_binding(&tenant, false);
                        role_binding.metadata.labels = Some(tenant.common_labels());
                        role_binding.metadata.owner_references = Some(vec![tenant.new_owner_ref()]);
                        serde_json::to_value(role_binding).expect("RoleBinding should serialize")
                    } else {
                        let mut role = legacy_role(&tenant, false);
                        role.metadata.labels = Some(tenant.common_labels());
                        role.metadata.owner_references = Some(vec![tenant.new_owner_ref()]);
                        serde_json::to_value(role).expect("Role should serialize")
                    };
                    Ok::<_, Infallible>(kube_response(StatusCode::OK, body))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        cleanup_legacy_tenant_rbac(&ctx, &tenant, "default")
            .await
            .expect("user-managed resources should be preserved");

        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn legacy_rbac_cleanup_attempts_both_resources_before_returning_error() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(request.method(), Method::GET);
                    let (status, code, reason) = match request_number {
                        0 => {
                            assert!(request.uri().path().contains("/roles/"));
                            (StatusCode::INTERNAL_SERVER_ERROR, 500, "InternalError")
                        }
                        1 => {
                            assert!(request.uri().path().contains("/rolebindings/"));
                            (StatusCode::NOT_FOUND, 404, "NotFound")
                        }
                        _ => unreachable!(),
                    };
                    Ok::<_, Infallible>(kube_response(
                        status,
                        json!({
                            "apiVersion":"v1",
                            "kind":"Status",
                            "status":"Failure",
                            "reason":reason,
                            "code":code
                        }),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        let error = cleanup_legacy_tenant_rbac(&ctx, &tenant, "default")
            .await
            .expect_err("non-404 cleanup errors should be returned");

        assert!(
            matches!(
                error,
                context::Error::Kube {
                    source: kube::Error::Api(response)
                } if response.code == 500
            ),
            "the first cleanup error should be preserved"
        );
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn existing_default_service_account_is_force_hardened() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let tenant = tenant.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let tenant = tenant.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(
                        request.uri().path(),
                        "/api/v1/namespaces/default/serviceaccounts/test-tenant-sa"
                    );
                    if request_number == 0 {
                        assert_eq!(request.method(), Method::GET);
                        let mut service_account = tenant.new_service_account();
                        service_account.metadata.uid = Some("service-account-uid".to_string());
                        service_account.metadata.resource_version = Some("11".to_string());
                        service_account.metadata.managed_fields = Some(operator_managed_fields());
                        service_account.automount_service_account_token = None;
                        return Ok::<_, Infallible>(kube_response(
                            StatusCode::OK,
                            serde_json::to_value(service_account)
                                .expect("ServiceAccount should serialize"),
                        ));
                    }

                    assert_eq!(request.method(), Method::PATCH);
                    let query = request.uri().query().unwrap_or_default().to_string();
                    let body: Value = serde_json::from_slice(
                        &request
                            .into_body()
                            .collect_bytes()
                            .await
                            .expect("apply body should read"),
                    )
                    .expect("apply body should be JSON");
                    assert_eq!(body["automountServiceAccountToken"], false);
                    assert_eq!(body["metadata"]["uid"], "service-account-uid");
                    assert_eq!(body["metadata"]["resourceVersion"], "11");
                    assert_eq!(request_number, 1);
                    assert!(query.contains("fieldManager=rustfs-operator-security"));
                    assert!(query.contains("force=true"));
                    Ok::<_, Infallible>(kube_response(
                        StatusCode::OK,
                        serde_json::to_value(tenant.new_service_account())
                            .expect("ServiceAccount should serialize"),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        harden_existing_default_service_account(&ctx, &tenant, "default")
            .await
            .expect("default ServiceAccount should be hardened");

        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn existing_statefulset_is_hardened_before_normal_reconcile() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("StatefulSet should render");
        statefulset.metadata.uid = Some("statefulset-uid".to_string());
        statefulset.metadata.resource_version = Some("19".to_string());
        statefulset.metadata.managed_fields = Some(operator_managed_fields());
        let spec = statefulset
            .spec
            .as_mut()
            .expect("StatefulSet should have spec");
        spec.update_strategy = Some(k8s_openapi::api::apps::v1::StatefulSetUpdateStrategy {
            type_: Some("OnDelete".to_string()),
            ..Default::default()
        });
        spec.template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .automount_service_account_token = None;

        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let statefulset = statefulset.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let statefulset = statefulset.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if request_number == 0 {
                        assert_eq!(request.method(), Method::GET);
                        assert_eq!(
                            request.uri().path(),
                            "/apis/apps/v1/namespaces/default/statefulsets"
                        );
                        assert!(
                            request
                                .uri()
                                .query()
                                .unwrap_or_default()
                                .contains("labelSelector=rustfs.tenant%3Dtest-tenant")
                        );
                        return Ok::<_, Infallible>(kube_response(
                            StatusCode::OK,
                            json!({
                                "apiVersion": "apps/v1",
                                "kind": "StatefulSetList",
                                "metadata": {},
                                "items": [statefulset]
                            }),
                        ));
                    }

                    assert_eq!(request_number, 1);
                    assert_eq!(request.method(), Method::PATCH);
                    let query = request.uri().query().unwrap_or_default().to_string();
                    let body: Value = serde_json::from_slice(
                        &request
                            .into_body()
                            .collect_bytes()
                            .await
                            .expect("apply body should read"),
                    )
                    .expect("apply body should be JSON");
                    assert!(query.contains("fieldManager=rustfs-operator-security"));
                    assert!(query.contains("force=true"));
                    assert_eq!(body["metadata"]["uid"], "statefulset-uid");
                    assert_eq!(body["metadata"]["resourceVersion"], "19");
                    assert_eq!(body["spec"]["updateStrategy"]["type"], "RollingUpdate");
                    assert_eq!(
                        body["spec"]["updateStrategy"]["rollingUpdate"]["partition"],
                        0
                    );
                    assert_eq!(
                        body["spec"]["template"]["spec"]["automountServiceAccountToken"],
                        false
                    );
                    Ok::<_, Infallible>(kube_response(
                        StatusCode::OK,
                        serde_json::to_value(statefulset).expect("StatefulSet should serialize"),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        harden_existing_statefulset_tokens(&ctx, &tenant, "default")
            .await
            .expect("existing StatefulSet should be hardened independently");

        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn custom_service_account_releases_security_field_ownership_once() {
        let tenant = crate::tests::create_test_tenant(Some("custom-api-sa".to_string()), None);
        let mut statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("StatefulSet should render");
        statefulset.metadata.uid = Some("statefulset-uid".to_string());
        statefulset.metadata.resource_version = Some("29".to_string());
        let mut managed_fields = operator_managed_fields();
        managed_fields.push(ManagedFieldsEntry {
            manager: Some("rustfs-operator-security".to_string()),
            ..Default::default()
        });
        statefulset.metadata.managed_fields = Some(managed_fields);

        let mut released_statefulset = statefulset.clone();
        released_statefulset.metadata.resource_version = Some("30".to_string());
        released_statefulset.metadata.managed_fields = Some(operator_managed_fields());

        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let statefulset = statefulset.clone();
            let released_statefulset = released_statefulset.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let statefulset = statefulset.clone();
                let released_statefulset = released_statefulset.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    match request_number {
                        0 | 2 => {
                            assert_eq!(request.method(), Method::GET);
                            assert_eq!(
                                request.uri().path(),
                                "/apis/apps/v1/namespaces/default/statefulsets"
                            );
                            assert!(
                                request
                                    .uri()
                                    .query()
                                    .unwrap_or_default()
                                    .contains("labelSelector=rustfs.tenant%3Dtest-tenant")
                            );
                            let item = if request_number == 0 {
                                statefulset
                            } else {
                                released_statefulset
                            };
                            Ok::<_, Infallible>(kube_response(
                                StatusCode::OK,
                                json!({
                                    "apiVersion": "apps/v1",
                                    "kind": "StatefulSetList",
                                    "metadata": {},
                                    "items": [item]
                                }),
                            ))
                        }
                        1 => {
                            assert_eq!(request.method(), Method::PATCH);
                            assert_eq!(
                                request.uri().path(),
                                "/apis/apps/v1/namespaces/default/statefulsets/test-tenant-pool-0"
                            );
                            let query = request.uri().query().unwrap_or_default().to_string();
                            let body: Value = serde_json::from_slice(
                                &request
                                    .into_body()
                                    .collect_bytes()
                                    .await
                                    .expect("release body should read"),
                            )
                            .expect("release body should be JSON");
                            assert!(query.contains("fieldManager=rustfs-operator-security"));
                            assert!(query.contains("force=true"));
                            assert_eq!(body["apiVersion"], "apps/v1");
                            assert_eq!(body["kind"], "StatefulSet");
                            assert_eq!(body["metadata"]["uid"], "statefulset-uid");
                            assert_eq!(body["metadata"]["resourceVersion"], "29");
                            assert!(body.get("spec").is_none());
                            Ok::<_, Infallible>(kube_response(
                                StatusCode::OK,
                                serde_json::to_value(released_statefulset)
                                    .expect("StatefulSet should serialize"),
                            ))
                        }
                        _ => panic!("security ownership should be released exactly once"),
                    }
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        harden_existing_statefulset_tokens(&ctx, &tenant, "default")
            .await
            .expect("custom ServiceAccount should release security field ownership");
        harden_existing_statefulset_tokens(&ctx, &tenant, "default")
            .await
            .expect("released security ownership should be idempotent");

        assert_eq!(request_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn same_name_user_managed_service_account_is_preserved() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(request.method(), Method::GET);
                    Ok::<_, Infallible>(kube_response(
                        StatusCode::OK,
                        serde_json::to_value(ServiceAccount {
                            metadata: ObjectMeta {
                                name: Some("test-tenant-sa".to_string()),
                                namespace: Some("default".to_string()),
                                ..Default::default()
                            },
                            automount_service_account_token: Some(true),
                            ..Default::default()
                        })
                        .expect("ServiceAccount should serialize"),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        reconcile_service_account(&ctx, &tenant, "default")
            .await
            .expect("same-name user-managed ServiceAccount should be preserved");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn predecessor_service_account_is_adopted_by_current_tenant() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut predecessor = tenant.new_service_account();
        predecessor
            .metadata
            .owner_references
            .as_mut()
            .expect("owner should exist")[0]
            .uid = "previous-tenant-uid".to_string();
        predecessor.metadata.uid = Some("service-account-uid".to_string());
        predecessor.metadata.resource_version = Some("23".to_string());
        predecessor.metadata.managed_fields = Some(operator_managed_fields());

        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let tenant = tenant.clone();
            let predecessor = predecessor.clone();
            let request_count = Arc::clone(&request_count);
            move |request: Request<Body>| {
                let tenant = tenant.clone();
                let predecessor = predecessor.clone();
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if request_number == 0 {
                        assert_eq!(request.method(), Method::GET);
                        return Ok::<_, Infallible>(kube_response(
                            StatusCode::OK,
                            serde_json::to_value(predecessor)
                                .expect("ServiceAccount should serialize"),
                        ));
                    }

                    assert_eq!(request_number, 1);
                    assert_eq!(request.method(), Method::PATCH);
                    let body: Value = serde_json::from_slice(
                        &request
                            .into_body()
                            .collect_bytes()
                            .await
                            .expect("apply body should read"),
                    )
                    .expect("apply body should be JSON");
                    assert_eq!(
                        body["metadata"]["ownerReferences"][0]["uid"],
                        "test-uid-123"
                    );
                    assert_eq!(body["automountServiceAccountToken"], false);
                    Ok::<_, Infallible>(kube_response(
                        StatusCode::OK,
                        serde_json::to_value(tenant.new_service_account())
                            .expect("ServiceAccount should serialize"),
                    ))
                }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        reconcile_service_account(&ctx, &tenant, "default")
            .await
            .expect("predecessor ServiceAccount should be adopted");

        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn custom_service_account_is_never_modified_by_operator() {
        let tenant =
            crate::tests::create_test_tenant(Some("cloud-identity-sa".to_string()), Some(true));
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |_request: Request<Body>| {
                request_count.fetch_add(1, Ordering::SeqCst);
                async move { Ok::<_, Infallible>(kube_response(StatusCode::OK, json!({}))) }
            }
        });
        let ctx = Context::new(Client::new(service, "default"));

        reconcile_service_account(&ctx, &tenant, "default")
            .await
            .expect("custom ServiceAccount should remain user-managed");

        assert_eq!(request_count.load(Ordering::SeqCst), 0);
    }

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

    #[test]
    fn pod_deletion_policy_requeues_not_ready_tenant() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pod_deletion_policy_when_node_is_down =
            Some(crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete);
        let summary = PoolReconcileSummary {
            total_replicas: 4,
            ready_replicas: 3,
            ..Default::default()
        };

        assert_eq!(
            pod_deletion_policy_requeue_after(&tenant, &summary),
            Some(POD_DELETION_POLICY_REQUEUE_INTERVAL)
        );
    }

    #[test]
    fn pod_deletion_policy_requeue_skips_ready_or_disabled_tenant() {
        let mut enabled = crate::tests::create_test_tenant(None, None);
        enabled.spec.pod_deletion_policy_when_node_is_down =
            Some(crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete);
        let ready = PoolReconcileSummary {
            total_replicas: 4,
            ready_replicas: 4,
            ..Default::default()
        };
        assert_eq!(pod_deletion_policy_requeue_after(&enabled, &ready), None);

        let disabled = crate::tests::create_test_tenant(None, None);
        let not_ready = PoolReconcileSummary {
            total_replicas: 4,
            ready_replicas: 3,
            ..Default::default()
        };
        assert_eq!(
            pod_deletion_policy_requeue_after(&disabled, &not_ready),
            None
        );
    }

    #[test]
    fn reconcile_requeue_after_prefers_existing_shorter_work() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pod_deletion_policy_when_node_is_down =
            Some(crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete);
        let summary = PoolReconcileSummary {
            any_updating: true,
            total_replicas: 4,
            ready_replicas: 3,
            ..Default::default()
        };

        assert_eq!(
            reconcile_requeue_after(&tenant, &summary, PodCleanupOutcome::Complete),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn failed_node_lookup_requeues_even_when_tenant_summary_is_ready() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pod_deletion_policy_when_node_is_down =
            Some(crate::types::v1alpha1::k8s::PodDeletionPolicyWhenNodeIsDown::ForceDelete);
        let summary = PoolReconcileSummary {
            total_replicas: 4,
            ready_replicas: 4,
            ..Default::default()
        };

        assert_eq!(
            reconcile_requeue_after(&tenant, &summary, PodCleanupOutcome::RetryNeeded),
            Some(POD_DELETION_POLICY_REQUEUE_INTERVAL)
        );
    }
}
