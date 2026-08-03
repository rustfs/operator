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

use crate::context::{self, Context};
use crate::sts::rustfs_client::{
    CreateBucketResult, RustfsAdminClient, RustfsClientError, client_from_tenant,
    client_from_tls_tenant_for_sts, load_tenant_credentials,
};
use crate::types::v1alpha1::provisioning::{
    ProvisioningBucket, ProvisioningPolicy, ProvisioningUser,
    duplicate_user_credentials_secret_names,
};
use crate::types::v1alpha1::status::Reason;
use crate::types::v1alpha1::status::provisioning::{
    ProvisioningItemState, ProvisioningItemStatus, ProvisioningPhase, ProvisioningStatus,
    ProvisioningUserOwnershipState, ProvisioningUserOwnershipStatus, ProvisioningUserStatus,
};
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tracing::{info, warn};

const CHECKPOINT_CONFLICT_RETRY: Duration = Duration::from_secs(2);
const CHECKPOINT_TRANSIENT_RETRY: Duration = Duration::from_secs(10);

pub(super) struct ProvisioningReconcileResult {
    pub status: ProvisioningStatus,
    pub outcome: ProvisioningOutcome,
}

pub(super) enum ProvisioningOutcome {
    Ready,
    Pending {
        message: String,
    },
    Failed {
        reason: Reason,
        message: String,
    },
    Retry {
        message: String,
        retry_after: Duration,
    },
}

#[derive(Clone, Debug)]
struct CheckpointRetry {
    message: String,
    retry_after: Duration,
}

#[derive(Debug)]
enum CheckpointError {
    Permanent { message: String },
    Retry(CheckpointRetry),
}

struct ProvisioningRun<'a> {
    ctx: &'a Context,
    tenant: &'a Tenant,
    namespace: &'a str,
    previous: ProvisioningStatus,
    now: String,
    status: ProvisioningStatus,
    failures: Vec<(Reason, String)>,
}

#[derive(Clone)]
struct UserCredentials {
    access_key: String,
    secret_key: String,
    secret_name: String,
    resource_version: Option<String>,
}

enum UserCredentialsCheck {
    DuplicateSecret,
    Checked {
        policy_error: Option<String>,
        credentials: Result<UserCredentials, String>,
    },
}

struct UserCredentialsPreflight {
    checks: Vec<UserCredentialsCheck>,
    duplicate_access_key_hashes: BTreeSet<String>,
}

enum UserReconcilePlan {
    Complete(Box<ProvisioningUserStatus>),
    Prepared(Box<PreparedUserReconcile>),
}

struct PreparedUserReconcile {
    user: ProvisioningUser,
    credentials: UserCredentials,
    exists: bool,
    ownership: ProvisioningUserOwnershipStatus,
    checkpoint: Option<ProvisioningUserStatus>,
}

struct PolicyDocument {
    raw: String,
    normalized: String,
}

#[derive(Debug, PartialEq, Eq)]
enum PolicyReconcileAction {
    Ready(&'static str),
    Apply(&'static str),
    Failed(Reason, &'static str),
}

impl PolicyDocument {
    fn parse(raw: &str) -> Result<Self, String> {
        Ok(Self {
            raw: raw.to_string(),
            normalized: normalize_policy_document(raw)?,
        })
    }

    fn hash(&self) -> String {
        hash_document(&self.normalized)
    }
}

impl ProvisioningRun<'_> {
    fn previous_policy(&self, name: &str) -> Option<&ProvisioningItemStatus> {
        self.previous.policies.iter().find(|item| item.name == name)
    }

    fn previous_user(&self, name: &str) -> Option<&ProvisioningUserStatus> {
        self.previous.users.iter().find(|item| item.name == name)
    }

    fn previous_bucket(&self, name: &str) -> Option<&ProvisioningItemStatus> {
        self.previous.buckets.iter().find(|item| item.name == name)
    }

    fn push_policy(&mut self, item: ProvisioningItemStatus) {
        self.log_item_transition("policy", self.previous_policy(&item.name), &item);
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.policies.push(item);
    }

    fn push_user(&mut self, item: ProvisioningUserStatus) {
        self.log_item_transition(
            "user",
            self.previous_user(&item.name).map(AsRef::as_ref),
            item.as_ref(),
        );
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.users.push(item);
    }

    fn push_bucket(&mut self, item: ProvisioningItemStatus) {
        self.log_item_transition("bucket", self.previous_bucket(&item.name), &item);
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.buckets.push(item);
    }

    fn log_item_transition(
        &self,
        item_type: &'static str,
        previous: Option<&ProvisioningItemStatus>,
        item: &ProvisioningItemStatus,
    ) {
        let changed = match previous {
            Some(previous) => {
                previous.state != item.state
                    || previous.reason != item.reason
                    || previous.message != item.message
            }
            None => true,
        };
        if !changed {
            return;
        }

        let message = item.message.as_deref().unwrap_or("");
        if item.state == ProvisioningItemState::Failed.as_str() {
            warn!(
                tenant = %self.tenant.name(),
                namespace = %self.namespace,
                item_type = %item_type,
                item = %item.name,
                state = %item.state,
                reason = %item.reason,
                message = %message,
                "RustFS provisioning item failed"
            );
        } else {
            info!(
                tenant = %self.tenant.name(),
                namespace = %self.namespace,
                item_type = %item_type,
                item = %item.name,
                state = %item.state,
                reason = %item.reason,
                message = %message,
                "RustFS provisioning item state changed"
            );
        }
    }

    fn item<P>(
        &self,
        previous: Option<&P>,
        name: &str,
        state: ProvisioningItemState,
        reason: Reason,
        message: impl Into<String>,
    ) -> ProvisioningItemStatus
    where
        P: AsRef<ProvisioningItemStatus> + ?Sized,
    {
        let previous = previous.map(AsRef::as_ref);
        let message = message.into();
        let mut item = ProvisioningItemStatus::new(name, state, reason.as_str());
        item.message = Some(message.clone());
        item.last_transition_time = match previous {
            Some(previous)
                if previous.state == item.state
                    && previous.reason == item.reason
                    && previous.message.as_deref() == Some(message.as_str()) =>
            {
                previous.last_transition_time.clone()
            }
            _ => Some(self.now.clone()),
        };
        item
    }

    fn retained_item(&self, previous: &ProvisioningItemStatus) -> ProvisioningItemStatus {
        let mut item = self.item(
            Some(previous),
            &previous.name,
            ProvisioningItemState::Retained,
            Reason::ProvisioningConfigured,
            "Item was removed from spec and retained in RustFS",
        );
        item.desired_hash = previous.desired_hash.clone();
        item.last_applied_hash = previous.last_applied_hash.clone();
        item.last_applied_generation = previous.last_applied_generation;
        item.observed_secret_resource_version = previous.observed_secret_resource_version.clone();
        item.observed_secret_name = previous.observed_secret_name.clone();
        item.last_applied_access_key_hash = previous.last_applied_access_key_hash.clone();
        item.policies = previous.policies.clone();
        item.region = previous.region.clone();
        item.object_lock = previous.object_lock;
        item
    }

    fn retained_user(&self, previous: &ProvisioningUserStatus) -> ProvisioningUserStatus {
        let mut item = ProvisioningUserStatus::new(self.retained_item(previous.as_ref()));
        item.ownership = previous.ownership.clone();
        item
    }

    fn mark_all_active(&mut self, state: ProvisioningItemState, reason: Reason, message: &str) {
        for policy in &self.tenant.spec.policies {
            let mut item = self.item(
                self.previous_policy(&policy.name),
                &policy.name,
                state.clone(),
                reason,
                message,
            );
            if let Some(previous) = self.previous_policy(&policy.name) {
                item.desired_hash = previous.desired_hash.clone();
                item.last_applied_hash = previous.last_applied_hash.clone();
                item.last_applied_generation = previous.last_applied_generation;
            }
            self.push_policy(item);
        }
        for user in &self.tenant.spec.users {
            let mut item = self.item(
                self.previous_user(&user.name),
                &user.name,
                state.clone(),
                reason,
                message,
            );
            if let Some(previous) = self.previous_user(&user.name) {
                item.observed_secret_resource_version =
                    previous.observed_secret_resource_version.clone();
                item.observed_secret_name = previous.observed_secret_name.clone();
                item.last_applied_access_key_hash = previous.last_applied_access_key_hash.clone();
                item.policies = previous.policies.clone();
            }
            let mut item = ProvisioningUserStatus::new(item);
            item.ownership = self
                .previous_user(&user.name)
                .and_then(|previous| previous.ownership.clone());
            self.push_user(item);
        }
        for bucket in &self.tenant.spec.buckets {
            let item = self.item(
                self.previous_bucket(&bucket.name),
                &bucket.name,
                state.clone(),
                reason,
                message,
            );
            self.push_bucket(item);
        }
    }

    fn fail_all_active(&mut self, reason: Reason, message: &str) {
        self.mark_all_active(ProvisioningItemState::Failed, reason, message);
    }

    fn add_retained_items(&mut self) {
        let policies = desired_names(self.tenant.spec.policies.iter().map(|policy| &policy.name));
        for previous in &self.previous.policies {
            if !policies.contains(&previous.name) {
                self.status.policies.push(self.retained_item(previous));
            }
        }

        let users = desired_names(self.tenant.spec.users.iter().map(|user| &user.name));
        for previous in &self.previous.users {
            if !users.contains(&previous.name) {
                self.status.users.push(self.retained_user(previous));
            }
        }

        let buckets = desired_names(self.tenant.spec.buckets.iter().map(|bucket| &bucket.name));
        for previous in &self.previous.buckets {
            if !buckets.contains(&previous.name) {
                self.status.buckets.push(self.retained_item(previous));
            }
        }
    }

    fn prepare_status(&mut self, phase: ProvisioningPhase) {
        self.add_retained_items();
        self.status.policies.sort_by(|a, b| a.name.cmp(&b.name));
        self.status.users.sort_by(|a, b| a.name.cmp(&b.name));
        self.status.buckets.sort_by(|a, b| a.name.cmp(&b.name));
        if !self.status.is_empty() {
            self.status.observed_generation = self.tenant.metadata.generation;
            self.status.phase = Some(phase);
        }
    }

    fn finish(mut self) -> ProvisioningReconcileResult {
        let outcome = self
            .failures
            .first()
            .map(|(reason, message)| ProvisioningOutcome::Failed {
                reason: *reason,
                message: message.clone(),
            })
            .unwrap_or(ProvisioningOutcome::Ready);
        let phase = match &outcome {
            ProvisioningOutcome::Ready => ProvisioningPhase::Ready,
            ProvisioningOutcome::Pending { .. } => ProvisioningPhase::Pending,
            ProvisioningOutcome::Failed { .. } => ProvisioningPhase::Failed,
            ProvisioningOutcome::Retry { .. } => ProvisioningPhase::Pending,
        };
        self.prepare_status(phase);

        ProvisioningReconcileResult {
            status: self.status,
            outcome,
        }
    }
}

pub(super) async fn reconcile_provisioning(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> ProvisioningReconcileResult {
    let previous = tenant
        .status
        .as_ref()
        .map(|status| status.provisioning.clone())
        .unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut run = ProvisioningRun {
        ctx,
        tenant,
        namespace,
        previous,
        now,
        status: ProvisioningStatus::default(),
        failures: Vec::new(),
    };

    if !has_active_spec(tenant) {
        run.prepare_status(ProvisioningPhase::Ready);
        return ProvisioningReconcileResult {
            status: run.status,
            outcome: ProvisioningOutcome::Ready,
        };
    }

    let client = match rustfs_admin_client(ctx, tenant).await {
        Ok(client) => client,
        Err(error) => {
            let (reason, message, pending) = client_error_outcome(error);
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                reason = reason.as_str(),
                pending,
                message = %message,
                "RustFS provisioning admin client unavailable"
            );
            if pending {
                run.mark_all_active(ProvisioningItemState::Pending, reason, &message);
            } else {
                run.fail_all_active(reason, &message);
            }
            let phase = if pending {
                ProvisioningPhase::Pending
            } else {
                ProvisioningPhase::Failed
            };
            run.prepare_status(phase);
            return ProvisioningReconcileResult {
                status: run.status,
                outcome: if pending {
                    ProvisioningOutcome::Pending { message }
                } else {
                    ProvisioningOutcome::Failed { reason, message }
                },
            };
        }
    };
    let user_credentials = preflight_user_credentials(&run).await;

    let mut live_policies = match load_live_policies(&client, tenant).await {
        Ok(policies) => policies,
        Err(message) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                reason = Reason::PolicyApplyFailed.as_str(),
                message = %message,
                "RustFS provisioning failed to load live policies"
            );
            run.fail_all_active(Reason::PolicyApplyFailed, &message);
            run.prepare_status(ProvisioningPhase::Failed);
            return ProvisioningReconcileResult {
                status: run.status,
                outcome: ProvisioningOutcome::Failed {
                    reason: Reason::PolicyApplyFailed,
                    message,
                },
            };
        }
    };

