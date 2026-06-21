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
use crate::sts::rustfs_client::{CreateBucketResult, RustfsAdminClient, RustfsClientError};
use crate::types::v1alpha1::provisioning::{
    ProvisioningBucket, ProvisioningPolicy, ProvisioningUser,
};
use crate::types::v1alpha1::status::Reason;
use crate::types::v1alpha1::status::provisioning::{
    ProvisioningItemState, ProvisioningItemStatus, ProvisioningPhase, ProvisioningStatus,
};
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct ProvisioningReconcileResult {
    pub status: ProvisioningStatus,
    pub outcome: ProvisioningOutcome,
}

pub(super) enum ProvisioningOutcome {
    Ready,
    Pending { message: String },
    Failed { reason: Reason, message: String },
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

struct UserCredentials {
    access_key: String,
    secret_key: String,
    resource_version: Option<String>,
}

impl ProvisioningRun<'_> {
    fn previous_policy(&self, name: &str) -> Option<&ProvisioningItemStatus> {
        self.previous.policies.iter().find(|item| item.name == name)
    }

    fn previous_user(&self, name: &str) -> Option<&ProvisioningItemStatus> {
        self.previous.users.iter().find(|item| item.name == name)
    }

    fn previous_bucket(&self, name: &str) -> Option<&ProvisioningItemStatus> {
        self.previous.buckets.iter().find(|item| item.name == name)
    }

    fn push_policy(&mut self, item: ProvisioningItemStatus) {
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.policies.push(item);
    }

    fn push_user(&mut self, item: ProvisioningItemStatus) {
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.users.push(item);
    }

    fn push_bucket(&mut self, item: ProvisioningItemStatus) {
        if item.state == ProvisioningItemState::Failed.as_str() {
            self.failures
                .push((reason_from_str(&item.reason), item_message(&item)));
        }
        self.status.buckets.push(item);
    }

    fn item(
        &self,
        previous: Option<&ProvisioningItemStatus>,
        name: &str,
        state: ProvisioningItemState,
        reason: Reason,
        message: impl Into<String>,
    ) -> ProvisioningItemStatus {
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
        item.policies = previous.policies.clone();
        item.region = previous.region.clone();
        item.object_lock = previous.object_lock;
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
                item.policies = previous.policies.clone();
            }
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
                self.status.users.push(self.retained_item(previous));
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

