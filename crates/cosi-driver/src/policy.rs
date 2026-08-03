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

//! IAM policy documents scoped to a single bucket for COSI BucketAccess grants.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPolicy {
    Readonly,
    ReadWrite,
}

impl AccessPolicy {
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("readonly") | Some("read-only") | Some("read") => Self::Readonly,
            _ => Self::ReadWrite,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Readonly => "readonly",
            Self::ReadWrite => "readwrite",
        }
    }
}

/// Build a canned IAM policy JSON document limited to `bucket`.
pub fn bucket_policy_document(bucket: &str, policy: AccessPolicy) -> String {
    let actions = match policy {
        AccessPolicy::Readonly => {
            r#"[
        "s3:GetBucketLocation",
        "s3:ListBucket",
        "s3:GetObject"
      ]"#
        }
        AccessPolicy::ReadWrite => {
            r#"[
        "s3:GetBucketLocation",
        "s3:ListBucket",
        "s3:GetObject",
        "s3:PutObject",
        "s3:DeleteObject"
      ]"#
        }
    };

    format!(
        r#"{{
  "Version": "2012-10-17",
  "Statement": [
    {{
      "Effect": "Allow",
      "Action": {actions},
      "Resource": [
        "arn:aws:s3:::{bucket}",
        "arn:aws:s3:::{bucket}/*"
      ]
    }}
  ]
}}"#
    )
}

/// Deterministic canned policy name for a COSI account + bucket pair.
pub fn policy_name_for(account_id: &str, bucket_id: &str) -> String {
    // RustFS policy names should stay reasonably short and DNS-safe.
    let raw = format!("cosi-{account_id}-{bucket_id}");
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .take(128)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_policy_defaults_to_readwrite() {
        assert_eq!(AccessPolicy::parse(None), AccessPolicy::ReadWrite);
        assert_eq!(AccessPolicy::parse(Some("")), AccessPolicy::ReadWrite);
        assert_eq!(
            AccessPolicy::parse(Some("readwrite")),
            AccessPolicy::ReadWrite
        );
        assert_eq!(
            AccessPolicy::parse(Some("readonly")),
            AccessPolicy::Readonly
        );
    }

    #[test]
    fn policy_document_includes_bucket_resources() {
        let doc = bucket_policy_document("my-bucket", AccessPolicy::Readonly);
        assert!(doc.contains("arn:aws:s3:::my-bucket"));
        assert!(doc.contains("s3:GetObject"));
        assert!(!doc.contains("s3:PutObject"));
    }

    #[test]
    fn policy_name_is_sanitized() {
        let name = policy_name_for("ba.uid", "bucket/name");
        assert!(!name.contains('.'));
        assert!(!name.contains('/'));
        assert!(name.starts_with("cosi-"));
    }
}
