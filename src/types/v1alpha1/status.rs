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
pub mod certificate;
pub mod pool;
pub mod state;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConditionType {
    Ready,
    Reconciling,
    Degraded,
    SpecValid,
    CredentialsReady,
    KmsReady,
    PoolsReady,
    WorkloadsReady,
}

impl ConditionType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Reconciling => "Reconciling",
            Self::Degraded => "Degraded",
            Self::SpecValid => "SpecValid",
            Self::CredentialsReady => "CredentialsReady",
            Self::KmsReady => "KmsReady",
            Self::PoolsReady => "PoolsReady",
            Self::WorkloadsReady => "WorkloadsReady",
        }
    }

    fn priority(type_: &str) -> Option<usize> {
        [
            Self::Ready,
            Self::Reconciling,
            Self::Degraded,
            Self::SpecValid,
            Self::CredentialsReady,
            Self::KmsReady,
            Self::PoolsReady,
            Self::WorkloadsReady,
        ]
        .iter()
        .position(|condition_type| condition_type.as_str() == type_)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConditionStatus {
    True,
    False,
    Unknown,
}

impl ConditionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::True => "True",
            Self::False => "False",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurrentState {
    Ready,
    Reconciling,
    Blocked,
    Degraded,
    NotReady,
    Unknown,
}

impl CurrentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Reconciling => "Reconciling",
            Self::Blocked => "Blocked",
            Self::Degraded => "Degraded",
            Self::NotReady => "NotReady",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    ReconcileStarted,
    ReconcileSucceeded,
    InvalidTenantName,
    ImmutableFieldModified,
    CredentialSecretNotFound,
    CredentialSecretMissingKey,
    CredentialSecretInvalidEncoding,
    CredentialSecretTooShort,
    KmsSecretNotFound,
    KmsSecretMissingKey,
    KmsConfigInvalid,
    PoolDeleteBlocked,
    StatefulSetApplyFailed,
    StatefulSetUpdateValidationFailed,
    RolloutInProgress,
    PodsNotReady,
    PoolDegraded,
    KubernetesApiError,
    StatusPatchFailed,
    ObservedGenerationStale,
}

impl Reason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReconcileStarted => "ReconcileStarted",
            Self::ReconcileSucceeded => "ReconcileSucceeded",
            Self::InvalidTenantName => "InvalidTenantName",
            Self::ImmutableFieldModified => "ImmutableFieldModified",
            Self::CredentialSecretNotFound => "CredentialSecretNotFound",
            Self::CredentialSecretMissingKey => "CredentialSecretMissingKey",
            Self::CredentialSecretInvalidEncoding => "CredentialSecretInvalidEncoding",
            Self::CredentialSecretTooShort => "CredentialSecretTooShort",
            Self::KmsSecretNotFound => "KmsSecretNotFound",
            Self::KmsSecretMissingKey => "KmsSecretMissingKey",
            Self::KmsConfigInvalid => "KmsConfigInvalid",
            Self::PoolDeleteBlocked => "PoolDeleteBlocked",
            Self::StatefulSetApplyFailed => "StatefulSetApplyFailed",
            Self::StatefulSetUpdateValidationFailed => "StatefulSetUpdateValidationFailed",
            Self::RolloutInProgress => "RolloutInProgress",
            Self::PodsNotReady => "PodsNotReady",
            Self::PoolDegraded => "PoolDegraded",
            Self::KubernetesApiError => "KubernetesApiError",
            Self::StatusPatchFailed => "StatusPatchFailed",
            Self::ObservedGenerationStale => "ObservedGenerationStale",
        }
    }
}

pub struct ConditionInput {
    pub type_: ConditionType,
    pub status: ConditionStatus,
    pub reason: Reason,
    pub message: String,
    pub observed_generation: Option<i64>,
    pub now: String,
}

/// Kubernetes standard condition for Tenant resources
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    /// Type of condition (Ready, Progressing, Degraded)
    #[serde(rename = "type")]
    pub type_: String,

    /// Status of the condition (True, False, Unknown)
    pub status: String,

    /// Last time the condition transitioned from one status to another
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,

    /// The generation of the Tenant resource that this condition reflects
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// One-word CamelCase reason for the condition's last transition
    pub reason: String,

    /// Human-readable message indicating details about the transition
    pub message: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current_state: String,

    pub available_replicas: i32,

    pub pools: Vec<pool::Pool>,

    /// The generation observed by the operator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// Kubernetes standard conditions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    // pub certificates: certificate::Status,
}

