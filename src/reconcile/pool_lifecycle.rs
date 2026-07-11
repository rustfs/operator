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

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use k8s_openapi::api::apps::v1::StatefulSet;
use kube::api::{DeleteParams, PropagationPolicy};
use kube::runtime::events::EventType;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::{Error, context};
use crate::context::Context;
use crate::sts::rustfs_client::{
    RustfsAdminClient, RustfsClientError, RustfsPoolDecommissionInfo, RustfsPoolListItem,
    RustfsPoolStatus,
};
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::pool_lifecycle::{DecommissionAction, DecommissionRequest};
use crate::types::v1alpha1::status::pool::{
    PoolDecommissionCleanupState, PoolDecommissionCleanupStatus, PoolDecommissionLastError,
    PoolDecommissionPhase, PoolDecommissionProgress, PoolDecommissionStatus, PoolLifecycleState,
};
use crate::types::v1alpha1::tenant::Tenant;

const POLL_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct PoolLifecycleDecisions {
    decisions: BTreeMap<String, PoolLifecycleDecision>,
    pub(super) any_reconciling: bool,
    pub(super) any_failed: bool,
    pub(super) any_canceled: bool,
    pub(super) requeue_after: Option<Duration>,
}

#[derive(Clone, Debug)]
pub(super) struct PoolLifecycleDecision {
    pub(super) state: PoolLifecycleState,
    pub(super) decommission: Option<PoolDecommissionStatus>,
    pub(super) skip_workload_reconcile: bool,
}

struct MatchedRustfsPool {
    item: RustfsPoolListItem,
    expected_cmd_line: String,
    expected_endpoint_set_hash: String,
}

struct PoolLifecycleSnapshot {
    state: Option<PoolLifecycleState>,
    decommission: Option<PoolDecommissionStatus>,
    block_start_decommission: bool,
}

enum PoolMappingError {
    ListFailed(String),
    NotFound(String),
    Ambiguous(String),
    TenantNamespace(String),
}

impl PoolMappingError {
    fn reason(&self) -> &'static str {
        match self {
            Self::ListFailed(_) => "RustfsPoolListFailed",
            Self::NotFound(_) => "RustfsPoolMappingFailed",
            Self::Ambiguous(_) => "RustfsPoolMappingAmbiguous",
            Self::TenantNamespace(_) => "TenantNamespaceMissing",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::ListFailed(message)
            | Self::NotFound(message)
            | Self::Ambiguous(message)
            | Self::TenantNamespace(message) => message,
        }
    }

    fn is_retriable(&self) -> bool {
        matches!(self, Self::ListFailed(_))
    }
}

impl PoolLifecycleDecisions {
    pub(super) fn decision_for(&self, pool_name: &str) -> Option<&PoolLifecycleDecision> {
        self.decisions.get(pool_name)
    }

    fn insert(&mut self, pool_name: String, decision: PoolLifecycleDecision) {
        match decision.state {
            PoolLifecycleState::Decommissioning => {
                self.any_reconciling = true;
                self.requeue_after = Some(POLL_INTERVAL);
            }
            PoolLifecycleState::Decommissioned
                if decision
                    .decommission
                    .as_ref()
                    .is_some_and(decommissioned_cleanup_needs_requeue) =>
            {
                self.any_reconciling = true;
                self.requeue_after = Some(POLL_INTERVAL);
            }
            PoolLifecycleState::DecommissionFailed => self.any_failed = true,
            PoolLifecycleState::DecommissionCanceled => self.any_canceled = true,
            _ => {}
        }

        self.decisions.insert(pool_name, decision);
    }
}

pub(super) async fn reconcile_pool_lifecycle(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<PoolLifecycleDecisions, Error> {
    let mut decisions = PoolLifecycleDecisions::default();
    let blocked_start_pools = start_decommission_requests_that_would_remove_all_live_pools(tenant);

    for pool in &tenant.spec.pools {
        let request = tenant
            .spec
            .pool_lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.request_for_pool(&pool.name));
        let existing = existing_decommission_status(tenant, &pool.name);
        let existing_state = existing_lifecycle_state(tenant, &pool.name);
        let snapshot = PoolLifecycleSnapshot {
            state: existing_state.clone(),
            decommission: existing,
            block_start_decommission: blocked_start_pools.contains(&pool.name),
        };

        let should_continue = matches!(
            existing_state,
            Some(
                PoolLifecycleState::Decommissioning
                    | PoolLifecycleState::Decommissioned
                    | PoolLifecycleState::DecommissionCanceled
                    | PoolLifecycleState::DecommissionFailed
            )
        );

        if request.is_none() && !should_continue {
            continue;
        }

        let decision =
            reconcile_single_pool_lifecycle(ctx, tenant, namespace, pool, request, snapshot).await;

        decisions.insert(pool.name.clone(), decision);
    }

    Ok(decisions)
}

