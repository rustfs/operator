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
const LOG_VOLUME_NAME: &str = "logs";
const LOG_VOLUME_MOUNT_PATH: &str = "/logs";
const DEFAULT_RUN_AS_USER: i64 = 10001;
const DEFAULT_RUN_AS_GROUP: i64 = 10001;
const DEFAULT_FS_GROUP: i64 = 10001;

fn volume_claim_template_name(shard: i32) -> String {
    format!("{VOLUME_CLAIM_TEMPLATE_PREFIX}-{shard}")
}

fn stateful_name(tenant: &Tenant, pool: &Pool) -> String {
    format!("{}-{}", tenant.name(), pool.name)
}

impl Tenant {
    /// Constructs the RUSTFS_VOLUMES environment variable value
    /// Format: http://{tenant}-{pool}-{0...servers-1}.{service}.{namespace}.svc.cluster.local:9000{path}/rustfs{0...volumes-1}
    /// All pools are combined into a space-separated string for a unified cluster
    /// Follows RustFS convention: /data/rustfs0, /data/rustfs1, etc.
    fn rustfs_volumes_env_value(&self) -> Result<String, types::error::Error> {
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
                // Follows RustFS convention: /data/rustfs{0...N}
                format!(
                    "http://{tenant_name}-{pool_name}-{{0...{}}}.{headless_service}.{namespace}.svc.cluster.local:9000{}/rustfs{{0...{}}}",
                    pool.servers - 1,
                    base_path.trim_end_matches('/'),
                    pool.persistence.volumes_per_server - 1
                )
            })
            .collect();

        Ok(volume_specs.join(" "))
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

        // Start with operator-managed labels (follows Kubernetes recommended labels)
        let mut labels = self.pool_labels(pool);

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
        let labels = self.pool_labels(pool);
        let selector_labels = self.pool_selector_labels(pool);

        // Generate volume claim templates using helper function
        let volume_claim_templates = self.volume_claim_templates(pool)?;

        // Generate volume mounts for each volume
        // Default path is /data if not specified
        // Volume mount names must match the volume claim template names (vol-0, vol-1, etc.)
        // Mount paths follow RustFS convention: /data/rustfs0, /data/rustfs1, etc.
        let base_path = pool.persistence.path.as_deref().unwrap_or("/data");
        let mut volume_mounts: Vec<corev1::VolumeMount> = (0..pool.persistence.volumes_per_server)
            .map(|i| corev1::VolumeMount {
                name: volume_claim_template_name(i),
                mount_path: format!("{}/rustfs{}", base_path.trim_end_matches('/'), i),
                ..Default::default()
            })
            .collect();

        // Mount in-memory volume for RustFS logs to avoid permissions issues on the root filesystem
        volume_mounts.push(corev1::VolumeMount {
            name: LOG_VOLUME_NAME.to_string(),
            mount_path: LOG_VOLUME_MOUNT_PATH.to_string(),
            ..Default::default()
        });

        // Generate environment variables: operator-managed + user-provided
        let mut env_vars = Vec::new();

        // Add RUSTFS_VOLUMES environment variable for multi-node communication
        let rustfs_volumes = self.rustfs_volumes_env_value()?;
        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_VOLUMES".to_owned(),
            value: Some(rustfs_volumes),
            ..Default::default()
        });

        // Add required RustFS environment variables
        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_ADDRESS".to_owned(),
            value: Some("0.0.0.0:9000".to_owned()),
            ..Default::default()
        });

        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_CONSOLE_ADDRESS".to_owned(),
            value: Some("0.0.0.0:9001".to_owned()),
            ..Default::default()
        });

        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_CONSOLE_ENABLE".to_owned(),
            value: Some("true".to_owned()),
            ..Default::default()
        });

        // Add credentials from Secret if credsSecret is specified
        if let Some(ref cfg) = self.spec.creds_secret
            && !cfg.name.is_empty()
        {
            env_vars.push(corev1::EnvVar {
                name: "RUSTFS_ACCESS_KEY".to_owned(),
                value_from: Some(corev1::EnvVarSource {
                    secret_key_ref: Some(corev1::SecretKeySelector {
                        name: cfg.name.clone(),
                        key: "accesskey".to_string(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });

            env_vars.push(corev1::EnvVar {
                name: "RUSTFS_SECRET_KEY".to_owned(),
                value_from: Some(corev1::EnvVarSource {
                    secret_key_ref: Some(corev1::SecretKeySelector {
                        name: cfg.name.clone(),
                        key: "secretkey".to_string(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        // Merge with user-provided environment variables
        // User-provided vars can override operator-managed ones
        for user_env in &self.spec.env {
            // Remove any existing var with the same name to allow override
            env_vars.retain(|e| e.name != user_env.name);
            env_vars.push(user_env.clone());
        }

        // Use an in-memory volume for logs to avoid permission issues on container filesystems
        let pod_volumes = vec![corev1::Volume {
            name: LOG_VOLUME_NAME.to_string(),
            empty_dir: Some(corev1::EmptyDirVolumeSource::default()),
            ..Default::default()
        }];

        // Enforce non-root execution and make mounted volumes writable by RustFS user
        let pod_security_context = Some(corev1::PodSecurityContext {
            run_as_user: Some(DEFAULT_RUN_AS_USER),
            run_as_group: Some(DEFAULT_RUN_AS_GROUP),
            fs_group: Some(DEFAULT_FS_GROUP),
            fs_group_change_policy: Some("OnRootMismatch".to_string()),
            ..Default::default()
        });

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
                    container_port: 9001,
                    name: Some("console".to_owned()),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                },
            ]),
            volume_mounts: Some(volume_mounts),
            lifecycle: self.spec.lifecycle.clone(),
            // Apply pool-level resource requirements to container
            resources: pool.scheduling.resources.clone(),
            image_pull_policy: self
                .spec
                .image_pull_policy
                .as_ref()
                .map(ToString::to_string),
            ..Default::default()
        };

        Ok(v1::StatefulSet {
            metadata: metav1::ObjectMeta {
                name: Some(stateful_name(self, pool)),
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
                    .map(ToString::to_string),
                selector: metav1::LabelSelector {
                    match_labels: Some(selector_labels),
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
                        security_context: pod_security_context,
                        volumes: Some(pod_volumes),
                        scheduler_name: self.spec.scheduler.clone(),
                        // Pool-level priority class overrides tenant-level
                        priority_class_name: pool
                            .scheduling
                            .priority_class_name
                            .clone()
                            .or_else(|| self.spec.priority_class_name.clone()),
                        // Pool-level scheduling controls
                        node_selector: pool.scheduling.node_selector.clone(),
                        affinity: pool.scheduling.affinity.clone(),
                        tolerations: pool.scheduling.tolerations.clone(),
                        topology_spread_constraints: pool
                            .scheduling
                            .topology_spread_constraints
                            .clone(),
                        ..Default::default()
                    }),
                },
                volume_claim_templates: Some(volume_claim_templates),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// Checks if a StatefulSet needs to be updated based on differences between
    /// the existing StatefulSet and the desired state defined in the Tenant spec.
    ///
    /// This method performs a semantic comparison of key StatefulSet fields to
    /// determine if an update is necessary, avoiding unnecessary API calls.
    ///
    /// # Returns
    /// - `Ok(true)` if the StatefulSet needs to be updated
    /// - `Ok(false)` if the StatefulSet matches the desired state
    /// - `Err` if comparison fails
    pub fn statefulset_needs_update(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
    ) -> Result<bool, types::error::Error> {
        let desired = self.new_statefulset(pool)?;

        // Compare key spec fields that should trigger updates
        let existing_spec = existing
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Existing StatefulSet missing spec".to_string(),
            })?;

        let desired_spec = desired
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Desired StatefulSet missing spec".to_string(),
            })?;

        // Check replicas (server count)
        if existing_spec.replicas != desired_spec.replicas {
            return Ok(true);
        }

        // Check pod management policy
        if existing_spec.pod_management_policy != desired_spec.pod_management_policy {
            return Ok(true);
        }

        // Compare pod template spec
        let existing_template = &existing_spec.template;
        let desired_template = &desired_spec.template;

        // Check if pod template metadata labels changed
        if existing_template
            .metadata
            .as_ref()
            .and_then(|m| m.labels.as_ref())
            != desired_template
                .metadata
                .as_ref()
                .and_then(|m| m.labels.as_ref())
        {
            return Ok(true);
        }

        let existing_pod_spec =
            existing_template
                .spec
                .as_ref()
                .ok_or(types::error::Error::InternalError {
                    msg: "Existing pod template missing spec".to_string(),
                })?;

        let desired_pod_spec =
            desired_template
                .spec
                .as_ref()
                .ok_or(types::error::Error::InternalError {
                    msg: "Desired pod template missing spec".to_string(),
                })?;

        // Check service account
        if existing_pod_spec.service_account_name != desired_pod_spec.service_account_name {
            return Ok(true);
        }

        // Check scheduler
        if existing_pod_spec.scheduler_name != desired_pod_spec.scheduler_name {
            return Ok(true);
        }

        // Check priority class
        if existing_pod_spec.priority_class_name != desired_pod_spec.priority_class_name {
            return Ok(true);
        }

        // Check node selector
        if existing_pod_spec.node_selector != desired_pod_spec.node_selector {
            return Ok(true);
        }

        // Check affinity (compare as JSON to handle deep equality)
        if serde_json::to_value(&existing_pod_spec.affinity)?
            != serde_json::to_value(&desired_pod_spec.affinity)?
        {
            return Ok(true);
        }

        // Check tolerations
        if serde_json::to_value(&existing_pod_spec.tolerations)?
            != serde_json::to_value(&desired_pod_spec.tolerations)?
        {
            return Ok(true);
        }

        // Check topology spread constraints
        if serde_json::to_value(&existing_pod_spec.topology_spread_constraints)?
            != serde_json::to_value(&desired_pod_spec.topology_spread_constraints)?
        {
            return Ok(true);
        }

        // Compare container specs
        if existing_pod_spec.containers.is_empty() || desired_pod_spec.containers.is_empty() {
            return Err(types::error::Error::InternalError {
                msg: "Pod spec missing container".to_string(),
            });
        }

        let existing_container = &existing_pod_spec.containers[0];
        let desired_container = &desired_pod_spec.containers[0];

        // Check image
        if existing_container.image != desired_container.image {
            return Ok(true);
        }

        // Check image pull policy
        if existing_container.image_pull_policy != desired_container.image_pull_policy {
            return Ok(true);
        }

        // Check environment variables (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.env)?
            != serde_json::to_value(&desired_container.env)?
        {
            return Ok(true);
        }

        // Check resources (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.resources)?
            != serde_json::to_value(&desired_container.resources)?
        {
            return Ok(true);
        }

        // Check lifecycle hooks
        if serde_json::to_value(&existing_container.lifecycle)?
            != serde_json::to_value(&desired_container.lifecycle)?
        {
            return Ok(true);
        }

        // Check volume mounts (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.volume_mounts)?
            != serde_json::to_value(&desired_container.volume_mounts)?
        {
            return Ok(true);
        }

        // If we reach here, no updates are needed
        Ok(false)
    }

    /// Validates that a StatefulSet update is safe by checking for changes to
    /// immutable fields that would cause API rejection.
    ///
    /// StatefulSet has several immutable fields that cannot be changed after creation:
    /// - spec.selector: Pod selector labels cannot be modified
    /// - spec.volumeClaimTemplates: PVC templates cannot be modified
    /// - spec.serviceName: Headless service name cannot be changed
    ///
    /// # Returns
    /// - `Ok(())` if the update is safe
    /// - `Err` if the update would modify immutable fields
    pub fn validate_statefulset_update(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
    ) -> Result<(), types::error::Error> {
        let desired = self.new_statefulset(pool)?;

        let existing_spec = existing
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Existing StatefulSet missing spec".to_string(),
            })?;

        let desired_spec = desired
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Desired StatefulSet missing spec".to_string(),
            })?;

        let ss_name = existing
            .metadata
            .name
            .as_ref()
            .unwrap_or(&"<unknown>".to_string())
            .clone();

        // Validate selector is unchanged (immutable field)
        if serde_json::to_value(&existing_spec.selector)?
            != serde_json::to_value(&desired_spec.selector)?
        {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.selector".to_string(),
                message: "StatefulSet selector cannot be modified. Pool name may have changed."
                    .to_string(),
            });
        }

        // Validate serviceName is unchanged (immutable field)
        if existing_spec.service_name != desired_spec.service_name {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.serviceName".to_string(),
                message: "StatefulSet serviceName cannot be modified.".to_string(),
            });
        }

        // Validate volumeClaimTemplates are unchanged (immutable field)
        // Note: This is a simplified check. In reality, you can only change certain fields
        // like storage size (depending on storage class), but template structure and names cannot change.
        let existing_vcts = existing_spec.volume_claim_templates.as_ref();
        let desired_vcts = desired_spec.volume_claim_templates.as_ref();

        // Check if the number of volume claim templates changed
        let existing_vct_count = existing_vcts.map(|v| v.len()).unwrap_or(0);
        let desired_vct_count = desired_vcts.map(|v| v.len()).unwrap_or(0);

        if existing_vct_count != desired_vct_count {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.volumeClaimTemplates".to_string(),
                message: format!(
                    "Cannot change volumesPerServer from {} to {}. This would modify volumeClaimTemplates which is immutable.",
                    existing_vct_count, desired_vct_count
                ),
            });
        }

        // Check if volume claim template names changed (indicates structure change)
        if let (Some(existing_vcts), Some(desired_vcts)) = (existing_vcts, desired_vcts) {
            for (i, (existing_vct, desired_vct)) in
                existing_vcts.iter().zip(desired_vcts.iter()).enumerate()
            {
                let existing_name = existing_vct.metadata.name.as_deref().unwrap_or("");
                let desired_name = desired_vct.metadata.name.as_deref().unwrap_or("");

                if existing_name != desired_name {
                    return Err(types::error::Error::ImmutableFieldModified {
                        name: ss_name,
                        field: format!("spec.volumeClaimTemplates[{}].metadata.name", i),
                        message: format!(
                            "Volume claim template name changed from '{}' to '{}'. This is not allowed.",
                            existing_name, desired_name
                        ),
                    });
                }

                // Check if storage class changed (also problematic)
                let existing_sc = existing_vct
                    .spec
                    .as_ref()
                    .and_then(|s| s.storage_class_name.as_ref());
                let desired_sc = desired_vct
                    .spec
                    .as_ref()
                    .and_then(|s| s.storage_class_name.as_ref());

                if existing_sc != desired_sc {
                    return Err(types::error::Error::ImmutableFieldModified {
                        name: ss_name.clone(),
                        field: format!("spec.volumeClaimTemplates[{}].spec.storageClassName", i),
                        message: format!(
                            "Storage class changed from '{:?}' to '{:?}'. This is not allowed.",
                            existing_sc, desired_sc
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        DEFAULT_FS_GROUP, DEFAULT_RUN_AS_GROUP, DEFAULT_RUN_AS_USER, LOG_VOLUME_MOUNT_PATH,
        LOG_VOLUME_NAME,
    };
    use k8s_openapi::api::core::v1 as corev1;

    // Test: Pod runs as non-root and mounts writable log volume
    #[test]
    fn test_statefulset_sets_security_context_and_log_volume() {
        let tenant = crate::tests::create_test_tenant(None, None);
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

        let security_context = pod_spec
            .security_context
            .as_ref()
            .expect("Pod should have securityContext");

        assert_eq!(
            security_context.run_as_user,
            Some(DEFAULT_RUN_AS_USER),
            "Pod should run as RustFS user"
        );
        assert_eq!(
            security_context.run_as_group,
            Some(DEFAULT_RUN_AS_GROUP),
            "Pod should use RustFS primary group"
        );
        assert_eq!(
            security_context.fs_group,
            Some(DEFAULT_FS_GROUP),
            "Mounted volumes should be owned by RustFS group"
        );
        assert_eq!(
            security_context.fs_group_change_policy,
            Some("OnRootMismatch".to_string()),
            "fsGroup change policy should be set for PVC mounts"
        );

        let volumes = pod_spec
            .volumes
            .as_ref()
            .expect("Pod should define volumes including logs");
        let log_volume = volumes
            .iter()
            .find(|v| v.name == LOG_VOLUME_NAME)
            .expect("Logs volume should be present");
        assert!(
            log_volume.empty_dir.is_some(),
            "Logs volume should be an EmptyDir"
        );

        let container = &pod_spec.containers[0];
        let log_mount = container
            .volume_mounts
            .as_ref()
            .and_then(|mounts| mounts.iter().find(|m| m.name == LOG_VOLUME_NAME))
            .expect("Container should mount logs volume");
        assert_eq!(
            log_mount.mount_path, LOG_VOLUME_MOUNT_PATH,
            "Logs volume should mount at /logs"
        );
    }

    // Test: StatefulSet uses correct service account
    #[test]
    fn test_statefulset_uses_default_sa() {
        let tenant = crate::tests::create_test_tenant(None, None);
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
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));
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

    // Test: StatefulSet applies pool-level node selector
    #[test]
    fn test_statefulset_applies_node_selector() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let mut node_selector = std::collections::BTreeMap::new();
        node_selector.insert("storage-type".to_string(), "nvme".to_string());
        tenant.spec.pools[0].scheduling.node_selector = Some(node_selector.clone());

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
            pod_spec.node_selector,
            Some(node_selector),
            "Pod should use pool-level node selector"
        );
    }

    // Test: StatefulSet applies pool-level tolerations
    #[test]
    fn test_statefulset_applies_tolerations() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let tolerations = vec![corev1::Toleration {
            key: Some("spot-instance".to_string()),
            operator: Some("Equal".to_string()),
            value: Some("true".to_string()),
            effect: Some("NoSchedule".to_string()),
            ..Default::default()
        }];
        tenant.spec.pools[0].scheduling.tolerations = Some(tolerations.clone());

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
            pod_spec.tolerations,
            Some(tolerations),
            "Pod should use pool-level tolerations"
        );
    }

    // Test: Pool-level priority class overrides tenant-level
    #[test]
    fn test_pool_priority_class_overrides_tenant() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.priority_class_name = Some("tenant-priority".to_string());
        tenant.spec.pools[0].scheduling.priority_class_name = Some("pool-priority".to_string());

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
            pod_spec.priority_class_name,
            Some("pool-priority".to_string()),
            "Pool-level priority class should override tenant-level"
        );
    }

    // Test: Tenant-level priority class used when pool-level not set
    #[test]
    fn test_tenant_priority_class_fallback() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.priority_class_name = Some("tenant-priority".to_string());
        // pool.priority_class_name remains None

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
            pod_spec.priority_class_name,
            Some("tenant-priority".to_string()),
            "Should fall back to tenant-level priority class when pool-level not set"
        );
    }

    // Test: Pool-level resources applied to container
    #[test]
    fn test_pool_resources_applied_to_container() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let mut requests = std::collections::BTreeMap::new();
        requests.insert(
            "cpu".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("4".to_string()),
        );
        requests.insert(
            "memory".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("16Gi".to_string()),
        );

        tenant.spec.pools[0].scheduling.resources = Some(corev1::ResourceRequirements {
            requests: Some(requests.clone()),
            limits: None,
            claims: None,
        });

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let container = &statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec")
            .containers[0];

        assert!(
            container.resources.is_some(),
            "Container should have resources"
        );
        assert_eq!(
            container.resources.as_ref().unwrap().requests,
            Some(requests),
            "Container should use pool-level resource requests"
        );
    }

    // Test: StatefulSet diff detection - no changes needed
    #[test]
    fn test_statefulset_no_update_needed() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Check if update is needed comparing StatefulSet to itself
        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            !needs_update,
            "StatefulSet should not need update when comparing to itself"
        );
    }

    // Test: StatefulSet diff detection - image change
    #[test]
    fn test_statefulset_image_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some("rustfs:v1".to_string());
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change image
        tenant.spec.image = Some("rustfs:v2".to_string());

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when image changes"
        );
    }

    // Test: StatefulSet diff detection - replicas change
    #[test]
    fn test_statefulset_replicas_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 4;
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change replicas
        tenant.spec.pools[0].servers = 6;
        let pool = &tenant.spec.pools[0];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when replicas change"
        );
    }

    // Test: StatefulSet diff detection - environment variable change
    #[test]
    fn test_statefulset_env_change_detected() {
        use k8s_openapi::api::core::v1 as corev1;

        let mut tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Add environment variable
        tenant.spec.env = vec![corev1::EnvVar {
            name: "NEW_VAR".to_string(),
            value: Some("value".to_string()),
            ..Default::default()
        }];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when env vars change"
        );
    }

    // Test: StatefulSet diff detection - resources change
    #[test]
    fn test_statefulset_resources_change_detected() {
        use k8s_openapi::api::core::v1 as corev1;

        let mut tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Add resource requirements
        let mut requests = std::collections::BTreeMap::new();
        requests.insert(
            "cpu".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("2".to_string()),
        );

        tenant.spec.pools[0].scheduling.resources = Some(corev1::ResourceRequirements {
            requests: Some(requests),
            limits: None,
            claims: None,
        });
        let pool = &tenant.spec.pools[0];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when resources change"
        );
    }

    // Test: StatefulSet validation - selector change rejected
    #[test]
    fn test_statefulset_selector_change_rejected() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Modify selector (immutable field)
        if let Some(ref mut spec) = statefulset.spec
            && let Some(ref mut labels) = spec.selector.match_labels
        {
            labels.insert("modified".to_string(), "true".to_string());
        }

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when selector changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, .. } => {
                assert_eq!(
                    field, "spec.selector",
                    "Error should indicate selector field"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - serviceName change rejected
    #[test]
    fn test_statefulset_service_name_change_rejected() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Modify serviceName (immutable field)
        if let Some(ref mut spec) = statefulset.spec {
            spec.service_name = Some("different-service".to_string());
        }

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when serviceName changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, .. } => {
                assert_eq!(
                    field, "spec.serviceName",
                    "Error should indicate serviceName field"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - volumesPerServer change rejected
    #[test]
    fn test_statefulset_volumes_per_server_change_rejected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].persistence.volumes_per_server = 2;
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change volumesPerServer (would modify volumeClaimTemplates - immutable)
        tenant.spec.pools[0].persistence.volumes_per_server = 4;
        let pool = &tenant.spec.pools[0];

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when volumesPerServer changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, message, .. } => {
                assert_eq!(
                    field, "spec.volumeClaimTemplates",
                    "Error should indicate volumeClaimTemplates field"
                );
                assert!(
                    message.contains("volumesPerServer"),
                    "Error message should mention volumesPerServer"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - safe update allowed
    #[test]
    fn test_statefulset_safe_update_allowed() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some("rustfs:v1".to_string());
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change image (safe update)
        tenant.spec.image = Some("rustfs:v2".to_string());

        // Validation should pass
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_ok(),
            "Validation should pass for safe updates like image changes"
        );
    }
}
