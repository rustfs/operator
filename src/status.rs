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

use crate::context;
use crate::types;
use crate::types::v1alpha1::status::{
    ConditionInput, ConditionStatus, ConditionType, Reason, Status, certificate, is_blocked_reason,
    pool, summarize_current_state,
};
use crate::types::v1alpha1::tenant::Tenant;
use crate::utils::sanitize::redact_sensitive_pairs;
use kube::runtime::events::EventType;

const LEGACY_PROGRESSING_CONDITION: &str = "Progressing";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusImpact {
    UserBlocked,
    Degraded,
    Transient,
}

#[derive(Clone, Debug)]
pub struct StatusError {
    pub reason: Reason,
    pub condition_type: ConditionType,
    pub impact: StatusImpact,
    pub safe_message: String,
    pub event_type: EventType,
}

impl StatusError {
    pub fn from_context_error(error: &context::Error) -> Self {
        match error {
            context::Error::CredentialSecretNotFound { name } => Self::blocked(
                Reason::CredentialSecretNotFound,
                ConditionType::CredentialsReady,
                format!("Credential Secret '{}' was not found", name),
            ),
            context::Error::CredentialSecretMissingKey { secret_name, key } => Self::blocked(
                Reason::CredentialSecretMissingKey,
                ConditionType::CredentialsReady,
                format!(
                    "Credential Secret '{}' is missing required key '{}'",
                    secret_name, key
                ),
            ),
            context::Error::CredentialSecretInvalidEncoding { secret_name, key } => Self::blocked(
                Reason::CredentialSecretInvalidEncoding,
                ConditionType::CredentialsReady,
                format!(
                    "Credential Secret '{}' key '{}' must contain valid UTF-8",
                    secret_name, key
                ),
            ),
            context::Error::CredentialSecretTooShort {
                secret_name, key, ..
            } => Self::blocked(
                Reason::CredentialSecretTooShort,
                ConditionType::CredentialsReady,
                format!(
                    "Credential Secret '{}' key '{}' must be at least 8 characters",
                    secret_name, key
                ),
            ),
            context::Error::RpcSecretNotFound { name } => Self::blocked(
                Reason::RpcSecretNotFound,
                ConditionType::RpcAuthReady,
                format!("RPC Secret '{}' was not found", name),
            ),
            context::Error::RpcSecretInvalidReference { field } => Self::blocked(
                Reason::RpcSecretInvalidReference,
                ConditionType::RpcAuthReady,
                format!("spec.rpcSecret.{} must not be blank", field),
            ),
            context::Error::RpcSecretMissingKey { secret_name, key } => Self::blocked(
                Reason::RpcSecretMissingKey,
                ConditionType::RpcAuthReady,
                format!(
                    "RPC Secret '{}' is missing required key '{}'",
                    secret_name, key
                ),
            ),
            context::Error::RpcSecretInvalidEncoding { secret_name, key } => Self::blocked(
                Reason::RpcSecretInvalidEncoding,
                ConditionType::RpcAuthReady,
                format!(
                    "RPC Secret '{}' key '{}' must contain valid UTF-8",
                    secret_name, key
                ),
            ),
            context::Error::RpcSecretInvalidValue { secret_name, key } => Self::blocked(
                Reason::RpcSecretInvalidValue,
                ConditionType::RpcAuthReady,
                format!(
                    "RPC Secret '{}' key '{}' must be non-blank, contain no NUL bytes, and must not use the RustFS default credential value",
                    secret_name, key
                ),
            ),
            context::Error::KmsSecretNotFound { name } => Self::blocked(
                Reason::KmsSecretNotFound,
                ConditionType::KmsReady,
                format!("KMS Secret '{}' was not found", name),
            ),
            context::Error::KmsSecretMissingKey { secret_name, key } => Self::blocked(
                Reason::KmsSecretMissingKey,
                ConditionType::KmsReady,
                format!(
                    "KMS Secret '{}' is missing required key '{}'",
                    secret_name, key
                ),
            ),
            context::Error::KmsConfigInvalid { message } => Self::blocked(
                Reason::KmsConfigInvalid,
                ConditionType::KmsReady,
                sanitize_message(message),
            ),
            context::Error::Types { source } => Self::from_types_error(source),
            context::Error::Kube { .. } => Self::transient(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                "Kubernetes API request failed".to_string(),
            ),
            context::Error::Record { .. } => Self::transient(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                "Kubernetes Event recording failed".to_string(),
            ),
            context::Error::Serde { .. } => Self::degraded(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                "Failed to serialize Kubernetes status patch".to_string(),
            ),
        }
    }

