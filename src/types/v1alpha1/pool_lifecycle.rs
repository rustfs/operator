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

use kube::KubeSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!has(self.decommissionRequests) || self.decommissionRequests.all(r, self.decommissionRequests.exists_one(other, other.poolName == r.poolName))").
    message("decommissionRequests must contain at most one entry per poolName"))]
pub struct PoolLifecycleSpec {
    #[serde(default, skip_serializing_if = "is_default_pvc_retention_policy")]
    pub pvc_retention_policy: PvcRetentionPolicy,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decommission_requests: Vec<DecommissionRequest>,
}

#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema, Display, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PvcRetentionPolicy {
    #[strum(to_string = "Retain")]
    #[default]
    Retain,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DecommissionRequest {
    pub pool_name: String,
    pub request_id: String,
    pub action: DecommissionAction,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Display, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum DecommissionAction {
    #[strum(to_string = "Start")]
    Start,

    #[strum(to_string = "Cancel")]
    Cancel,
}

fn is_default_pvc_retention_policy(policy: &PvcRetentionPolicy) -> bool {
    policy == &PvcRetentionPolicy::Retain
}

impl PoolLifecycleSpec {
    pub fn request_for_pool(&self, pool_name: &str) -> Option<&DecommissionRequest> {
        self.decommission_requests
            .iter()
            .find(|request| request.pool_name == pool_name)
    }
}
