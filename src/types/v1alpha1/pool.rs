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

use k8s_openapi::api::core::v1 as corev1;
use kube::KubeSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::types::v1alpha1::persistence::PersistenceConfig;

/// Kubernetes scheduling and placement configuration for pools.
/// Groups related scheduling fields for better code organization.
/// Uses #[serde(flatten)] to maintain flat YAML structure.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct SchedulingConfig {
    /// NodeSelector is a selector which must be true for the pod to fit on a node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<std::collections::BTreeMap<String, String>>,

    /// Affinity is a group of affinity scheduling rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affinity: Option<corev1::Affinity>,

    /// Tolerations allow pods to schedule onto nodes with matching taints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerations: Option<Vec<corev1::Toleration>>,

    /// TopologySpreadConstraints describes how pods should spread across topology domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_spread_constraints: Option<Vec<corev1::TopologySpreadConstraint>>,

    /// Resources describes the compute resource requirements for the pool's containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<corev1::ResourceRequirements>,

    /// PriorityClassName indicates the pod's priority. Overrides tenant-level priority class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    #[schemars(
        length(min = 1, max = 63),
        regex(pattern = r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$")
    )]
    pub name: String,

    #[x_kube(validation = Rule::new("self > 0").message("servers must be greater than 0"))]
    #[x_kube(validation = Rule::new("self == oldSelf").message("servers is immutable"))]
    pub servers: i32,

    pub persistence: PersistenceConfig,

    /// Kubernetes scheduling and placement configuration.
    /// Flattened to maintain backward compatibility with YAML structure.
    #[serde(flatten)]
    pub scheduling: SchedulingConfig,
}

impl Pool {
    pub fn is_single_node_single_disk(&self) -> bool {
        self.servers == 1 && self.persistence.volumes_per_server == 1
    }
}

/// Validate a pool name used in labels and RustFS peer DNS names.
pub fn validate_pool_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("pool name must be not empty".to_string());
    }
    if name.len() > 63 {
        return Err(format!(
            "pool name must be at most 63 characters, got {}",
            name.len()
        ));
    }

    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(
            "pool name must start with a lowercase alphanumeric character (a-z, 0-9)".to_string(),
        );
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(
            "pool name must end with a lowercase alphanumeric character (a-z, 0-9)".to_string(),
        );
    }
    for &b in bytes {
        if !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'-' {
            return Err(format!(
                "pool name contains invalid character '{}'; only lowercase alphanumeric and '-' are allowed",
                b as char
            ));
        }
    }

    Ok(())
}

pub fn validate_pool_collection(tenant_name: &str, pools: &[Pool]) -> Result<(), String> {
    if pools.is_empty() {
        return Err("pools must be configured".to_string());
    }

    let mut names = HashSet::new();
    for pool in pools {
        validate_pool_name(&pool.name)?;
        if !names.insert(pool.name.as_str()) {
            return Err(format!("pool names must be unique: '{}'", pool.name));
        }
        validate_rustfs_peer_dns_label(tenant_name, pool)?;
    }

    Ok(())
}

pub fn validate_pool_shape_immutable(existing: &[Pool], desired: &[Pool]) -> Result<(), String> {
    for desired_pool in desired {
        let Some(existing_pool) = existing
            .iter()
            .find(|existing_pool| existing_pool.name == desired_pool.name)
        else {
            continue;
        };

        if existing_pool.servers != desired_pool.servers {
            return Err(format!(
                "pool '{}' servers is immutable ({} -> {})",
                desired_pool.name, existing_pool.servers, desired_pool.servers
            ));
        }

        if existing_pool.persistence.volumes_per_server
            != desired_pool.persistence.volumes_per_server
        {
            return Err(format!(
                "pool '{}' volumesPerServer is immutable ({} -> {})",
                desired_pool.name,
                existing_pool.persistence.volumes_per_server,
                desired_pool.persistence.volumes_per_server
            ));
        }
    }

    Ok(())
}

fn validate_rustfs_peer_dns_label(tenant_name: &str, pool: &Pool) -> Result<(), String> {
    let max_ordinal = pool.servers.saturating_sub(1).max(0);
    let dns_label_len = tenant_name.len() + 1 + pool.name.len() + 1 + ordinal_digits(max_ordinal);

    if dns_label_len > 63 {
        return Err(format!(
            "pool '{}' makes RustFS peer DNS label too long: '{}-{}-<ordinal>' is {} characters at max ordinal {}, must be at most 63",
            pool.name, tenant_name, pool.name, dns_label_len, max_ordinal
        ));
    }

    Ok(())
}

fn ordinal_digits(value: i32) -> usize {
    value.to_string().len()
}

#[cfg(test)]
mod tests {
    use super::{validate_pool_collection, validate_pool_name};
    use crate::types::v1alpha1::persistence::PersistenceConfig;
    use crate::types::v1alpha1::pool::Pool;

    #[test]
    fn validates_pool_name_as_rfc1123_label() {
        assert!(validate_pool_name("pool-0").is_ok());
        assert!(validate_pool_name("0-pool").is_ok());
        assert!(validate_pool_name("Pool-0").is_err());
        assert!(validate_pool_name("-pool").is_err());
        assert!(validate_pool_name("pool-").is_err());
    }

    #[test]
    fn allows_rustfs_to_validate_storage_layouts() {
        let pools = vec![
            test_pool("pool-0", 1, 2),
            test_pool("pool-1", 2, 1),
            test_pool("pool-2", 3, 1),
            test_pool("pool-3", 17, 1),
        ];

        assert!(validate_pool_collection("tenant", &pools).is_ok());
    }

    #[test]
    fn rejects_duplicate_pool_names() {
        let pools = vec![test_pool("pool-0", 1, 1), test_pool("pool-0", 1, 2)];

        let err = validate_pool_collection("tenant", &pools).unwrap_err();

        assert!(err.contains("pool names must be unique"));
    }

    #[test]
    fn rejects_too_long_rustfs_peer_dns_labels() {
        let pools = vec![test_pool(
            "pool-name-that-makes-the-peer-label-too-long",
            4,
            4,
        )];

        let err =
            validate_pool_collection("tenant-name-that-is-already-quite-long", &pools).unwrap_err();

        assert!(err.contains("RustFS peer DNS label too long"));
    }

    fn test_pool(name: &str, servers: i32, volumes_per_server: i32) -> Pool {
        Pool {
            name: name.to_string(),
            servers,
            persistence: PersistenceConfig {
                volumes_per_server,
                ..Default::default()
            },
            scheduling: Default::default(),
        }
    }
}