    pub fn status_patch_failed(reason: Reason) -> Self {
        Self {
            reason: Reason::StatusPatchFailed,
            condition_type: ConditionType::Ready,
            impact: StatusImpact::Transient,
            safe_message: format!(
                "Failed to patch Tenant status for reason {}",
                reason.as_str()
            ),
            event_type: EventType::Warning,
        }
    }

    pub fn from_types_error(error: &types::error::Error) -> Self {
        match error {
            types::error::Error::InvalidTenantName { reason, .. } => Self::blocked(
                Reason::InvalidTenantName,
                ConditionType::SpecValid,
                sanitize_message(reason),
            ),
            types::error::Error::InvalidPoolSpec { message, .. } => Self::blocked(
                Reason::InvalidPoolSpec,
                ConditionType::SpecValid,
                sanitize_message(message),
            ),
            types::error::Error::ImmutableFieldModified { field, .. } => Self::blocked(
                Reason::ImmutableFieldModified,
                ConditionType::SpecValid,
                format!("Immutable field '{}' was modified", field),
            ),
            types::error::Error::PoolDeleteBlocked { message, .. } => Self::blocked(
                Reason::PoolDeleteBlocked,
                ConditionType::SpecValid,
                sanitize_message(message),
            ),
            types::error::Error::KmsMigrationBlocked { message, .. } => Self::blocked(
                Reason::KmsConfigInvalid,
                ConditionType::KmsReady,
                sanitize_message(message),
            ),
            types::error::Error::InvalidWorkloadSecurityProfile { message, .. } => Self::blocked(
                Reason::InvalidWorkloadSecurityProfile,
                ConditionType::SpecValid,
                sanitize_message(message),
            ),
            types::error::Error::WorkloadSecurityIncompatible { message, .. } => Self::blocked(
                Reason::WorkloadSecurityIncompatible,
                ConditionType::WorkloadsReady,
                sanitize_message(message),
            ),
            types::error::Error::NoNamespace => Self::transient(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                "Tenant namespace is not available".to_string(),
            ),
            types::error::Error::InternalError { msg } => Self::degraded(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                sanitize_message(msg),
            ),
            types::error::Error::SerdeJson { .. } => Self::degraded(
                Reason::KubernetesApiError,
                ConditionType::Ready,
                "Failed to serialize Kubernetes object".to_string(),
            ),
        }
    }

    pub fn statefulset_apply_failed(name: &str) -> Self {
        Self::degraded(
            Reason::StatefulSetApplyFailed,
            ConditionType::WorkloadsReady,
            format!("Failed to apply StatefulSet '{}'", name),
        )
    }

    pub fn statefulset_update_validation_failed(name: &str) -> Self {
        Self::blocked(
            Reason::StatefulSetUpdateValidationFailed,
            ConditionType::WorkloadsReady,
            format!("StatefulSet '{}' update validation failed", name),
        )
    }

    pub fn tls_blocked(reason: Reason, safe_message: String) -> Self {
        Self::blocked(reason, ConditionType::TlsReady, safe_message)
    }

    pub fn tls_reconciling(reason: Reason, safe_message: String) -> Self {
        Self::transient(reason, ConditionType::TlsReady, safe_message)
    }

    fn blocked(reason: Reason, condition_type: ConditionType, safe_message: String) -> Self {
        Self {
            reason,
            condition_type,
            impact: StatusImpact::UserBlocked,
            safe_message,
            event_type: EventType::Warning,
        }
    }

    fn degraded(reason: Reason, condition_type: ConditionType, safe_message: String) -> Self {
        Self {
            reason,
            condition_type,
            impact: StatusImpact::Degraded,
            safe_message,
            event_type: EventType::Warning,
        }
    }

