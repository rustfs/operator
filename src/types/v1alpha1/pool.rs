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
#[x_kube(validation = Rule::new("self.servers * self.persistence.volumesPerServer >= 4"))]
pub struct Pool {
    #[x_kube(validation = Rule::new("self != ''").message("pool name must be not empty"))]
    pub name: String,

    #[x_kube(validation = Rule::new("self > 0").message("servers must be gather than 0"))]
    pub servers: i32,

    pub persistence: PersistenceConfig,

    /// Kubernetes scheduling and placement configuration.
    /// Flattened to maintain backward compatibility with YAML structure.
    #[serde(flatten)]
    pub scheduling: SchedulingConfig,
}