async fn reconcile_single_pool_lifecycle(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    request: Option<&DecommissionRequest>,
    existing: PoolLifecycleSnapshot,
) -> PoolLifecycleDecision {
    let PoolLifecycleSnapshot {
        state: existing_state,
        decommission: existing,
        block_start_decommission,
    } = existing;

    if matches!(existing_state, Some(PoolLifecycleState::Decommissioned)) {
        let status = existing.unwrap_or_else(|| PoolDecommissionStatus {
            phase: Some(PoolDecommissionPhase::Complete),
            ..empty_decommission_status()
        });

        if cleanup_already_authorized_or_complete(&status) {
            return cleanup_decommissioned_pool(ctx, tenant, namespace, pool, status).await;
        }

        let client = match rustfs_admin_client(ctx, tenant).await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    reason = "RustfsAdminClientError",
                    %error,
                    "RustFS admin client unavailable for decommissioned pool cleanup"
                );
                return cleanup_retriable_decision(
                    status,
                    "RustfsAdminClientError",
                    &error.to_string(),
                );
            }
        };

        let status =
            match verify_decommissioned_pool_for_cleanup(&client, tenant, namespace, pool, &status)
                .await
            {
                Ok(status) => status,
                Err(decision) => return decision,
            };

        return cleanup_decommissioned_pool(ctx, tenant, namespace, pool, status).await;
    }

    let Some(request) = request else {
        return terminal_decision_from_existing(existing_state, existing);
    };

    if request.request_id.trim().is_empty() {
        return failed_decision(
            Some(request.request_id.clone()),
            "InvalidRequest",
            "pool decommission requestID must not be empty",
        );
    }

    if should_block_last_live_pool_decommission(request, block_start_decommission) {
        return failed_decision(
            Some(request.request_id.clone()),
            "LastPoolBlocked",
            "cannot decommission the last live pool in a tenant",
        );
    }

    let client = match rustfs_admin_client(ctx, tenant).await {
        Ok(client) => client,
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                pool = %pool.name,
                request_id = %request.request_id,
                reason = "RustfsAdminClientError",
                %error,
                "RustFS admin client unavailable for pool lifecycle request"
            );
            return retriable_decision(
                Some(request.request_id.clone()),
                "RustfsAdminClientError",
                &error.to_string(),
            );
        }
    };

    let matched_pool = match find_rustfs_pool(&client, tenant, namespace, pool).await {
        Ok(pool_item) => pool_item,
        Err(error) if error.is_retriable() => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                pool = %pool.name,
                request_id = %request.request_id,
                reason = error.reason(),
                message = error.message(),
                "RustFS pool mapping is not ready"
            );
            return retriable_decision(
                Some(request.request_id.clone()),
                error.reason(),
                error.message(),
            );
        }
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                pool = %pool.name,
                request_id = %request.request_id,
                reason = error.reason(),
                message = error.message(),
                "RustFS pool mapping failed"
            );
            return failed_decision(
                Some(request.request_id.clone()),
                error.reason(),
                error.message(),
            );
        }
    };
    let pool_id = matched_pool.item.id.to_string();

    if cancel_without_decommission_info_is_noop(request, matched_pool.item.decommission.as_ref()) {
        return active_lifecycle_decision();
    }

    let should_start = request.action == DecommissionAction::Start
        && match should_start_decommission(
            existing_state.as_ref(),
            existing.as_ref(),
            &matched_pool.item,
            request,
        ) {
            Ok(should_start) => should_start,
            Err(message) => {
                return failed_decision(
                    Some(request.request_id.clone()),
                    "RustfsDecommissionAlreadyRunning",
                    &message,
                );
            }
        };
    if should_start {
        match client.start_pool_decommission_by_id(&pool_id).await {
            Ok(()) => {
                info!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    rustfs_pool_id = %pool_id,
                    request_id = %request.request_id,
                    "started RustFS pool decommission"
                );
            }
            Err(error) => {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    rustfs_pool_id = %pool_id,
                    request_id = %request.request_id,
                    reason = "RustfsDecommissionStartFailed",
                    %error,
                    "failed to start RustFS pool decommission"
                );
                return retriable_decision(
                    Some(request.request_id.clone()),
                    "RustfsDecommissionStartFailed",
                    &error.to_string(),
                );
            }
        }
    }

    let should_cancel = request.action == DecommissionAction::Cancel
        && !matches!(
            existing_state,
            Some(PoolLifecycleState::DecommissionCanceled)
        );
    if should_cancel {
        match client.cancel_pool_decommission_by_id(&pool_id).await {
            Ok(()) => {
                info!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    rustfs_pool_id = %pool_id,
                    request_id = %request.request_id,
                    "canceled RustFS pool decommission"
                );
            }
            Err(error) => {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    rustfs_pool_id = %pool_id,
                    request_id = %request.request_id,
                    reason = "RustfsDecommissionCancelFailed",
                    %error,
                    "failed to cancel RustFS pool decommission"
                );
                return retriable_decision(
                    Some(request.request_id.clone()),
                    "RustfsDecommissionCancelFailed",
                    &error.to_string(),
                );
            }
        }
    }

    let rustfs_status = match client.pool_status_by_id(&pool_id).await {
        Ok(status) => status,
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                pool = %pool.name,
                rustfs_pool_id = %pool_id,
                request_id = %request.request_id,
                reason = "RustfsDecommissionStatusFailed",
                %error,
                "failed to query RustFS pool decommission status"
            );
            return retriable_decision(
                Some(request.request_id.clone()),
                "RustfsDecommissionStatusFailed",
                &error.to_string(),
            );
        }
    };

    if cancel_without_decommission_info_is_noop(request, rustfs_status.decommission.as_ref()) {
        return active_lifecycle_decision();
    }

    let status = match decommission_status_from_rustfs(
        request,
        &rustfs_status,
        &matched_pool.expected_cmd_line,
        &matched_pool.expected_endpoint_set_hash,
    ) {
        Ok(status) => status,
        Err(message) => {
            return failed_decision(
                Some(request.request_id.clone()),
                "RustfsPoolIdentityMismatch",
                &message,
            );
        }
    };
    match lifecycle_state_from_status(&status) {
        PoolLifecycleState::Decommissioned => {
            cleanup_decommissioned_pool(ctx, tenant, namespace, pool, status).await
        }
        state => PoolLifecycleDecision {
            state,
            decommission: Some(status),
            skip_workload_reconcile: true,
        },
    }
}

