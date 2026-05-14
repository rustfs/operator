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

use crate::types::v1alpha1::encryption::{EncryptionConfig, PodSecurityContextOverride};
use crate::types::v1alpha1::k8s;
use crate::types::v1alpha1::logging::LoggingConfig;
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::tls::TlsConfig;
use crate::types::{self, error::NoNamespaceSnafu};
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::{CustomResource, KubeSchema, Resource, ResourceExt};
use serde::{Deserialize, Serialize};
use snafu::OptionExt;

// Submodules for resource factory methods
mod helper;
mod rbac;
mod services;
mod workloads;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "Tenant",
    namespaced,
    status = "crate::types::v1alpha1::status::Status",
    shortname = "tenant",
    plural = "tenants",
    singular = "tenant",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.currentState"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    crates(serde_json = "k8s_openapi::serde_json")
)]
#[serde(rename_all = "camelCase")]
pub struct TenantSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,

    #[x_kube(validation = Rule::new("self.size() > 0").message("pools must be configured"))]
    pub pools: Vec<Pool>,

    #[serde(
        default = "helper::get_rustfs_image",
        skip_serializing_if = "Option::is_none"
    )]
    pub image: Option<String>,

    #[serde(
        default = "helper::get_rustfs_mount_path",
        skip_serializing_if = "Option::is_none"
    )]
    pub mount_path: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pull_secret: Option<corev1::LocalObjectReference>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_management_policy: Option<k8s::PodManagementPolicy>,

    /// Controls how the operator handles Pods when the node hosting them is down (NotReady/Unknown).
    ///
    /// Typical use-case: a StatefulSet Pod gets stuck in Terminating when the node goes down.
    /// Setting this to `ForceDelete` allows the operator to force delete the Pod object so the
    /// StatefulSet controller can recreate it elsewhere.
    ///
    /// Values: DoNothing | Delete | ForceDelete
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_deletion_policy_when_node_is_down: Option<k8s::PodDeletionPolicyWhenNodeIsDown>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<corev1::EnvVar>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub request_auto_cert: Option<bool>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub cert_expiry_alert_threshold: Option<i32>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub liveness: Option<corev1::Probe>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub readiness: Option<corev1::Probe>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub startup: Option<corev1::Probe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // features: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // cert_config: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // kes: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // prometheus_operator: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // prometheus_operator_scrape_metrics_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_service_account_rbac: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pull_policy: Option<k8s::ImagePullPolicy>,

    /// Logging configuration for RustFS
    ///
    /// Controls how RustFS outputs logs. Defaults to stdout (cloud-native best practice).
    /// Can also configure emptyDir (temporary) or persistent (PVC-backed) logging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingConfig>,

    // // #[serde(default, skip_serializing_if = "Option::is_none")]
    // // pub side_cars: Option<SideCars>,
    /// Optional reference to a Secret containing RustFS credentials.
    /// The Secret must contain 'accesskey' and 'secretkey' keys (both required, minimum 8 characters each).
    /// If not specified, credentials can be provided via environment variables in 'env'.
    /// Priority: Secret credentials > Environment variables > RustFS built-in defaults.
    /// For production use, always configure credentials via Secret or environment variables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creds_secret: Option<corev1::LocalObjectReference>,

    /// Encryption / KMS configuration for server-side encryption.
    /// When enabled, the operator injects KMS environment variables and mounts
    /// secrets into RustFS pods so the in-process `rustfs-kms` library is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<EncryptionConfig>,

    /// Override the default Pod SecurityContext (runAsUser/runAsGroup/fsGroup = 10001).
    /// Applies to all RustFS pods in this Tenant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_context: Option<PodSecurityContextOverride>,
}

impl Tenant {
    pub fn namespace(&self) -> Result<String, types::error::Error> {
        ResourceExt::namespace(self).context(NoNamespaceSnafu)
    }

    pub fn name(&self) -> String {
        ResourceExt::name_any(self)
    }

    /// Validate the tenant name conforms to DNS-1035 label rules.
    /// Kubernetes Services derived from the tenant name (e.g. `{name}-io`)
    /// require DNS-1035 compliance: lowercase alphanumeric or '-',
    /// must start with a letter, end with an alphanumeric, max 63 chars.
    pub fn validate_name(&self) -> Result<(), types::error::Error> {
        validate_dns1035_label(&self.name())
    }

