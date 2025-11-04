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

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub priority_class_name: Option<String>,
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

    /// Constructs the RUSTFS_VOLUMES environment variable value
    /// Format: http://{tenant}-{pool}-{0...servers-1}.{service}.{namespace}.svc.cluster.local:9000{path}/{0...volumes-1}
    /// All pools are combined into a space-separated string for a unified cluster
    pub fn rustfs_volumes_env_value(&self) -> Result<String, types::error::Error> {
        let namespace = self.namespace()?;
        let tenant_name = self.name();
        let headless_service = self.headless_service_name();

        let volume_specs: Vec<String> = self
            .spec
            .pools
            .iter()
            .map(|pool| {
                let base_path = pool.persistence.path.as_deref().unwrap_or("/data");
                let pool_name = &pool.name;

                // Construct volume specification with range notation
                format!(
                    "http://{}-{}-{{0...{}}}.{}.{}.svc.cluster.local:9000{}/{{0...{}}}",
                    tenant_name,
                    pool_name,
                    pool.servers - 1,
                    headless_service,
                    namespace,
                    base_path.trim_end_matches('/'),
                    pool.persistence.volumes_per_server - 1
                )
            })
            .collect();

        Ok(volume_specs.join(" "))
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
}