    let mut live_policies = match load_live_policies(&client, tenant).await {
        Ok(policies) => policies,
        Err(message) => {
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
    reconcile_users(&mut run, &client, &live_policies).await;
    reconcile_buckets(&mut run, &client).await;
    run.finish()
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

    let desired_hash = hash_document(&document);
    let mut item = match live_policies.get(&policy.name) {
        Some(live_document) => {
            let live_hash = hash_document(live_document);
            match previous.and_then(|item| item.last_applied_hash.as_deref()) {
                None if live_hash == desired_hash => run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Ready,
                    Reason::ProvisioningConfigured,
                    "Existing RustFS policy matches spec and was adopted",
                ),
                None => run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Failed,
                    Reason::PolicyConflict,
                    "Live RustFS policy differs from spec and is not owned by this status",
                ),
                Some(last_applied_hash) if last_applied_hash == live_hash => {
                    if live_hash == desired_hash {
                        run.item(
                            previous,
                            &policy.name,
                            ProvisioningItemState::Ready,
                            Reason::ProvisioningConfigured,
                            "RustFS policy already matches spec",
                        )
                    } else {
                        match apply_policy(client, live_policies, &policy.name, &document).await {
                            Ok(applied_hash) => {
                                let mut item = run.item(
                                    previous,
                                    &policy.name,
                                    ProvisioningItemState::Ready,
                                    Reason::ProvisioningConfigured,
                                    "RustFS policy was applied",
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
                }
                Some(_) if live_hash == desired_hash => run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Ready,
                    Reason::ProvisioningConfigured,
                    "RustFS policy matches spec",
                ),
                Some(_) => run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Failed,
                    Reason::PolicyConflict,
                    "Live RustFS policy changed since the operator last applied it",
                ),
            }
        }
        None => match apply_policy(client, live_policies, &policy.name, &document).await {
            Ok(applied_hash) => {
                let mut item = run.item(
                    previous,
                    &policy.name,
                    ProvisioningItemState::Ready,
                    Reason::ProvisioningConfigured,
                    "RustFS policy was created",
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
        },
    };

    item.desired_hash = Some(desired_hash);
    if item.last_applied_hash.is_none() && item.state == ProvisioningItemState::Ready.as_str() {
        item.last_applied_hash = live_policies
            .get(&policy.name)
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
        (Some(_), _) if item.state == ProvisioningItemState::Ready.as_str() => {
            run.tenant.metadata.generation
        }
        _ => previous.and_then(|item| item.last_applied_generation),
    };
    item
}

async fn load_policy_document(
    run: &ProvisioningRun<'_>,
    policy: &ProvisioningPolicy,
) -> Result<String, (Reason, String)> {
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

    normalize_policy_document(raw).map_err(|message| (Reason::PolicyApplyFailed, message))
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
) {
    let failed_spec_policies = run
        .status
        .policies
        .iter()
        .filter(|item| item.state == ProvisioningItemState::Failed.as_str())
        .map(|item| item.name.clone())
        .collect::<BTreeSet<_>>();

    for user in &run.tenant.spec.users {
        let item = reconcile_user(run, client, live_policies, &failed_spec_policies, user).await;
        run.push_user(item);
    }
}

async fn reconcile_user(
    run: &ProvisioningRun<'_>,
    client: &RustfsAdminClient,
    live_policies: &BTreeMap<String, String>,
    failed_spec_policies: &BTreeSet<String>,
    user: &ProvisioningUser,
) -> ProvisioningItemStatus {
    let previous = run.previous_user(&user.name);
    if let Err(message) = validate_user_policies(user) {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserPolicyInvalid,
            message,
        );
        return annotate_user_item(item, user, None);
    }

    let credentials = match load_user_secret(run, user).await {
        Ok(credentials) => credentials,
        Err(message) => {
            let item = run.item(
                previous,
                &user.name,
                ProvisioningItemState::Failed,
                Reason::UserSecretInvalid,
                message,
            );
            return annotate_user_item(item, user, None);
        }
    };

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
        return annotate_user_item(item, user, credentials.resource_version);
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
        return annotate_user_item(item, user, credentials.resource_version);
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
            return annotate_user_item(item, user, credentials.resource_version);
        }
    };

    if !exists
        && let Err(error) = client
            .add_user(&credentials.access_key, &credentials.secret_key)
            .await
    {
        let item = run.item(
            previous,
            &user.name,
            ProvisioningItemState::Failed,
            Reason::UserSecretInvalid,
            format!("failed to create RustFS user: {error}"),
        );
        return annotate_user_item(item, user, credentials.resource_version);
    }

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
        return annotate_user_item(item, user, credentials.resource_version);
    }

    let reason = if exists {
        "UserAlreadyExistsPolicySet"
    } else {
        Reason::ProvisioningConfigured.as_str()
    };
    let mut item =
        ProvisioningItemStatus::new(&user.name, ProvisioningItemState::Ready, reason.to_string());
    let message = if exists {
        "RustFS user already existed; direct policy mapping was applied"
    } else {
        "RustFS user was created and direct policy mapping was applied"
    };
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
    annotate_user_item(item, user, credentials.resource_version)
}

fn annotate_user_item(
    mut item: ProvisioningItemStatus,
    user: &ProvisioningUser,
    resource_version: Option<String>,
) -> ProvisioningItemStatus {
    item.observed_secret_resource_version = resource_version;
    item.policies = user.policies.clone();
    item
}