    reconcile_policies(&mut run, &client, &mut live_policies).await;
    if let Some(retry) = reconcile_users(&mut run, &client, &live_policies, &user_credentials).await
    {
        return ProvisioningReconcileResult {
            status: run.status,
            outcome: ProvisioningOutcome::Retry {
                message: retry.message,
                retry_after: retry.retry_after,
            },
        };
    }
    reconcile_buckets(&mut run, &client).await;
    run.finish()
}

async fn rustfs_admin_client(
    ctx: &Context,
    tenant: &Tenant,
) -> Result<RustfsAdminClient, RustfsClientError> {
    let credentials = load_tenant_credentials(&ctx.client, tenant).await?;
    if tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
        client_from_tls_tenant_for_sts(&ctx.client, tenant, credentials, ctx.cluster_domain()).await
    } else {
        client_from_tenant(tenant, credentials)
    }
}

fn client_error_outcome(error: RustfsClientError) -> (Reason, String, bool) {
    match error {
        RustfsClientError::MissingCredsSecret => (
            Reason::ProvisioningUnsupported,
            "configure spec.credsSecret before enabling provisioning".to_string(),
            false,
        ),
        RustfsClientError::TenantTlsClientCertificateRequired => (
            Reason::ProvisioningUnsupported,
            "tenant TLS client certificate authentication is not supported for provisioning yet"
                .to_string(),
            false,
        ),
        RustfsClientError::TenantTlsNotReady => (
            Reason::ProvisioningPending,
            "tenant TLS is not ready for provisioning".to_string(),
            true,
        ),
        error => (
            Reason::ProvisioningFailed,
            format!("failed to create RustFS admin client: {error}"),
            false,
        ),
    }
}

async fn load_live_policies(
    client: &RustfsAdminClient,
    tenant: &Tenant,
) -> Result<BTreeMap<String, String>, String> {
    if tenant.spec.policies.is_empty()
        && tenant
            .spec
            .users
            .iter()
            .all(|user| user.policies.is_empty())
    {
        return Ok(BTreeMap::new());
    }

    let mut policies = client
        .list_canned_policies()
        .await
        .map_err(|error| format!("failed to list RustFS canned policies: {error}"))?;

    for (name, document) in &mut policies {
        *document = normalize_policy_document(document)
            .map_err(|error| format!("failed to normalize live RustFS policy '{name}': {error}"))?;
    }

    Ok(policies)
}

async fn reconcile_policies(
    run: &mut ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &mut BTreeMap<String, String>,
) {
    for policy in &run.tenant.spec.policies {
        let item = reconcile_policy(run, client, live_policies, policy).await;
        run.push_policy(item);
    }
}

async fn reconcile_policy(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &mut BTreeMap<String, String>,
    policy: &ProvisioningPolicy,
) -> ProvisioningItemStatus {
    let previous = run.previous_policy(&policy.name);
    let document = match load_policy_document(run, policy).await {
        Ok(document) => document,
        Err((reason, message)) => {
            return run.item(
                previous,
                &policy.name,
                ProvisioningItemState::Failed,
                reason,
                message,
            );
        }
    };

    let desired_hash = document.hash();
    let live_hash = live_policies
        .get(&policy.name)
        .map(|live_document| hash_document(live_document));
    let item = match policy_reconcile_action(previous, live_hash.as_deref(), &desired_hash) {
        PolicyReconcileAction::Ready(message) => run.item(
            previous,
            &policy.name,
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured,
            message,
        ),
        PolicyReconcileAction::Apply(message) => {
            match apply_policy(client, live_policies, &policy.name, &document.raw).await {
                Ok(applied_hash) => {
                    let mut item = run.item(
                        previous,
                        &policy.name,
                        ProvisioningItemState::Ready,
                        Reason::ProvisioningConfigured,
                        message,
                    );
                    item.last_applied_hash = Some(applied_hash);
                    item
                }
                Err(message) => run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Failed,
                    Reason::PolicyApplyFailed,
                    message,
                ),
            }
        }
        PolicyReconcileAction::Failed(reason, message) => run.item(
            previous,
            &policy.name,
            ProvisioningItemState::Failed,
            reason,
            message,
        ),
    };

    finalize_policy_item_status(
        item,
        previous,
        &policy.name,
        desired_hash,
        live_policies,
        run.tenant.metadata.generation,
    )
}

fn policy_reconcile_action(
    previous: Option<&ProvisioningItemStatus>,
    live_hash: Option<&str>,
    desired_hash: &str,
) -> PolicyReconcileAction {
    let Some(live_hash) = live_hash else {
        return PolicyReconcileAction::Apply("RustFS policy was created");
    };

    match previous.and_then(|item| item.last_applied_hash.as_deref()) {
        None if live_hash == desired_hash => {
            PolicyReconcileAction::Ready("Existing RustFS policy matches spec and was adopted")
        }
        None => PolicyReconcileAction::Failed(
            Reason::PolicyConflict,
            "Live RustFS policy differs from spec and is not owned by this status",
        ),
        Some(last_applied_hash) if last_applied_hash == live_hash => {
            if live_hash == desired_hash {
                PolicyReconcileAction::Ready("RustFS policy already matches spec")
            } else {
                PolicyReconcileAction::Apply("RustFS policy was applied")
            }
        }
        Some(_) if live_hash == desired_hash => {
            PolicyReconcileAction::Ready("RustFS policy matches spec")
        }
        Some(_) => PolicyReconcileAction::Failed(
            Reason::PolicyConflict,
            "Live RustFS policy changed since the operator last applied it",
        ),
    }
}

fn finalize_policy_item_status(
    mut item: ProvisioningItemStatus,
    previous: Option<&ProvisioningItemStatus>,
    policy_name: &str,
    desired_hash: String,
    live_policies: &BTreeMap<String, String>,
    generation: Option<i64>,
) -> ProvisioningItemStatus {
    item.desired_hash = Some(desired_hash);
    if item.last_applied_hash.is_none() && item.state == ProvisioningItemState::Ready.as_str() {
        item.last_applied_hash = live_policies
            .get(policy_name)
            .map(|live_document| hash_document(live_document))
            .or_else(|| item.desired_hash.clone());
    }
    if item.last_applied_hash.is_none() {
        item.last_applied_hash = previous.and_then(|item| item.last_applied_hash.clone());
    }
    item.last_applied_generation = match (
        item.last_applied_hash.as_deref(),
        previous.and_then(|item| item.last_applied_hash.as_deref()),
    ) {
        (Some(current), Some(previous_hash)) if current == previous_hash => {
            previous.and_then(|item| item.last_applied_generation)
        }
        (Some(_), _) if item.state == ProvisioningItemState::Ready.as_str() => generation,
        _ => previous.and_then(|item| item.last_applied_generation),
    };
    item
}

async fn load_policy_document(
    run: &ProvisioningRun<'_>,
    policy: &ProvisioningPolicy,
) -> Result<PolicyDocument, (Reason, String)> {
    let reference = &policy.document.config_map_key_ref;
    let config_map: ConfigMap =
        run.ctx
            .get(&reference.name, run.namespace)
            .await
            .map_err(|error| {
                if context::is_kube_not_found(&error) {
                    (
                        Reason::PolicyDocumentConfigMapNotFound,
                        format!("policy ConfigMap '{}' was not found", reference.name),
                    )
                } else {
                    (
                        Reason::PolicyApplyFailed,
                        format!(
                            "failed to read policy ConfigMap '{}': {error}",
                            reference.name
                        ),
                    )
                }
            })?;

    let raw = config_map
        .data
        .as_ref()
        .and_then(|data| data.get(&reference.key))
        .ok_or_else(|| {
            (
                Reason::PolicyDocumentKeyNotFound,
                format!(
                    "policy ConfigMap '{}' is missing key '{}'",
                    reference.name, reference.key
                ),
            )
        })?;

    PolicyDocument::parse(raw).map_err(|message| (Reason::PolicyApplyFailed, message))
}

async fn apply_policy(
    client: &RustfsAdminClient,
    live_policies: &mut BTreeMap<String, String>,
    name: &str,
    document: &str,
) -> Result<String, String> {
    client
        .add_canned_policy(name, document)
        .await
        .map_err(|error| format!("failed to apply RustFS policy '{name}': {error}"))?;

    let live_document = client
        .get_canned_policy(name)
        .await
        .map_err(|error| format!("failed to read RustFS policy '{name}' after apply: {error}"))?;
    let live_document = normalize_policy_document(&live_document)?;
    let live_hash = hash_document(&live_document);
    live_policies.insert(name.to_string(), live_document);
    Ok(live_hash)
}

async fn reconcile_users(
    run: &mut ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &BTreeMap<String, String>,
    credentials_preflight: &UserCredentialsPreflight,
) -> Option<CheckpointRetry> {
    let failed_spec_policies = run
        .status
        .policies
        .iter()
        .filter(|item| item.state == ProvisioningItemState::Failed.as_str())
        .map(|item| item.name.clone())
        .collect::<BTreeSet<_>>();
    let mut plans = Vec::with_capacity(run.tenant.spec.users.len());

    for (user, preflight) in run
        .tenant
        .spec
        .users
        .iter()
        .zip(credentials_preflight.checks.iter())
    {
        let (policy_error, credentials) = match preflight {
            UserCredentialsCheck::DuplicateSecret => {
                let previous = run.previous_user(&user.name);
                let item = run.item(
                    previous,
                    &user.name,
                    ProvisioningItemState::Failed,
                    Reason::UserSecretInvalid,
                    format!(
                        "credentials Secret '{}' is referenced by multiple provisioning users",
                        user.credentials_secret_name()
                    ),
                );
                let item = annotate_user_item(item, user, previous, None);
                plans.push(UserReconcilePlan::Complete(Box::new(item)));
                continue;
            }
            UserCredentialsCheck::Checked {
                policy_error,
                credentials,
            } => (policy_error, credentials),
        };
        if let Ok(credentials) = credentials
            && credentials_preflight
                .duplicate_access_key_hashes
                .contains(&access_key_hash(&credentials.access_key))
        {
            let previous = run.previous_user(&user.name);
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserSecretInvalid,
                format!(
                    "credentials Secret '{}' resolves to an access key used by multiple provisioning users",
                    credentials.secret_name
                ),
            );
            let item = annotate_user_item(item, user, previous, None);
            plans.push(UserReconcilePlan::Complete(Box::new(item)));
            continue;
        }
        if let Some(message) = policy_error {
            let previous = run.previous_user(&user.name);
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserPolicyInvalid,
                message,
            );
            let item = annotate_user_item(item, user, previous, None);
            plans.push(UserReconcilePlan::Complete(Box::new(item)));
            continue;
        }
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(message) => {
                let previous = run.previous_user(&user.name);
                let item = run.item(
                    previous,
                    &user.name,
                    ProvisioningItemState::Failed,
                    Reason::UserSecretInvalid,
                    message,
                );
                let item = annotate_user_item(item, user, previous, None);
                plans.push(UserReconcilePlan::Complete(Box::new(item)));
                continue;
            }
        };

        plans.push(
            prepare_user_reconcile(
                run,
                client,
                live_policies,
                &failed_spec_policies,
                user,
                credentials,
            )
            .await,
        );
    }

    let checkpoints = plans
        .iter()
        .filter_map(|plan| match plan {
            UserReconcilePlan::Prepared(prepared) => prepared.checkpoint.clone(),
            UserReconcilePlan::Complete(_) => None,
        })
        .collect::<Vec<_>>();
    if !checkpoints.is_empty()
        && let Err(error) = persist_user_ownership_checkpoints(run, &checkpoints).await
    {
        match error {
            CheckpointError::Retry(retry) => return Some(retry),
            CheckpointError::Permanent { message } => {
                for plan in &mut plans {
                    let replacement = match plan {
                        UserReconcilePlan::Prepared(prepared) if prepared.checkpoint.is_some() => {
                            let previous = run.previous_user(&prepared.user.name);
                            let item = run.item(
                                previous,
                                &prepared.user.name,
                                ProvisioningItemState::Failed,
                                Reason::UserOwnershipCheckpointFailed,
                                message.clone(),
                            );
                            Some(UserReconcilePlan::Complete(Box::new(annotate_user_item(
                                item,
                                &prepared.user,
                                previous,
                                None,
                            ))))
                        }
                        UserReconcilePlan::Prepared(_) | UserReconcilePlan::Complete(_) => None,
                    };
                    if let Some(replacement) = replacement {
                        *plan = replacement;
                    }
                }
            }
        }
    }

    for plan in plans {
        let item = match plan {
            UserReconcilePlan::Complete(item) => *item,
            UserReconcilePlan::Prepared(prepared) => {
                execute_prepared_user(run, client, *prepared).await
            }
        };
        run.push_user(item);
    }
    None
}

