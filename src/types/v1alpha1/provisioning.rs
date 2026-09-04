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
use std::collections::BTreeSet;
use utoipa::ToSchema;

pub(crate) const MAX_CONFIG_MAP_REF_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_CONFIG_MAP_REF_KEY_LENGTH: u32 = 253;
pub(crate) const MAX_PROVISIONING_POLICY_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_PROVISIONING_USER_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_USER_CREDENTIALS_SECRET_NAME_LENGTH: u32 = 253;
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

/// Reference to a user credentials Secret in the Tenant namespace.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserCredentialsSecretRef {
    #[schemars(length(min = 1, max = MAX_USER_CREDENTIALS_SECRET_NAME_LENGTH))]
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningUser {
    #[schemars(length(min = 1, max = MAX_PROVISIONING_USER_NAME_LENGTH), regex(pattern = r"^\S+$"))]
    pub name: String,

    /// Optional credentials Secret reference. Defaults to a Secret named after the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creds_secret: Option<UserCredentialsSecretRef>,

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

impl ProvisioningUser {
    /// Resolves the credentials Secret name while preserving the legacy same-name convention.
    pub fn credentials_secret_name(&self) -> &str {
        self.creds_secret
            .as_ref()
            .map_or(self.name.as_str(), |reference| reference.name.as_str())
    }
}

/// Returns credentials Secret names selected by more than one provisioning user.
pub(crate) fn duplicate_user_credentials_secret_names(
    users: &[ProvisioningUser],
) -> BTreeSet<&str> {
    let mut seen = BTreeSet::new();
    users
        .iter()
        .filter_map(|user| {
            let secret_name = user.credentials_secret_name();
            (!seen.insert(secret_name)).then_some(secret_name)
        })
        .collect()
}

#[derive(
    Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq,
)]
#[serde(rename_all = "PascalCase")]
pub enum BucketAnonymousAccess {
    #[default]
    Private,
    Download,
    Upload,
    Public,
}

impl BucketAnonymousAccess {
    pub(crate) fn is_private(&self) -> bool {
        matches!(self, Self::Private)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!(has(self.policy) && has(self.anonymous))").message("bucket policy and anonymous access are mutually exclusive"))]
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

    /// Canned anonymous access for this bucket. Mutually exclusive with `policy`. Explicit
    /// `Private` removes an operator-managed bucket policy; omission leaves the live policy alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<BucketAnonymousAccess>,

    /// Custom bucket policy document sourced from a ConfigMap. Mutually exclusive with `anonymous`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyDocumentSource>,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
}

impl ProvisioningBucket {
    pub fn object_lock_enabled(&self) -> bool {
        self.object_lock.unwrap_or(false)
    }

    pub(crate) fn has_custom_policy(&self) -> bool {
        self.policy.is_some()
    }

    pub(crate) fn has_anonymous_access(&self) -> bool {
        self.anonymous.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProvisioningUser, UserCredentialsSecretRef, duplicate_user_credentials_secret_names,
    };
    use std::collections::BTreeSet;

    #[test]
    fn user_credentials_secret_defaults_to_user_name() {
        let user: ProvisioningUser = serde_json::from_value(serde_json::json!({
            "name": "app-user",
            "policies": ["app-readwrite"]
        }))
        .expect("legacy ProvisioningUser should deserialize");

        assert_eq!(user.credentials_secret_name(), "app-user");
        let value = serde_json::to_value(user).expect("ProvisioningUser should serialize");
        assert!(value.get("credsSecret").is_none());
    }

    #[test]
    fn user_credentials_secret_uses_explicit_reference() {
        let user = ProvisioningUser {
            name: "app-user".to_string(),
            creds_secret: Some(UserCredentialsSecretRef {
                name: "rustfs-user-app-user".to_string(),
            }),
            ..Default::default()
        };

        assert_eq!(user.credentials_secret_name(), "rustfs-user-app-user");
    }

    #[test]
    fn duplicate_credentials_secret_names_include_legacy_resolution() {
        let users = [
            ProvisioningUser {
                name: "shared-secret".to_string(),
                ..Default::default()
            },
            ProvisioningUser {
                name: "app-user".to_string(),
                creds_secret: Some(UserCredentialsSecretRef {
                    name: "shared-secret".to_string(),
                }),
                ..Default::default()
            },
            ProvisioningUser {
                name: "report-user".to_string(),
                creds_secret: Some(UserCredentialsSecretRef {
                    name: "report-user-secret".to_string(),
                }),
                ..Default::default()
            },
        ];

        assert_eq!(
            duplicate_user_credentials_secret_names(&users),
            BTreeSet::from(["shared-secret"])
        );
    }

    #[test]
    fn omitted_and_explicit_private_anonymous_access_remain_distinct() {
        let bucket = super::ProvisioningBucket {
            name: "app-data".to_string(),
            ..Default::default()
        };
        let value = serde_json::to_value(&bucket).expect("bucket serializes");
        assert!(value.get("anonymous").is_none());
        assert!(value.get("policy").is_none());

        let private: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "app-data",
            "anonymous": "Private"
        }))
        .expect("private anonymous deserializes");
        assert_eq!(
            private.anonymous,
            Some(super::BucketAnonymousAccess::Private)
        );
        assert_eq!(
            serde_json::to_value(&private).expect("private bucket serializes")["anonymous"],
            "Private"
        );

        let public: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "app-data",
            "anonymous": "Public"
        }))
        .expect("public anonymous deserializes");
        assert!(public.has_anonymous_access());
        assert!(!public.has_custom_policy());
    }
}
