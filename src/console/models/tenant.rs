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

use crate::types::v1alpha1::{
    status::{
        ConditionStatus, ConditionType, CurrentState, Reason, Status, canonical_filter_state,
        canonical_state, next_actions_for_reason, primary_condition, summarize_current_state,
    },
    tenant::Tenant,
};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Single tenant row in a list view
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListItem {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub ready: bool,
    pub reconciling: bool,
    pub degraded: bool,
    pub primary_reason: Option<String>,
    pub generation: Option<i64>,
    pub observed_generation: Option<i64>,
    pub stale: bool,
    pub created_at: Option<String>,
}

/// Pool summary embedded in tenant list/detail
#[derive(Debug, Serialize, ToSchema)]
pub struct PoolInfo {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
}

/// Response listing tenants
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantListItem>,
}

/// Query parameters for listing tenants
#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct TenantListQuery {
    /// Filter by tenant state (case-insensitive)
    pub state: Option<String>,
}

/// Per-state tenant counts
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantStateCountsResponse {
    /// Total number of tenants
    pub total: u32,
    /// Counts keyed by state, e.g. Ready/Reconciling/Blocked/Degraded/NotReady/Unknown
    pub counts: std::collections::BTreeMap<String, u32>,
}

/// Full tenant detail for the UI
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantDetailsResponse {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<PoolInfo>,
    pub state: String,
    pub status_summary: TenantStatusSummary,
    pub conditions: Vec<TenantCondition>,
    pub next_actions: Vec<String>,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub created_at: Option<String>,
    pub services: Vec<ServiceInfo>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct TenantCondition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: String,
    pub message: String,
    pub last_transition_time: Option<String>,
    pub observed_generation: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct TenantStatusSummary {
    pub current_state: String,
    pub ready: bool,
    pub reconciling: bool,
    pub degraded: bool,
    pub primary_reason: Option<String>,
    pub primary_message: Option<String>,
    pub observed_generation: Option<i64>,
    pub stale: bool,
    pub next_actions: Vec<String>,
}

/// Exposed Service summary
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceInfo {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
}

/// Port mapping for a Service
#[derive(Debug, Serialize, ToSchema)]
pub struct ServicePort {
    pub name: String,
    pub port: i32,
    pub target_port: String,
}

/// SecurityContext for create/update (Pod runAsUser, runAsGroup, fsGroup, runAsNonRoot).
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSecurityContextRequest {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
}

/// Request body to create a tenant
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTenantRequest {
    pub name: String,
    pub namespace: String,
    pub pools: Vec<CreatePoolRequest>,
    pub image: Option<String>,
    pub mount_path: Option<String>,
    pub creds_secret: Option<String>,
    /// Optional Pod SecurityContext override (runAsUser, runAsGroup, fsGroup, runAsNonRoot).
    pub security_context: Option<CreateSecurityContextRequest>,
}

/// Pool spec embedded in create-tenant request
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePoolRequest {
    pub name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_size: String,
    pub storage_class: Option<String>,
}

/// Response after deleting a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteTenantResponse {
    pub success: bool,
    pub message: String,
}

/// Partial update payload for a tenant
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTenantRequest {
    /// New container image
    pub image: Option<String>,

    /// New volume mount path
    pub mount_path: Option<String>,

    /// Replace env vars
    pub env: Option<Vec<EnvVar>>,

    /// Reference to credentials Secret
    pub creds_secret: Option<String>,

    /// Pod management policy
    pub pod_management_policy: Option<String>,

    /// Image pull policy
    pub image_pull_policy: Option<String>,

    /// Logging sidecar / volume settings
    pub logging: Option<LoggingConfig>,
}

/// Key/value environment variable
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct EnvVar {
    pub name: String,
    pub value: Option<String>,
}

/// Tenant logging configuration
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    pub log_type: String, // "stdout" | "emptyDir" | "persistent"
    pub volume_size: Option<String>,
    pub storage_class: Option<String>,
}

/// Response after updating a tenant
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateTenantResponse {
    pub success: bool,
    pub message: String,
    pub tenant: TenantListItem,
}

/// Raw Tenant manifest get/update payload
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TenantYAML {
    pub yaml: String,
}