    /// a new owner reference for tenant
    pub fn new_owner_ref(&self) -> metav1::OwnerReference {
        metav1::OwnerReference {
            api_version: Self::api_version(&()).to_string(),
            kind: Self::kind(&()).to_string(),
            name: self.name(),
            uid: self.meta().uid.clone().unwrap_or_default(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }
    }

    pub(crate) fn headless_service_name(&self) -> String {
        format!("{}-hl", self.name())
    }

    pub fn service_account_name(&self) -> String {
        self.spec
            .service_account_name
            .clone()
            .unwrap_or_else(|| format!("{}-sa", self.name()))
    }

    /// Returns common labels that should be applied to all Tenant-owned resources.
    /// These labels follow Kubernetes recommended label conventions.
    pub(crate) fn common_labels(&self) -> std::collections::BTreeMap<String, String> {
        [
            ("app.kubernetes.io/name".to_owned(), "rustfs".to_owned()),
            ("app.kubernetes.io/instance".to_owned(), self.name()),
            (
                "app.kubernetes.io/managed-by".to_owned(),
                "rustfs-operator".to_owned(),
            ),
            ("rustfs.tenant".to_owned(), self.name()),
        ]
        .into_iter()
        .collect()
    }

    /// Returns labels for pool-specific resources (StatefulSets, PVCs).
    /// Includes common labels plus pool-specific labels.
    pub(crate) fn pool_labels(&self, pool: &Pool) -> std::collections::BTreeMap<String, String> {
        let mut labels = self.common_labels();
        labels.insert("rustfs.pool".to_owned(), pool.name.clone());
        labels.insert(
            "app.kubernetes.io/component".to_owned(),
            "storage".to_owned(),
        );
        labels
    }

    /// Returns selector labels for Services and StatefulSets.
    /// These should be a stable subset of the full labels.
    pub(crate) fn selector_labels(&self) -> std::collections::BTreeMap<String, String> {
        [("rustfs.tenant".to_owned(), self.name())]
            .into_iter()
            .collect()
    }

    /// Returns selector labels for pool-specific resources.
    pub(crate) fn pool_selector_labels(
        &self,
        pool: &Pool,
    ) -> std::collections::BTreeMap<String, String> {
        let mut labels = self.selector_labels();
        labels.insert("rustfs.pool".to_owned(), pool.name.clone());
        labels
    }

    /// Build pool status from a StatefulSet.
    /// This method extracts replica counts, revisions, and determines the pool state
    /// based on the StatefulSet's status.
    pub(crate) fn build_pool_status(
        &self,
        pool_name: &str,
        ss: &k8s_openapi::api::apps::v1::StatefulSet,
    ) -> crate::types::v1alpha1::status::pool::Pool {
        use crate::types::v1alpha1::status::pool::PoolState;

        let ss_name = format!("{}-{}", self.name(), pool_name);
        let status = ss.status.as_ref();

        // Extract replica counts
        let replicas = status.map(|s| s.replicas);
        let ready_replicas = status.and_then(|s| s.ready_replicas);
        let current_replicas = status.and_then(|s| s.current_replicas);
        let updated_replicas = status.and_then(|s| s.updated_replicas);

        // Extract revisions
        let current_revision = status.and_then(|s| s.current_revision.clone());
        let update_revision = status.and_then(|s| s.update_revision.clone());

        // Determine pool state based on StatefulSet status. Kubernetes StatefulSet
        // status is authoritative only after the controller has observed the latest
        // generation; revision mismatch also means a rollout is still in progress.
        let state = if let Some(status) = status {
            let desired = ss
                .spec
                .as_ref()
                .and_then(|spec| spec.replicas)
                .unwrap_or(status.replicas);
            let ready = status.ready_replicas.unwrap_or(0);
            let updated = status.updated_replicas.unwrap_or(0);
            let current = status.current_replicas.unwrap_or(0);
            let observed_current = match (status.observed_generation, ss.metadata.generation) {
                (Some(observed), Some(generation)) => observed >= generation,
                (Some(_), None) | (None, None) => true,
                (None, Some(_)) => false,
            };
            let revisions_match = match (&status.current_revision, &status.update_revision) {
                (Some(current_revision), Some(update_revision)) => {
                    current_revision == update_revision
                }
                (None, None) => true,
                _ => false,
            };

            if desired == 0 {
                PoolState::NotCreated
            } else if !observed_current
                || !revisions_match
                || updated < desired
                || current < desired
            {
                PoolState::Updating
            } else if ready == desired && updated == desired {
                PoolState::RolloutComplete
            } else if ready < desired {
                PoolState::Degraded
            } else {
                PoolState::Initialized
            }
        } else {
            PoolState::NotCreated
        };

        // Get current time for last_update_time
        let last_update_time =
            Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

        crate::types::v1alpha1::status::pool::Pool {
            ss_name,
            state,
            replicas,
            ready_replicas,
            current_replicas,
            updated_replicas,
            current_revision,
            update_revision,
            last_update_time,
        }
    }
}

/// Validate a name conforms to DNS-1035 label rules:
/// `[a-z]([-a-z0-9]*[a-z0-9])?`, max 63 characters.
pub fn validate_dns1035_label(name: &str) -> Result<(), types::error::Error> {
    if name.is_empty() {
        return Err(types::error::Error::InvalidTenantName {
            name: name.to_string(),
            reason: "name must not be empty".to_string(),
        });
    }

    // Longest derived DNS label is "{name}-console" (+8). RFC 1123 labels max 63 chars ⇒ |name| ≤ 55.
    if name.len() > 55 {
        return Err(types::error::Error::InvalidTenantName {
            name: name.to_string(),
            reason: format!(
                "name must be at most 55 characters (longest derived name is {{name}}-console), got {}",
                name.len()
            ),
        });
    }

    let bytes = name.as_bytes();

    if !bytes[0].is_ascii_lowercase() {
        return Err(types::error::Error::InvalidTenantName {
            name: name.to_string(),
            reason: "must start with a lowercase letter (a-z), not a digit or symbol".to_string(),
        });
    }

    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(types::error::Error::InvalidTenantName {
            name: name.to_string(),
            reason: "must end with a lowercase alphanumeric character (a-z, 0-9)".to_string(),
        });
    }