async fn load_user_secret(
    run: &ProvisioningRun<'_>,
    user: &ProvisioningUser,
) -> Result<UserCredentials, String> {
    let secret: Secret = run
        .ctx
        .get(&user.name, run.namespace)
        .await
        .map_err(|error| {
            if context::is_kube_not_found(&error) {
                format!("user Secret '{}' was not found", user.name)
            } else {
                format!("failed to read user Secret '{}': {error}", user.name)
            }
        })?;
    let data = secret
        .data
        .as_ref()
        .ok_or_else(|| format!("user Secret '{}' has no data", user.name))?;

    let access_key = read_compatible_secret_value(
        data,
        "accesskey",
        "CONSOLE_ACCESS_KEY",
        &user.name,
        "access key",
    )?;
    let secret_key = read_compatible_secret_value(
        data,
        "secretkey",
        "CONSOLE_SECRET_KEY",
        &user.name,
        "secret key",
    )?;

    validate_user_access_key(&access_key)?;
    validate_user_secret_key(&secret_key)?;

    Ok(UserCredentials {
        access_key,
        secret_key,
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
    let Some(object) = value.as_object() else {
        return value;
    };

    if !object.contains_key("Statement") {
        return value;
    }

    let mut normalized = serde_json::Map::new();
    if let Some(version) = object.get("Version") {
        normalized.insert("Version".to_string(), version.clone());
    }
    if let Some(statements) = object.get("Statement").and_then(Value::as_array) {
        let mut normalized_statements = statements
            .iter()
            .map(normalize_policy_statement)
            .collect::<Vec<_>>();
        normalized_statements.sort_by_key(statement_sort_key);
        normalized.insert("Statement".to_string(), Value::Array(normalized_statements));
    }

    Value::Object(normalized)
}

fn normalize_policy_statement(statement: &Value) -> Value {
    let Some(object) = statement.as_object() else {
        return statement.clone();
    };

    let mut normalized = serde_json::Map::new();
    if let Some(effect) = object.get("Effect") {
        normalized.insert("Effect".to_string(), effect.clone());
    }
    if let Some(action) = object.get("Action") {
        normalized.insert(
            "Action".to_string(),
            normalize_string_or_string_array(action),
        );
    }
    if let Some(resource) = object.get("Resource") {
        normalized.insert(
            "Resource".to_string(),
            normalize_string_or_string_array(resource),
        );
    }
    if let Some(sid) = object
        .get("Sid")
        .and_then(Value::as_str)
        .filter(|sid| !sid.is_empty())
    {
        normalized.insert("Sid".to_string(), Value::String(sid.to_string()));
    }
    if let Some(condition) = object
        .get("Condition")
        .filter(|condition| is_non_empty_json_object(condition))
    {
        normalized.insert("Condition".to_string(), condition.clone());
    }

    Value::Object(normalized)
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

fn is_non_empty_json_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
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
        "PolicyDocumentConfigMapNotFound" => Reason::PolicyDocumentConfigMapNotFound,
        "PolicyDocumentKeyNotFound" => Reason::PolicyDocumentKeyNotFound,
        "PolicyApplyFailed" => Reason::PolicyApplyFailed,
        "PolicyConflict" => Reason::PolicyConflict,
        "UserSecretInvalid" => Reason::UserSecretInvalid,
        "UserPolicyNotFound" => Reason::UserPolicyNotFound,
        "UserPolicyInvalid" => Reason::UserPolicyInvalid,
        "UserPolicySetFailed" => Reason::UserPolicySetFailed,
        "BucketCreateFailed" => Reason::BucketCreateFailed,
        "BucketObjectLockConflict" => Reason::BucketObjectLockConflict,
        _ => Reason::ProvisioningFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::ByteString;

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

    #[test]
    fn user_policy_list_must_not_be_empty() {
        let user = ProvisioningUser {
            name: "app-user".to_string(),
            policies: Vec::new(),
            deletion_policy: Default::default(),
        };

        let error =
            validate_user_policies(&user).expect_err("empty policy list should be rejected");

        assert!(error.contains("at least one policy"));
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