async fn rustfs_admin_client(
    ctx: &Context,
    tenant: &Tenant,
) -> Result<RustfsAdminClient, RustfsClientError> {
    let credentials = RustfsAdminClient::load_tenant_credentials(&ctx.client, tenant).await?;
    if tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
        RustfsAdminClient::from_tls_tenant_for_sts(&ctx.client, tenant, credentials).await
    } else {
        RustfsAdminClient::from_tenant(tenant, credentials)
    }
}

async fn find_rustfs_pool(
    client: &RustfsAdminClient,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
) -> Result<MatchedRustfsPool, PoolMappingError> {
    let expected_cmd_line = expected_pool_cmd_line(tenant, namespace, pool)
        .map_err(PoolMappingError::TenantNamespace)?;
    let expected_endpoint_set_hash = endpoint_set_hash(&expected_cmd_line);
    let pools = client
        .list_pools()
        .await
        .map_err(|error| PoolMappingError::ListFailed(error.to_string()))?;

    let mut matches = pools
        .into_iter()
        .filter(|item| same_cmd_line(&item.cmd_line, &expected_cmd_line))
        .collect::<Vec<_>>();

    if matches.len() > 1 {
        return Err(PoolMappingError::Ambiguous(format!(
            "RustFS admin pool list returned {} pools matching cmdline '{}'",
            matches.len(),
            expected_cmd_line
        )));
    }

    matches
        .pop()
        .map(|item| MatchedRustfsPool {
            item,
            expected_cmd_line: expected_cmd_line.clone(),
            expected_endpoint_set_hash,
        })
        .ok_or_else(|| {
            PoolMappingError::NotFound(format!(
                "RustFS admin pool list did not contain expected cmdline '{}'",
                expected_cmd_line
            ))
        })
}

fn expected_pool_cmd_line(tenant: &Tenant, namespace: &str, pool: &Pool) -> Result<String, String> {
    let scheme = if tenant
        .spec
        .tls
        .as_ref()
        .is_some_and(|tls| tls.enable_internode_https)
    {
        "https"
    } else {
        "http"
    };
    if pool.servers <= 0 || pool.persistence.volumes_per_server <= 0 {
        return Err(format!(
            "pool '{}' has invalid servers or volumesPerServer",
            pool.name
        ));
    }

    Ok(tenant.rustfs_pool_volume_spec(pool, scheme, namespace))
}

fn same_cmd_line(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn validate_rustfs_pool_identity(
    status: &RustfsPoolStatus,
    expected_cmd_line: &str,
    expected_endpoint_set_hash: &str,
) -> Result<(), String> {
    if !same_cmd_line(&status.cmd_line, expected_cmd_line) {
        return Err(format!(
            "RustFS status cmdline '{}' does not match expected cmdline '{}'",
            status.cmd_line, expected_cmd_line
        ));
    }

    let observed_hash = endpoint_set_hash(&status.cmd_line);
    if observed_hash != expected_endpoint_set_hash {
        return Err(format!(
            "RustFS endpoint set hash '{}' does not match expected hash '{}'",
            observed_hash, expected_endpoint_set_hash
        ));
    }

    Ok(())
}

async fn verify_decommissioned_pool_for_cleanup(
    client: &RustfsAdminClient,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    existing: &PoolDecommissionStatus,
) -> Result<PoolDecommissionStatus, PoolLifecycleDecision> {
    let matched_pool = match find_rustfs_pool(client, tenant, namespace, pool).await {
        Ok(matched_pool) => matched_pool,
        Err(error) if error.is_retriable() => {
            return Err(cleanup_retriable_decision(
                existing.clone(),
                error.reason(),
                error.message(),
            ));
        }
        Err(error) => {
            return Err(failed_decision(
                existing.request_id.clone(),
                error.reason(),
                error.message(),
            ));
        }
    };
    let pool_id = matched_pool.item.id.to_string();

    let Some(existing_pool_id) = existing.rustfs_pool_id.as_deref() else {
        return Err(failed_decision(
            existing.request_id.clone(),
            "RustfsPoolIdentityMissing",
            "recorded decommission status is missing rustfsPoolID; refusing cleanup",
        ));
    };
    if existing_pool_id != pool_id {
        let message = format!(
            "recorded RustFS pool id '{}' no longer matches observed pool id '{}'",
            existing_pool_id, pool_id
        );
        return Err(failed_decision(
            existing.request_id.clone(),
            "RustfsPoolIdentityMismatch",
            &message,
        ));
    }

    let Some(existing_hash) = existing.endpoint_set_hash.as_deref() else {
        return Err(failed_decision(
            existing.request_id.clone(),
            "RustfsPoolIdentityMissing",
            "recorded decommission status is missing endpointSetHash; refusing cleanup",
        ));
    };
    if existing_hash != matched_pool.expected_endpoint_set_hash {
        return Err(failed_decision(
            existing.request_id.clone(),
            "RustfsPoolIdentityMismatch",
            "recorded endpoint set hash no longer matches the expected pool cmdline",
        ));
    }

    let rustfs_status = match client.pool_status_by_id(&pool_id).await {
        Ok(status) => status,
        Err(error) => {
            return Err(cleanup_retriable_decision(
                existing.clone(),
                "RustfsDecommissionStatusFailed",
                &error.to_string(),
            ));
        }
    };

    let request_id = existing
        .request_id
        .clone()
        .unwrap_or_else(|| "recorded-decommission".to_string());
    let request = DecommissionRequest {
        pool_name: pool.name.clone(),
        request_id,
        action: DecommissionAction::Start,
        requested_at: None,
        cancel_requested_at: None,
        reason: None,
    };

    let status = decommission_status_from_rustfs(
        &request,
        &rustfs_status,
        &matched_pool.expected_cmd_line,
        &matched_pool.expected_endpoint_set_hash,
    )
    .map_err(|message| {
        failed_decision(
            existing.request_id.clone(),
            "RustfsPoolIdentityMismatch",
            &message,
        )
    })?;

    if !matches!(status.phase, Some(PoolDecommissionPhase::Complete)) {
        return Err(failed_decision(
            existing.request_id.clone(),
            "RustfsDecommissionNotComplete",
            "RustFS no longer reports the pool decommission as complete; refusing cleanup",
        ));
    }

    Ok(status)
}

fn should_start_decommission(
    existing_state: Option<&PoolLifecycleState>,
    existing: Option<&PoolDecommissionStatus>,
    pool_item: &RustfsPoolListItem,
    request: &DecommissionRequest,
) -> Result<bool, String> {
    if pool_item.status == "running" {
        if existing
            .and_then(|status| status.request_id.as_deref())
            .is_some_and(|request_id| request_id == request.request_id)
            || decommission_started_after_request(pool_item, request)
        {
            return Ok(false);
        }

        return Err(format!(
            "RustFS already reports pool '{}' as decommissioning before request '{}' was observed",
            pool_item.cmd_line, request.request_id
        ));
    }

    if matches!(existing_state, Some(PoolLifecycleState::Decommissioning))
        && existing
            .and_then(|status| status.request_id.as_deref())
            .is_some_and(|request_id| request_id == request.request_id)
    {
        return Ok(false);
    }

    Ok(true)
}

fn decommission_started_after_request(
    pool_item: &RustfsPoolListItem,
    request: &DecommissionRequest,
) -> bool {
    let Some(requested_at) = request.requested_at.as_deref() else {
        return false;
    };
    let Some(started_at) = pool_item
        .decommission
        .as_ref()
        .and_then(|info| info.start_time.as_deref())
    else {
        return false;
    };

    let Ok(requested_at) = chrono::DateTime::parse_from_rfc3339(requested_at) else {
        return false;
    };
    let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return false;
    };

    started_at >= requested_at
}

