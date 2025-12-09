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
pub mod certificate;
pub mod pool;
pub mod state;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kubernetes standard condition for Tenant resources
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    /// Type of condition (Ready, Progressing, Degraded)
    #[serde(rename = "type")]
    pub type_: String,

    /// Status of the condition (True, False, Unknown)
    pub status: String,

    /// Last time the condition transitioned from one status to another
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,

    /// The generation of the Tenant resource that this condition reflects
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// One-word CamelCase reason for the condition's last transition
    pub reason: String,

    /// Human-readable message indicating details about the transition
    pub message: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current_state: String,

    pub available_replicas: i32,

    pub pools: Vec<pool::Pool>,

    /// The generation observed by the operator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// Kubernetes standard conditions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    // pub certificates: certificate::Status,
}
