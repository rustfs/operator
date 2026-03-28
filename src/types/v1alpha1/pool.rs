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
#[x_kube(validation = Rule::new("self.servers * self.persistence.volumesPerServer >= 4").
    message(Message::Expression(r#""pool " + self.name + " must have at least 4 total volumes (servers × volumesPerServer)""#.into())).
    reason(Reason::FieldValueInvalid))
]
#[x_kube(validation = Rule::new("self.servers != 3 || self.servers * self.persistence.volumesPerServer >= 6").
    message(Message::Expression(r#""pool " + self.name + " with 3 servers must have at least 6 volumes in total""#.into())).
    reason(Reason::FieldValueInvalid))
]
pub struct Pool {
    #[x_kube(validation = Rule::new("self != ''").message("pool name must be not empty"))]
    pub name: String,

    #[x_kube(validation = Rule::new("self > 0").message("servers must be greater than 0"))]
    pub servers: i32,

    pub persistence: PersistenceConfig,

    /// Kubernetes scheduling and placement configuration.
    /// Flattened to maintain backward compatibility with YAML structure.
    #[serde(flatten)]
    pub scheduling: SchedulingConfig,
}

/// Validates total volume count (`servers * volumesPerServer`) for RustFS erasure coding.
/// Same rules as CRD CEL on [`Pool`] and the operator console API (`validate_pool_volumes`).
pub fn validate_pool_total_volumes(servers: i32, volumes_per_server: i32) -> Result<i32, String> {
    let total = servers * volumes_per_server;
    if servers <= 0 || volumes_per_server <= 0 {
        return Err("servers and volumes_per_server must be positive".to_string());
    }
    if servers == 2 && total < 4 {
        return Err("Pool with 2 servers must have at least 4 volumes in total".to_string());
    }
    if servers == 3 && total < 6 {
        return Err("Pool with 3 servers must have at least 6 volumes in total".to_string());
    }
    if total < 4 {
        return Err(format!(
            "Pool must have at least 4 total volumes (got {} servers × {} volumes = {})",
            servers, volumes_per_server, total
        ));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::validate_pool_total_volumes;

    #[test]
    fn rejects_non_positive_inputs() {
        assert!(validate_pool_total_volumes(0, 4).is_err());
        assert!(validate_pool_total_volumes(4, 0).is_err());
    }

    #[test]
    fn rejects_total_under_four_when_not_caught_by_special_cases() {
        assert!(validate_pool_total_volumes(1, 3).is_err());
        assert!(validate_pool_total_volumes(1, 2).is_err());
    }

    #[test]
    fn four_servers_one_volume_each_is_ok() {
        assert_eq!(validate_pool_total_volumes(4, 1).unwrap(), 4);
    }

    #[test]
    fn two_servers_need_at_least_four_total() {
        assert!(validate_pool_total_volumes(2, 1).is_err());
        assert_eq!(validate_pool_total_volumes(2, 2).unwrap(), 4);
    }

    #[test]
    fn three_servers_need_at_least_six_total() {
        assert!(validate_pool_total_volumes(3, 1).is_err());
        assert_eq!(validate_pool_total_volumes(3, 2).unwrap(), 6);
    }

    #[test]
    fn accepts_common_valid_configs() {
        assert_eq!(validate_pool_total_volumes(1, 4).unwrap(), 4);
        assert_eq!(validate_pool_total_volumes(4, 1).unwrap(), 4);
        assert_eq!(validate_pool_total_volumes(2, 2).unwrap(), 4);
    }
}