async fn preflight_user_credentials(run: &ProvisioningRun<'_>) -> UserCredentialsPreflight {
    let duplicate_secret_names = duplicate_user_credentials_secret_names(&run.tenant.spec.users);
    let mut checks = Vec::with_capacity(run.tenant.spec.users.len());
    for user in &run.tenant.spec.users {
        if duplicate_secret_names.contains(user.credentials_secret_name()) {
            checks.push(UserCredentialsCheck::DuplicateSecret);
            continue;
        }
        checks.push(UserCredentialsCheck::Checked {
            policy_error: validate_user_policies(user).err(),
            credentials: load_user_secret(run, user).await,
        });
    }
    let duplicate_access_key_hashes = duplicate_user_access_key_hashes(&checks);
    UserCredentialsPreflight {
        checks,
        duplicate_access_key_hashes,
    }
}

fn duplicate_user_access_key_hashes(
    credentials_preflight: &[UserCredentialsCheck],
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    credentials_preflight
        .iter()
        .filter_map(|preflight| match preflight {
            UserCredentialsCheck::Checked {
                credentials: Ok(credentials),
                ..
            } => Some(credentials),
            UserCredentialsCheck::DuplicateSecret
            | UserCredentialsCheck::Checked {
                credentials: Err(_),
                ..
            } => None,
        })
        .filter_map(|credentials| {
            let hash = access_key_hash(&credentials.access_key);
            (!seen.insert(hash.clone())).then_some(hash)
        })
        .collect()
}

async fn prepare_user_reconcile(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &BTreeMap<String, String>,
    failed_spec_policies: &BTreeSet<String>,
    user: &ProvisioningUser,
    credentials: &UserCredentials,
) -> UserReconcilePlan {
    let previous = run.previous_user(&user.name);
    if user_access_key_changed(previous, credentials) {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::ImmutableFieldModified,
            "user access key is immutable after provisioning; create a new user entry to migrate it",
        );
        return UserReconcilePlan::Complete(Box::new(annotate_user_item(
            item, user, previous, None,
        )));
    }

    if let Some(policy_name) = user
        .policies
        .iter()
        .find(|policy_name| failed_spec_policies.contains(*policy_name))
    {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserPolicySetFailed,
            format!("referenced policy '{policy_name}' is not ready"),
        );
        return UserReconcilePlan::Complete(Box::new(annotate_user_item(
            item, user, previous, None,
        )));
    }

    if let Some(policy_name) = user
        .policies
        .iter()
        .find(|policy_name| !live_policies.contains_key(*policy_name))
    {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserPolicyNotFound,
            format!("referenced policy '{policy_name}' does not exist"),
        );
        return UserReconcilePlan::Complete(Box::new(annotate_user_item(
            item, user, previous, None,
        )));
    }

    let exists = match client.user_exists(&credentials.access_key).await {
        Ok(exists) => exists,
        Err(error) => {
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserSecretInvalid,
                format!("failed to query RustFS user: {error}"),
            );
            return UserReconcilePlan::Complete(Box::new(annotate_user_item(
                item, user, previous, None,
            )));
        }
    };

    let mut ownership = match matching_user_ownership(previous, run.tenant, user, credentials) {
        Ok(ownership) => ownership,
        Err(message) => {
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserOwnershipConflict,
                message,
            );
            return UserReconcilePlan::Complete(Box::new(annotate_user_item(
                item, user, previous, None,
            )));
        }
    };
    let mut checkpoint_update = None;

    if exists && ownership.is_none() {
        if legacy_user_status_can_migrate(previous, user, credentials) {
            let managed_ownership = match user_ownership(
                run.tenant,
                user,
                credentials,
                ProvisioningUserOwnershipState::Managed,
            ) {
                Ok(ownership) => ownership,
                Err(message) => {
                    let item = run.item(
                        previous,
                        &user.name,
                        ProvisioningItemState::Failed,
                        Reason::UserOwnershipCheckpointFailed,
                        message,
                    );
                    return UserReconcilePlan::Complete(Box::new(annotate_user_item(
                        item, user, previous, None,
                    )));
                }
            };
            let managed_checkpoint = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Ready,
                Reason::ProvisioningConfigured,
                "Legacy operator-managed RustFS user ownership was migrated",
            );
            // Preserve the legacy observed Secret version so a concurrently rotated Secret is
            // still applied after the ownership checkpoint has been persisted.
            let mut managed_checkpoint =
                annotate_user_item(managed_checkpoint, user, previous, None);
            managed_checkpoint.ownership = Some(managed_ownership.clone());
            checkpoint_update = Some(managed_checkpoint);
            ownership = Some(managed_ownership);
        } else {
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserOwnershipConflict,
                "RustFS user already exists without a matching operator ownership checkpoint; choose a different access key or remove the unmanaged user",
            );
            return UserReconcilePlan::Complete(Box::new(annotate_user_item(
                item, user, previous, None,
            )));
        }
    }

    if !exists && ownership.is_none() {
        let pending_ownership = match user_ownership(
            run.tenant,
            user,
            credentials,
            ProvisioningUserOwnershipState::PendingCreate,
        ) {
            Ok(ownership) => ownership,
            Err(message) => {
                let item = run.item(
                    previous,
                    &user.name,
                    ProvisioningItemState::Failed,
                    Reason::UserOwnershipCheckpointFailed,
                    message,
                );
                return UserReconcilePlan::Complete(Box::new(annotate_user_item(
                    item, user, previous, None,
                )));
            }
        };
        let pending_checkpoint = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Pending,
            Reason::ProvisioningPending,
            "Operator ownership checkpoint was persisted before creating the RustFS user",
        );
        let mut pending_checkpoint =
            annotate_user_item(pending_checkpoint, user, previous, Some(credentials));
        pending_checkpoint.ownership = Some(pending_ownership.clone());
        checkpoint_update = Some(pending_checkpoint);
        ownership = Some(pending_ownership);
    } else if !exists
        && ownership.as_ref().is_some_and(|ownership| {
            ownership.state == ProvisioningUserOwnershipState::PendingCreate
        })
    {
        // Refresh the persisted intent before retrying an external create after a process crash.
        // This recovery relies on per-Tenant controller serialization; it does not provide
        // exactly-once delivery across Kubernetes and independent RustFS actors.
        let pending_checkpoint = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Pending,
            Reason::ProvisioningPending,
            "Operator is resuming a pending RustFS user creation",
        );
        let mut pending_checkpoint =
            annotate_user_item(pending_checkpoint, user, previous, Some(credentials));
        pending_checkpoint.ownership = ownership.clone();
        checkpoint_update = Some(pending_checkpoint);
    }

    let Some(ownership) = ownership else {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserOwnershipCheckpointFailed,
            "Operator ownership checkpoint is required before synchronizing RustFS user credentials",
        );
        return UserReconcilePlan::Complete(Box::new(annotate_user_item(
            item, user, previous, None,
        )));
    };

    UserReconcilePlan::Prepared(Box::new(PreparedUserReconcile {
        user: user.clone(),
        credentials: credentials.clone(),
        exists,
        ownership,
        checkpoint: checkpoint_update,
    }))
}

async fn execute_prepared_user(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    prepared: PreparedUserReconcile,
) -> ProvisioningUserStatus {
    let PreparedUserReconcile {
        user,
        credentials,
        exists,
        mut ownership,
        ..
    } = prepared;
    let previous = run.previous_user(&user.name);

    let credentials_applied =
        match sync_user_credentials(client, previous, &credentials, exists).await {
            Ok(applied) => applied,
            Err(error) => {
                let item = run.item(
                    previous,
                    &user.name,
                    ProvisioningItemState::Failed,
                    Reason::UserSecretInvalid,
                    format!("failed to update RustFS user credentials: {error}"),
                );
                let mut item = annotate_user_item(item, &user, previous, None);
                item.ownership = Some(ownership);
                return item;
            }
        };

    ownership.state = ProvisioningUserOwnershipState::Managed;

    if let Err(error) = client
        .set_user_policy(&credentials.access_key, &user.policies)
        .await
    {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserPolicySetFailed,
            format!("failed to set RustFS user policy mapping: {error}"),
        );
        let mut item = annotate_user_item(item, &user, previous, Some(&credentials));
        item.ownership = Some(ownership);
        return item;
    }

    let message = if !exists {
        "RustFS user was created and direct policy mapping was applied"
    } else if credentials_applied {
        "RustFS user credentials were updated and direct policy mapping was applied"
    } else {
        "RustFS user already matched the observed Secret; direct policy mapping was applied"
    };
    let mut item = ProvisioningItemStatus::new(
        &user.name,
        ProvisioningItemState::Ready,
        Reason::ProvisioningConfigured.as_str(),
    );
    item.message = Some(message.to_string());
    item.last_transition_time = match previous {
        Some(previous)
            if previous.state == item.state
                && previous.reason == item.reason
                && previous.message.as_deref() == item.message.as_deref() =>
        {
            previous.last_transition_time.clone()
        }
        _ => Some(run.now.clone()),
    };
    let mut item = annotate_user_item(item, &user, previous, Some(&credentials));
    item.ownership = Some(ownership);
    item
}

#[cfg(test)]
async fn reconcile_user(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &BTreeMap<String, String>,
    failed_spec_policies: &BTreeSet<String>,
    user: &ProvisioningUser,
    credentials: &UserCredentials,
) -> ProvisioningUserStatus {
    match prepare_user_reconcile(
        run,
        client,
        live_policies,
        failed_spec_policies,
        user,
        credentials,
    )
    .await
    {
        UserReconcilePlan::Complete(item) => *item,
        UserReconcilePlan::Prepared(prepared) => {
            if let Some(checkpoint) = prepared.checkpoint.as_ref()
                && let Err(error) =
                    persist_user_ownership_checkpoints(run, std::slice::from_ref(checkpoint)).await
            {
                let previous = run.previous_user(&prepared.user.name);
                let item = run.item(
                    previous,
                    &prepared.user.name,
                    ProvisioningItemState::Failed,
                    Reason::UserOwnershipCheckpointFailed,
                    checkpoint_error_message(error),
                );
                return annotate_user_item(item, &prepared.user, previous, None);
            }
            execute_prepared_user(run, client, *prepared).await
        }
    }
}

fn matching_user_ownership(
    previous: Option<&ProvisioningUserStatus>,
    tenant: &Tenant,
    user: &ProvisioningUser,
    credentials: &UserCredentials,
) -> Result<Option<ProvisioningUserOwnershipStatus>, &'static str> {
    let Some(ownership) = previous.and_then(|item| item.ownership.as_ref()) else {
        return Ok(None);
    };
    let Some(tenant_uid) = tenant.metadata.uid.as_deref() else {
        return Err(
            "Tenant UID is unavailable, so the operator cannot verify RustFS user ownership",
        );
    };
    let current_access_key_hash = access_key_hash(&credentials.access_key);
    if ownership.tenant_uid != tenant_uid
        || ownership.user_name != user.name
        || ownership.access_key_hash != current_access_key_hash
    {
        return Err(
            "RustFS user ownership checkpoint does not match this Tenant UID, provisioning user, or access key",
        );
    }

    Ok(Some(ownership.clone()))
}

fn legacy_user_status_can_migrate(
    previous: Option<&ProvisioningUserStatus>,
    user: &ProvisioningUser,
    credentials: &UserCredentials,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let current_access_key_hash = access_key_hash(&credentials.access_key);
    matches!(
        previous.state.as_str(),
        state if state == ProvisioningItemState::Ready.as_str()
            || state == ProvisioningItemState::Retained.as_str()
    ) && previous.last_applied_access_key_hash.as_deref() == Some(current_access_key_hash.as_str())
        && previous.observed_secret_name.as_deref() == Some(user.credentials_secret_name())
}

fn user_ownership(
    tenant: &Tenant,
    user: &ProvisioningUser,
    credentials: &UserCredentials,
    state: ProvisioningUserOwnershipState,
) -> Result<ProvisioningUserOwnershipStatus, &'static str> {
    let Some(tenant_uid) = tenant.metadata.uid.as_deref() else {
        return Err(
            "Tenant UID is unavailable, so the operator cannot persist a RustFS user ownership checkpoint",
        );
    };
    Ok(ProvisioningUserOwnershipStatus {
        state,
        tenant_uid: tenant_uid.to_string(),
        user_name: user.name.clone(),
        access_key_hash: access_key_hash(&credentials.access_key),
    })
}

