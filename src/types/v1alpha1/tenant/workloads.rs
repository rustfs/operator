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

use super::Tenant;
use crate::types;
use crate::types::v1alpha1::pool::Pool;
use k8s_openapi::api::apps::v1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

const VOLUME_CLAIM_TEMPLATE_PREFIX: &str = "vol";

fn volume_claim_template_name(shard: i32) -> String {
    format!("{}-{}", VOLUME_CLAIM_TEMPLATE_PREFIX, shard)
}

impl Tenant {
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
}

#[cfg(test)]
mod tests {
    // Test: StatefulSet uses correct service account
    #[test]
    fn test_statefulset_uses_default_sa() {
        let tenant = super::super::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.service_account_name,
            Some("test-tenant-sa".to_string()),
            "Pod should use default service account"
        );
    }

    // Test: StatefulSet uses custom service account
    #[test]
    fn test_statefulset_uses_custom_sa() {
        let tenant =
            super::super::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.service_account_name,
            Some("my-custom-sa".to_string()),
            "Pod should use custom service account"
        );
    }
}
