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
use crate::types::v1alpha1::pool::Pool;
use k8s_openapi::api::apps::v1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::rbac::v1 as rbacv1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr;
use k8s_openapi::{Resource as _, schemars};
use kube::{CustomResource, KubeSchema, Resource, ResourceExt};
use serde::{Deserialize, Serialize};
use snafu::OptionExt;

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

    pub pools: Vec<Pool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub image_pull_secret: Option<corev1::LocalObjectReference>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub pod_management_policy: Option<String>,
    //
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
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub service_account_name: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub priority_class_name: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub image_pull_policy: Option<String>,
    //
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

    pub fn new_role_binding(
        &self,
        sa: &corev1::ServiceAccount,
        role: &rbacv1::Role,
    ) -> rbacv1::RoleBinding {
        rbacv1::RoleBinding {
            metadata: metav1::ObjectMeta {
                name: Some(self.role_binding_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            subjects: Some(vec![rbacv1::Subject {
                kind: corev1::ServiceAccount::KIND.to_owned(),
                namespace: ResourceExt::namespace(sa),
                name: sa.name_any(),
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

    pub fn new_statefulset(&self, pool: &Pool) -> v1::StatefulSet {
        v1::StatefulSet {
            ..Default::default()
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
        format!("{}-sa", self.name())
    }

    pub fn statefulset_name(&self, pool: &Pool) -> String {
        format!("{}-{}", self.name(), pool.name)
    }

    pub fn secret_name(&self) -> String {
        format!("{}-tls", self.name())
    }
}
