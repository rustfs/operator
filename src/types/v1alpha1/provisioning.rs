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
pub(crate) const MAX_BUCKET_LIFECYCLE_RULES: u32 = 1_000;
pub(crate) const MAX_BUCKET_LIFECYCLE_RULE_ID_LENGTH: u32 = 255;
pub(crate) const MAX_BUCKET_OBJECT_LOCK_RETENTION_DAYS: u32 = 36_500;
pub(crate) const MAX_PROVISIONING_TIER_NAME_LENGTH: u32 = 253;
pub(crate) const MAX_TIER_CREDENTIALS_SECRET_NAME_LENGTH: u32 = 253;

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

/// Remote backend of a tier. All three use the S3 API with static credentials.
#[derive(
    Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, Default, PartialEq, Eq,
)]
pub enum ProvisioningTierType {
    #[default]
    #[serde(rename = "S3")]
    S3,
    #[serde(rename = "RustFS")]
    RustFS,
    #[serde(rename = "MinIO")]
    MinIO,
}

impl ProvisioningTierType {
    /// Backend type as named by the RustFS tier admin API.
    pub const fn rustfs_type(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::RustFS => "rustfs",
            Self::MinIO => "minio",
        }
    }
}

/// Reference to a tier credentials Secret in the Tenant namespace.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TierCredentialsSecretRef {
    #[schemars(length(min = 1, max = MAX_TIER_CREDENTIALS_SECRET_NAME_LENGTH))]
    pub name: String,
}

