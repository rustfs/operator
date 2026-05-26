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
    #[x_kube(validation = Rule::new("self != ''").message("configMapKeyRef name must be not empty"))]
    pub name: String,

    #[x_kube(validation = Rule::new("self != ''").message("configMapKeyRef key must be not empty"))]
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
    #[x_kube(validation = Rule::new("self != '' && !self.matches('.*\\\\s.*')").message("policy name must be not empty and must not contain whitespace"))]
    pub name: String,

    pub document: PolicyDocumentSource,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningUser {
    #[x_kube(validation = Rule::new("self != '' && !self.matches('.*\\\\s.*')").message("user Secret name must be not empty and must not contain whitespace"))]
    pub name: String,

    #[x_kube(validation = Rule::new("self.all(x, x != '' && !x.matches('.*\\\\s.*'))").message("user policy names must be not empty and must not contain whitespace"))]
    #[x_kube(validation = Rule::new("self.all(x, self.filter(y, y == x).size() == 1)").message("user policy names must be unique"))]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningBucket {
    #[x_kube(validation = Rule::new("self.size() >= 3 && self.size() <= 63 && self != 'rustfs' && !self.matches('^(\\\\d+\\\\.){3}\\\\d+$') && !self.contains('..') && !self.contains('.-') && !self.contains('-.') && self.matches('^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$')").message("bucket name must be a valid RustFS/S3 bucket name"))]
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
