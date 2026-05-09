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
    ConditionInput, ConditionStatus, ConditionType, Reason, Status, is_blocked_reason, pool,
    summarize_current_state,
};
use crate::types::v1alpha1::tenant::Tenant;
use kube::runtime::events::EventType;

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
    next: Status,
}

impl StatusBuilder {
    pub fn from_tenant(tenant: &Tenant) -> Self {
        Self {
            generation: tenant.metadata.generation,
            now: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            next: tenant.status.clone().unwrap_or_default(),
        }
    }

    pub fn set_pool_statuses(&mut self, pools: Vec<pool::Pool>) {
        self.next.available_replicas = pools.iter().filter_map(|pool| pool.ready_replicas).sum();
        self.next.pools = pools;
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

    pub fn build(mut self) -> Status {
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
        ] {
            self.set_condition(
                condition_type,
                ConditionStatus::True,
                Reason::ReconcileSucceeded,
                format!("{} is ready", condition_type.as_str()),
            );
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

fn sanitize_message(message: &str) -> String {
    redact_sensitive_pairs(message)
}

fn redact_sensitive_pairs(message: &str) -> String {
    const SENSITIVE_KEYS: [&str; 4] = ["token", "password", "accesskey", "secretkey"];

    fn is_sensitive_key(key: &str) -> bool {
        matches!(key, "token" | "password" | "accesskey" | "secretkey")
    }

    fn normalize_key(raw: &str) -> String {
        raw.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
            .to_ascii_lowercase()
    }

    fn parse_value_end(input: &str, start: usize) -> usize {
        let bytes = input.as_bytes();
        if start >= bytes.len() {
            return start;
        }
        let first = bytes[start];
        if first == b'"' || first == b'\'' {
            let quote = first;
            let mut index = start + 1;
            while index < bytes.len() {
                if bytes[index] == quote && bytes.get(index.wrapping_sub(1)) != Some(&b'\\') {
                    return index + 1;
                }
                index += 1;
            }
            return bytes.len();
        }

        let mut index = start;
        while index < bytes.len() {
            let ch = bytes[index] as char;
            if ch.is_whitespace() || matches!(ch, ',' | ';' | '}' | ']' | ')') {
                break;
            }
            index += 1;
        }
        index
    }

    fn redacted_value(original: &str) -> String {
        if original.len() >= 2 {
            let bytes = original.as_bytes();
            let first = bytes[0];
            let last = bytes[bytes.len() - 1];
            if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
                let quote = first as char;
                return format!("{quote}<redacted>{quote}");
            }
        }
        "<redacted>".to_string()
    }

    let bytes = message.as_bytes();
    let mut output = String::with_capacity(message.len());
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let mut matched = false;

        for key in SENSITIVE_KEYS {
            let key_len = key.len();

            let unquoted_match = cursor + key_len <= bytes.len()
                && message[cursor..cursor + key_len].eq_ignore_ascii_case(key);
            let quoted_match = cursor + key_len + 2 <= bytes.len()
                && matches!(bytes[cursor] as char, '"' | '\'')
                && bytes[cursor + key_len + 1] == bytes[cursor]
                && message[cursor + 1..cursor + 1 + key_len].eq_ignore_ascii_case(key);

            let (key_start, key_end, cursor_after_key) = if unquoted_match {
                if cursor > 0 {
                    let prev = bytes[cursor - 1] as char;
                    if prev.is_ascii_alphanumeric() || prev == '_' || prev == '-' {
                        continue;
                    }
                }
                (cursor, cursor + key_len, cursor + key_len)
            } else if quoted_match {
                let key_start = cursor + 1;
                (key_start, key_start + key_len, key_start + key_len + 1)
            } else {
                continue;
            };

            let candidate = &message[key_start..key_end];

            let mut sep_index = cursor_after_key;
            while sep_index < bytes.len() && (bytes[sep_index] as char).is_whitespace() {
                sep_index += 1;
            }
            if sep_index >= bytes.len() || !matches!(bytes[sep_index] as char, '=' | ':') {
                continue;
            }

            let mut value_start = sep_index + 1;
            while value_start < bytes.len() && (bytes[value_start] as char).is_whitespace() {
                value_start += 1;
            }
            let value_end = parse_value_end(message, value_start);
            if value_end <= value_start {
                continue;
            }

            let normalized = normalize_key(candidate);
            if !is_sensitive_key(&normalized) {
                continue;
            }

            output.push_str(&message[cursor..value_start]);
            output.push_str(&redacted_value(&message[value_start..value_end]));
            cursor = value_end;
            matched = true;
            break;
        }

        if !matched {
            output.push(bytes[cursor] as char);
            cursor += 1;
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::status::Condition;

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