    fn transient(reason: Reason, condition_type: ConditionType, safe_message: String) -> Self {
        Self {
            reason,
            condition_type,
            impact: StatusImpact::Transient,
            safe_message,
            event_type: EventType::Warning,
        }
    }
}

pub struct StatusBuilder {
    generation: Option<i64>,
    now: String,
    rpc_secret_configured: bool,
    next: Status,
}

impl StatusBuilder {
    pub fn from_tenant(tenant: &Tenant) -> Self {
        Self {
            generation: tenant.metadata.generation,
            now: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            rpc_secret_configured: tenant.spec.rpc_secret.is_some(),
            next: tenant.status.clone().unwrap_or_default(),
        }
    }

    pub fn set_pool_statuses(&mut self, pools: Vec<pool::Pool>) {
        self.next.available_replicas = pools.iter().filter_map(|pool| pool.ready_replicas).sum();
        self.next.pools = pools;
    }

    pub fn set_tls_status(&mut self, tls: certificate::TlsCertificateStatus) {
        let ready = tls.ready;
        self.next.certificates.tls = Some(tls);
        if ready {
            self.set_condition(
                ConditionType::TlsReady,
                ConditionStatus::True,
                Reason::TlsConfigured,
                "TLS is configured for RustFS workloads".to_string(),
            );
        }
    }

    pub fn set_provisioning_status(
        &mut self,
        provisioning: crate::types::v1alpha1::status::provisioning::ProvisioningStatus,
    ) {
        self.next.provisioning = provisioning;
    }

    pub fn mark_started(&mut self) {
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::False,
            Reason::ReconcileStarted,
            "Reconcile has started".to_string(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::True,
            Reason::ReconcileStarted,
            "Operator is reconciling the latest Tenant generation".to_string(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::False,
            Reason::ReconcileStarted,
            "No persistent degradation has been confirmed".to_string(),
        );
    }

    pub fn mark_error(&mut self, error: &StatusError) {
        self.clear_stale_blocked_conditions(
            error.condition_type,
            error.reason,
            &error.safe_message,
        );

        match error.impact {
            StatusImpact::UserBlocked => {
                self.set_condition(
                    ConditionType::Ready,
                    ConditionStatus::False,
                    error.reason,
                    error.safe_message.clone(),
                );
                self.set_condition(
                    ConditionType::Reconciling,
                    ConditionStatus::False,
                    error.reason,
                    "Reconcile is blocked by user-fixable configuration".to_string(),
                );
                self.set_condition(
                    ConditionType::Degraded,
                    ConditionStatus::True,
                    error.reason,
                    error.safe_message.clone(),
                );
                self.set_condition(
                    error.condition_type,
                    ConditionStatus::False,
                    error.reason,
                    error.safe_message.clone(),
                );
            }
            StatusImpact::Degraded => {
                self.finish_degraded(
                    error.reason,
                    error.condition_type,
                    error.safe_message.clone(),
                );
                self.set_condition(
                    error.condition_type,
                    ConditionStatus::False,
                    error.reason,
                    error.safe_message.clone(),
                );
            }
            StatusImpact::Transient => {
                self.set_condition(
                    ConditionType::Ready,
                    ConditionStatus::Unknown,
                    error.reason,
                    error.safe_message.clone(),
                );
                self.set_condition(
                    ConditionType::Reconciling,
                    ConditionStatus::True,
                    error.reason,
                    "Reconcile will retry after a Kubernetes API error".to_string(),
                );
                self.set_condition(
                    ConditionType::Degraded,
                    ConditionStatus::False,
                    error.reason,
                    "No persistent degradation has been confirmed".to_string(),
                );
                self.set_condition(
                    error.condition_type,
                    ConditionStatus::Unknown,
                    error.reason,
                    error.safe_message.clone(),
                );
            }
        }
    }

    pub fn finish_success(&mut self) {
        self.mark_default_components_ready();
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::True,
            Reason::ReconcileSucceeded,
            "Tenant is ready".to_string(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::False,
            Reason::ReconcileSucceeded,
            "Reconcile completed successfully".to_string(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::False,
            Reason::ReconcileSucceeded,
            "Tenant is not degraded".to_string(),
        );
    }

    pub fn finish_reconciling(&mut self, reason: Reason, message: String) {
        self.mark_default_components_ready();
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::False,
            reason,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::True,
            reason,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::False,
            reason,
            "Tenant is progressing".to_string(),
        );
        self.set_condition(
            ConditionType::WorkloadsReady,
            ConditionStatus::False,
            reason,
            message,
        );
    }

    pub fn finish_degraded(
        &mut self,
        reason: Reason,
        condition_type: ConditionType,
        message: String,
    ) {
        self.mark_default_components_ready();
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::False,
            reason,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::False,
            reason,
            "Reconcile is not actively progressing".to_string(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::True,
            reason,
            message.clone(),
        );
        self.set_condition(condition_type, ConditionStatus::False, reason, message);
    }

    pub fn finish_provisioning_ready(&mut self) {
        self.finish_success();
        self.set_condition(
            ConditionType::ProvisioningReady,
            ConditionStatus::True,
            Reason::ProvisioningConfigured,
            "Tenant provisioning is configured".to_string(),
        );
    }

    pub fn finish_provisioning_pending(&mut self, message: String) {
        self.mark_default_components_ready();
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::False,
            Reason::ProvisioningPending,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::True,
            Reason::ProvisioningPending,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::False,
            Reason::ProvisioningPending,
            "Tenant provisioning is progressing".to_string(),
        );
        self.set_condition(
            ConditionType::WorkloadsReady,
            ConditionStatus::True,
            Reason::ReconcileSucceeded,
            "WorkloadsReady is ready".to_string(),
        );
        self.set_condition(
            ConditionType::ProvisioningReady,
            ConditionStatus::False,
            Reason::ProvisioningPending,
            message,
        );
    }