fn decommission_status_from_rustfs(
    request: &DecommissionRequest,
    status: &RustfsPoolStatus,
    expected_cmd_line: &str,
    expected_endpoint_set_hash: &str,
) -> Result<PoolDecommissionStatus, String> {
    validate_rustfs_pool_identity(status, expected_cmd_line, expected_endpoint_set_hash)?;
    let info = status.decommission.as_ref();
    Ok(PoolDecommissionStatus {
        request_id: Some(request.request_id.clone()),
        rustfs_pool_id: Some(status.id.to_string()),
        endpoint_set_hash: Some(endpoint_set_hash(&status.cmd_line)),
        phase: Some(decommission_phase(info)),
        started_at: info.and_then(|info| info.start_time.clone()),
        last_poll_time: Some(now_rfc3339()),
        completed_at: completed_at(info),
        progress: Some(decommission_progress(info)),
        cleanup: None,
        last_error: None,
    })
}

fn lifecycle_state_from_status(status: &PoolDecommissionStatus) -> PoolLifecycleState {
    match status.phase {
        Some(PoolDecommissionPhase::Complete) => PoolLifecycleState::Decommissioned,
        Some(PoolDecommissionPhase::Canceled) => PoolLifecycleState::DecommissionCanceled,
        Some(PoolDecommissionPhase::Failed) => PoolLifecycleState::DecommissionFailed,
        _ => PoolLifecycleState::Decommissioning,
    }
}

fn decommission_phase(info: Option<&RustfsPoolDecommissionInfo>) -> PoolDecommissionPhase {
    let Some(info) = info else {
        return PoolDecommissionPhase::Running;
    };

    if info.canceled.unwrap_or(false) {
        PoolDecommissionPhase::Canceled
    } else if info.failed.unwrap_or(false) {
        PoolDecommissionPhase::Failed
    } else if info.complete.unwrap_or(false) {
        PoolDecommissionPhase::Complete
    } else {
        PoolDecommissionPhase::Running
    }
}

fn completed_at(info: Option<&RustfsPoolDecommissionInfo>) -> Option<String> {
    let info = info?;
    if info.complete.unwrap_or(false)
        || info.canceled.unwrap_or(false)
        || info.failed.unwrap_or(false)
    {
        Some(now_rfc3339())
    } else {
        None
    }
}

fn decommission_progress(info: Option<&RustfsPoolDecommissionInfo>) -> PoolDecommissionProgress {
    let Some(info) = info else {
        return PoolDecommissionProgress::default();
    };

    PoolDecommissionProgress {
        objects_migrated: info.objects_decommissioned.map(u64_to_i64_saturating),
        bytes_migrated: info.bytes_decommissioned.map(u64_to_i64_saturating),
        objects_failed: info
            .objects_decommissioned_failed
            .map(u64_to_i64_saturating),
        bytes_failed: info.bytes_decommissioned_failed.map(u64_to_i64_saturating),
    }
}