async fn persist_user_ownership_checkpoints(
    run: &ProvisioningRun<'_>,
    checkpoints: &[ProvisioningUserStatus],
) -> Result<(), CheckpointError> {
    if checkpoints.is_empty() {
        return Ok(());
    }
    let api: Api<Tenant> = Api::namespaced(run.ctx.client.clone(), run.namespace);
    let latest = api
        .get(&run.tenant.name())
        .await
        .map_err(classify_checkpoint_kube_error)?;
    if latest.metadata.uid != run.tenant.metadata.uid
        || latest.metadata.generation != run.tenant.metadata.generation
    {
        return Err(CheckpointError::Retry(CheckpointRetry {
            message: "Tenant identity or generation changed before persisting the RustFS user ownership checkpoint"
                .to_string(),
            retry_after: CHECKPOINT_CONFLICT_RETRY,
        }));
    }
    for checkpoint in checkpoints {
        let previous_user = run
            .previous
            .users
            .iter()
            .find(|item| item.name == checkpoint.name);
        let latest_user = latest.status.as_ref().and_then(|status| {
            status
                .provisioning
                .users
                .iter()
                .find(|item| item.name == checkpoint.name)
        });
        if latest_user != previous_user {
            return Err(CheckpointError::Retry(CheckpointRetry {
                message: format!(
                    "Tenant user '{}' provisioning status changed before persisting the RustFS user ownership checkpoint",
                    checkpoint.name
                ),
                retry_after: CHECKPOINT_CONFLICT_RETRY,
            }));
        }
    }
    let Some(resource_version) = latest.metadata.resource_version.clone() else {
        return Err(CheckpointError::Permanent {
            message: "Tenant resourceVersion is unavailable, so the operator cannot safely persist the RustFS user ownership checkpoint"
                .to_string(),
        });
    };

    let mut provisioning = latest
        .status
        .as_ref()
        .map(|status| status.provisioning.clone())
        .unwrap_or_default();
    merge_provisioning_items(&mut provisioning.policies, &run.status.policies);
    merge_provisioning_user_items(&mut provisioning.users, &run.status.users);
    merge_provisioning_items(&mut provisioning.buckets, &run.status.buckets);
    merge_provisioning_user_items(&mut provisioning.users, checkpoints);
    provisioning.observed_generation = run.tenant.metadata.generation;
    provisioning.phase = Some(ProvisioningPhase::Pending);
    provisioning
        .policies
        .sort_by(|left, right| left.name.cmp(&right.name));
    provisioning
        .users
        .sort_by(|left, right| left.name.cmp(&right.name));
    provisioning
        .buckets
        .sort_by(|left, right| left.name.cmp(&right.name));

    let mut status = latest.status.unwrap_or_default();
    status.provisioning = provisioning;
    let status_patch = serde_json::json!({
        "metadata": { "resourceVersion": resource_version },
        "status": status,
    });
    let updated = api
        .patch_status(
            &run.tenant.name(),
            &PatchParams::default(),
            &Patch::Merge(&status_patch),
        )
        .await
        .map_err(classify_checkpoint_kube_error)?;
    if updated.metadata.resource_version.is_none() {
        return Err(CheckpointError::Retry(CheckpointRetry {
            message: "Kubernetes accepted the RustFS user ownership checkpoint but omitted resourceVersion; retrying from fresh state"
                .to_string(),
            retry_after: CHECKPOINT_TRANSIENT_RETRY,
        }));
    }
    for checkpoint in checkpoints {
        let persisted_checkpoint = updated.status.as_ref().and_then(|status| {
            status
                .provisioning
                .users
                .iter()
                .find(|item| item.name == checkpoint.name)
        });
        if persisted_checkpoint.is_none_or(|persisted| {
            persisted.state != checkpoint.state || persisted.ownership != checkpoint.ownership
        }) {
            return Err(CheckpointError::Permanent {
                message: format!(
                    "Kubernetes accepted the RustFS user ownership checkpoint request but did not persist the expected state and ownership proof for user '{}'; ensure the Tenant CRD is upgraded before the Operator",
                    checkpoint.name
                ),
            });
        }
    }
    Ok(())
}

fn classify_checkpoint_kube_error(error: kube::Error) -> CheckpointError {
    match error {
        kube::Error::Api(response) if response.code == 409 => {
            CheckpointError::Retry(CheckpointRetry {
                message: "Tenant status changed while persisting the RustFS user ownership checkpoint"
                    .to_string(),
                retry_after: CHECKPOINT_CONFLICT_RETRY,
            })
        }
        kube::Error::Api(response)
            if response.code == 408 || response.code == 429 || response.code >= 500 =>
        {
            CheckpointError::Retry(CheckpointRetry {
                message: format!(
                    "Kubernetes temporarily rejected the RustFS user ownership checkpoint ({} {})",
                    response.code, response.reason
                ),
                retry_after: CHECKPOINT_TRANSIENT_RETRY,
            })
        }
        kube::Error::Api(response) if (400..500).contains(&response.code) => {
            CheckpointError::Permanent {
                message: format!(
                    "Kubernetes rejected the RustFS user ownership checkpoint ({} {})",
                    response.code, response.reason
                ),
            }
        }
        _ => CheckpointError::Retry(CheckpointRetry {
            message: "The Kubernetes result for the RustFS user ownership checkpoint is uncertain; retrying from fresh state"
                .to_string(),
            retry_after: CHECKPOINT_TRANSIENT_RETRY,
        }),
    }
}

#[cfg(test)]
fn checkpoint_error_message(error: CheckpointError) -> String {
    match error {
        CheckpointError::Permanent { message } => message,
        CheckpointError::Retry(retry) => retry.message,
    }
}

fn merge_provisioning_items(
    destination: &mut Vec<ProvisioningItemStatus>,
    updates: &[ProvisioningItemStatus],
) {
    for update in updates {
        if let Some(item) = destination.iter_mut().find(|item| item.name == update.name) {
            *item = update.clone();
        } else {
            destination.push(update.clone());
        }
    }
}

fn merge_provisioning_user_items(
    destination: &mut Vec<ProvisioningUserStatus>,
    updates: &[ProvisioningUserStatus],
) {
    for update in updates {
        if let Some(item) = destination.iter_mut().find(|item| item.name == update.name) {
            *item = update.clone();
        } else {
            destination.push(update.clone());
        }
    }
}

fn annotate_user_item(
    mut item: ProvisioningItemStatus,
    user: &ProvisioningUser,
    previous: Option<&ProvisioningUserStatus>,
    applied_credentials: Option<&UserCredentials>,
) -> ProvisioningUserStatus {
    match applied_credentials {
        Some(credentials) => {
            item.observed_secret_resource_version = credentials.resource_version.clone();
            item.observed_secret_name = Some(credentials.secret_name.clone());
            item.last_applied_access_key_hash = Some(access_key_hash(&credentials.access_key));
        }
        None => {
            item.observed_secret_resource_version =
                previous.and_then(|item| item.observed_secret_resource_version.clone());
            item.observed_secret_name = previous.and_then(|item| item.observed_secret_name.clone());
            item.last_applied_access_key_hash =
                previous.and_then(|item| item.last_applied_access_key_hash.clone());
        }
    }
    item.policies = user.policies.clone();
    let mut item = ProvisioningUserStatus::new(item);
    item.ownership = previous.and_then(|item| item.ownership.clone());
    item
}

fn user_access_key_changed(
    previous: Option<&ProvisioningUserStatus>,
    credentials: &UserCredentials,
) -> bool {
    let current_hash = access_key_hash(&credentials.access_key);
    previous
        .and_then(|item| item.last_applied_access_key_hash.as_deref())
        .is_some_and(|previous_hash| previous_hash != current_hash)
}

async fn sync_user_credentials(
    client: &RustfsAdminClient,
    previous: Option<&ProvisioningUserStatus>,
    credentials: &UserCredentials,
    exists: bool,
) -> Result<bool, RustfsClientError> {
    if !user_credentials_need_apply(previous, credentials, exists) {
        return Ok(false);
    }

    client
        .add_user(&credentials.access_key, &credentials.secret_key)
        .await?;
    Ok(true)
}

fn user_credentials_need_apply(
    previous: Option<&ProvisioningUserStatus>,
    credentials: &UserCredentials,
    exists: bool,
) -> bool {
    if !exists {
        return true;
    }

    let Some(resource_version) = credentials.resource_version.as_deref() else {
        return true;
    };
    let Some(previous) = previous else {
        return true;
    };
    let current_hash = access_key_hash(&credentials.access_key);

    previous.observed_secret_resource_version.as_deref() != Some(resource_version)
        || previous.observed_secret_name.as_deref() != Some(credentials.secret_name.as_str())
        || previous.last_applied_access_key_hash.as_deref() != Some(current_hash.as_str())
}

fn access_key_hash(access_key: &str) -> String {
    hash_document(access_key)
}

async fn load_user_secret(
    run: &ProvisioningRun<'_>,
    user: &ProvisioningUser,
) -> Result<UserCredentials, String> {
    let secret_name = user.credentials_secret_name();
    let secret: Secret = run
        .ctx
        .get(secret_name, run.namespace)
        .await
        .map_err(|error| {
            if context::is_kube_not_found(&error) {
                format!("user Secret '{secret_name}' was not found")
            } else {
                format!("failed to read user Secret '{secret_name}': {error}")
            }
        })?;
    let data = secret
        .data
        .as_ref()
        .ok_or_else(|| format!("user Secret '{secret_name}' has no data"))?;

    let access_key = read_compatible_secret_value(
        data,
        "accesskey",
        "CONSOLE_ACCESS_KEY",
        secret_name,
        "access key",
    )?;
    let secret_key = read_compatible_secret_value(
        data,
        "secretkey",
        "CONSOLE_SECRET_KEY",
        secret_name,
        "secret key",
    )?;

    validate_user_access_key(&access_key)?;
    validate_user_secret_key(&secret_key)?;

    Ok(UserCredentials {
        access_key,
        secret_key,
        secret_name: secret_name.to_string(),
        resource_version: secret.metadata.resource_version,
    })
}

fn read_compatible_secret_value(
    data: &BTreeMap<String, ByteString>,
    native_key: &'static str,
    minio_key: &'static str,
    secret_name: &str,
    label: &str,
) -> Result<String, String> {
    let native = read_optional_secret_value(data, native_key, secret_name)?;
    let minio = read_optional_secret_value(data, minio_key, secret_name)?;

    match (native, minio) {
        (Some(native), Some(minio)) if native == minio => Ok(native),
        (Some(_), Some(_)) => Err(format!(
            "user Secret '{secret_name}' has conflicting {label} values"
        )),
        (Some(value), None) | (None, Some(value)) => Ok(value),
        (None, None) => Err(format!(
            "user Secret '{secret_name}' is missing '{native_key}' or '{minio_key}'"
        )),
    }
}

fn read_optional_secret_value(
    data: &BTreeMap<String, ByteString>,
    key: &'static str,
    secret_name: &str,
) -> Result<Option<String>, String> {
    let Some(raw) = data.get(key) else {
        return Ok(None);
    };
    let value = String::from_utf8(raw.0.clone())
        .map_err(|_| format!("user Secret '{secret_name}' key '{key}' must be valid UTF-8"))?;
    Ok(Some(value.trim().to_string()))
}

fn validate_user_access_key(access_key: &str) -> Result<(), String> {
    if access_key.len() < 8 {
        return Err("user access key must be at least 8 characters".to_string());
    }
    if access_key.chars().any(char::is_whitespace) {
        return Err("user access key must not contain whitespace".to_string());
    }
    if access_key.contains('=') || access_key.contains(',') {
        return Err("user access key must not contain reserved characters '=' or ','".to_string());
    }
    Ok(())
}

fn validate_user_policies(user: &ProvisioningUser) -> Result<(), String> {
    if user.policies.is_empty() {
        return Err("user must reference at least one policy".to_string());
    }
    Ok(())
}

fn validate_user_secret_key(secret_key: &str) -> Result<(), String> {
    if secret_key.len() < 8 {
        return Err("user secret key must be at least 8 characters".to_string());
    }
    Ok(())
}

async fn reconcile_buckets(run: &mut ProvisioningRun<'_>, client: &RustfsAdminClient) {
    for bucket in &run.tenant.spec.buckets {
        let item = reconcile_bucket(run, client, bucket).await;
        run.push_bucket(item);
    }
}