    for &b in bytes {
        if !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'-' {
            return Err(types::error::Error::InvalidTenantName {
                name: name.to_string(),
                reason: format!(
                    "contains invalid character '{}'; only lowercase alphanumeric and '-' are allowed",
                    b as char
                ),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::types::v1alpha1::status::pool::PoolState;
    use k8s_openapi::api::apps::v1::{StatefulSet, StatefulSetSpec, StatefulSetStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn statefulset_with_status(
        generation: i64,
        observed_generation: i64,
        replicas: i32,
        ready_replicas: i32,
        updated_replicas: i32,
        current_revision: &str,
        update_revision: &str,
    ) -> StatefulSet {
        StatefulSet {
            metadata: ObjectMeta {
                name: Some("test-tenant-pool-0".to_string()),
                generation: Some(generation),
                ..Default::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(replicas),
                ..Default::default()
            }),
            status: Some(StatefulSetStatus {
                observed_generation: Some(observed_generation),
                replicas,
                ready_replicas: Some(ready_replicas),
                updated_replicas: Some(updated_replicas),
                current_replicas: Some(replicas),
                current_revision: Some(current_revision.to_string()),
                update_revision: Some(update_revision.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn pool_status_treats_stale_statefulset_observation_as_updating() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let ss = statefulset_with_status(2, 1, 4, 4, 4, "rev-a", "rev-a");

        let pool_status = tenant.build_pool_status("pool-0", &ss);

        assert_eq!(pool_status.state, PoolState::Updating);
    }

    #[test]
    fn pool_status_requires_current_and_update_revisions_to_match() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let ss = statefulset_with_status(2, 2, 4, 4, 4, "rev-a", "rev-b");

        let pool_status = tenant.build_pool_status("pool-0", &ss);

        assert_eq!(pool_status.state, PoolState::Updating);
    }

    // Test 1: Default behavior - no custom SA
    #[test]
    fn test_service_account_name_default() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let sa_name = tenant.service_account_name();

        assert_eq!(
            sa_name, "test-tenant-sa",
            "Default service account name should be {{tenant-name}}-sa"
        );
    }

    // Test 2: Custom SA specified
    #[test]
    fn test_service_account_name_custom() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), None);

        let sa_name = tenant.service_account_name();

        assert_eq!(
            sa_name, "my-custom-sa",
            "Should return custom service account name when specified"
        );
    }

    // Test 3: Edge case - empty string for custom SA (treated as-is)
    #[test]
    fn test_service_account_name_empty_string() {
        let tenant = crate::tests::create_test_tenant(Some("".to_string()), None);

        let sa_name = tenant.service_account_name();

        // Empty string should be returned as-is, not converted to default
        assert_eq!(
            sa_name, "",
            "Empty string should be returned as-is, not converted to default"
        );
    }

    // Test 4: Common labels include Kubernetes recommended labels
    #[test]
    fn test_common_labels() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let labels = tenant.common_labels();

        // Verify Kubernetes recommended labels
        assert_eq!(
            labels.get("app.kubernetes.io/name"),
            Some(&"rustfs".to_string())
        );
        assert_eq!(
            labels.get("app.kubernetes.io/instance"),
            Some(&"test-tenant".to_string())
        );
        assert_eq!(
            labels.get("app.kubernetes.io/managed-by"),
            Some(&"rustfs-operator".to_string())
        );
        assert_eq!(
            labels.get("rustfs.tenant"),
            Some(&"test-tenant".to_string())
        );
        assert_eq!(labels.len(), 4, "Should have exactly 4 common labels");
    }

    // Test 5: Pool labels include common labels plus pool-specific labels
    #[test]
    fn test_pool_labels() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let labels = tenant.pool_labels(pool);

        // Should include all common labels
        assert_eq!(
            labels.get("app.kubernetes.io/name"),
            Some(&"rustfs".to_string())
        );
        assert_eq!(
            labels.get("rustfs.tenant"),
            Some(&"test-tenant".to_string())
        );

        // Plus pool-specific labels
        assert_eq!(labels.get("rustfs.pool"), Some(&"pool-0".to_string()));
        assert_eq!(
            labels.get("app.kubernetes.io/component"),
            Some(&"storage".to_string())
        );

        assert_eq!(
            labels.len(),
            6,
            "Should have 4 common + 2 pool-specific labels"
        );
    }

    // Test 6: Selector labels are stable subset
    #[test]
    fn test_selector_labels() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let labels = tenant.selector_labels();

        assert_eq!(
            labels.get("rustfs.tenant"),
            Some(&"test-tenant".to_string())
        );
        assert_eq!(
            labels.len(),
            1,
            "Selector should only have tenant label for stability"
        );
    }