async fn cleanup_decommissioned_pool(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    mut status: PoolDecommissionStatus,
) -> PoolLifecycleDecision {
    let ss_name = format!("{}-{}", tenant.name(), pool.name);
    match ctx.get::<StatefulSet>(&ss_name, namespace).await {
        Ok(statefulset) if statefulset.metadata.deletion_timestamp.is_none() => {
            let delete_params = DeleteParams {
                propagation_policy: Some(PropagationPolicy::Background),
                ..DeleteParams::default()
            };
            match ctx
                .delete_with_params::<StatefulSet>(&ss_name, namespace, &delete_params)
                .await
            {
                Ok(()) => {
                    let _ = ctx
                        .record(
                            tenant,
                            EventType::Normal,
                            "PoolDecommissionCleanupStarted",
                            &format!(
                                "Deleting StatefulSet '{}' after RustFS decommission completed; PVCs are retained",
                                ss_name
                            ),
                        )
                        .await;
                    set_cleanup_status(
                        &mut status,
                        PoolDecommissionCleanupState::StatefulSetDeleting,
                    );
                }
                Err(error) if is_not_found_context_error(&error) => {
                    set_cleanup_status(&mut status, PoolDecommissionCleanupState::PvcRetained);
                }
                Err(error) => {
                    warn!(
                        tenant = %tenant.name(),
                        namespace = %namespace,
                        statefulset = %ss_name,
                        %error,
                        "failed to delete decommissioned pool StatefulSet"
                    );
                    status.last_error = Some(PoolDecommissionLastError {
                        reason: Some("StatefulSetDeleteFailed".to_string()),
                        message: Some(
                            "failed to delete decommissioned pool StatefulSet".to_string(),
                        ),
                    });
                    return cleanup_retriable_decision(
                        status,
                        "StatefulSetDeleteFailed",
                        "failed to delete decommissioned pool StatefulSet",
                    );
                }
            }
        }
        Ok(_) => {
            set_cleanup_status(
                &mut status,
                PoolDecommissionCleanupState::StatefulSetDeleting,
            );
        }
        Err(error) if is_not_found_context_error(&error) => {
            let was_retained = matches!(
                status.cleanup.as_ref().map(|cleanup| &cleanup.state),
                Some(PoolDecommissionCleanupState::PvcRetained)
            );
            set_cleanup_status(&mut status, PoolDecommissionCleanupState::PvcRetained);
            if !was_retained {
                let _ = ctx
                    .record(
                        tenant,
                        EventType::Normal,
                        "PvcRetained",
                        &format!(
                            "StatefulSet '{}' is deleted after decommission; PVCs are retained",
                            ss_name
                        ),
                    )
                    .await;
            }
        }
        Err(error) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                statefulset = %ss_name,
                %error,
                "failed to inspect decommissioned pool StatefulSet"
            );
            status.last_error = Some(PoolDecommissionLastError {
                reason: Some("StatefulSetInspectFailed".to_string()),
                message: Some("failed to inspect decommissioned pool StatefulSet".to_string()),
            });
            return cleanup_retriable_decision(
                status,
                "StatefulSetInspectFailed",
                "failed to inspect decommissioned pool StatefulSet",
            );
        }
    }

    PoolLifecycleDecision {
        state: PoolLifecycleState::Decommissioned,
        decommission: Some(status),
        skip_workload_reconcile: true,
    }
}

fn terminal_decision_from_existing(
    existing_state: Option<PoolLifecycleState>,
    existing: Option<PoolDecommissionStatus>,
) -> PoolLifecycleDecision {
    let state = existing_state.unwrap_or(PoolLifecycleState::Active);
    if matches!(
        state,
        PoolLifecycleState::DecommissionCanceled | PoolLifecycleState::DecommissionFailed
    ) {
        return active_lifecycle_decision();
    }

    PoolLifecycleDecision {
        skip_workload_reconcile: !matches!(state, PoolLifecycleState::Active),
        state,
        decommission: existing,
    }
}