async fn reconcile_bucket(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    bucket: &ProvisioningBucket,
) -> ProvisioningItemStatus {
    let previous = run.previous_bucket(&bucket.name);
    if let Err(message) = validate_bucket_name(&bucket.name) {
        let item = run.item(
            previous,
            &bucket.name,
            ProvisioningItemState::Failed,
            Reason::BucketCreateFailed,
            message,
        );
        return annotate_bucket_item(item, bucket);
    }

    let create_result = match client
        .create_bucket(
            &bucket.name,
            bucket.region.as_deref(),
            bucket.object_lock_enabled(),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let item = run.item(
                previous,
                &bucket.name,
                ProvisioningItemState::Failed,
                Reason::BucketCreateFailed,
                format!("failed to create RustFS bucket: {error}"),
            );
            return annotate_bucket_item(item, bucket);
        }
    };

    if bucket.object_lock_enabled() {
        match client.bucket_object_lock_enabled(&bucket.name).await {
            Ok(true) => {
                let message = match create_result {
                    CreateBucketResult::Created => {
                        "RustFS bucket was created with object lock enabled"
                    }
                    CreateBucketResult::AlreadyExists => {
                        "Bucket already existed with object lock enabled"
                    }
                };
                let item = run.item(
                    previous,
                    &bucket.name,
                    ProvisioningItemState::Ready,
                    Reason::ProvisioningConfigured,
                    message,
                );
                return annotate_bucket_item(item, bucket);
            }
            Ok(false) => {
                let message = match create_result {
                    CreateBucketResult::Created => {
                        "Bucket was created but object lock is not enabled"
                    }
                    CreateBucketResult::AlreadyExists => {
                        "Bucket already exists but object lock is not enabled"
                    }
                };
                let item = run.item(
                    previous,
                    &bucket.name,
                    ProvisioningItemState::Failed,
                    Reason::BucketObjectLockConflict,
                    message,
                );
                return annotate_bucket_item(item, bucket);
            }
            Err(error) => {
                let message = match create_result {
                    CreateBucketResult::Created => {
                        format!("failed to verify created bucket object lock: {error}")
                    }
                    CreateBucketResult::AlreadyExists => {
                        format!("failed to verify existing bucket object lock: {error}")
                    }
                };
                let item = run.item(
                    previous,
                    &bucket.name,
                    ProvisioningItemState::Failed,
                    Reason::BucketObjectLockConflict,
                    message,
                );
                return annotate_bucket_item(item, bucket);
            }
        }
    }

    let message = match create_result {
        CreateBucketResult::Created => "RustFS bucket was created",
        CreateBucketResult::AlreadyExists => "RustFS bucket already exists",
    };
    let item = run.item(
        previous,
        &bucket.name,
        ProvisioningItemState::Ready,
        Reason::ProvisioningConfigured,
        message,
    );
    annotate_bucket_item(item, bucket)
}

fn annotate_bucket_item(
    mut item: ProvisioningItemStatus,
    bucket: &ProvisioningBucket,
) -> ProvisioningItemStatus {
    item.region = bucket.region.clone();
    item.object_lock = Some(bucket.object_lock_enabled());
    item
}

fn has_active_spec(tenant: &Tenant) -> bool {
    !tenant.spec.policies.is_empty()
        || !tenant.spec.users.is_empty()
        || !tenant.spec.buckets.is_empty()
}

fn desired_names<'a>(names: impl Iterator<Item = &'a String>) -> BTreeSet<String> {
    names.cloned().collect()
}

fn validate_bucket_name(bucket_name: &str) -> Result<(), String> {
    if bucket_name.trim() != bucket_name {
        return Err("bucket name must not contain leading or trailing whitespace".to_string());
    }
    if bucket_name.is_empty() {
        return Err("bucket name cannot be empty".to_string());
    }
    if bucket_name.len() < 3 {
        return Err("bucket name cannot be shorter than 3 characters".to_string());
    }
    if bucket_name.len() > 63 {
        return Err("bucket name cannot be longer than 63 characters".to_string());
    }
    if bucket_name == "rustfs" {
        return Err("bucket name cannot be rustfs".to_string());
    }
    if is_ipv4_address_like(bucket_name) {
        return Err("bucket name cannot be an IP address".to_string());
    }
    if bucket_name.contains("..") || bucket_name.contains(".-") || bucket_name.contains("-.") {
        return Err("bucket name contains invalid dot or hyphen sequence".to_string());
    }
    let mut chars = bucket_name.chars();
    let Some(first) = chars.next() else {
        return Err("bucket name cannot be empty".to_string());
    };
    let Some(last) = bucket_name.chars().next_back() else {
        return Err("bucket name cannot be empty".to_string());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("bucket name must start with a lowercase letter or digit".to_string());
    }
    if !last.is_ascii_lowercase() && !last.is_ascii_digit() {
        return Err("bucket name must end with a lowercase letter or digit".to_string());
    }
    if !bucket_name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-')
    {
        return Err(
            "bucket name must contain only lowercase letters, digits, dots, or hyphens".to_string(),
        );
    }
    Ok(())
}

fn is_ipv4_address_like(value: &str) -> bool {
    let mut parts = value.split('.');
    (0..4).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    }) && parts.next().is_none()
}

fn normalize_policy_document(document: &str) -> Result<String, String> {
    let value = serde_json::from_str::<Value>(document)
        .map_err(|error| format!("policy document must be valid JSON: {error}"))?;
    let normalized = normalize_policy_value(value);
    serde_json::to_string(&normalized)
        .map_err(|error| format!("failed to normalize policy document: {error}"))
}

fn normalize_policy_value(value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            if object
                .get("ID")
                .and_then(Value::as_str)
                .is_some_and(|id| id.is_empty())
            {
                object.remove("ID");
            }

            match object.get("Statement").cloned() {
                Some(Value::Array(statements)) => {
                    let mut normalized_statements = statements
                        .iter()
                        .map(normalize_policy_statement)
                        .collect::<Vec<_>>();
                    normalized_statements.sort_by_key(statement_sort_key);
                    object.insert("Statement".to_string(), Value::Array(normalized_statements));
                }
                Some(statement) => {
                    object.insert(
                        "Statement".to_string(),
                        normalize_policy_statement(&statement),
                    );
                }
                None => {}
            }

            Value::Object(object)
        }
        value => value,
    }
}

fn normalize_policy_statement(statement: &Value) -> Value {
    match statement {
        Value::Object(object) => {
            let mut normalized = object.clone();
            for key in ["Action", "NotAction", "Resource", "NotResource"] {
                if let Some(value) = normalized.get(key).cloned() {
                    normalized.insert(key.to_string(), normalize_string_or_string_array(&value));
                }
            }
            if normalized
                .get("Sid")
                .and_then(Value::as_str)
                .is_some_and(|sid| sid.is_empty())
            {
                normalized.remove("Sid");
            }
            if normalized
                .get("Condition")
                .is_some_and(is_empty_json_object)
            {
                normalized.remove("Condition");
            }
            Value::Object(normalized)
        }
        statement => statement.clone(),
    }
}

fn normalize_string_or_string_array(value: &Value) -> Value {
    match value {
        Value::String(action) => Value::String(action.clone()),
        Value::Array(items) => {
            let mut normalized = items.clone();
            normalized.sort_by(|left, right| {
                left.as_str()
                    .unwrap_or_default()
                    .cmp(right.as_str().unwrap_or_default())
            });
            Value::Array(normalized)
        }
        _ => value.clone(),
    }
}

fn is_empty_json_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.is_empty())
}

fn statement_sort_key(statement: &Value) -> String {
    normalize_policy_statement(statement).to_string()
}