    // Test 7: Pool selector labels include pool
    #[test]
    fn test_pool_selector_labels() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let labels = tenant.pool_selector_labels(pool);

        assert_eq!(
            labels.get("rustfs.tenant"),
            Some(&"test-tenant".to_string())
        );
        assert_eq!(labels.get("rustfs.pool"), Some(&"pool-0".to_string()));
        assert_eq!(
            labels.len(),
            2,
            "Pool selector should have tenant + pool labels"
        );
    }

    // Test 8: DNS-1035 validation - valid names
    #[test]
    fn test_validate_dns1035_valid_names() {
        use super::validate_dns1035_label;

        assert!(validate_dns1035_label("my-tenant").is_ok());
        assert!(validate_dns1035_label("a").is_ok());
        assert!(validate_dns1035_label("abc-123").is_ok());
        assert!(validate_dns1035_label("example-tenant").is_ok());
        assert!(validate_dns1035_label("a1").is_ok());
    }

    // Test 9: DNS-1035 validation - name starting with digit rejected
    #[test]
    fn test_validate_dns1035_digit_start() {
        use super::validate_dns1035_label;

        let err = validate_dns1035_label("111").unwrap_err();
        assert!(
            err.to_string()
                .contains("must start with a lowercase letter"),
            "Error should mention starting with a letter, got: {}",
            err
        );
    }

    // Test 10: DNS-1035 validation - empty name rejected
    #[test]
    fn test_validate_dns1035_empty() {
        use super::validate_dns1035_label;

        let err = validate_dns1035_label("").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    // Test 11: DNS-1035 validation - uppercase rejected
    #[test]
    fn test_validate_dns1035_uppercase() {
        use super::validate_dns1035_label;

        let err = validate_dns1035_label("MyTenant").unwrap_err();
        assert!(
            err.to_string()
                .contains("must start with a lowercase letter")
        );
    }

    // Test 12: DNS-1035 validation - trailing hyphen rejected
    #[test]
    fn test_validate_dns1035_trailing_hyphen() {
        use super::validate_dns1035_label;

        let err = validate_dns1035_label("my-tenant-").unwrap_err();
        assert!(err.to_string().contains("must end with"));
    }

    // Test 13: DNS-1035 validation - too long rejected
    #[test]
    fn test_validate_dns1035_too_long() {
        use super::validate_dns1035_label;

        let long_name = format!("a{}", "b".repeat(55));
        let err = validate_dns1035_label(&long_name).unwrap_err();
        assert!(err.to_string().contains("at most 55 characters"));
    }

    // Test 14: DNS-1035 validation - underscore rejected
    #[test]
    fn test_validate_dns1035_underscore() {
        use super::validate_dns1035_label;

        let err = validate_dns1035_label("my_tenant").unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }
}