fn start_decommission_requests_that_would_remove_all_live_pools(
    tenant: &Tenant,
) -> BTreeSet<String> {
    let live_pool_names = live_pool_names_for_decommission_guard(tenant);
    if live_pool_names.is_empty() {
        return BTreeSet::new();
    }

    let requested_live_starts = tenant
        .spec
        .pool_lifecycle
        .as_ref()
        .map(|lifecycle| {
            lifecycle
                .decommission_requests
                .iter()
                .filter(|request| {
                    request.action == DecommissionAction::Start
                        && live_pool_names.contains(&request.pool_name)
                })
                .map(|request| request.pool_name.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    if requested_live_starts.len() >= live_pool_names.len() {
        requested_live_starts
    } else {
        BTreeSet::new()
    }
}

fn should_block_last_live_pool_decommission(
    request: &DecommissionRequest,
    block_start_decommission: bool,
) -> bool {
    request.action == DecommissionAction::Start && block_start_decommission
}

fn live_pool_names_for_decommission_guard(tenant: &Tenant) -> BTreeSet<String> {
    tenant
        .spec
        .pools
        .iter()
        .filter(|pool| {
            pool_counts_as_live_for_decommission_guard(existing_lifecycle_state(tenant, &pool.name))
        })
        .map(|pool| pool.name.clone())
        .collect()
}

fn pool_counts_as_live_for_decommission_guard(state: Option<PoolLifecycleState>) -> bool {
    !matches!(
        state,
        Some(PoolLifecycleState::Decommissioning | PoolLifecycleState::Decommissioned)
    )
}

fn active_lifecycle_decision() -> PoolLifecycleDecision {
    PoolLifecycleDecision {
        state: PoolLifecycleState::Active,
        decommission: None,
        skip_workload_reconcile: false,
    }
}

fn cancel_without_decommission_info_is_noop(
    request: &DecommissionRequest,
    decommission: Option<&RustfsPoolDecommissionInfo>,
) -> bool {
    request.action == DecommissionAction::Cancel && decommission.is_none()
}

fn failed_decision(
    request_id: Option<String>,
    reason: &str,
    message: &str,
) -> PoolLifecycleDecision {
    PoolLifecycleDecision {
        state: PoolLifecycleState::DecommissionFailed,
        decommission: Some(PoolDecommissionStatus {
            request_id,
            phase: Some(PoolDecommissionPhase::Failed),
            last_poll_time: Some(now_rfc3339()),
            last_error: Some(PoolDecommissionLastError {
                reason: Some(reason.to_string()),
                message: Some(message.to_string()),
            }),
            ..empty_decommission_status()
        }),
        skip_workload_reconcile: true,
    }
}

fn retriable_decision(
    request_id: Option<String>,
    reason: &str,
    message: &str,
) -> PoolLifecycleDecision {
    PoolLifecycleDecision {
        state: PoolLifecycleState::Decommissioning,
        decommission: Some(PoolDecommissionStatus {
            request_id,
            phase: Some(PoolDecommissionPhase::Pending),
            last_poll_time: Some(now_rfc3339()),
            last_error: Some(PoolDecommissionLastError {
                reason: Some(reason.to_string()),
                message: Some(message.to_string()),
            }),
            ..empty_decommission_status()
        }),
        skip_workload_reconcile: true,
    }
}

fn cleanup_retriable_decision(
    mut status: PoolDecommissionStatus,
    reason: &str,
    message: &str,
) -> PoolLifecycleDecision {
    if status.cleanup.is_none() {
        set_cleanup_status(&mut status, PoolDecommissionCleanupState::Pending);
    }
    status.last_poll_time = Some(now_rfc3339());
    status.last_error = Some(PoolDecommissionLastError {
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
    });

    PoolLifecycleDecision {
        state: PoolLifecycleState::Decommissioned,
        decommission: Some(status),
        skip_workload_reconcile: true,
    }
}

fn empty_decommission_status() -> PoolDecommissionStatus {
    PoolDecommissionStatus {
        request_id: None,
        rustfs_pool_id: None,
        endpoint_set_hash: None,
        phase: None,
        started_at: None,
        last_poll_time: None,
        completed_at: None,
        progress: None,
        cleanup: None,
        last_error: None,
    }
}

fn cleanup_already_authorized_or_complete(status: &PoolDecommissionStatus) -> bool {
    status.cleanup.as_ref().is_some_and(|cleanup| {
        matches!(
            cleanup.state,
            PoolDecommissionCleanupState::StatefulSetDeleting
                | PoolDecommissionCleanupState::PvcRetained
        )
    })
}

fn decommissioned_cleanup_needs_requeue(status: &PoolDecommissionStatus) -> bool {
    !status
        .cleanup
        .as_ref()
        .is_some_and(|cleanup| matches!(cleanup.state, PoolDecommissionCleanupState::PvcRetained))
}

fn cleanup_status(state: PoolDecommissionCleanupState) -> PoolDecommissionCleanupStatus {
    let stateful_set_deleted_at =
        matches!(state, PoolDecommissionCleanupState::PvcRetained).then(now_rfc3339);

    PoolDecommissionCleanupStatus {
        state,
        stateful_set_deleted_at,
        pvc_retention_policy: Some("Retain".to_string()),
    }
}

fn set_cleanup_status(status: &mut PoolDecommissionStatus, state: PoolDecommissionCleanupState) {
    if status
        .cleanup
        .as_ref()
        .is_some_and(|cleanup| cleanup.state == state)
    {
        return;
    }

    status.cleanup = Some(cleanup_status(state));
}

fn existing_lifecycle_state(tenant: &Tenant, pool_name: &str) -> Option<PoolLifecycleState> {
    existing_pool_status(tenant, pool_name).and_then(|status| status.lifecycle_state.clone())
}

fn existing_decommission_status(
    tenant: &Tenant,
    pool_name: &str,
) -> Option<PoolDecommissionStatus> {
    existing_pool_status(tenant, pool_name).and_then(|status| status.decommission.clone())
}

fn existing_pool_status<'a>(
    tenant: &'a Tenant,
    pool_name: &str,
) -> Option<&'a crate::types::v1alpha1::status::pool::Pool> {
    let ss_name = format!("{}-{}", tenant.name(), pool_name);
    tenant
        .status
        .as_ref()?
        .pools
        .iter()
        .find(|status| status.name.as_deref() == Some(pool_name) || status.ss_name == ss_name)
}

fn endpoint_set_hash(cmd_line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cmd_line.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn is_not_found_context_error(error: &context::Error) -> bool {
    matches!(
        error,
        context::Error::Kube {
            source: kube::Error::Api(api_error)
        } if api_error.code == 404
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::persistence::PersistenceConfig;
    use crate::types::v1alpha1::pool::SchedulingConfig;
    use crate::types::v1alpha1::tenant::TenantSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn test_pool(name: &str) -> Pool {
        Pool {
            name: name.to_string(),
            servers: 4,
            persistence: PersistenceConfig {
                volumes_per_server: 2,
                ..Default::default()
            },
            scheduling: SchedulingConfig::default(),
        }
    }

    fn test_tenant(pool: Pool) -> Tenant {
        test_tenant_with_pools(vec![pool])
    }

    fn test_tenant_with_pools(pools: Vec<Pool>) -> Tenant {
        Tenant {
            metadata: ObjectMeta {
                name: Some("logs".to_string()),
                namespace: Some("rustfs-system".to_string()),
                ..Default::default()
            },
            spec: TenantSpec {
                pools,
                ..Default::default()
            },
            status: None,
        }
    }

    fn test_pool_status(
        pool_name: &str,
        lifecycle_state: PoolLifecycleState,
    ) -> crate::types::v1alpha1::status::pool::Pool {
        crate::types::v1alpha1::status::pool::Pool {
            name: Some(pool_name.to_string()),
            ss_name: format!("logs-{pool_name}"),
            state: crate::types::v1alpha1::status::pool::PoolState::RolloutComplete,
            lifecycle_state: Some(lifecycle_state),
            workload_state: Some(crate::types::v1alpha1::status::pool::PoolState::RolloutComplete),
            decommission: None,
            replicas: Some(4),
            ready_replicas: Some(4),
            current_replicas: Some(4),
            updated_replicas: Some(4),
            current_revision: Some("rev-a".to_string()),
            update_revision: Some("rev-a".to_string()),
            last_update_time: None,
        }
    }

    fn start_request(pool_name: &str) -> DecommissionRequest {
        start_request_with_id(pool_name, "request-1")
    }

    fn start_request_with_id(pool_name: &str, request_id: &str) -> DecommissionRequest {
        DecommissionRequest {
            pool_name: pool_name.to_string(),
            request_id: request_id.to_string(),
            action: DecommissionAction::Start,
            requested_at: Some("2026-05-20T00:00:05Z".to_string()),
            cancel_requested_at: None,
            reason: None,
        }
    }

    fn set_decommission_requests(tenant: &mut Tenant, requests: Vec<DecommissionRequest>) {
        tenant.spec.pool_lifecycle =
            Some(crate::types::v1alpha1::pool_lifecycle::PoolLifecycleSpec {
                pvc_retention_policy:
                    crate::types::v1alpha1::pool_lifecycle::PvcRetentionPolicy::Retain,
                decommission_requests: requests,
            });
    }

    #[test]
    fn lifecycle_phase_maps_current_rustfs_terminal_flags() {
        let complete = RustfsPoolDecommissionInfo {
            complete: Some(true),
            ..Default::default()
        };
        let canceled = RustfsPoolDecommissionInfo {
            canceled: Some(true),
            ..Default::default()
        };
        let failed = RustfsPoolDecommissionInfo {
            failed: Some(true),
            ..Default::default()
        };

        assert_eq!(
            decommission_phase(Some(&complete)),
            PoolDecommissionPhase::Complete
        );
        assert_eq!(
            decommission_phase(Some(&canceled)),
            PoolDecommissionPhase::Canceled
        );
        assert_eq!(
            decommission_phase(Some(&failed)),
            PoolDecommissionPhase::Failed
        );
        assert_eq!(decommission_phase(None), PoolDecommissionPhase::Running);

        let canceled_and_complete = RustfsPoolDecommissionInfo {
            complete: Some(true),
            canceled: Some(true),
            ..Default::default()
        };
        assert_eq!(
            decommission_phase(Some(&canceled_and_complete)),
            PoolDecommissionPhase::Canceled
        );

        let failed_and_complete = RustfsPoolDecommissionInfo {
            complete: Some(true),
            failed: Some(true),
            ..Default::default()
        };
        assert_eq!(
            decommission_phase(Some(&failed_and_complete)),
            PoolDecommissionPhase::Failed
        );
    }

    #[test]
    fn decommissioned_cleanup_skips_workload_reconcile() {
        let decision = terminal_decision_from_existing(
            Some(PoolLifecycleState::Decommissioned),
            Some(empty_decommission_status()),
        );

        assert!(decision.skip_workload_reconcile);
        assert_eq!(decision.state, PoolLifecycleState::Decommissioned);
    }

    #[test]
    fn canceled_or_failed_terminal_state_without_request_restores_active_reconcile() {
        for state in [
            PoolLifecycleState::DecommissionCanceled,
            PoolLifecycleState::DecommissionFailed,
        ] {
            let decision =
                terminal_decision_from_existing(Some(state), Some(empty_decommission_status()));

            assert_eq!(decision.state, PoolLifecycleState::Active);
            assert!(!decision.skip_workload_reconcile);
            assert!(decision.decommission.is_none());
        }
    }

    #[test]
    fn start_decommission_blocks_last_live_pool() {
        let old_pool = test_pool("old-pool");
        let active_pool = test_pool("active-pool");
        let mut tenant = test_tenant_with_pools(vec![old_pool, active_pool.clone()]);
        let request = start_request("active-pool");
        set_decommission_requests(&mut tenant, vec![request.clone()]);
        tenant.status = Some(crate::types::v1alpha1::status::Status {
            pools: vec![
                test_pool_status("old-pool", PoolLifecycleState::Decommissioned),
                test_pool_status("active-pool", PoolLifecycleState::Active),
            ],
            ..Default::default()
        });

        assert_eq!(live_pool_names_for_decommission_guard(&tenant).len(), 1);
        let blocked_pools = start_decommission_requests_that_would_remove_all_live_pools(&tenant);
        assert!(blocked_pools.contains("active-pool"));
        assert!(should_block_last_live_pool_decommission(
            &request,
            blocked_pools.contains(&active_pool.name)
        ));
    }

    #[test]
    fn start_decommission_allows_one_of_multiple_live_pools() {
        let pool_a = test_pool("pool-a");
        let pool_b = test_pool("pool-b");
        let mut tenant = test_tenant_with_pools(vec![pool_a.clone(), pool_b]);
        let request = start_request("pool-a");
        set_decommission_requests(&mut tenant, vec![request.clone()]);

        assert_eq!(live_pool_names_for_decommission_guard(&tenant).len(), 2);
        let blocked_pools = start_decommission_requests_that_would_remove_all_live_pools(&tenant);
        assert!(blocked_pools.is_empty());
        assert!(!should_block_last_live_pool_decommission(
            &request,
            blocked_pools.contains(&pool_a.name)
        ));
    }

    #[test]
    fn start_decommission_blocks_batch_that_would_remove_all_live_pools() {
        let pool_a = test_pool("pool-a");
        let pool_b = test_pool("pool-b");
        let mut tenant = test_tenant_with_pools(vec![pool_a.clone(), pool_b.clone()]);
        let request_a = start_request_with_id("pool-a", "request-a");
        let request_b = start_request_with_id("pool-b", "request-b");
        set_decommission_requests(&mut tenant, vec![request_a.clone(), request_b.clone()]);

        assert_eq!(live_pool_names_for_decommission_guard(&tenant).len(), 2);
        let blocked_pools = start_decommission_requests_that_would_remove_all_live_pools(&tenant);
        assert_eq!(blocked_pools.len(), 2);
        assert!(should_block_last_live_pool_decommission(
            &request_a,
            blocked_pools.contains(&pool_a.name)
        ));
        assert!(should_block_last_live_pool_decommission(
            &request_b,
            blocked_pools.contains(&pool_b.name)
        ));
    }

    #[test]
    fn decommissioned_cleanup_pending_keeps_tenant_reconciling() {
        let mut status = empty_decommission_status();
        status.cleanup = Some(cleanup_status(
            PoolDecommissionCleanupState::StatefulSetDeleting,
        ));

        let mut decisions = PoolLifecycleDecisions::default();
        decisions.insert(
            "pool-a".to_string(),
            PoolLifecycleDecision {
                state: PoolLifecycleState::Decommissioned,
                decommission: Some(status),
                skip_workload_reconcile: true,
            },
        );

        assert!(decisions.any_reconciling);
        assert_eq!(decisions.requeue_after, Some(POLL_INTERVAL));
    }

    #[test]
    fn expected_pool_cmd_line_matches_workload_volume_format() {
        let pool = test_pool("pool-a");
        let tenant = test_tenant(pool.clone());

        assert_eq!(
            expected_pool_cmd_line(&tenant, "rustfs-system", &pool).unwrap(),
            "http://logs-pool-a-{0...3}.logs-hl.rustfs-system.svc.cluster.local:9000/data/rustfs{0...1}"
        );
    }

    #[test]
    fn decommission_started_after_request_uses_rustfs_start_time() {
        let pool_item = RustfsPoolListItem {
            id: 1,
            cmd_line: "pool-a".to_string(),
            last_update: "2026-05-20T00:00:05Z".to_string(),
            total_size: None,
            current_size: None,
            used_size: None,
            used: None,
            status: "running".to_string(),
            decommission: Some(RustfsPoolDecommissionInfo {
                start_time: Some("2026-05-20T00:00:05Z".to_string()),
                ..Default::default()
            }),
        };
        let request = DecommissionRequest {
            pool_name: "pool-a".to_string(),
            request_id: "request-1".to_string(),
            action: DecommissionAction::Start,
            requested_at: Some("2026-05-20T00:00:00Z".to_string()),
            cancel_requested_at: None,
            reason: None,
        };

        assert!(decommission_started_after_request(&pool_item, &request));
    }

    #[test]
    fn running_decommission_before_request_is_not_adopted() {
        let pool_item = RustfsPoolListItem {
            id: 1,
            cmd_line: "pool-a".to_string(),
            last_update: "2026-05-20T00:00:00Z".to_string(),
            total_size: None,
            current_size: None,
            used_size: None,
            used: None,
            status: "running".to_string(),
            decommission: Some(RustfsPoolDecommissionInfo {
                start_time: Some("2026-05-20T00:00:00Z".to_string()),
                ..Default::default()
            }),
        };
        let request = DecommissionRequest {
            pool_name: "pool-a".to_string(),
            request_id: "request-1".to_string(),
            action: DecommissionAction::Start,
            requested_at: Some("2026-05-20T00:00:05Z".to_string()),
            cancel_requested_at: None,
            reason: None,
        };

        assert!(should_start_decommission(None, None, &pool_item, &request).is_err());
    }

    #[test]
    fn cancel_without_decommission_info_maps_to_active_noop() {
        let request = DecommissionRequest {
            pool_name: "pool-a".to_string(),
            request_id: "request-1".to_string(),
            action: DecommissionAction::Cancel,
            requested_at: None,
            cancel_requested_at: Some("2026-05-20T00:00:05Z".to_string()),
            reason: None,
        };
        let status = RustfsPoolStatus {
            id: 1,
            cmd_line: "pool-a".to_string(),
            last_update: "2026-05-20T00:00:10Z".to_string(),
            decommission: None,
        };

        assert!(cancel_without_decommission_info_is_noop(
            &request,
            status.decommission.as_ref()
        ));
        let decision = active_lifecycle_decision();
        assert_eq!(decision.state, PoolLifecycleState::Active);
        assert!(!decision.skip_workload_reconcile);
        assert!(decision.decommission.is_none());
    }

    #[test]
    fn start_without_decommission_info_is_not_noop() {
        let request = DecommissionRequest {
            pool_name: "pool-a".to_string(),
            request_id: "request-1".to_string(),
            action: DecommissionAction::Start,
            requested_at: Some("2026-05-20T00:00:05Z".to_string()),
            cancel_requested_at: None,
            reason: None,
        };
        let status = RustfsPoolStatus {
            id: 1,
            cmd_line: "pool-a".to_string(),
            last_update: "2026-05-20T00:00:10Z".to_string(),
            decommission: None,
        };

        assert!(!cancel_without_decommission_info_is_noop(
            &request,
            status.decommission.as_ref()
        ));
    }

    #[test]
    fn cleanup_deleting_state_does_not_claim_statefulset_deleted() {
        let status = cleanup_status(PoolDecommissionCleanupState::StatefulSetDeleting);

        assert_eq!(
            status.state,
            PoolDecommissionCleanupState::StatefulSetDeleting
        );
        assert!(status.stateful_set_deleted_at.is_none());
    }
}