pub fn tenant_status_summary(tenant: &Tenant) -> TenantStatusSummary {
    let status = tenant.status.as_ref();
    let generation = tenant.metadata.generation;
    let observed_generation = status.and_then(|status| status.observed_generation);
    let stale = match (observed_generation, generation) {
        (Some(observed), Some(generation)) => observed < generation,
        (None, Some(_)) => true,
        _ => false,
    };

    let has_conditions = status.is_some_and(|status| !status.conditions.is_empty());
    let derived_state = status
        .map(|status| {
            if has_conditions {
                summarize_current_state(status)
            } else {
                status_current_state(status)
            }
        })
        .unwrap_or_else(|| CurrentState::Unknown.as_str().to_string());

    let mut current_state = derived_state;
    let mut ready = if has_conditions {
        condition_is_true(status, ConditionType::Ready.as_str())
    } else {
        legacy_ready_for_state(&current_state)
    };
    let mut reconciling = if has_conditions {
        condition_is_true(status, ConditionType::Reconciling.as_str())
            || condition_is_true(status, "Progressing")
    } else {
        legacy_reconciling_for_state(&current_state)
    };
    let degraded = if has_conditions {
        condition_is_true(status, ConditionType::Degraded.as_str())
    } else {
        legacy_degraded_for_state(&current_state)
    };

    let primary = status.and_then(primary_condition);
    let mut primary_reason = primary.map(|condition| condition.reason.clone());
    let mut primary_message = primary.map(|condition| condition.message.clone());

    if stale {
        current_state = CurrentState::Reconciling.as_str().to_string();
        ready = false;
        reconciling = true;
        primary_reason = Some(Reason::ObservedGenerationStale.as_str().to_string());
        primary_message =
            Some("Tenant spec changed and has not been observed by the operator yet".to_string());
    }

    let next_actions = primary_reason
        .as_deref()
        .map(next_actions_for_reason)
        .unwrap_or_default()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();

    TenantStatusSummary {
        current_state,
        ready,
        reconciling,
        degraded,
        primary_reason,
        primary_message,
        observed_generation,
        stale,
        next_actions,
    }
}