/// Remote storage tier registered in RustFS for bucket lifecycle transitions.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningTier {
    /// Tier name, referenced as `storageClass` by bucket lifecycle transitions.
    #[schemars(length(min = 1, max = MAX_PROVISIONING_TIER_NAME_LENGTH))]
    #[x_kube(validation = Rule::new("self == self.upperAscii()").message("tier name must be uppercase"))]
    #[x_kube(validation = Rule::new("self != 'STANDARD' && self != 'REDUCED_REDUNDANCY'").message("tier name must not be a reserved storage class"))]
    pub name: String,

    #[serde(rename = "type")]
    pub tier_type: ProvisioningTierType,

    /// Remote S3 endpoint URL, for example `https://gateway.storjshare.io`.
    #[schemars(length(min = 1))]
    pub endpoint: String,

    /// Remote bucket that receives transitioned objects.
    #[schemars(length(min = 1))]
    pub bucket: String,

    /// Object prefix inside the remote bucket.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub region: String,

    /// Secret with `accesskey` and `secretkey` for the remote bucket. Updating the Secret
    /// rotates the tier credentials in RustFS.
    pub creds_secret: TierCredentialsSecretRef,

    /// Register the tier even when the remote prefix already holds objects, for example a
    /// prefix written by this Tenant before a restore. RustFS refuses such a prefix otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,

    #[serde(default, skip_serializing_if = "is_retain")]
    pub deletion_policy: ProvisioningDeletionPolicy,
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

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BucketLifecycleState {
    Present,
    Absent,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BucketLifecycleRuleStatus {
    Enabled,
    Disabled,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BucketObjectLockRetentionMode {
    Governance,
    Compliance,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum BucketObjectLockConfigurationState {
    Present,
    Absent,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("(!has(self.state) || self.state == 'Present') ? has(self.mode) && has(self.days) : !has(self.mode) && !has(self.days)").message("Present object lock configuration requires mode and days; Absent forbids them"))]
pub struct BucketObjectLockConfiguration {
    /// Omission is equivalent to `Present`, preserving the concise issue-compatible YAML shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<BucketObjectLockConfigurationState>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<BucketObjectLockRetentionMode>,

    #[schemars(range(min = 1, max = MAX_BUCKET_OBJECT_LOCK_RETENTION_DAYS))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<i32>,
}

impl BucketObjectLockConfiguration {
    pub(crate) fn state(&self) -> BucketObjectLockConfigurationState {
        self.state
            .unwrap_or(BucketObjectLockConfigurationState::Present)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketLifecycleRuleFilter {
    /// Object-key prefix selected by this rule. An empty prefix selects all objects.
    pub prefix: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketLifecycleExpiration {
    #[schemars(range(min = 1))]
    pub days: i32,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketLifecycleAbortIncompleteMultipartUpload {
    #[schemars(range(min = 1))]
    pub days_after_initiation: i32,
}

/// Moves current object versions to a remote tier after `days`. `storageClass` names a tier
/// registered in RustFS (uppercase); RustFS rejects a lifecycle that references an unknown tier.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketLifecycleTransition {
    #[schemars(range(min = 0))]
    pub days: i32,

    #[schemars(length(min = 1))]
    pub storage_class: String,
}

/// Moves noncurrent object versions to a remote tier `noncurrentDays` after they become
/// noncurrent. Only meaningful on versioned buckets.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketLifecycleNoncurrentVersionTransition {
    #[schemars(range(min = 0))]
    pub noncurrent_days: i32,

    #[schemars(length(min = 1))]
    pub storage_class: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.expiration) || has(self.abortIncompleteMultipartUpload) || has(self.transition) || has(self.noncurrentVersionTransition)").message("lifecycle rule must configure expiration, abortIncompleteMultipartUpload, transition, or noncurrentVersionTransition"))]
pub struct BucketLifecycleRule {
    #[schemars(length(min = 1, max = MAX_BUCKET_LIFECYCLE_RULE_ID_LENGTH))]
    pub id: String,

    pub status: BucketLifecycleRuleStatus,

    pub filter: BucketLifecycleRuleFilter,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration: Option<BucketLifecycleExpiration>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_incomplete_multipart_upload: Option<BucketLifecycleAbortIncompleteMultipartUpload>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<BucketLifecycleTransition>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noncurrent_version_transition: Option<BucketLifecycleNoncurrentVersionTransition>,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.state == 'Present' ? has(self.rules) && self.rules.size() > 0 : !has(self.rules) || self.rules.size() == 0").message("Present lifecycle requires rules and Absent lifecycle forbids rules"))]
pub struct BucketLifecycleSpec {
    pub state: BucketLifecycleState,

    #[schemars(
        length(max = MAX_BUCKET_LIFECYCLE_RULES),
        extend("x-kubernetes-list-type" = "map", "x-kubernetes-list-map-keys" = ["id"])
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<BucketLifecycleRule>,
}

impl BucketAnonymousAccess {
    pub(crate) fn is_private(&self) -> bool {
        matches!(self, Self::Private)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!(has(self.policy) && has(self.anonymous) && self.anonymous != 'Private')").message("bucket policy and non-private anonymous access are mutually exclusive"))]
#[x_kube(validation = Rule::new("!has(self.objectLockConfiguration) || (has(self.objectLock) && self.objectLock)").message("objectLockConfiguration requires objectLock: true"))]
#[x_kube(validation = Rule::new("!(has(self.versioning) && self.versioning == false && ((has(self.objectLock) && self.objectLock) || has(self.objectLockConfiguration)))").message("versioning cannot be disabled when object lock is configured"))]
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

    /// Declarative S3 bucket versioning. `false` keeps a never-versioned bucket unchanged or
    /// suspends versioning after it has been enabled. Omission leaves versioning unmanaged unless
    /// Object Lock requires it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versioning: Option<bool>,

    /// Default Object Lock retention. Omission leaves the live rule unmanaged; `Absent`
    /// removes only a rule previously owned by the operator and never disables Object Lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_lock_configuration: Option<BucketObjectLockConfiguration>,

    /// Canned anonymous access for this bucket. Non-private values are mutually exclusive with
    /// `policy`. Explicit `Private` removes an operator-managed bucket policy when `policy` is
    /// absent; omission leaves the live policy alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<BucketAnonymousAccess>,

    /// Custom bucket policy document sourced from a ConfigMap. Mutually exclusive with non-private
    /// `anonymous`; legacy `Private` plus `policy` configurations keep the custom policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyDocumentSource>,

    /// Declarative S3 bucket lifecycle configuration. Omission leaves the live lifecycle
    /// configuration unmanaged; `Absent` removes only a configuration previously owned by the
    /// operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<BucketLifecycleSpec>,

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

    pub(crate) fn has_non_private_anonymous_access(&self) -> bool {
        self.anonymous
            .as_ref()
            .is_some_and(|access| !access.is_private())
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
        assert!(public.has_non_private_anonymous_access());
        assert!(!public.has_custom_policy());

        let legacy_private_policy: super::ProvisioningBucket =
            serde_json::from_value(serde_json::json!({
                "name": "app-data",
                "anonymous": "Private",
                "policy": {
                    "configMapKeyRef": {
                        "name": "bucket-policy",
                        "key": "policy.json"
                    }
                }
            }))
            .expect("legacy private plus policy configuration deserializes");
        assert!(legacy_private_policy.has_custom_policy());
        assert!(!legacy_private_policy.has_non_private_anonymous_access());
    }

    #[test]
    fn bucket_lifecycle_deserializes_present_and_absent_states() {
        let present: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "app-data",
            "lifecycle": {
                "state": "Present",
                "rules": [{
                    "id": "expire-logs",
                    "status": "Enabled",
                    "filter": {"prefix": "logs/"},
                    "expiration": {"days": 30},
                    "abortIncompleteMultipartUpload": {"daysAfterInitiation": 1}
                }]
            }
        }))
        .expect("Present lifecycle should deserialize");
        let lifecycle = present.lifecycle.expect("lifecycle is present");
        assert_eq!(lifecycle.state, super::BucketLifecycleState::Present);
        assert_eq!(lifecycle.rules[0].filter.prefix, "logs/");

        let absent: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "app-data",
            "lifecycle": {"state": "Absent"}
        }))
        .expect("Absent lifecycle should deserialize without rules");
        assert!(
            absent
                .lifecycle
                .expect("lifecycle is present")
                .rules
                .is_empty()
        );
    }

    #[test]
    fn bucket_lifecycle_deserializes_transition_actions() {
        let bucket: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "app-data",
            "lifecycle": {
                "state": "Present",
                "rules": [{
                    "id": "to-cold",
                    "status": "Enabled",
                    "filter": {"prefix": ""},
                    "transition": {"days": 1, "storageClass": "COLD"},
                    "noncurrentVersionTransition": {"noncurrentDays": 7, "storageClass": "COLD"}
                }]
            }
        }))
        .expect("transition lifecycle should deserialize");
        let rule = &bucket.lifecycle.expect("lifecycle is present").rules[0];
        assert_eq!(
            rule.transition,
            Some(super::BucketLifecycleTransition {
                days: 1,
                storage_class: "COLD".to_string(),
            })
        );
        assert_eq!(
            rule.noncurrent_version_transition,
            Some(super::BucketLifecycleNoncurrentVersionTransition {
                noncurrent_days: 7,
                storage_class: "COLD".to_string(),
            })
        );
        assert!(rule.expiration.is_none());
    }

    #[test]
    fn tier_deserializes_issue_yaml_shape_and_omits_defaults() {
        let tier: super::ProvisioningTier = serde_json::from_value(serde_json::json!({
            "name": "STORJ",
            "type": "S3",
            "endpoint": "https://gateway.storjshare.io",
            "bucket": "rustfs-ilm",
            "prefix": "tenant-a/",
            "credsSecret": {"name": "storj-tier"}
        }))
        .expect("tier should deserialize");

        assert_eq!(tier.tier_type, super::ProvisioningTierType::S3);
        assert_eq!(tier.tier_type.rustfs_type(), "s3");
        assert!(tier.region.is_empty());
        assert!(!tier.force);
        let value = serde_json::to_value(&tier).expect("tier serializes");
        assert!(value.get("region").is_none());
        assert!(value.get("force").is_none());
        assert!(value.get("deletionPolicy").is_none());
        assert_eq!(value["type"], "S3");

        for (name, rustfs_type) in [("RustFS", "rustfs"), ("MinIO", "minio")] {
            let tier_type: super::ProvisioningTierType =
                serde_json::from_value(serde_json::json!(name)).expect("tier type deserializes");
            assert_eq!(tier_type.rustfs_type(), rustfs_type);
        }
    }

    #[test]
    fn bucket_object_lock_configuration_preserves_issue_yaml_shape() {
        let bucket: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "secure-bucket",
            "versioning": true,
            "objectLock": true,
            "objectLockConfiguration": {
                "mode": "Compliance",
                "days": 30
            }
        }))
        .expect("Object Lock bucket should deserialize");

        assert_eq!(bucket.versioning, Some(true));
        let configuration = bucket
            .object_lock_configuration
            .expect("Object Lock configuration is present");
        assert_eq!(
            configuration.state(),
            super::BucketObjectLockConfigurationState::Present
        );
        assert_eq!(
            configuration.mode,
            Some(super::BucketObjectLockRetentionMode::Compliance)
        );
        assert_eq!(configuration.days, Some(30));
    }

    #[test]
    fn bucket_object_lock_configuration_supports_explicit_absence() {
        let bucket: super::ProvisioningBucket = serde_json::from_value(serde_json::json!({
            "name": "secure-bucket",
            "objectLock": true,
            "objectLockConfiguration": {"state": "Absent"}
        }))
        .expect("Absent Object Lock configuration should deserialize");

        let configuration = bucket
            .object_lock_configuration
            .expect("Object Lock configuration is present");
        assert_eq!(
            configuration.state(),
            super::BucketObjectLockConfigurationState::Absent
        );
        assert!(configuration.mode.is_none());
        assert!(configuration.days.is_none());
    }
}
