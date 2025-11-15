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

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersistenceConfig {
    #[x_kube(validation = Rule::new("self > 0").message("volumesPerServer must be greater than 0"))]
    pub volumes_per_server: i32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_claim_template: Option<corev1::PersistentVolumeClaimSpec>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[x_kube(validation = Rule::new("self != ''").message("path must be not empty when specified"))]
    pub path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<std::collections::BTreeMap<String, String>>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            volumes_per_server: 4, // Default to 4 volumes to satisfy validation (must be > 0)
            volume_claim_template: None,
            path: None,
            labels: None,
            annotations: None,
        }
    }
}