impl Status {
    pub fn upsert_condition(&mut self, input: ConditionInput) {
        let type_ = input.type_.as_str();
        if let Some(condition) = self
            .conditions
            .iter_mut()
            .find(|condition| condition.type_ == type_)
        {
            if condition.status != input.status.as_str() {
                condition.last_transition_time = Some(input.now);
            }
            condition.status = input.status.as_str().to_string();
            condition.reason = input.reason.as_str().to_string();
            condition.message = input.message;
            condition.observed_generation = input.observed_generation;
        } else {
            self.conditions.push(Condition {
                type_: type_.to_string(),
                status: input.status.as_str().to_string(),
                last_transition_time: Some(input.now),
                observed_generation: input.observed_generation,
                reason: input.reason.as_str().to_string(),
                message: input.message,
            });
        }
        self.sort_conditions();
    }

    pub fn condition(&self, type_: ConditionType) -> Option<&Condition> {
        self.condition_by_type(type_.as_str())
    }

    pub fn condition_by_type(&self, type_: &str) -> Option<&Condition> {
        self.conditions.iter().find(|condition| {
            condition.type_ == type_ && condition_matches_observed_generation(self, condition)
        })
    }

    pub fn sort_conditions(&mut self) {
        self.conditions.sort_by(|left, right| {
            match (
                ConditionType::priority(&left.type_),
                ConditionType::priority(&right.type_),
            ) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.type_.cmp(&right.type_),
            }
        });
    }

    pub fn condition_is_true(&self, type_: ConditionType) -> bool {
        self.condition(type_)
            .is_some_and(|condition| condition.status == ConditionStatus::True.as_str())
    }

    pub fn condition_is_false(&self, type_: ConditionType) -> bool {
        self.condition(type_)
            .is_some_and(|condition| condition.status == ConditionStatus::False.as_str())
    }
}

pub fn canonical_state(state: Option<&str>) -> String {
    canonical_known_state(state)
        .unwrap_or(CurrentState::Unknown.as_str())
        .to_string()
}

pub fn canonical_filter_state(state: Option<&str>) -> Option<String> {
    canonical_known_state(state).map(ToOwned::to_owned)
}

fn canonical_known_state(state: Option<&str>) -> Option<&'static str> {
    let normalized = state?
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "");
    match normalized.as_str() {
        "ready" | "initialized" | "running" => Some(CurrentState::Ready.as_str()),
        "reconciling" | "updating" | "creating" | "progressing" | "provisioning" => {
            Some(CurrentState::Reconciling.as_str())
        }
        "blocked" => Some(CurrentState::Blocked.as_str()),
        "degraded" => Some(CurrentState::Degraded.as_str()),
        "notready" | "failed" | "error" => Some(CurrentState::NotReady.as_str()),
        "unknown" | "stopped" => Some(CurrentState::Unknown.as_str()),
        _ => None,
    }
}

pub fn summarize_current_state(status: &Status) -> String {
    if status.condition_is_true(ConditionType::Ready)
        && !status.condition_is_true(ConditionType::Degraded)
        && !status.condition_is_true(ConditionType::Reconciling)
    {
        return CurrentState::Ready.as_str().to_string();
    }

    if has_blocked_condition(status) {
        return CurrentState::Blocked.as_str().to_string();
    }

    if status.condition_is_true(ConditionType::Reconciling) {
        return CurrentState::Reconciling.as_str().to_string();
    }

    if status.condition_is_true(ConditionType::Degraded) {
        return CurrentState::Degraded.as_str().to_string();
    }

    if status.condition_is_false(ConditionType::Ready) {
        return CurrentState::NotReady.as_str().to_string();
    }

    CurrentState::Unknown.as_str().to_string()
}

pub fn primary_condition(status: &Status) -> Option<&Condition> {
    status
        .conditions
        .iter()
        .find(|condition| {
            condition_matches_observed_generation(status, condition)
                && condition.status == ConditionStatus::False.as_str()
                && is_blocked_reason(&condition.reason)
        })
        .or_else(|| {
            status.conditions.iter().find(|condition| {
                condition_matches_observed_generation(status, condition)
                    && condition.type_ == ConditionType::Degraded.as_str()
                    && condition.status == ConditionStatus::True.as_str()
            })
        })
        .or_else(|| {
            status.conditions.iter().find(|condition| {
                condition_matches_observed_generation(status, condition)
                    && condition.type_ == ConditionType::Reconciling.as_str()
                    && condition.status == ConditionStatus::True.as_str()
            })
        })
        .or_else(|| {
            status.conditions.iter().find(|condition| {
                condition_matches_observed_generation(status, condition)
                    && condition.type_ == ConditionType::Ready.as_str()
                    && condition.status == ConditionStatus::False.as_str()
            })
        })
}

