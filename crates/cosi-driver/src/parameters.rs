//! RustFS COSI driver parameters (BucketClass / BucketAccessClass).

use std::collections::HashMap;

use thiserror::Error;

pub const DRIVER_NAME: &str = "rustfs.objectstorage.k8s.io";

#[derive(Debug, Clone)]
pub struct BackendParameters {
    pub endpoint: String,
    pub object_store_user_secret_name: String,
    pub object_store_user_secret_namespace: String,
    pub region: String,
    pub policy: Option<String>,
    pub tls_ca_configmap_name: Option<String>,
    pub tls_ca_configmap_namespace: Option<String>,
    /// Preferred S3 bucket name (overrides COSI-generated CreateBucket name).
    pub bucket_name: Option<String>,
    /// Comma-separated bucket list to create / authorize (`*` = full access).
    pub buckets: Option<String>,
    /// Preferred access-key / account name for GrantBucketAccess.
    ///
    /// Must be unique per BucketAccess. Reusing the same value across claims is
    /// rejected by the driver (Ceph-style isolation). Prefer omitting this so the
    /// COSI grant name (`ba-<UID>`) is used as the account id.
    pub preferred_access_key: Option<String>,
}

#[derive(Debug, Error)]
pub enum ParameterError {
    #[error("missing required parameter `{0}`")]
    MissingRequired(&'static str),
    #[error("parameter `{0}` is empty")]
    Empty(&'static str),
}

fn required(map: &HashMap<String, String>, key: &'static str) -> Result<String, ParameterError> {
    let value = map
        .get(key)
        .cloned()
        .ok_or(ParameterError::MissingRequired(key))?;
    if value.trim().is_empty() {
        return Err(ParameterError::Empty(key));
    }
    Ok(value)
}

fn optional(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .cloned()
        .filter(|value| !value.trim().is_empty())
}

impl BackendParameters {
    pub fn from_map(map: &HashMap<String, String>) -> Result<Self, ParameterError> {
        Ok(Self {
            endpoint: required(map, "endpoint")?,
            object_store_user_secret_name: required(map, "objectStoreUserSecretName")?,
            object_store_user_secret_namespace: required(map, "objectStoreUserSecretNamespace")?,
            region: optional(map, "region").unwrap_or_else(|| "us-east-1".to_string()),
            policy: optional(map, "policy"),
            tls_ca_configmap_name: optional(map, "tlsCAConfigMapName"),
            tls_ca_configmap_namespace: optional(map, "tlsCAConfigMapNamespace"),
            bucket_name: optional(map, "bucketName"),
            buckets: optional(map, "buckets"),
            preferred_access_key: optional(map, "preferredAccessKey")
                .or_else(|| optional(map, "accessKey")),
        })
    }

    /// Buckets to create (excludes `*`). Primary bucket_id is the first entry.
    pub fn buckets_to_create(&self, fallback_name: &str) -> Vec<String> {
        let raw = self
            .buckets
            .as_deref()
            .or(self.bucket_name.as_deref())
            .unwrap_or(fallback_name);
        raw.split(',')
            .map(str::trim)
            .filter(|b| !b.is_empty() && *b != "*")
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Full bucket list for IAM policy (may include `*`).
    pub fn buckets_for_policy(&self, fallback_name: &str) -> Vec<String> {
        let raw = self
            .buckets
            .as_deref()
            .or(self.bucket_name.as_deref())
            .unwrap_or(fallback_name);
        let list: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if list.is_empty() {
            vec![fallback_name.to_string()]
        } else {
            list
        }
    }

    pub fn primary_bucket_id(&self, cosi_name: &str) -> String {
        self.bucket_name
            .clone()
            .or_else(|| self.buckets_to_create(cosi_name).into_iter().next())
            .unwrap_or_else(|| cosi_name.to_string())
    }
}

pub fn bucket_policy_document_for(buckets: &[String]) -> String {
    let has_wildcard = buckets.iter().any(|b| b == "*");
    let resources: Vec<String> = if has_wildcard {
        vec!["arn:aws:s3:::*".to_string(), "arn:aws:s3:::*/*".to_string()]
    } else {
        buckets
            .iter()
            .flat_map(|b| [format!("arn:aws:s3:::{b}"), format!("arn:aws:s3:::{b}/*")])
            .collect()
    };
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:*"],
            "Resource": resources
        }]
    })
    .to_string()
}

pub fn sanitize_policy_fragment(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn policy_name_for(bucket: &str) -> String {
    format!("cosi-{}", sanitize_policy_fragment(bucket))
}

/// Owner marker policy bound to a specific COSI grant `name` (`ba-<UID>`).
pub fn grant_owner_policy_name(grant_name: &str) -> String {
    format!("cosi-grant-{}", sanitize_policy_fragment(grant_name))
}

/// Minimal canned policy used only as an ownership marker for a BucketAccess grant.
pub fn grant_owner_policy_document() -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "CosiGrantOwner",
            "Effect": "Allow",
            "Action": ["s3:ListAllMyBuckets"],
            "Resource": ["arn:aws:s3:::*"]
        }]
    })
    .to_string()
}

/// Deterministic secret so DriverGrantBucketAccess is idempotent across sidecar retries.
pub fn credentials_for_account(account_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("rustfs-cosi-v1:{account_id}").as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::{credentials_for_account, grant_owner_policy_name};

    #[test]
    fn credentials_are_deterministic_and_long_enough() {
        let a = credentials_for_account("ba-test-uid");
        let b = credentials_for_account("ba-test-uid");
        assert_eq!(a, b);
        assert!(a.len() >= 8);
        assert_ne!(a, credentials_for_account("other-account"));
    }

    #[test]
    fn grant_owner_policy_name_sanitizes() {
        assert_eq!(
            grant_owner_policy_name("ba-81733d1a-ac7a-4759-96f3-fbcc07c0cee9"),
            "cosi-grant-ba-81733d1a-ac7a-4759-96f3-fbcc07c0cee9"
        );
        assert_eq!(
            grant_owner_policy_name("ba/weird.name"),
            "cosi-grant-ba-weird-name"
        );
    }
}