    pub fn finish_provisioning_failed(&mut self, reason: Reason, message: String) {
        self.mark_default_components_ready();
        self.set_condition(
            ConditionType::Ready,
            ConditionStatus::False,
            reason,
            message.clone(),
        );
        self.set_condition(
            ConditionType::Reconciling,
            ConditionStatus::False,
            reason,
            "Reconcile is blocked by provisioning failure".to_string(),
        );
        self.set_condition(
            ConditionType::Degraded,
            ConditionStatus::True,
            reason,
            message.clone(),
        );
        self.set_condition(
            ConditionType::WorkloadsReady,
            ConditionStatus::True,
            Reason::ReconcileSucceeded,
            "WorkloadsReady is ready".to_string(),
        );
        self.set_condition(
            ConditionType::ProvisioningReady,
            ConditionStatus::False,
            reason,
            message,
        );
    }

    pub fn build(mut self) -> Status {
        self.next
            .remove_condition_by_type(LEGACY_PROGRESSING_CONDITION);
        self.next.observed_generation = self.generation;
        self.next.current_state = summarize_current_state(&self.next);
        self.next.sort_conditions();
        self.next
    }

    fn mark_default_components_ready(&mut self) {
        for condition_type in [
            ConditionType::SpecValid,
            ConditionType::CredentialsReady,
            ConditionType::KmsReady,
            ConditionType::PoolsReady,
            ConditionType::WorkloadsReady,
            ConditionType::ProvisioningReady,
        ] {
            self.set_condition(
                condition_type,
                ConditionStatus::True,
                Reason::ReconcileSucceeded,
                format!("{} is ready", condition_type.as_str()),
            );
        }

        if self.rpc_secret_configured {
            self.set_condition(
                ConditionType::RpcAuthReady,
                ConditionStatus::True,
                Reason::ReconcileSucceeded,
                "Configured RPC Secret is valid".to_string(),
            );
        } else {
            self.next
                .remove_condition_by_type(ConditionType::RpcAuthReady.as_str());
        }
    }

    fn clear_stale_blocked_conditions(
        &mut self,
        current_condition_type: ConditionType,
        reason: Reason,
        message: &str,
    ) {
        let current_type = current_condition_type.as_str();
        for condition in &mut self.next.conditions {
            if condition.type_ == current_type
                || is_summary_condition(&condition.type_)
                || condition.status != ConditionStatus::False.as_str()
                || !is_blocked_reason(&condition.reason)
            {
                continue;
            }

            if condition.status != ConditionStatus::Unknown.as_str() {
                condition.last_transition_time = Some(self.now.clone());
            }
            condition.status = ConditionStatus::Unknown.as_str().to_string();
            condition.reason = reason.as_str().to_string();
            condition.message = format!(
                "Condition was not confirmed during the current reconcile: {}",
                message
            );
            condition.observed_generation = self.generation;
        }
    }

