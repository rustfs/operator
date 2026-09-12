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
use std::ops::{Deref, DerefMut};
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
    pub users: Vec<ProvisioningUserStatus>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<ProvisioningBucketStatus>,
}

impl ProvisioningStatus {
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty() && self.users.is_empty() && self.buckets.is_empty()
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
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

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ProvisioningUserOwnershipState {
    PendingCreate,
    Managed,
}

/// Durable proof that the operator claimed a RustFS user identity before mutating it.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningUserOwnershipStatus {
    pub state: ProvisioningUserOwnershipState,
    pub tenant_uid: String,
    pub user_name: String,
    pub access_key_hash: String,
}

/// User-specific provisioning status. The flattened item preserves the existing status wire
/// format while keeping ownership metadata out of policy and bucket status schemas.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningUserStatus {
    #[serde(flatten)]
    pub item: ProvisioningItemStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<ProvisioningUserOwnershipStatus>,
}

impl ProvisioningUserStatus {
    pub fn new(item: ProvisioningItemStatus) -> Self {
        Self {
            item,
            ownership: None,
        }
    }
}

impl Deref for ProvisioningUserStatus {
    type Target = ProvisioningItemStatus;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl DerefMut for ProvisioningUserStatus {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl AsRef<ProvisioningItemStatus> for ProvisioningUserStatus {
    fn as_ref(&self) -> &ProvisioningItemStatus {
        &self.item
    }
}

/// Bucket-specific provisioning status. Lifecycle ownership hashes are separate from the
/// existing bucket-policy hashes while preserving the established flattened wire format.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningBucketStatus {
    #[serde(flatten)]
    pub item: ProvisioningItemStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_desired_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_last_applied_hash: Option<String>,
}

impl ProvisioningBucketStatus {
    pub fn new(item: ProvisioningItemStatus) -> Self {
        Self {
            item,
            lifecycle_desired_hash: None,
            lifecycle_last_applied_hash: None,
        }
    }
}

impl Deref for ProvisioningBucketStatus {
    type Target = ProvisioningItemStatus;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl DerefMut for ProvisioningBucketStatus {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl AsRef<ProvisioningItemStatus> for ProvisioningBucketStatus {
    fn as_ref(&self) -> &ProvisioningItemStatus {
        &self.item
    }
}

impl From<ProvisioningItemStatus> for ProvisioningBucketStatus {
    fn from(item: ProvisioningItemStatus) -> Self {
        Self::new(item)
    }
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
    pub observed_secret_name: Option<String>,

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

impl AsRef<ProvisioningItemStatus> for ProvisioningItemStatus {
    fn as_ref(&self) -> &ProvisioningItemStatus {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bucket_status_deserializes_without_lifecycle_hashes() {
        let status: ProvisioningStatus = serde_json::from_value(serde_json::json!({
            "buckets": [{
                "name": "app-data",
                "state": "Ready",
                "reason": "ProvisioningConfigured",
                "lastAppliedHash": "bucket-policy-hash"
            }]
        }))
        .expect("legacy bucket status should deserialize");

        assert_eq!(
            status.buckets[0].last_applied_hash.as_deref(),
            Some("bucket-policy-hash")
        );
        assert!(status.buckets[0].lifecycle_desired_hash.is_none());
        assert!(status.buckets[0].lifecycle_last_applied_hash.is_none());
    }
}
