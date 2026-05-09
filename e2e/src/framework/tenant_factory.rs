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
    EnvVar, LocalObjectReference, PersistentVolumeClaimSpec, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use operator::types::v1alpha1::k8s::ImagePullPolicy;
use operator::types::v1alpha1::persistence::PersistenceConfig;
use operator::types::v1alpha1::pool::{Pool, SchedulingConfig};
use operator::types::v1alpha1::tenant::{Tenant, TenantSpec};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantTemplate {
    pub namespace: String,
    pub name: String,
    pub image: String,
    pub storage_class: String,
    pub credential_secret_name: String,
    pub servers: i32,
    pub volumes_per_server: i32,
    pub unsafe_bypass_disk_check: bool,
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
            unsafe_bypass_disk_check: true,
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
                            [("storage".to_string(), Quantity("10Gi".to_string()))]
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
                node_selector: Some(
                    [("rustfs-storage".to_string(), "true".to_string())]
                        .into_iter()
                        .collect::<BTreeMap<_, _>>(),
                ),
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
    }
}