pub fn is_blocked_reason(reason: &str) -> bool {
    matches!(
        reason,
        "InvalidTenantName"
            | "ImmutableFieldModified"
            | "CredentialSecretNotFound"
            | "CredentialSecretMissingKey"
            | "CredentialSecretInvalidEncoding"
            | "CredentialSecretTooShort"
            | "KmsSecretNotFound"
            | "KmsSecretMissingKey"
            | "KmsConfigInvalid"
            | "PoolDeleteBlocked"
            | "StatefulSetUpdateValidationFailed"
    )
}

fn has_blocked_condition(status: &Status) -> bool {
    status.conditions.iter().any(|condition| {
        condition_matches_observed_generation(status, condition)
            && condition.status == ConditionStatus::False.as_str()
            && is_blocked_reason(&condition.reason)
    })
}

fn condition_matches_observed_generation(status: &Status, condition: &Condition) -> bool {
    match (status.observed_generation, condition.observed_generation) {
        (Some(status_generation), Some(condition_generation)) => {
            condition_generation == status_generation
        }
        _ => true,
    }
}

pub fn next_actions_for_reason(reason: &str) -> Vec<&'static str> {
    match reason {
        "CredentialSecretNotFound" => vec!["createCredentialSecret"],
        "CredentialSecretMissingKey" => vec!["addRequiredSecretKey"],
        "CredentialSecretInvalidEncoding" => vec!["replaceSecretValueWithUtf8"],
        "CredentialSecretTooShort" => vec!["rotateCredentialSecret"],
        "KmsSecretNotFound" => vec!["createKmsSecret"],
        "KmsSecretMissingKey" => vec!["addRequiredKmsSecretKey"],
        "KmsConfigInvalid" => vec!["fixKmsConfig"],
        "InvalidTenantName" => vec!["renameTenant"],
        "ImmutableFieldModified" => vec!["restoreImmutableField"],
        "PoolDeleteBlocked" => vec!["restorePoolSpec", "startDecommissionAfterRestore"],
        "DecommissionRequired" => vec!["startDecommission", "inspectPoolStatus"],
        "StatefulSetUpdateValidationFailed" => vec!["restoreImmutableField"],
        "StatefulSetApplyFailed" => vec!["retry", "inspectOperatorLogs"],
        "RolloutInProgress" => vec!["waitForRollout"],
        "PodsNotReady" => vec!["inspectPods", "inspectEvents"],
        "PoolDegraded" => vec![
            "inspectPools",
            "inspectPods",
            "inspectEvents",
            "inspectOperatorLogs",
        ],
        "KubernetesApiError" => vec!["retry", "inspectOperatorLogs"],
        "ObservedGenerationStale" => vec!["waitForReconcile"],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_ignores_conditions_from_older_observed_generation() {
        let status = Status {
            current_state: "Blocked".to_string(),
            observed_generation: Some(2),
            conditions: vec![
                condition(
                    "CredentialsReady",
                    "False",
                    "CredentialSecretNotFound",
                    Some(1),
                ),
                condition("Reconciling", "True", "KubernetesApiError", Some(2)),
                condition("Ready", "Unknown", "KubernetesApiError", Some(2)),
            ],
            ..Default::default()
        };

        assert_eq!(summarize_current_state(&status), "Reconciling");
        assert_eq!(
            primary_condition(&status).map(|condition| condition.reason.as_str()),
            Some("KubernetesApiError")
        );
    }

    #[test]
    fn canonical_state_accepts_case_insensitive_aliases() {
        assert_eq!(canonical_state(Some("ready")), "Ready");
        assert_eq!(canonical_state(Some("Updating")), "Reconciling");
        assert_eq!(canonical_state(Some("creating")), "Reconciling");
    }

    #[test]
    fn next_actions_cover_degraded_and_stale_statuses() {
        assert_eq!(
            next_actions_for_reason("PoolDegraded"),
            vec![
                "inspectPools",
                "inspectPods",
                "inspectEvents",
                "inspectOperatorLogs"
            ]
        );
        assert_eq!(
            next_actions_for_reason("ObservedGenerationStale"),
            vec!["waitForReconcile"]
        );
        assert_eq!(
            next_actions_for_reason("DecommissionRequired"),
            vec!["startDecommission", "inspectPoolStatus"]
        );
    }

    fn condition(
        type_: &str,
        status: &str,
        reason: &str,
        observed_generation: Option<i64>,
    ) -> Condition {
        Condition {
            type_: type_.to_string(),
            status: status.to_string(),
            last_transition_time: Some("now".to_string()),
            observed_generation,
            reason: reason.to_string(),
            message: reason.to_string(),
        }
    }
}
