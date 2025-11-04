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
use k8s_openapi::Resource as _;
use k8s_openapi::api::apps::v1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::rbac::v1 as rbacv1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr;
use kube::{CustomResource, KubeSchema, Resource, ResourceExt};
use serde::{Deserialize, Serialize};
use snafu::OptionExt;

const VOLUME_CLAIM_TEMPLATE_PREFIX: &str = "vol";

fn volume_claim_template_name(shard: i32) -> String {
    format!("{}-{}", VOLUME_CLAIM_TEMPLATE_PREFIX, shard)
}

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

    /// a new io Service for tenant
    pub fn new_io_service(&self) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some("rustfs".to_owned()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                selector: Some(
                    [("rustfs.tenant".to_owned(), self.name())]
                        .into_iter()
                        .collect(),
                ),
                ports: Some(vec![corev1::ServicePort {
                    port: 90,
                    target_port: Some(intstr::IntOrString::Int(9000)),
                    name: Some("http-rustfs".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// a new console Service for tenant
    pub fn new_console_service(&self) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some(self.console_service_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                selector: Some(
                    [("rustfs.tenant".to_owned(), self.name())]
                        .into_iter()
                        .collect(),
                ),
                ports: Some(vec![corev1::ServicePort {
                    port: 9090,
                    target_port: Some(intstr::IntOrString::Int(9090)),
                    name: Some("http-console".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// a new headless Service for tenant
    pub fn new_headless_service(&self) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some(self.headless_service_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                cluster_ip: Some("None".to_owned()),
                publish_not_ready_addresses: Some(true),
                selector: Some(
                    [("rustfs.tenant".to_owned(), self.name())]
                        .into_iter()
                        .collect(),
                ),
                ports: Some(vec![corev1::ServicePort {
                    port: 9000,
                    name: Some("http-rustfs".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub fn new_role_binding(&self, sa_name: &str, role: &rbacv1::Role) -> rbacv1::RoleBinding {
        rbacv1::RoleBinding {
            metadata: metav1::ObjectMeta {
                name: Some(self.role_binding_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            subjects: Some(vec![rbacv1::Subject {
                kind: corev1::ServiceAccount::KIND.to_owned(),
                namespace: self.namespace().ok(),
                name: sa_name.to_owned(),
                ..Default::default()
            }]),
            role_ref: rbacv1::RoleRef {
                api_group: rbacv1::Role::GROUP.to_owned(),
                kind: rbacv1::Role::KIND.to_owned(),
                name: role.name_any(),
            },
        }
    }

    pub fn new_role(&self) -> rbacv1::Role {
        rbacv1::Role {
            metadata: metav1::ObjectMeta {
                name: Some(self.role_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            rules: Some(vec![
                rbacv1::PolicyRule {
                    api_groups: Some(vec![String::new()]),
                    resources: Some(vec!["secrets".to_owned()]),
                    verbs: vec!["get".to_owned(), "list".to_owned(), "watch".to_owned()],
                    ..Default::default()
                },
                rbacv1::PolicyRule {
                    api_groups: Some(vec![String::new()]),
                    resources: Some(vec!["services".to_owned()]),
                    verbs: vec!["create".to_owned(), "delete".to_owned(), "get".to_owned()],
                    ..Default::default()
                },
                rbacv1::PolicyRule {
                    api_groups: Some(vec![Self::group(&()).to_string()]),
                    resources: Some(vec![Self::plural(&()).to_string()]),
                    verbs: vec!["get".to_owned(), "list".to_owned(), "watch".to_owned()],
                    ..Default::default()
                },
            ]),
        }
    }

    pub fn new_service_account(&self) -> corev1::ServiceAccount {
        corev1::ServiceAccount {
            metadata: metav1::ObjectMeta {
                name: Some(self.service_account_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Creates volume claim templates for a pool
    /// Returns a vector of PersistentVolumeClaim templates for StatefulSet
    fn volume_claim_templates(
        &self,
        pool: &Pool,
    ) -> Result<Vec<corev1::PersistentVolumeClaim>, types::error::Error> {
        // Get PVC spec or create default (ReadWriteOnce, 10Gi)
        let spec = pool
            .persistence
            .volume_claim_template
            .clone()
            .unwrap_or_else(|| {
                let mut resources = std::collections::BTreeMap::new();
                resources.insert(
                    "storage".to_string(),
                    k8s_openapi::apimachinery::pkg::api::resource::Quantity("10Gi".to_string()),
                );

                corev1::PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(corev1::VolumeResourceRequirements {
                        requests: Some(resources),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            });

        let tenant = self.name();
        let pool_name = pool.name.clone();

        // Create operator-managed labels
        let mut labels = std::collections::BTreeMap::new();
        labels.insert(
            "app.kubernetes.io/managed-by".to_owned(),
            "rustfs-operator".to_owned(),
        );
        labels.insert("rustfs.tenant".to_owned(), tenant.clone());
        labels.insert("rustfs.pool".to_owned(), pool_name.clone());
        labels.insert("rustfs.tenant.namespace".to_owned(), self.namespace()?);

        // Merge with user-provided labels (user labels can override)
        if let Some(user_labels) = &pool.persistence.labels {
            labels.extend(user_labels.clone());
        }

        // Get annotations from persistence config
        let annotations = pool.persistence.annotations.clone();

        // Generate volume claim templates for each volume
        let templates: Vec<_> = (0..pool.persistence.volumes_per_server)
            .map(|i| corev1::PersistentVolumeClaim {
                metadata: metav1::ObjectMeta {
                    name: Some(volume_claim_template_name(i)),
                    labels: Some(labels.clone()),
                    annotations: annotations.clone(),
                    ..Default::default()
                },
                spec: Some(spec.clone()),
                ..Default::default()
            })
            .collect();

        Ok(templates)
    }

    pub fn new_statefulset(&self, pool: &Pool) -> Result<v1::StatefulSet, types::error::Error> {
        let labels: std::collections::BTreeMap<String, String> = [
            ("rustfs.tenant".to_owned(), self.name()),
            ("rustfs.pool".to_owned(), pool.name.clone()),
        ]
        .into_iter()
        .collect();

        // Generate PVC name prefix: {tenantName}-{poolName}
        let pvc_name_prefix = format!("{}-{}", self.name(), pool.name);

        // Generate volume claim templates using helper function
        let volume_claim_templates = self.volume_claim_templates(pool)?;

        // Generate volume mounts for each volume
        // Default path is /data if not specified
        // Volume mount names must match the volume claim template names (vol-0, vol-1, etc.)
        let base_path = pool.persistence.path.as_deref().unwrap_or("/data");
        let volume_mounts: Vec<corev1::VolumeMount> = (0..pool.persistence.volumes_per_server)
            .map(|i| corev1::VolumeMount {
                name: volume_claim_template_name(i),
                mount_path: format!("{}/{}", base_path.trim_end_matches('/'), i),
                ..Default::default()
            })
            .collect();

        // Generate environment variables: operator-managed + user-provided
        let mut env_vars = Vec::new();

        // Add RUSTFS_VOLUMES environment variable for multi-node communication
        let rustfs_volumes = self.rustfs_volumes_env_value()?;
        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_VOLUMES".to_owned(),
            value: Some(rustfs_volumes),
            ..Default::default()
        });

        // Merge with user-provided environment variables
        // User-provided vars can override operator-managed ones
        for user_env in &self.spec.env {
            // Remove any existing var with the same name to allow override
            env_vars.retain(|e| e.name != user_env.name);
            env_vars.push(user_env.clone());
        }

        let container = corev1::Container {
            name: "rustfs".to_owned(),
            image: self.spec.image.clone(),
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            ports: Some(vec![
                corev1::ContainerPort {
                    container_port: 9000,
                    name: Some("http".to_owned()),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                },
                corev1::ContainerPort {
                    container_port: 9090,
                    name: Some("console".to_owned()),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                },
            ]),
            volume_mounts: Some(volume_mounts),
            ..Default::default()
        };

        Ok(v1::StatefulSet {
            metadata: metav1::ObjectMeta {
                name: Some(self.statefulset_name(pool)),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(v1::StatefulSetSpec {
                replicas: Some(pool.servers),
                service_name: Some(self.headless_service_name()),
                pod_management_policy: self
                    .spec
                    .pod_management_policy
                    .as_ref()
                    .and_then(|p| serde_json::to_string(p).ok())
                    .map(|s| s.trim_matches('"').to_owned())
                    .or(Some("Parallel".to_owned())),
                selector: metav1::LabelSelector {
                    match_labels: Some(labels.clone()),
                    ..Default::default()
                },
                template: corev1::PodTemplateSpec {
                    metadata: Some(metav1::ObjectMeta {
                        labels: Some(labels),
                        ..Default::default()
                    }),
                    spec: Some(corev1::PodSpec {
                        service_account_name: Some(self.service_account_name()),
                        containers: vec![container],
                        scheduler_name: self.spec.scheduler.clone(),
                        ..Default::default()
                    }),
                },
                volume_claim_templates: Some(volume_claim_templates),
                ..Default::default()
            }),
            ..Default::default()
        })
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
