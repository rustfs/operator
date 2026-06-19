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

use k8s_openapi::api::core::v1::{
    Affinity, EnvVar, LocalObjectReference, PersistentVolumeClaimSpec, PodAffinityTerm,
    PodAntiAffinity, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use operator::types::v1alpha1::k8s::ImagePullPolicy;
use operator::types::v1alpha1::k8s::PodManagementPolicy;
use operator::types::v1alpha1::persistence::PersistenceConfig;
use operator::types::v1alpha1::pool::{Pool, SchedulingConfig};
use operator::types::v1alpha1::tenant::{Tenant, TenantSpec};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct TenantTemplate {
    pub namespace: String,
    pub name: String,
    pub image: String,
    pub storage_class: String,
    pub credential_secret_name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub storage_request: String,
    pub pod_management_policy: Option<PodManagementPolicy>,
    pub unsafe_bypass_disk_check: bool,
    pub node_selector: Option<BTreeMap<String, String>>,
    pub affinity: Option<Affinity>,
}

impl TenantTemplate {
    pub fn kind_local(
        namespace: impl Into<String>,
        name: impl Into<String>,
        image: impl Into<String>,
        storage_class: impl Into<String>,
        credential_secret_name: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            image: image.into(),
            storage_class: storage_class.into(),
            credential_secret_name: credential_secret_name.into(),
            servers: 4,
            volumes_per_server: 2,
            storage_request: "10Gi".to_string(),
            pod_management_policy: Some(PodManagementPolicy::Parallel),
            unsafe_bypass_disk_check: true,
            node_selector: Some(
                [("rustfs-storage".to_string(), "true".to_string())]
                    .into_iter()
                    .collect(),
            ),
            affinity: None,
        }
    }

    pub fn real_cluster(
        namespace: impl Into<String>,
        name: impl Into<String>,
        image: impl Into<String>,
        storage_class: impl Into<String>,
        credential_secret_name: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            namespace: namespace.into(),
            name: name.clone(),
            image: image.into(),
            storage_class: storage_class.into(),
            credential_secret_name: credential_secret_name.into(),
            servers: 4,
            volumes_per_server: 1,
            storage_request: "80Gi".to_string(),
            pod_management_policy: Some(PodManagementPolicy::Parallel),
            unsafe_bypass_disk_check: false,
            node_selector: None,
            affinity: Some(fault_tenant_pod_anti_affinity(&name)),
        }
    }

    pub fn build(&self) -> Tenant {
        let pool = Pool {
            name: "primary".to_string(),
            servers: self.servers,
            persistence: PersistenceConfig {
                volumes_per_server: self.volumes_per_server,
                volume_claim_template: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(
                            [(
                                "storage".to_string(),
                                Quantity(self.storage_request.clone()),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                        ..Default::default()
                    }),
                    storage_class_name: Some(self.storage_class.clone()),
                    ..Default::default()
                }),
                ..PersistenceConfig::default()
            },
            scheduling: SchedulingConfig {
                node_selector: self.node_selector.clone(),
                affinity: self.affinity.clone(),
                ..SchedulingConfig::default()
            },
        };

        let mut env = vec![EnvVar {
            name: "RUST_LOG".to_string(),
            value: Some("info".to_string()),
            ..EnvVar::default()
        }];

        if self.unsafe_bypass_disk_check {
            env.push(EnvVar {
                name: "RUSTFS_UNSAFE_BYPASS_DISK_CHECK".to_string(),
                value: Some("true".to_string()),
                ..EnvVar::default()
            });
        }

        let spec = TenantSpec {
            pools: vec![pool],
            image: Some(self.image.clone()),
            image_pull_policy: Some(ImagePullPolicy::IfNotPresent),
            pod_management_policy: self.pod_management_policy.clone(),
            creds_secret: Some(LocalObjectReference {
                name: self.credential_secret_name.clone(),
            }),
            env,
            ..TenantSpec::default()
        };

        let mut tenant = Tenant::new(&self.name, spec);
        tenant.metadata.namespace = Some(self.namespace.clone());
        tenant
    }
}

fn fault_tenant_pod_anti_affinity(tenant_name: &str) -> Affinity {
    Affinity {
        pod_anti_affinity: Some(PodAntiAffinity {
            required_during_scheduling_ignored_during_execution: Some(vec![PodAffinityTerm {
                label_selector: Some(LabelSelector {
                    match_labels: Some(
                        [("rustfs.tenant".to_string(), tenant_name.to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    ..LabelSelector::default()
                }),
                topology_key: "kubernetes.io/hostname".to_string(),
                ..PodAffinityTerm::default()
            }]),
            ..PodAntiAffinity::default()
        }),
        ..Affinity::default()
    }
}

#[cfg(test)]
mod tests {
    use super::TenantTemplate;

    #[test]
    fn kind_local_tenant_uses_local_image_policy_and_disk_bypass() {
        let tenant = TenantTemplate::kind_local(
            "rustfs-e2e",
            "tenant-a",
            "rustfs/rustfs:e2e",
            "local-storage",
            "tenant-a-credentials",
        )
        .build();

        assert_eq!(tenant.metadata.namespace.as_deref(), Some("rustfs-e2e"));
        assert_eq!(tenant.spec.image.as_deref(), Some("rustfs/rustfs:e2e"));
        assert_eq!(
            tenant
                .spec
                .creds_secret
                .as_ref()
                .map(|secret| secret.name.as_str()),
            Some("tenant-a-credentials")
        );
        assert_eq!(
            tenant.spec.pools[0]
                .persistence
                .volume_claim_template
                .as_ref()
                .and_then(|claim| claim.storage_class_name.as_deref()),
            Some("local-storage")
        );
        assert!(tenant.spec.image_pull_policy.is_some());
        assert!(
            tenant
                .spec
                .env
                .iter()
                .any(|env| env.name == "RUSTFS_UNSAFE_BYPASS_DISK_CHECK"
                    && env.value.as_deref() == Some("true"))
        );
        assert_eq!(
            tenant.spec.pools[0]
                .scheduling
                .node_selector
                .as_ref()
                .and_then(|selector| selector.get("rustfs-storage"))
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn real_cluster_tenant_uses_fault_storage_spread_and_disk_checks() {
        let tenant = TenantTemplate::real_cluster(
            "rustfs-fault-test",
            "fault-test-tenant",
            "rustfs/rustfs:latest",
            "fast-csi",
            "fault-test-tenant-credentials",
        )
        .build();

        assert_eq!(tenant.spec.pools[0].persistence.volumes_per_server, 1);
        assert_eq!(
            tenant.spec.pools[0]
                .scheduling
                .affinity
                .as_ref()
                .and_then(|affinity| affinity.pod_anti_affinity.as_ref())
                .and_then(|anti_affinity| {
                    anti_affinity
                        .required_during_scheduling_ignored_during_execution
                        .as_ref()
                })
                .and_then(|terms| terms.first())
                .map(|term| term.topology_key.as_str()),
            Some("kubernetes.io/hostname")
        );
        assert_eq!(
            tenant.spec.pools[0]
                .persistence
                .volume_claim_template
                .as_ref()
                .and_then(|claim| claim.resources.as_ref())
                .and_then(|resources| resources.requests.as_ref())
                .and_then(|requests| requests.get("storage"))
                .map(|quantity| quantity.0.as_str()),
            Some("80Gi")
        );
        assert!(tenant.spec.pools[0].scheduling.node_selector.is_none());
        assert!(
            tenant
                .spec
                .env
                .iter()
                .all(|env| env.name != "RUSTFS_UNSAFE_BYPASS_DISK_CHECK")
        );
    }
}