fn hash_document(document: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(document.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn item_message(item: &ProvisioningItemStatus) -> String {
    item.message
        .clone()
        .unwrap_or_else(|| "Tenant provisioning failed".to_string())
}

fn reason_from_str(reason: &str) -> Reason {
    match reason {
        "ProvisioningUnsupported" => Reason::ProvisioningUnsupported,
        "ImmutableFieldModified" => Reason::ImmutableFieldModified,
        "PolicyDocumentConfigMapNotFound" => Reason::PolicyDocumentConfigMapNotFound,
        "PolicyDocumentKeyNotFound" => Reason::PolicyDocumentKeyNotFound,
        "PolicyApplyFailed" => Reason::PolicyApplyFailed,
        "PolicyConflict" => Reason::PolicyConflict,
        "UserSecretInvalid" => Reason::UserSecretInvalid,
        "UserPolicyNotFound" => Reason::UserPolicyNotFound,
        "UserPolicyInvalid" => Reason::UserPolicyInvalid,
        "UserPolicySetFailed" => Reason::UserPolicySetFailed,
        "UserOwnershipConflict" => Reason::UserOwnershipConflict,
        "UserOwnershipCheckpointFailed" => Reason::UserOwnershipCheckpointFailed,
        "BucketCreateFailed" => Reason::BucketCreateFailed,
        "BucketObjectLockConflict" => Reason::BucketObjectLockConflict,
        _ => Reason::ProvisioningFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        routing::{any, get, put},
    };
    use http_body_util::BodyExt;
    use k8s_openapi::ByteString;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::{Client, client::Body as KubeBody};
    use std::convert::Infallible;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Mutex;
    use tower::service_fn;

    #[derive(Clone, Default)]
    struct PolicyApplyCapture {
        body: Arc<Mutex<String>>,
    }

    #[derive(Clone, Default)]
    struct UserCredentialCapture {
        body: Arc<Mutex<String>>,
    }

    #[test]
    fn compatible_secret_values_are_trimmed_and_must_match() {
        let data = BTreeMap::from([
            ("accesskey".to_string(), ByteString(b" app ".to_vec())),
            (
                "CONSOLE_ACCESS_KEY".to_string(),
                ByteString(b"app".to_vec()),
            ),
        ]);

        let value =
            read_compatible_secret_value(&data, "accesskey", "CONSOLE_ACCESS_KEY", "user", "ak")
                .expect("trimmed values should match");

        assert_eq!(value, "app");
    }

    #[test]
    fn access_key_rejects_reserved_characters() {
        let error = validate_user_access_key("app=user")
            .expect_err("reserved characters should be rejected");

        assert!(error.contains("reserved characters"));
    }

    #[test]
    fn access_key_requires_security_baseline_length() {
        let error =
            validate_user_access_key("app").expect_err("short access keys should be rejected");

        assert!(error.contains("at least 8 characters"));
    }

    #[tokio::test]
    async fn duplicate_access_keys_fail_before_any_rustfs_user_request() {
        let kube_service = service_fn(move |request: http::Request<KubeBody>| async move {
            let secret_name = request
                .uri()
                .path()
                .rsplit('/')
                .next()
                .expect("Secret request should include a name");
            let secret_key = match secret_name {
                "user-secret-a" => b"secret-value-a".to_vec(),
                "user-secret-b" => b"secret-value-b".to_vec(),
                _ => panic!("unexpected Secret request: {secret_name}"),
            };
            let secret = Secret {
                metadata: ObjectMeta {
                    name: Some(secret_name.to_string()),
                    namespace: Some("storage".to_string()),
                    resource_version: Some("1".to_string()),
                    ..Default::default()
                },
                data: Some(BTreeMap::from([
                    (
                        "accesskey".to_string(),
                        ByteString(b"shareduser01".to_vec()),
                    ),
                    ("secretkey".to_string(), ByteString(secret_key)),
                ])),
                ..Default::default()
            };
            Ok::<_, Infallible>(
                http::Response::builder()
                    .body(KubeBody::from(
                        serde_json::to_vec(&secret).expect("Secret should serialize"),
                    ))
                    .expect("response should build"),
            )
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let mut colliding_with_invalid_policy =
            provisioning_user("logical-b", "user-secret-b", "readonly");
        colliding_with_invalid_policy.policies.clear();
        let tenant = Tenant {
            metadata: ObjectMeta {
                name: Some("tenant-a".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            spec: crate::types::v1alpha1::tenant::TenantSpec {
                users: vec![
                    provisioning_user("logical-a", "user-secret-a", "readwrite"),
                    colliding_with_invalid_policy,
                ],
                ..Default::default()
            },
            status: None,
        };
        let mut run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-07-18T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let request_count = Arc::new(AtomicUsize::new(0));
        let route_count = request_count.clone();
        let router = Router::new().fallback(any(move || {
            let route_count = route_count.clone();
            async move {
                route_count.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let live_policies = BTreeMap::from([("readwrite".to_string(), "{}".to_string())]);

        let credentials_preflight = preflight_user_credentials(&run).await;
        reconcile_users(&mut run, &client, &live_policies, &credentials_preflight).await;

        assert_eq!(request_count.load(Ordering::SeqCst), 0);
        assert_eq!(run.status.users.len(), 2);
        assert!(run.status.users.iter().all(|item| {
            item.state == ProvisioningItemState::Failed.as_str()
                && item.reason == Reason::UserSecretInvalid.as_str()
                && item
                    .message
                    .as_deref()
                    .is_some_and(|message| !message.contains("shareduser01"))
        }));
        server.abort();
    }

    #[tokio::test]
    async fn user_preflight_loads_credentials_without_changing_policy_error_priority() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let service_count = request_count.clone();
        let kube_service = service_fn(move |_request: http::Request<KubeBody>| {
            let service_count = service_count.clone();
            async move {
                service_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(KubeBody::empty())
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let tenant = Tenant {
            metadata: ObjectMeta {
                name: Some("tenant-a".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            spec: crate::types::v1alpha1::tenant::TenantSpec {
                users: vec![ProvisioningUser {
                    name: "invalid-user".to_string(),
                    creds_secret: Some(
                        crate::types::v1alpha1::provisioning::UserCredentialsSecretRef {
                            name: "missing-secret".to_string(),
                        },
                    ),
                    policies: Vec::new(),
                    deletion_policy: Default::default(),
                }],
                ..Default::default()
            },
            status: None,
        };
        let mut run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-07-18T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let client = RustfsAdminClient::new_with_base_url(
            "http://127.0.0.1:1".to_string(),
            "access",
            "secret",
        );

        let credentials_preflight = preflight_user_credentials(&run).await;
        reconcile_users(&mut run, &client, &BTreeMap::new(), &credentials_preflight).await;

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(run.status.users.len(), 1);
        assert_eq!(
            run.status.users[0].reason,
            Reason::UserPolicyInvalid.as_str()
        );
        assert!(
            run.status.users[0]
                .message
                .as_deref()
                .is_some_and(|message| message.contains("at least one policy"))
        );
    }

    #[test]
    fn user_policy_list_must_not_be_empty() {
        let user = ProvisioningUser {
            name: "app-user".to_string(),
            creds_secret: None,
            policies: Vec::new(),
            deletion_policy: Default::default(),
        };

        let error =
            validate_user_policies(&user).expect_err("empty policy list should be rejected");

        assert!(error.contains("at least one policy"));
    }

    fn provisioning_user(name: &str, secret_name: &str, policy: &str) -> ProvisioningUser {
        ProvisioningUser {
            name: name.to_string(),
            creds_secret: Some(
                crate::types::v1alpha1::provisioning::UserCredentialsSecretRef {
                    name: secret_name.to_string(),
                },
            ),
            policies: vec![policy.to_string()],
            deletion_policy: Default::default(),
        }
    }

    fn provisioning_test_tenant(
        user: ProvisioningUser,
        provisioning: ProvisioningStatus,
    ) -> Tenant {
        Tenant {
            metadata: ObjectMeta {
                name: Some("tenant-a".to_string()),
                namespace: Some("storage".to_string()),
                uid: Some("tenant-uid-a".to_string()),
                resource_version: Some("17".to_string()),
                generation: Some(3),
                ..Default::default()
            },
            spec: crate::types::v1alpha1::tenant::TenantSpec {
                users: vec![user],
                ..Default::default()
            },
            status: Some(crate::types::v1alpha1::status::Status {
                provisioning,
                ..Default::default()
            }),
        }
    }

    fn user_credentials(resource_version: &str) -> UserCredentials {
        UserCredentials {
            access_key: "appuser01".to_string(),
            secret_key: "super-secret-value".to_string(),
            secret_name: "app-user-secret".to_string(),
            resource_version: Some(resource_version.to_string()),
        }
    }

    fn owned_user_status(
        state: ProvisioningUserOwnershipState,
        secret_resource_version: &str,
    ) -> ProvisioningUserStatus {
        let item = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        let mut item = ProvisioningUserStatus::new(item);
        item.observed_secret_resource_version = Some(secret_resource_version.to_string());
        item.observed_secret_name = Some("app-user-secret".to_string());
        item.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        item.ownership = Some(ProvisioningUserOwnershipStatus {
            state,
            tenant_uid: "tenant-uid-a".to_string(),
            user_name: "app-user".to_string(),
            access_key_hash: access_key_hash("appuser01"),
        });
        item
    }

    #[test]
    fn legacy_user_migration_requires_complete_matching_ready_or_retained_status() {
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let credentials = user_credentials("5");
        let mut previous = owned_user_status(ProvisioningUserOwnershipState::Managed, "4");
        previous.ownership = None;

        assert!(legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));
        previous.state = ProvisioningItemState::Retained.as_str().to_string();
        assert!(legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));

        previous.state = ProvisioningItemState::Failed.as_str().to_string();
        assert!(!legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));
        previous.state = ProvisioningItemState::Ready.as_str().to_string();
        previous.last_applied_access_key_hash = None;
        assert!(!legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));
        previous.last_applied_access_key_hash = Some(access_key_hash("different-user"));
        assert!(!legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        previous.observed_secret_name = Some("different-secret".to_string());
        assert!(!legacy_user_status_can_migrate(
            Some(&previous),
            &user,
            &credentials
        ));
    }

    #[tokio::test]
    async fn stale_reconciler_retries_without_returning_overwriting_status() {
        let requests = Arc::new(AtomicUsize::new(0));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let tenant = provisioning_test_tenant(user, ProvisioningStatus::default());
        let mut checkpoint = owned_user_status(ProvisioningUserOwnershipState::PendingCreate, "5");
        checkpoint.state = ProvisioningItemState::Pending.as_str().to_string();
        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        let mut persisted_tenant = latest_tenant.clone();
        persisted_tenant.metadata.resource_version = Some("19".to_string());
        persisted_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![checkpoint.clone()];
        let service_requests = requests.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let service_requests = service_requests.clone();
            let latest_tenant = latest_tenant.clone();
            let persisted_tenant = persisted_tenant.clone();
            async move {
                let attempt = service_requests.fetch_add(1, Ordering::SeqCst);
                let response = match attempt {
                    0 => {
                        assert_eq!(request.method(), http::Method::GET);
                        http::Response::builder()
                            .header("content-type", "application/json")
                            .body(KubeBody::from(
                                serde_json::to_vec(&latest_tenant)
                                    .expect("Tenant response should serialize"),
                            ))
                    }
                    1 => {
                        assert_eq!(request.method(), http::Method::PATCH);
                        http::Response::builder()
                            .header("content-type", "application/json")
                            .body(KubeBody::from(
                                serde_json::to_vec(&persisted_tenant)
                                    .expect("Tenant response should serialize"),
                            ))
                    }
                    2 => {
                        assert_eq!(request.method(), http::Method::GET);
                        http::Response::builder()
                            .header("content-type", "application/json")
                            .body(KubeBody::from(
                                serde_json::to_vec(&persisted_tenant)
                                    .expect("Tenant response should serialize"),
                            ))
                    }
                    _ => panic!("unexpected Kubernetes request {attempt}"),
                };
                Ok::<_, Infallible>(response.expect("response should build"))
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let make_run = || ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };
        let winner = make_run();
        let loser = make_run();

        persist_user_ownership_checkpoints(&winner, std::slice::from_ref(&checkpoint))
            .await
            .expect("first reconciler should persist its checkpoint");
        let error = persist_user_ownership_checkpoints(&loser, std::slice::from_ref(&checkpoint))
            .await
            .expect_err("stale reconciler should lose the CAS");

        match error {
            CheckpointError::Retry(CheckpointRetry { retry_after, .. }) => {
                assert_eq!(retry_after, CHECKPOINT_CONFLICT_RETRY);
            }
            _ => panic!("stale checkpoint writer should retry"),
        }
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn multiple_user_ownership_checkpoints_use_one_cas_patch() {
        let requests = Arc::new(AtomicUsize::new(0));
        let captured_patch = Arc::new(Mutex::new(Value::Null));
        let user = provisioning_user("app-user-a", "app-user-secret-a", "readwrite");
        let tenant = provisioning_test_tenant(user, ProvisioningStatus::default());
        let mut first = owned_user_status(ProvisioningUserOwnershipState::PendingCreate, "5");
        first.name = "app-user-a".to_string();
        first.state = ProvisioningItemState::Pending.as_str().to_string();
        first
            .ownership
            .as_mut()
            .expect("ownership should exist")
            .user_name = first.name.clone();
        let mut second = first.clone();
        second.name = "app-user-b".to_string();
        second
            .ownership
            .as_mut()
            .expect("ownership should exist")
            .user_name = second.name.clone();

        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        let mut persisted_tenant = latest_tenant.clone();
        persisted_tenant.metadata.resource_version = Some("19".to_string());
        persisted_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![first.clone(), second.clone()];

        let service_requests = requests.clone();
        let service_patch = captured_patch.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let service_requests = service_requests.clone();
            let service_patch = service_patch.clone();
            let latest_tenant = latest_tenant.clone();
            let persisted_tenant = persisted_tenant.clone();
            async move {
                let attempt = service_requests.fetch_add(1, Ordering::SeqCst);
                let response_tenant = match attempt {
                    0 => {
                        assert_eq!(request.method(), http::Method::GET);
                        latest_tenant
                    }
                    1 => {
                        assert_eq!(request.method(), http::Method::PATCH);
                        let body = request
                            .into_body()
                            .collect()
                            .await
                            .expect("status patch body should be readable")
                            .to_bytes();
                        *service_patch.lock().await =
                            serde_json::from_slice(&body).expect("status patch should be JSON");
                        persisted_tenant
                    }
                    _ => panic!("unexpected Kubernetes request {attempt}"),
                };
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .header("content-type", "application/json")
                        .body(KubeBody::from(
                            serde_json::to_vec(&response_tenant)
                                .expect("Tenant response should serialize"),
                        ))
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        persist_user_ownership_checkpoints(&run, &[first, second])
            .await
            .expect("all checkpoints should be persisted together");

        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let patch = captured_patch.lock().await;
        assert_eq!(patch["metadata"]["resourceVersion"], "18");
        assert_eq!(
            patch["status"]["provisioning"]["users"]
                .as_array()
                .expect("users should be an array")
                .len(),
            2
        );
    }

    #[test]
    fn forbidden_checkpoint_write_is_permanent_but_conflict_retries() {
        let api_error = |code, reason: &str| {
            kube::Error::Api(kube::error::ErrorResponse {
                status: "Failure".to_string(),
                message: reason.to_string(),
                reason: reason.to_string(),
                code,
            })
        };

        assert!(matches!(
            classify_checkpoint_kube_error(api_error(403, "Forbidden")),
            CheckpointError::Permanent { .. }
        ));
        assert!(matches!(
            classify_checkpoint_kube_error(api_error(409, "Conflict")),
            CheckpointError::Retry(CheckpointRetry {
                retry_after: CHECKPOINT_CONFLICT_RETRY,
                ..
            })
        ));
        assert!(matches!(
            classify_checkpoint_kube_error(api_error(503, "ServiceUnavailable")),
            CheckpointError::Retry(CheckpointRetry {
                retry_after: CHECKPOINT_TRANSIENT_RETRY,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn unmanaged_existing_user_fails_before_credentials_or_policy_writes() {
        let kube_requests = Arc::new(AtomicUsize::new(0));
        let kube_request_count = kube_requests.clone();
        let kube_service = service_fn(move |_request: http::Request<KubeBody>| {
            let kube_request_count = kube_request_count.clone();
            async move {
                kube_request_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(KubeBody::empty())
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let tenant = provisioning_test_tenant(user.clone(), ProvisioningStatus::default());
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let write_requests = Arc::new(AtomicUsize::new(0));
        let add_requests = write_requests.clone();
        let policy_requests = write_requests.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/user-info",
                get(|| async { StatusCode::OK }),
            )
            .route(
                "/rustfs/admin/v3/add-user",
                put(move || {
                    let add_requests = add_requests.clone();
                    async move {
                        add_requests.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/set-policy",
                put(move || {
                    let policy_requests = policy_requests.clone();
                    async move {
                        policy_requests.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let credentials = user_credentials("1");

        let item = reconcile_user(
            &run,
            &client,
            &BTreeMap::from([("readwrite".to_string(), "{}".to_string())]),
            &BTreeSet::new(),
            &user,
            &credentials,
        )
        .await;

        assert_eq!(item.state, ProvisioningItemState::Failed.as_str());
        assert_eq!(item.reason, Reason::UserOwnershipConflict.as_str());
        assert_eq!(write_requests.load(Ordering::SeqCst), 0);
        assert_eq!(kube_requests.load(Ordering::SeqCst), 0);
        let serialized = serde_json::to_string(&item).expect("status should serialize");
        assert!(!serialized.contains(&credentials.access_key));
        assert!(!serialized.contains(&credentials.secret_key));
        server.abort();
    }

    #[tokio::test]
    async fn new_user_persists_pending_ownership_before_external_writes() {
        let sequence = Arc::new(AtomicUsize::new(0));
        let captured_patch = Arc::new(Mutex::new(Value::Null));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let tenant = provisioning_test_tenant(user.clone(), ProvisioningStatus::default());
        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        latest_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .current_state = "latest-controller-state".to_string();
        let mut persisted_tenant = latest_tenant.clone();
        persisted_tenant.metadata.resource_version = Some("19".to_string());
        let mut persisted_checkpoint =
            owned_user_status(ProvisioningUserOwnershipState::PendingCreate, "5");
        persisted_checkpoint.state = ProvisioningItemState::Pending.as_str().to_string();
        persisted_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![persisted_checkpoint];
        let kube_sequence = sequence.clone();
        let kube_patch = captured_patch.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let kube_sequence = kube_sequence.clone();
            let kube_patch = kube_patch.clone();
            let latest_tenant = latest_tenant.clone();
            let persisted_tenant = persisted_tenant.clone();
            async move {
                let response_tenant = if request.method() == http::Method::GET {
                    assert!(request.uri().path().ends_with("/tenants/tenant-a"));
                    assert_eq!(kube_sequence.load(Ordering::SeqCst), 1);
                    kube_sequence.store(2, Ordering::SeqCst);
                    latest_tenant
                } else {
                    assert_eq!(request.method(), http::Method::PATCH);
                    assert!(request.uri().path().ends_with("/tenants/tenant-a/status"));
                    assert_eq!(kube_sequence.load(Ordering::SeqCst), 2);
                    let body = request
                        .into_body()
                        .collect()
                        .await
                        .expect("status patch body should be readable")
                        .to_bytes();
                    let patch: Value =
                        serde_json::from_slice(&body).expect("status patch should be JSON");
                    *kube_patch.lock().await = patch;
                    kube_sequence.store(3, Ordering::SeqCst);
                    persisted_tenant
                };
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .header("content-type", "application/json")
                        .body(KubeBody::from(
                            serde_json::to_vec(&response_tenant)
                                .expect("Tenant response should serialize"),
                        ))
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let get_sequence = sequence.clone();
        let add_sequence = sequence.clone();
        let policy_sequence = sequence.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/user-info",
                get(move || {
                    let get_sequence = get_sequence.clone();
                    async move {
                        assert_eq!(get_sequence.load(Ordering::SeqCst), 0);
                        get_sequence.store(1, Ordering::SeqCst);
                        StatusCode::NOT_FOUND
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/add-user",
                put(move || {
                    let add_sequence = add_sequence.clone();
                    async move {
                        assert_eq!(add_sequence.load(Ordering::SeqCst), 3);
                        add_sequence.store(4, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/set-policy",
                put(move || {
                    let policy_sequence = policy_sequence.clone();
                    async move {
                        assert_eq!(policy_sequence.load(Ordering::SeqCst), 4);
                        policy_sequence.store(5, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let credentials = user_credentials("5");

        let item = reconcile_user(
            &run,
            &client,
            &BTreeMap::from([("readwrite".to_string(), "{}".to_string())]),
            &BTreeSet::new(),
            &user,
            &credentials,
        )
        .await;

        assert_eq!(sequence.load(Ordering::SeqCst), 5);
        assert_eq!(item.state, ProvisioningItemState::Ready.as_str());
        assert_eq!(
            item.ownership.as_ref().map(|ownership| ownership.state),
            Some(ProvisioningUserOwnershipState::Managed)
        );
        let patch = captured_patch.lock().await;
        assert_eq!(patch["metadata"]["resourceVersion"], "18");
        assert_eq!(patch["status"]["currentState"], "latest-controller-state");
        let checkpoint = &patch["status"]["provisioning"]["users"][0];
        assert_eq!(checkpoint["name"], "app-user");
        assert_eq!(checkpoint["state"], "Pending");
        assert_eq!(checkpoint["ownership"]["state"], "PendingCreate");
        assert_eq!(checkpoint["ownership"]["tenantUid"], "tenant-uid-a");
        assert_eq!(checkpoint["ownership"]["userName"], "app-user");
        assert_eq!(
            checkpoint["ownership"]["accessKeyHash"],
            access_key_hash("appuser01")
        );
        let serialized_patch = patch.to_string();
        assert!(!serialized_patch.contains(&credentials.access_key));
        assert!(!serialized_patch.contains(&credentials.secret_key));
        server.abort();
    }

    #[tokio::test]
    async fn pruned_checkpoint_response_fails_before_external_user_writes() {
        let kube_requests = Arc::new(AtomicUsize::new(0));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let tenant = provisioning_test_tenant(user.clone(), ProvisioningStatus::default());
        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        let mut pruned_tenant = latest_tenant.clone();
        pruned_tenant.metadata.resource_version = Some("19".to_string());
        pruned_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![ProvisioningUserStatus::new(ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Pending,
            Reason::ProvisioningPending.as_str(),
        ))];
        let service_requests = kube_requests.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let service_requests = service_requests.clone();
            let latest_tenant = latest_tenant.clone();
            let pruned_tenant = pruned_tenant.clone();
            async move {
                let attempt = service_requests.fetch_add(1, Ordering::SeqCst);
                let response_tenant = match attempt {
                    0 => {
                        assert_eq!(request.method(), http::Method::GET);
                        latest_tenant
                    }
                    1 => {
                        assert_eq!(request.method(), http::Method::PATCH);
                        pruned_tenant
                    }
                    _ => panic!("unexpected Kubernetes request {attempt}"),
                };
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .header("content-type", "application/json")
                        .body(KubeBody::from(
                            serde_json::to_vec(&response_tenant)
                                .expect("Tenant response should serialize"),
                        ))
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let write_requests = Arc::new(AtomicUsize::new(0));
        let add_requests = write_requests.clone();
        let policy_requests = write_requests.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/user-info",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/rustfs/admin/v3/add-user",
                put(move || {
                    let add_requests = add_requests.clone();
                    async move {
                        add_requests.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/set-policy",
                put(move || {
                    let policy_requests = policy_requests.clone();
                    async move {
                        policy_requests.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let item = reconcile_user(
            &run,
            &client,
            &BTreeMap::from([("readwrite".to_string(), "{}".to_string())]),
            &BTreeSet::new(),
            &user,
            &user_credentials("5"),
        )
        .await;

        assert_eq!(item.state, ProvisioningItemState::Failed.as_str());
        assert_eq!(item.reason, Reason::UserOwnershipCheckpointFailed.as_str());
        assert!(
            item.message
                .as_deref()
                .is_some_and(|message| message.contains("CRD"))
        );
        assert_eq!(kube_requests.load(Ordering::SeqCst), 2);
        assert_eq!(write_requests.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn checkpoint_response_with_rewritten_state_is_rejected() {
        let requests = Arc::new(AtomicUsize::new(0));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let tenant = provisioning_test_tenant(user, ProvisioningStatus::default());
        let mut checkpoint = owned_user_status(ProvisioningUserOwnershipState::PendingCreate, "5");
        checkpoint.state = ProvisioningItemState::Pending.as_str().to_string();
        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        let mut rewritten_tenant = latest_tenant.clone();
        rewritten_tenant.metadata.resource_version = Some("19".to_string());
        let mut rewritten_checkpoint = checkpoint.clone();
        rewritten_checkpoint.state = ProvisioningItemState::Ready.as_str().to_string();
        rewritten_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![rewritten_checkpoint];
        let service_requests = requests.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let service_requests = service_requests.clone();
            let latest_tenant = latest_tenant.clone();
            let rewritten_tenant = rewritten_tenant.clone();
            async move {
                let attempt = service_requests.fetch_add(1, Ordering::SeqCst);
                let response_tenant = match attempt {
                    0 => {
                        assert_eq!(request.method(), http::Method::GET);
                        latest_tenant
                    }
                    1 => {
                        assert_eq!(request.method(), http::Method::PATCH);
                        rewritten_tenant
                    }
                    _ => panic!("unexpected Kubernetes request {attempt}"),
                };
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .header("content-type", "application/json")
                        .body(KubeBody::from(
                            serde_json::to_vec(&response_tenant)
                                .expect("Tenant response should serialize"),
                        ))
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous: ProvisioningStatus::default(),
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let error = persist_user_ownership_checkpoints(&run, std::slice::from_ref(&checkpoint))
            .await
            .expect_err("rewritten checkpoint state must be rejected");

        assert!(matches!(error, CheckpointError::Permanent { .. }));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn pending_user_checkpoint_recovers_after_create_before_status_crash() {
        let kube_requests = Arc::new(AtomicUsize::new(0));
        let kube_request_count = kube_requests.clone();
        let kube_service = service_fn(move |_request: http::Request<KubeBody>| {
            let kube_request_count = kube_request_count.clone();
            async move {
                kube_request_count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(KubeBody::empty())
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let previous_user = owned_user_status(ProvisioningUserOwnershipState::PendingCreate, "5");
        let previous = ProvisioningStatus {
            users: vec![previous_user],
            ..Default::default()
        };
        let tenant = provisioning_test_tenant(user.clone(), previous.clone());
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous,
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let add_requests = Arc::new(AtomicUsize::new(0));
        let policy_requests = Arc::new(AtomicUsize::new(0));
        let add_count = add_requests.clone();
        let policy_count = policy_requests.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/user-info",
                get(|| async { StatusCode::OK }),
            )
            .route(
                "/rustfs/admin/v3/add-user",
                put(move || {
                    let add_count = add_count.clone();
                    async move {
                        add_count.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/set-policy",
                put(move || {
                    let policy_count = policy_count.clone();
                    async move {
                        policy_count.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let item = reconcile_user(
            &run,
            &client,
            &BTreeMap::from([("readwrite".to_string(), "{}".to_string())]),
            &BTreeSet::new(),
            &user,
            &user_credentials("5"),
        )
        .await;

        assert_eq!(add_requests.load(Ordering::SeqCst), 0);
        assert_eq!(policy_requests.load(Ordering::SeqCst), 1);
        assert_eq!(kube_requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            item.ownership.as_ref().map(|ownership| ownership.state),
            Some(ProvisioningUserOwnershipState::Managed)
        );
        server.abort();
    }

    #[tokio::test]
    async fn retained_legacy_user_is_checkpointed_before_readd_and_secret_rotation() {
        let sequence = Arc::new(AtomicUsize::new(0));
        let user = provisioning_user("app-user", "app-user-secret", "readwrite");
        let mut previous_user = owned_user_status(ProvisioningUserOwnershipState::Managed, "4");
        previous_user.ownership = None;
        previous_user.state = ProvisioningItemState::Retained.as_str().to_string();
        let previous = ProvisioningStatus {
            users: vec![previous_user],
            ..Default::default()
        };
        let tenant = provisioning_test_tenant(user.clone(), previous.clone());
        let mut latest_tenant = tenant.clone();
        latest_tenant.metadata.resource_version = Some("18".to_string());
        let mut persisted_tenant = latest_tenant.clone();
        persisted_tenant.metadata.resource_version = Some("19".to_string());
        let persisted_checkpoint = owned_user_status(ProvisioningUserOwnershipState::Managed, "4");
        persisted_tenant
            .status
            .as_mut()
            .expect("Tenant should have status")
            .provisioning
            .users = vec![persisted_checkpoint];
        let kube_sequence = sequence.clone();
        let kube_service = service_fn(move |request: http::Request<KubeBody>| {
            let kube_sequence = kube_sequence.clone();
            let latest_tenant = latest_tenant.clone();
            let persisted_tenant = persisted_tenant.clone();
            async move {
                let response_tenant = if request.method() == http::Method::GET {
                    assert_eq!(kube_sequence.load(Ordering::SeqCst), 1);
                    kube_sequence.store(2, Ordering::SeqCst);
                    latest_tenant
                } else {
                    assert_eq!(request.method(), http::Method::PATCH);
                    assert_eq!(kube_sequence.load(Ordering::SeqCst), 2);
                    kube_sequence.store(3, Ordering::SeqCst);
                    persisted_tenant
                };
                Ok::<_, Infallible>(
                    http::Response::builder()
                        .header("content-type", "application/json")
                        .body(KubeBody::from(
                            serde_json::to_vec(&response_tenant)
                                .expect("Tenant response should serialize"),
                        ))
                        .expect("response should build"),
                )
            }
        });
        let ctx = Context::new(Client::new(kube_service, "default"));
        let run = ProvisioningRun {
            ctx: &ctx,
            tenant: &tenant,
            namespace: "storage",
            previous,
            now: "2026-08-02T00:00:00Z".to_string(),
            status: ProvisioningStatus::default(),
            failures: Vec::new(),
        };

        let add_requests = Arc::new(AtomicUsize::new(0));
        let policy_requests = Arc::new(AtomicUsize::new(0));
        let add_count = add_requests.clone();
        let policy_count = policy_requests.clone();
        let get_sequence = sequence.clone();
        let add_sequence = sequence.clone();
        let policy_sequence = sequence.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/user-info",
                get(move || {
                    let get_sequence = get_sequence.clone();
                    async move {
                        assert_eq!(get_sequence.load(Ordering::SeqCst), 0);
                        get_sequence.store(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/add-user",
                put(move || {
                    let add_count = add_count.clone();
                    let add_sequence = add_sequence.clone();
                    async move {
                        assert_eq!(add_sequence.load(Ordering::SeqCst), 3);
                        add_sequence.store(4, Ordering::SeqCst);
                        add_count.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/rustfs/admin/v3/set-policy",
                put(move || {
                    let policy_count = policy_count.clone();
                    let policy_sequence = policy_sequence.clone();
                    async move {
                        assert_eq!(policy_sequence.load(Ordering::SeqCst), 4);
                        policy_sequence.store(5, Ordering::SeqCst);
                        policy_count.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let item = reconcile_user(
            &run,
            &client,
            &BTreeMap::from([("readwrite".to_string(), "{}".to_string())]),
            &BTreeSet::new(),
            &user,
            &user_credentials("5"),
        )
        .await;

        assert_eq!(add_requests.load(Ordering::SeqCst), 1);
        assert_eq!(policy_requests.load(Ordering::SeqCst), 1);
        assert_eq!(sequence.load(Ordering::SeqCst), 5);
        assert_eq!(item.state, ProvisioningItemState::Ready.as_str());
        assert_eq!(item.observed_secret_resource_version.as_deref(), Some("5"));
        assert_eq!(
            item.ownership.as_ref().map(|ownership| ownership.state),
            Some(ProvisioningUserOwnershipState::Managed)
        );
        server.abort();
    }

    #[test]
    fn policy_document_hash_uses_compact_json() {
        let normalized = normalize_policy_document(
            r#"{
                "Version": "2012-10-17",
                "Statement": []
            }"#,
        )
        .expect("policy should normalize");

        assert_eq!(normalized, r#"{"Statement":[],"Version":"2012-10-17"}"#);
        assert!(hash_document(&normalized).starts_with("sha256:"));
    }

    #[test]
    fn policy_document_preserves_raw_write_document() {
        let raw = r#"{
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": ["s3:PutObject", "s3:GetObject"],
                    "Resource": "arn:aws:s3:::app-data/*"
                }
            ]
        }"#;

        let document = PolicyDocument::parse(raw).expect("policy should parse");
        let normalized: Value =
            serde_json::from_str(&document.normalized).expect("normalized policy should be JSON");

        assert_eq!(document.raw, raw);
        assert_eq!(
            normalized["Statement"][0]["Action"],
            serde_json::json!(["s3:GetObject", "s3:PutObject"])
        );
    }

    #[tokio::test]
    async fn apply_policy_sends_raw_document_to_rustfs() {
        let raw = r#"{
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Deny",
                    "NotAction": ["s3:PutObject", "s3:GetObject"],
                    "NotResource": [
                        "arn:aws:s3:::app-data/public/*",
                        "arn:aws:s3:::app-data/private/*"
                    ]
                }
            ]
        }"#;
        let capture = PolicyApplyCapture::default();
        let route_capture = capture.clone();
        let live_document = raw.to_string();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/add-canned-policy",
                put(
                    move |State(c): State<PolicyApplyCapture>, req: Request<Body>| async move {
                        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                            .await
                            .expect("request body should be readable");
                        *c.body.lock().await =
                            String::from_utf8(body_bytes.to_vec()).expect("body should be UTF-8");

                        StatusCode::OK
                    },
                ),
            )
            .route(
                "/rustfs/admin/v3/info-canned-policy",
                get(move || {
                    let live_document = live_document.clone();
                    async move { live_document }
                }),
            )
            .with_state(route_capture);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let mut live_policies = BTreeMap::new();

        let applied_hash = apply_policy(&client, &mut live_policies, "tenant-policy", raw)
            .await
            .expect("policy should apply");

        assert_eq!(&*capture.body.lock().await, raw);
        assert_eq!(
            applied_hash,
            hash_document(
                live_policies
                    .get("tenant-policy")
                    .expect("live policy should be cached")
            )
        );
        server.abort();
    }

    #[tokio::test]
    async fn apply_policy_includes_upstream_policy_parse_error() {
        let router = Router::new().route(
            "/rustfs/admin/v3/add-canned-policy",
            put(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    r#"<Error><Code>InvalidRequest</Code><Message>invalid resource: unknown &quot;*&quot;</Message></Error>"#,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let mut live_policies = BTreeMap::new();
        let raw = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#;

        let error = apply_policy(&client, &mut live_policies, "tenant-policy", raw)
            .await
            .expect_err("RustFS policy parse error should fail provisioning");

        assert_eq!(
            error,
            r#"failed to apply RustFS policy 'tenant-policy': upstream returned 400 Bad Request: InvalidRequest: invalid resource: unknown "*""#
        );
        server.abort();
    }

    #[tokio::test]
    async fn rotated_user_secret_is_upserted_for_existing_user() {
        let capture = UserCredentialCapture::default();
        let route_capture = capture.clone();
        let router = Router::new()
            .route(
                "/rustfs/admin/v3/add-user",
                put(
                    move |State(c): State<UserCredentialCapture>, req: Request<Body>| async move {
                        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                            .await
                            .expect("request body should be readable");
                        *c.body.lock().await =
                            String::from_utf8(body_bytes.to_vec()).expect("body should be UTF-8");

                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve")
        });
        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let mut previous = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.observed_secret_resource_version = Some("1".to_string());
        previous.observed_secret_name = Some("app-user".to_string());
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        let previous = ProvisioningUserStatus::new(previous);
        let credentials = UserCredentials {
            access_key: "appuser01".to_string(),
            secret_key: "rotated-secret".to_string(),
            secret_name: "app-user".to_string(),
            resource_version: Some("2".to_string()),
        };

        let applied = sync_user_credentials(&client, Some(&previous), &credentials, true)
            .await
            .expect("rotated secret should be applied");

        assert!(applied);
        assert_eq!(
            serde_json::from_str::<Value>(&capture.body.lock().await)
                .expect("request body should be JSON"),
            serde_json::json!({"secretKey": "rotated-secret", "status": "enabled"})
        );
        server.abort();
    }

    #[test]
    fn unchanged_user_secret_version_skips_credentials_write() {
        let mut previous = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.observed_secret_resource_version = Some("1".to_string());
        previous.observed_secret_name = Some("app-user".to_string());
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        let previous = ProvisioningUserStatus::new(previous);
        let credentials = UserCredentials {
            access_key: "appuser01".to_string(),
            secret_key: "unchanged-secret".to_string(),
            secret_name: "app-user".to_string(),
            resource_version: Some("1".to_string()),
        };

        assert!(!user_credentials_need_apply(
            Some(&previous),
            &credentials,
            true
        ));
    }

    #[test]
    fn changed_user_secret_reference_requires_credentials_write() {
        let mut previous = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.observed_secret_resource_version = Some("1".to_string());
        previous.observed_secret_name = Some("app-user".to_string());
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        let previous = ProvisioningUserStatus::new(previous);
        let credentials = UserCredentials {
            access_key: "appuser01".to_string(),
            secret_key: "rotated-secret".to_string(),
            secret_name: "rustfs-user-app-user".to_string(),
            resource_version: Some("1".to_string()),
        };

        assert!(user_credentials_need_apply(
            Some(&previous),
            &credentials,
            true
        ));
    }

    #[test]
    fn legacy_user_status_records_secret_identity_with_one_credentials_write() {
        let mut previous = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.observed_secret_resource_version = Some("1".to_string());
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        let previous = ProvisioningUserStatus::new(previous);
        let credentials = UserCredentials {
            access_key: "appuser01".to_string(),
            secret_key: "unchanged-secret".to_string(),
            secret_name: "app-user".to_string(),
            resource_version: Some("1".to_string()),
        };

        assert!(user_credentials_need_apply(
            Some(&previous),
            &credentials,
            true
        ));
    }

    #[test]
    fn changed_user_access_key_is_rejected() {
        let mut previous = ProvisioningItemStatus::new(
            "app-user",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.last_applied_access_key_hash = Some(access_key_hash("appuser01"));
        let previous = ProvisioningUserStatus::new(previous);
        let credentials = UserCredentials {
            access_key: "otheruser".to_string(),
            secret_key: "rotated-secret".to_string(),
            secret_name: "app-user".to_string(),
            resource_version: Some("2".to_string()),
        };

        assert!(user_access_key_changed(Some(&previous), &credentials));
    }

    #[test]
    fn policy_document_normalization_preserves_deny_fields() {
        let normalized = normalize_policy_document(
            r#"{
                "ID": "policy-id",
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "deny-selected",
                        "Effect": "Deny",
                        "NotAction": ["s3:PutObject", "s3:GetObject"],
                        "NotResource": [
                            "arn:aws:s3:::app-data/public/*",
                            "arn:aws:s3:::app-data/private/*"
                        ],
                        "Condition": {
                            "StringLike": {
                                "s3:prefix": ["private/*"]
                            }
                        },
                        "Principal": "*"
                    }
                ]
            }"#,
        )
        .expect("policy should normalize");
        let normalized: Value =
            serde_json::from_str(&normalized).expect("normalized policy should be JSON");
        let statement = &normalized["Statement"][0];

        assert_eq!(normalized["ID"], "policy-id");
        assert_eq!(statement["Sid"], "deny-selected");
        assert_eq!(
            statement["NotAction"],
            serde_json::json!(["s3:GetObject", "s3:PutObject"])
        );
        assert_eq!(
            statement["NotResource"],
            serde_json::json!([
                "arn:aws:s3:::app-data/private/*",
                "arn:aws:s3:::app-data/public/*"
            ])
        );
        assert_eq!(statement["Principal"], "*");
        assert!(statement["Condition"].is_object());
    }

    #[test]
    fn stale_policy_status_is_ready_when_live_policy_matches_desired() {
        let mut previous = ProvisioningItemStatus::new(
            "app-policy",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.last_applied_hash = Some("sha256:old".to_string());

        let action = policy_reconcile_action(Some(&previous), Some("sha256:new"), "sha256:new");

        assert_eq!(
            action,
            PolicyReconcileAction::Ready("RustFS policy matches spec")
        );
    }

    #[test]
    fn stale_policy_status_updates_last_applied_metadata_when_live_matches_desired() {
        let live_document = normalize_policy_document(
            r#"{
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Action": ["s3:GetObject"],
                        "Resource": ["arn:aws:s3:::app-data/*"]
                    }
                ]
            }"#,
        )
        .expect("policy should normalize");
        let desired_hash = hash_document(&live_document);
        let live_policies = BTreeMap::from([("app-policy".to_string(), live_document)]);
        let mut previous = ProvisioningItemStatus::new(
            "app-policy",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );
        previous.last_applied_hash = Some("sha256:old".to_string());
        previous.last_applied_generation = Some(7);
        let item = ProvisioningItemStatus::new(
            "app-policy",
            ProvisioningItemState::Ready,
            Reason::ProvisioningConfigured.as_str(),
        );

        let item = finalize_policy_item_status(
            item,
            Some(&previous),
            "app-policy",
            desired_hash.clone(),
            &live_policies,
            Some(8),
        );

        assert_eq!(item.desired_hash.as_deref(), Some(desired_hash.as_str()));
        assert_eq!(
            item.last_applied_hash.as_deref(),
            Some(desired_hash.as_str())
        );
        assert_eq!(item.last_applied_generation, Some(8));
    }

    #[test]
    fn rustfs_server_policy_matches_configmap_spec() {
        let spec = r#"{
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": ["s3:ListBucket"],
                    "Resource": ["arn:aws:s3:::rfsd01-data"]
                },
                {
                    "Effect": "Allow",
                    "Action": ["s3:GetObject", "s3:DeleteObject", "s3:PutObject"],
                    "Resource": ["arn:aws:s3:::rfsd01-data/*"]
                }
            ]
        }"#;
        let server = r#"{
            "ID": "",
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Sid": "",
                    "Effect": "Allow",
                    "Action": ["s3:ListBucket"],
                    "Resource": ["arn:aws:s3:::rfsd01-data"],
                    "Condition": {}
                },
                {
                    "Sid": "",
                    "Effect": "Allow",
                    "Action": ["s3:PutObject", "s3:DeleteObject", "s3:GetObject"],
                    "Resource": ["arn:aws:s3:::rfsd01-data/*"],
                    "Condition": {}
                }
            ]
        }"#;

        let spec_normalized = normalize_policy_document(spec).expect("spec should normalize");
        let server_normalized = normalize_policy_document(server).expect("server should normalize");

        assert_eq!(spec_normalized, server_normalized);
        assert_eq!(
            hash_document(&spec_normalized),
            hash_document(&server_normalized)
        );
    }

    #[test]
    fn bucket_name_validation_matches_rustfs_strict_rules() {
        assert!(validate_bucket_name("app-data").is_ok());
        assert!(validate_bucket_name("my.bucket.name").is_ok());

        for invalid in [
            "ab",
            "rustfs",
            "192.168.1.1",
            "MyBucket",
            "my_bucket",
            "my..bucket",
        ] {
            assert!(
                validate_bucket_name(invalid).is_err(),
                "{invalid} should be rejected"
            );
        }
    }
}
