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

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<ProvisioningPhase>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<ProvisioningItemStatus>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<ProvisioningItemStatus>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<ProvisioningItemStatus>,
}

impl ProvisioningStatus {
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty() && self.users.is_empty() && self.buckets.is_empty()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningPhase {
    Pending,
    Ready,
    Failed,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningItemState {
    Pending,
    Ready,
    Failed,
    Retained,
}

impl ProvisioningItemState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Ready => "Ready",
            Self::Failed => "Failed",
            Self::Retained => "Retained",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningItemStatus {
    pub name: String,

    pub state: String,

    pub reason: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_transition_time: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_generation: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_secret_resource_version: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_access_key_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_lock: Option<bool>,
}

impl ProvisioningItemStatus {
    pub fn new(
        name: impl Into<String>,
        state: ProvisioningItemState,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            state: state.as_str().to_string(),
            reason: reason.into(),
            ..Default::default()
        }
    }
}