    fn set_condition(
        &mut self,
        type_: ConditionType,
        status: ConditionStatus,
        reason: Reason,
        message: String,
    ) {
        self.next.upsert_condition(ConditionInput {
            type_,
            status,
            reason,
            message,
            observed_generation: self.generation,
            now: self.now.clone(),
        });
    }
}

fn is_summary_condition(type_: &str) -> bool {
    matches!(
        type_,
        "Ready" | "Reconciling" | "Degraded" | LEGACY_PROGRESSING_CONDITION
    )
}

fn sanitize_message(message: &str) -> String {
    redact_sensitive_pairs(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::status::Condition;

    #[test]
    fn user_ownership_provisioning_failures_block_top_level_status() {
        for reason in [
            Reason::UserOwnershipConflict,
            Reason::UserOwnershipCheckpointFailed,
        ] {
            let tenant = crate::tests::create_test_tenant(None, None);
            let mut builder = StatusBuilder::from_tenant(&tenant);
            builder.finish_provisioning_failed(reason, "ownership safety check failed".to_string());
            let status = builder.build();

            assert_eq!(status.current_state, "Blocked");
            assert_eq!(
                crate::types::v1alpha1::status::primary_condition(&status)
                    .map(|condition| condition.reason.as_str()),
                Some(reason.as_str())
            );
            assert_eq!(
                status
                    .condition(ConditionType::ProvisioningReady)
                    .map(|condition| condition.status.as_str()),
                Some("False")
            );
        }
    }

    #[test]
    fn status_builder_maps_credential_missing_key() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let err = context::Error::CredentialSecretMissingKey {
            secret_name: "creds".to_string(),
            key: "accesskey".to_string(),
        };

        let status_error = StatusError::from_context_error(&err);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        let condition = status.condition(ConditionType::CredentialsReady).unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "CredentialSecretMissingKey");
        assert_eq!(status.current_state, "Blocked");
        assert!(status.condition(ConditionType::KmsReady).is_none());
        assert!(status.condition(ConditionType::WorkloadsReady).is_none());
    }

    #[test]
    fn status_builder_maps_rpc_secret_invalid_value() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.rpc_secret = Some(crate::types::v1alpha1::tenant::RpcSecretRef {
            name: "rpc-auth".to_string(),
            key: "rpc-secret".to_string(),
        });
        let err = context::Error::RpcSecretInvalidValue {
            secret_name: "rpc-auth".to_string(),
            key: "rpc-secret".to_string(),
        };

        let status_error = StatusError::from_context_error(&err);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        let condition = status.condition(ConditionType::RpcAuthReady).unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "RpcSecretInvalidValue");
        assert_eq!(status.current_state, "Blocked");
        assert!(status.condition(ConditionType::CredentialsReady).is_none());
        assert!(status.condition(ConditionType::WorkloadsReady).is_none());
    }

    #[test]
    fn status_builder_blocks_incompatible_workload_security() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let err = types::error::Error::WorkloadSecurityIncompatible {
            name: tenant.name(),
            message: "upgrade the RustFS image".to_string(),
        };

        let status_error = StatusError::from_types_error(&err);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        let condition = status.condition(ConditionType::WorkloadsReady).unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "WorkloadSecurityIncompatible");
        assert_eq!(status.current_state, "Blocked");
        assert_eq!(
            crate::types::v1alpha1::status::next_actions_for_reason(&condition.reason),
            vec!["upgradeRustfsImage", "configureCompatibleSeccompProfile"]
        );
    }

    #[test]
    fn status_builder_marks_invalid_workload_security_profile_as_invalid_spec() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let err = types::error::Error::InvalidWorkloadSecurityProfile {
            name: tenant.name(),
            message: "spec.containerSecurityContext.appArmorProfile is invalid".to_string(),
        };

        let status_error = StatusError::from_types_error(&err);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        let condition = status.condition(ConditionType::SpecValid).unwrap();
        assert_eq!(condition.status, "False");
        assert_eq!(condition.reason, "InvalidWorkloadSecurityProfile");
        assert_eq!(status.current_state, "Blocked");
        assert_eq!(
            crate::types::v1alpha1::status::next_actions_for_reason(&condition.reason),
            vec!["fixWorkloadSecurityProfile"]
        );
    }

    #[test]
    fn successful_status_reports_rpc_auth_ready_only_for_managed_secret() {
        let mut unmanaged = crate::tests::create_test_tenant(None, None);
        unmanaged.spec.env.push(k8s_openapi::api::core::v1::EnvVar {
            name: "RUSTFS_RPC_SECRET".to_string(),
            value: Some("legacy-user-value".to_string()),
            ..Default::default()
        });
        let mut builder = StatusBuilder::from_tenant(&unmanaged);
        builder.finish_success();
        let status = builder.build();

        assert!(status.condition(ConditionType::RpcAuthReady).is_none());

        let mut managed = crate::tests::create_test_tenant(None, None);
        managed.spec.rpc_secret = Some(crate::types::v1alpha1::tenant::RpcSecretRef {
            name: "rpc-auth".to_string(),
            key: "rpc-secret".to_string(),
        });
        let mut builder = StatusBuilder::from_tenant(&managed);
        builder.finish_success();
        let status = builder.build();

        let condition = status.condition(ConditionType::RpcAuthReady).unwrap();
        assert_eq!(condition.status, "True");
        assert_eq!(condition.message, "Configured RPC Secret is valid");
    }

    #[test]
    fn successful_status_prunes_rpc_auth_ready_after_secret_is_unconfigured() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(2);
        tenant.status = Some(Status {
            observed_generation: Some(1),
            conditions: vec![condition(
                ConditionType::RpcAuthReady.as_str(),
                "True",
                "ReconcileSucceeded",
            )],
            ..Default::default()
        });

        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.finish_success();
        let status = builder.build();

        assert!(
            status
                .conditions
                .iter()
                .all(|condition| condition.type_ != ConditionType::RpcAuthReady.as_str())
        );
    }

    #[test]
    fn transition_time_is_preserved_when_status_does_not_change() {
        let mut status = Status {
            conditions: vec![Condition {
                type_: "CredentialsReady".to_string(),
                status: "False".to_string(),
                last_transition_time: Some("old".to_string()),
                observed_generation: Some(1),
                reason: "CredentialSecretNotFound".to_string(),
                message: "old".to_string(),
            }],
            ..Default::default()
        };

        status.upsert_condition(ConditionInput {
            type_: ConditionType::CredentialsReady,
            status: ConditionStatus::False,
            reason: Reason::CredentialSecretMissingKey,
            message: "new".to_string(),
            observed_generation: Some(2),
            now: "new-time".to_string(),
        });

        let condition = status.condition(ConditionType::CredentialsReady).unwrap();
        assert_eq!(condition.last_transition_time.as_deref(), Some("old"));
        assert_eq!(condition.reason, "CredentialSecretMissingKey");
        assert_eq!(condition.observed_generation, Some(2));
    }

    #[test]
    fn repeated_blocked_error_keeps_status_unchanged() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(1);
        let error = context::Error::CredentialSecretMissingKey {
            secret_name: "creds".to_string(),
            key: "accesskey".to_string(),
        };
        let status_error = StatusError::from_context_error(&error);

        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let mut first = builder.build();
        for condition in &mut first.conditions {
            condition.last_transition_time = Some("2026-01-01T00:00:00Z".to_string());
        }

        tenant.status = Some(first.clone());
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let second = builder.build();

        assert_eq!(second, first);
    }

    #[test]
    fn conditions_are_sorted_by_core_priority_then_name() {
        let mut status = Status {
            conditions: vec![
                condition("ZFeatureReady", "True", "ReconcileSucceeded"),
                condition("CredentialsReady", "True", "ReconcileSucceeded"),
                condition("Ready", "True", "ReconcileSucceeded"),
                condition("AFeatureReady", "True", "ReconcileSucceeded"),
                condition("Degraded", "False", "ReconcileSucceeded"),
            ],
            ..Default::default()
        };

        status.sort_conditions();
        let types: Vec<_> = status
            .conditions
            .iter()
            .map(|condition| condition.type_.as_str())
            .collect();

        assert_eq!(
            types,
            vec![
                "Ready",
                "Degraded",
                "CredentialsReady",
                "AFeatureReady",
                "ZFeatureReady"
            ]
        );
    }

    #[test]
    fn blocked_reason_wins_current_state_summary() {
        let status = Status {
            conditions: vec![
                condition("Reconciling", "True", "RolloutInProgress"),
                condition("CredentialsReady", "False", "CredentialSecretNotFound"),
                condition("Degraded", "True", "CredentialSecretNotFound"),
            ],
            ..Default::default()
        };

        assert_eq!(
            crate::types::v1alpha1::status::summarize_current_state(&status),
            "Blocked"
        );
        assert_eq!(
            crate::types::v1alpha1::status::primary_condition(&status)
                .map(|condition| condition.reason.as_str()),
            Some("CredentialSecretNotFound")
        );
    }

    #[test]
    fn transient_error_does_not_keep_old_blocked_component_primary() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(7);
        tenant.status = Some(Status {
            current_state: "Blocked".to_string(),
            available_replicas: 0,
            pools: Vec::new(),
            observed_generation: Some(7),
            conditions: vec![condition(
                "CredentialsReady",
                "False",
                "CredentialSecretNotFound",
            )],
            ..Default::default()
        });
        let status_error = StatusError {
            reason: Reason::KubernetesApiError,
            condition_type: ConditionType::Ready,
            impact: StatusImpact::Transient,
            safe_message: "Kubernetes API request failed".to_string(),
            event_type: EventType::Warning,
        };

        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        assert_eq!(status.current_state, "Reconciling");
        assert_eq!(
            crate::types::v1alpha1::status::primary_condition(&status)
                .map(|condition| condition.reason.as_str()),
            Some("KubernetesApiError")
        );
        assert_eq!(
            status
                .condition(ConditionType::CredentialsReady)
                .map(|condition| condition.status.as_str()),
            Some("Unknown")
        );
    }

    #[test]
    fn next_actions_are_registry_driven() {
        assert_eq!(
            crate::types::v1alpha1::status::next_actions_for_reason("PoolDeleteBlocked"),
            vec!["restorePoolSpec", "startDecommissionAfterRestore"]
        );
        assert!(
            crate::types::v1alpha1::status::next_actions_for_reason("UnknownReason").is_empty()
        );
    }

    #[test]
    fn degraded_status_targets_requested_component() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.finish_degraded(
            Reason::StatefulSetApplyFailed,
            ConditionType::WorkloadsReady,
            "failed".to_string(),
        );
        let status = builder.build();

        assert_eq!(
            status
                .condition(ConditionType::WorkloadsReady)
                .map(|condition| condition.status.as_str()),
            Some("False")
        );
        assert_eq!(
            status
                .condition(ConditionType::PoolsReady)
                .map(|condition| condition.status.as_str()),
            Some("True")
        );
    }

    #[test]
    fn mark_started_sets_reconciling_condition() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_started();
        let status = builder.build();

        assert_eq!(status.current_state, "Reconciling");
        assert_eq!(
            status
                .condition(ConditionType::Reconciling)
                .map(|condition| condition.reason.as_str()),
            Some("ReconcileStarted")
        );
    }

    #[test]
    fn status_builder_prunes_legacy_progressing_condition() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(1);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            observed_generation: Some(1),
            conditions: vec![
                condition("Ready", "True", "ReconcileSucceeded"),
                condition("Reconciling", "False", "ReconcileSucceeded"),
                condition("Degraded", "False", "ReconcileSucceeded"),
                condition("Progressing", "True", "RolloutInProgress"),
            ],
            ..Default::default()
        });

        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.finish_success();
        let status = builder.build();

        assert_eq!(status.current_state, "Ready");
        assert!(
            status
                .conditions
                .iter()
                .all(|condition| condition.type_ != "Progressing")
        );
        assert_eq!(
            status
                .condition(ConditionType::Reconciling)
                .map(|condition| condition.status.as_str()),
            Some("False")
        );
    }

    #[test]
    fn transient_error_does_not_keep_previous_blocked_condition_current() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(7);
        tenant.status = Some(Status {
            current_state: "Blocked".to_string(),
            observed_generation: Some(7),
            conditions: vec![
                condition("CredentialsReady", "False", "CredentialSecretNotFound"),
                condition("Degraded", "True", "CredentialSecretNotFound"),
            ],
            ..Default::default()
        });
        let status_error = StatusError {
            reason: Reason::KubernetesApiError,
            condition_type: ConditionType::Ready,
            impact: StatusImpact::Transient,
            safe_message: "Kubernetes API request failed".to_string(),
            event_type: EventType::Warning,
        };

        let mut builder = StatusBuilder::from_tenant(&tenant);
        builder.mark_error(&status_error);
        let status = builder.build();

        assert_eq!(status.current_state, "Reconciling");
        assert_eq!(
            crate::types::v1alpha1::status::primary_condition(&status)
                .map(|condition| condition.reason.as_str()),
            Some("KubernetesApiError")
        );
        assert_eq!(
            status
                .condition(ConditionType::CredentialsReady)
                .map(|condition| condition.status.as_str()),
            Some("Unknown")
        );
    }

    #[test]
    fn sanitize_message_preserves_required_key_names() {
        let message = "Vault backend requires kmsSecret referencing a Secret with key vault-token";

        assert_eq!(sanitize_message(message), message);
    }

    #[test]
    fn sanitize_message_redacts_colon_and_json_secret_values() {
        let message =
            "kms config token: tok_123 password: p@ss accesskey: AKIA_TEST secretkey: SK_TEST";

        let sanitized = sanitize_message(message);

        assert!(sanitized.contains("token"));
        assert!(sanitized.contains("password"));
        assert!(sanitized.contains("accesskey"));
        assert!(sanitized.contains("secretkey"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("p@ss"));
        assert!(!sanitized.contains("AKIA_TEST"));
        assert!(!sanitized.contains("SK_TEST"));
    }

    #[test]
    fn sanitize_message_redacts_json_key_value_pairs() {
        let message = "{\"accesskey\":\"AKIA_JSON\",\"secretkey\":\"SECRET_JSON\"}";

        let sanitized = sanitize_message(message);

        assert!(sanitized.contains("\"accesskey\""));
        assert!(sanitized.contains("\"secretkey\""));
        assert!(!sanitized.contains("AKIA_JSON"));
        assert!(!sanitized.contains("SECRET_JSON"));
    }

    #[test]
    fn sanitize_message_handles_unicode_without_panicking() {
        let message = "错误🔐 token: tok_123 用户=测试 secretkey: SK_TEST 完成";

        let sanitized = sanitize_message(message);

        assert!(sanitized.contains("错误🔐"));
        assert!(sanitized.contains("用户=测试"));
        assert!(sanitized.contains("完成"));
        assert!(sanitized.contains("token: <redacted>"));
        assert!(sanitized.contains("secretkey: <redacted>"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("SK_TEST"));
    }

    #[test]
    fn sanitize_message_redacts_unicode_quoted_values() {
        let message = "{\"说明\":\"🔐\",\"secretkey\":\"秘密值\"}";

        let sanitized = sanitize_message(message);

        assert!(sanitized.contains("\"说明\":\"🔐\""));
        assert!(sanitized.contains("\"secretkey\":\"<redacted>\""));
        assert!(!sanitized.contains("秘密值"));
    }

    #[test]
    fn sanitize_message_redacts_after_unicode_whitespace() {
        let message = "token:\u{3000}tok_123 secretkey:\u{2003}SK_TEST";

        let sanitized = sanitize_message(message);

        assert!(sanitized.contains("token:\u{3000}<redacted>"));
        assert!(sanitized.contains("secretkey:\u{2003}<redacted>"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("SK_TEST"));
    }

    fn condition(type_: &str, status: &str, reason: &str) -> Condition {
        Condition {
            type_: type_.to_string(),
            status: status.to_string(),
            last_transition_time: Some("now".to_string()),
            observed_generation: Some(1),
            reason: reason.to_string(),
            message: reason.to_string(),
        }
    }
}