pub fn tenant_conditions(tenant: &Tenant) -> Vec<TenantCondition> {
    tenant
        .status
        .as_ref()
        .map(|status| {
            status
                .conditions
                .iter()
                .map(|condition| TenantCondition {
                    type_: condition.type_.clone(),
                    status: condition.status.clone(),
                    reason: condition.reason.clone(),
                    message: condition.message.clone(),
                    last_transition_time: condition.last_transition_time.clone(),
                    observed_generation: condition.observed_generation,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn tenant_to_list_item(tenant: Tenant) -> TenantListItem {
    let summary = tenant_status_summary(&tenant);
    TenantListItem {
        name: tenant.name_any(),
        namespace: tenant.namespace().unwrap_or_default(),
        pools: tenant
            .spec
            .pools
            .iter()
            .map(|p| PoolInfo {
                name: p.name.clone(),
                servers: p.servers,
                volumes_per_server: p.persistence.volumes_per_server,
            })
            .collect(),
        state: summary.current_state,
        ready: summary.ready,
        reconciling: summary.reconciling,
        degraded: summary.degraded,
        primary_reason: summary.primary_reason,
        generation: tenant.metadata.generation,
        observed_generation: summary.observed_generation,
        stale: summary.stale,
        created_at: tenant
            .metadata
            .creation_timestamp
            .map(|ts| ts.0.to_rfc3339()),
    }
}

pub fn canonical_console_state(state: Option<&str>) -> String {
    canonical_state(state)
}

pub fn canonical_console_state_filter(state: Option<&str>) -> Option<String> {
    canonical_filter_state(state)
}

fn status_current_state(status: &Status) -> String {
    if status.current_state.is_empty() {
        summarize_current_state(status)
    } else {
        canonical_console_state(Some(&status.current_state))
    }
}

fn legacy_ready_for_state(state: &str) -> bool {
    canonical_console_state(Some(state)) == CurrentState::Ready.as_str()
}

fn legacy_reconciling_for_state(state: &str) -> bool {
    canonical_console_state(Some(state)) == CurrentState::Reconciling.as_str()
}

fn legacy_degraded_for_state(state: &str) -> bool {
    matches!(
        canonical_console_state(Some(state)).as_str(),
        "Blocked" | "Degraded"
    )
}

fn condition_is_true(status: Option<&Status>, type_: &str) -> bool {
    status.is_some_and(|status| {
        status
            .condition_by_type(type_)
            .is_some_and(|condition| condition.status == ConditionStatus::True.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::status::{Condition, Status};

    #[test]
    fn tenant_summary_prefers_blocked_reason_and_actions() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(7);
        tenant.status = Some(Status {
            current_state: "Blocked".to_string(),
            available_replicas: 0,
            pools: Vec::new(),
            observed_generation: Some(7),
            conditions: vec![
                condition("Reconciling", "True", "RolloutInProgress"),
                condition("CredentialsReady", "False", "CredentialSecretNotFound"),
                condition("Degraded", "True", "CredentialSecretNotFound"),
            ],
        });

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Blocked");
        assert_eq!(
            summary.primary_reason.as_deref(),
            Some("CredentialSecretNotFound")
        );
        assert_eq!(summary.next_actions, vec!["createCredentialSecret"]);
        assert!(!summary.stale);
    }

    #[test]
    fn tenant_summary_detects_stale_generation_as_reconciling() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(8);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            available_replicas: 4,
            pools: Vec::new(),
            observed_generation: Some(7),
            conditions: vec![
                condition("Ready", "True", "ReconcileSucceeded"),
                condition("Degraded", "False", "ReconcileSucceeded"),
            ],
        });

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Reconciling");
        assert!(!summary.ready);
        assert!(summary.reconciling);
        assert!(summary.stale);
        assert_eq!(
            summary.primary_reason.as_deref(),
            Some("ObservedGenerationStale")
        );
        assert_eq!(summary.next_actions, vec!["waitForReconcile"]);
    }

    #[test]
    fn tenant_summary_treats_missing_observed_generation_as_stale() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(9);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            available_replicas: 4,
            pools: Vec::new(),
            observed_generation: None,
            conditions: vec![condition("Ready", "True", "ReconcileSucceeded")],
        });

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Reconciling");
        assert!(summary.stale);
        assert!(summary.reconciling);
        assert!(!summary.ready);
        assert_eq!(
            summary.primary_reason.as_deref(),
            Some("ObservedGenerationStale")
        );
        assert_eq!(summary.next_actions, vec!["waitForReconcile"]);
    }

    #[test]
    fn tenant_summary_treats_missing_status_as_stale() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(2);
        tenant.status = None;

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Reconciling");
        assert!(summary.stale);
        assert!(summary.reconciling);
        assert!(!summary.ready);
        assert_eq!(
            summary.primary_reason.as_deref(),
            Some("ObservedGenerationStale")
        );
    }

    #[test]
    fn tenant_summary_uses_legacy_current_state_when_conditions_are_missing() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(3);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            observed_generation: Some(3),
            ..Default::default()
        });

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Ready");
        assert!(summary.ready);
        assert!(!summary.reconciling);
        assert!(!summary.degraded);
    }

    #[test]
    fn tenant_summary_prefers_conditions_over_legacy_current_state() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.generation = Some(3);
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            observed_generation: Some(3),
            conditions: vec![condition("Reconciling", "True", "RolloutInProgress")],
            ..Default::default()
        });

        let summary = tenant_status_summary(&tenant);

        assert_eq!(summary.current_state, "Reconciling");
        assert!(!summary.ready);
        assert!(summary.reconciling);
    }

    #[test]
    fn canonical_console_state_accepts_legacy_updating() {
        assert_eq!(
            canonical_console_state(Some("Updating")),
            "Reconciling".to_string()
        );
        assert_eq!(
            canonical_console_state(Some("Initialized")),
            "Ready".to_string()
        );
    }

    fn condition(type_: &str, status: &str, reason: &str) -> Condition {
        Condition {
            type_: type_.to_string(),
            status: status.to_string(),
            last_transition_time: Some("now".to_string()),
            observed_generation: None,
            reason: reason.to_string(),
            message: reason.to_string(),
        }
    }
}
