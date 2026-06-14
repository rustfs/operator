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
use utoipa::ToSchema;

pub(crate) const MAX_CONFIG_MAP_REF_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_CONFIG_MAP_REF_KEY_LENGTH: u32 = 253;
pub(crate) const MAX_PROVISIONING_POLICY_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_PROVISIONING_USER_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_POLICIES_PER_USER: u32 = 64;
pub(crate) const MAX_USER_POLICY_NAME_LENGTH: u32 = 253;
pub(crate) const MIN_BUCKET_NAME_LENGTH: u32 = 3;
pub(crate) const MAX_BUCKET_NAME_LENGTH: u32 = 63;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningDeletionPolicy {
    #[default]
    Retain,
}

pub fn is_retain(policy: &ProvisioningDeletionPolicy) -> bool {
    matches!(policy, ProvisioningDeletionPolicy::Retain)
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMapKeyReference {
    #[schemars(length(min = 1, max = MAX_CONFIG_MAP_REF_NAME_LENGTH))]
    pub name: String,

    #[schemars(length(min = 1, max = MAX_CONFIG_MAP_REF_KEY_LENGTH))]
    pub key: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDocumentSource {
    pub config_map_key_ref: ConfigMapKeyReference,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningPolicy {
    #[schemars(length(min = 1, max = MAX_PROVISIONING_POLICY_NAME_LENGTH), regex(pattern = r"^\S+$"))]
    pub name: String,

    pub document: PolicyDocumentSource,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningUser {
    #[schemars(length(min = 1, max = MAX_PROVISIONING_USER_NAME_LENGTH), regex(pattern = r"^\S+$"))]
    pub name: String,

    /// Canned policies to map directly to this user.
    #[schemars(
        length(min = 1, max = MAX_POLICIES_PER_USER),
        inner(length(min = 1, max = MAX_USER_POLICY_NAME_LENGTH), regex(pattern = r"^\S+$")),
        extend("x-kubernetes-list-type" = "set")
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningBucket {
    #[schemars(
        length(min = MIN_BUCKET_NAME_LENGTH, max = MAX_BUCKET_NAME_LENGTH),
        regex(pattern = r"^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$")
    )]
    #[x_kube(validation = Rule::new("self != 'rustfs' && !self.matches('^(\\\\d+\\\\.){3}\\\\d+$') && !self.contains('..') && !self.contains('.-') && !self.contains('-.')").message("bucket name must be a valid RustFS/S3 bucket name"))]
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_lock: Option<bool>,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

impl ProvisioningBucket {
    pub fn object_lock_enabled(&self) -> bool {
        self.object_lock.unwrap_or(false)
    }
}
