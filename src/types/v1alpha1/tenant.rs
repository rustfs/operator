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

use crate::types;
use crate::types::error::NoNamespaceSnafu;
use crate::types::v1alpha1::k8s;
use crate::types::v1alpha1::pool::Pool;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::{CustomResource, KubeSchema, Resource, ResourceExt};
use serde::{Deserialize, Serialize};
use snafu::OptionExt;

// Submodules for resource factory methods
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
    printcolumn = r#"{"name":"Health", "type":"string", "jsonPath":".status.healthStatus"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    crates(serde_json = "k8s_openapi::serde_json")
)]
#[serde(rename_all = "camelCase")]
pub struct TenantSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,

    #[x_kube(validation = Rule::new("self.size() > 0").message("pools must be configured"))]
    pub pools: Vec<Pool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pull_secret: Option<corev1::LocalObjectReference>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_management_policy: Option<k8s::PodManagementPolicy>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<corev1::EnvVar>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub mount_path: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub sub_path: Option<String>,
    //
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
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub lifecycle: Option<corev1::Lifecycle>,

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

    // // #[serde(default, skip_serializing_if = "Option::is_none")]
    // // pub side_cars: Option<SideCars>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<corev1::LocalObjectReference>,
}

impl Tenant {
    pub fn namespace(&self) -> Result<String, types::error::Error> {
        ResourceExt::namespace(self).context(NoNamespaceSnafu)
    }

    pub fn name(&self) -> String {
        ResourceExt::name_any(self)
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

    pub fn console_service_name(&self) -> String {
        format!("{}-console", self.name())
    }

    pub fn headless_service_name(&self) -> String {
        format!("{}-hl", self.name())
    }

    pub fn role_binding_name(&self) -> String {
        format!("{}-role-binding", self.name())
    }

    pub fn role_name(&self) -> String {
        format!("{}-role", self.name())
    }

    pub fn service_account_name(&self) -> String {
        self.spec
            .service_account_name
            .clone()
            .unwrap_or_else(|| format!("{}-sa", self.name()))
    }

    pub fn statefulset_name(&self, pool: &Pool) -> String {
        format!("{}-{}", self.name(), pool.name)
    }

    pub fn secret_name(&self) -> String {
        format!("{}-tls", self.name())
    }

    /// Returns common labels that should be applied to all Tenant-owned resources.
    /// These labels follow Kubernetes recommended label conventions.
    pub fn common_labels(&self) -> std::collections::BTreeMap<String, String> {
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
    pub fn pool_labels(&self, pool: &Pool) -> std::collections::BTreeMap<String, String> {
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
    pub fn selector_labels(&self) -> std::collections::BTreeMap<String, String> {
        [("rustfs.tenant".to_owned(), self.name())]
            .into_iter()
            .collect()
    }

    /// Returns selector labels for pool-specific resources.
    pub fn pool_selector_labels(&self, pool: &Pool) -> std::collections::BTreeMap<String, String> {
        let mut labels = self.selector_labels();
        labels.insert("rustfs.pool".to_owned(), pool.name.clone());
        labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::persistence::PersistenceConfig;

    // Helper function to create a test tenant (available to submodule tests via super::tests)
    pub(super) fn create_test_tenant(
        service_account_name: Option<String>,
        create_service_account_rbac: Option<bool>,
    ) -> Tenant {
        Tenant {
            metadata: metav1::ObjectMeta {
                name: Some("test-tenant".to_string()),
                namespace: Some("default".to_string()),
                uid: Some("test-uid-123".to_string()),
                ..Default::default()
            },
            spec: TenantSpec {
                pools: vec![Pool {
                    name: "pool-0".to_string(),
                    servers: 4,
                    persistence: PersistenceConfig {
                        volumes_per_server: 4,
                        ..Default::default()
                    },
                }],
                service_account_name,
                create_service_account_rbac,
                ..Default::default()
            },
            status: None,
        }
    }

    // Test 1: Default behavior - no custom SA
    #[test]
    fn test_service_account_name_default() {
        let tenant = create_test_tenant(None, None);

        let sa_name = tenant.service_account_name();

        assert_eq!(
            sa_name, "test-tenant-sa",
            "Default service account name should be {{tenant-name}}-sa"
        );
    }

    // Test 2: Custom SA specified
    #[test]
    fn test_service_account_name_custom() {
        let tenant = create_test_tenant(Some("my-custom-sa".to_string()), None);

        let sa_name = tenant.service_account_name();

        assert_eq!(
            sa_name, "my-custom-sa",
            "Should return custom service account name when specified"
        );
    }

    // Test 3: Edge case - empty string for custom SA (treated as-is)
    #[test]
    fn test_service_account_name_empty_string() {
        let tenant = create_test_tenant(Some("".to_string()), None);

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
        let tenant = create_test_tenant(None, None);

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
        let tenant = create_test_tenant(None, None);
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
        let tenant = create_test_tenant(None, None);

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
        let tenant = create_test_tenant(None, None);
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
}
