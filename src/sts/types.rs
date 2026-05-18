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

use serde::Deserialize;

use crate::sts::error::{STS_REQUEST_ID, STS_XML_NAMESPACE, StsError, escape_xml};

pub const STS_API_VERSION: &str = "2011-06-15";
pub const STS_WEB_IDENTITY_ACTION: &str = "AssumeRoleWithWebIdentity";
pub const STS_DEFAULT_DURATION_SECONDS: u64 = 3600;
pub const STS_MIN_DURATION_SECONDS: u64 = 900;
pub const STS_MAX_DURATION_SECONDS: u64 = 31_536_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StsParsedRequest {
    pub tenant_namespace: String,
    pub tenant_name: String,
    pub version: String,
    pub action: String,
    pub web_identity_token: String,
    pub duration_seconds: u64,
    pub policy: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AssumeRoleWithWebIdentityForm {
    pub version: Option<String>,
    pub action: Option<String>,
    pub web_identity_token: Option<String>,
    pub duration_seconds: Option<String>,
    pub policy: Option<String>,
}

/// Parse a form-style STS request and validate required parameters.
pub fn parse_sts_form(
    tenant_namespace: String,
    tenant_name: String,
    form: AssumeRoleWithWebIdentityForm,
) -> Result<StsParsedRequest, StsError> {
    let version = form
        .version
        .filter(|value| !value.trim().is_empty())
        .ok_or(StsError::MissingParameter {
            parameter: "Version",
        })?;

    if version != STS_API_VERSION {
        return Err(StsError::InvalidParameterValue {
            parameter: "Version",
        });
    }

    let action =
        form.action
            .filter(|value| !value.trim().is_empty())
            .ok_or(StsError::MissingParameter {
                parameter: "Action",
            })?;

    if action != STS_WEB_IDENTITY_ACTION {
        return Err(StsError::InvalidParameterValue {
            parameter: "Action",
        });
    }

    let web_identity_token = form
        .web_identity_token
        .filter(|value| !value.trim().is_empty())
        .ok_or(StsError::MissingParameter {
            parameter: "WebIdentityToken",
        })?;

    let duration_seconds = match form.duration_seconds {
        Some(raw) => {
            let duration =
                raw.trim()
                    .parse::<u64>()
                    .map_err(|_| StsError::InvalidParameterValue {
                        parameter: "DurationSeconds",
                    })?;

            if !(STS_MIN_DURATION_SECONDS..=STS_MAX_DURATION_SECONDS).contains(&duration) {
                return Err(StsError::InvalidParameterValue {
                    parameter: "DurationSeconds",
                });
            }

            duration
        }
        None => STS_DEFAULT_DURATION_SECONDS,
    };

    Ok(StsParsedRequest {
        tenant_namespace,
        tenant_name,
        version,
        action,
        web_identity_token,
        duration_seconds,
        policy: form.policy,
    })
}

#[derive(Debug, Clone)]
pub struct StsAssumeRoleCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: String,
}

#[derive(Debug, Clone)]
pub struct StsWebIdentityResponseContext {
    pub subject: String,
    pub audience: String,
    pub provider: String,
    pub assumed_role_arn: String,
    pub assumed_role_id: String,
    pub packed_policy_size: u8,
}

/// Build an AWS STS AssumeRoleWithWebIdentity success XML payload.
pub fn render_assume_role_with_web_identity_response(
    credentials: &StsAssumeRoleCredentials,
    context: &StsWebIdentityResponseContext,
) -> String {
    let access_key_id = escape_xml(&credentials.access_key_id);
    let secret_access_key = escape_xml(&credentials.secret_access_key);
    let session_token = escape_xml(&credentials.session_token);
    let expiration = escape_xml(&credentials.expiration);
    let subject = escape_xml(&context.subject);
    let audience = escape_xml(&context.audience);
    let provider = escape_xml(&context.provider);
    let assumed_role_arn = escape_xml(&context.assumed_role_arn);
    let assumed_role_id = escape_xml(&context.assumed_role_id);
    let packed_policy_size = context.packed_policy_size;

    format!(
        "<AssumeRoleWithWebIdentityResponse xmlns=\"{ns}\"><AssumeRoleWithWebIdentityResult><SubjectFromWebIdentityToken>{subject}</SubjectFromWebIdentityToken><Audience>{audience}</Audience><AssumedRoleUser><Arn>{assumed_role_arn}</Arn><AssumedRoleId>{assumed_role_id}</AssumedRoleId></AssumedRoleUser><Credentials><AccessKeyId>{access_key_id}</AccessKeyId><SecretAccessKey>{secret_access_key}</SecretAccessKey><SessionToken>{session_token}</SessionToken><Expiration>{expiration}</Expiration></Credentials><PackedPolicySize>{packed_policy_size}</PackedPolicySize><Provider>{provider}</Provider></AssumeRoleWithWebIdentityResult><ResponseMetadata><RequestId>{request_id}</RequestId></ResponseMetadata></AssumeRoleWithWebIdentityResponse>",
        ns = STS_XML_NAMESPACE,
        request_id = STS_REQUEST_ID,
    )
}

/// Build a deterministic stub XML error for the current implementation phase.
pub fn render_not_implemented_response() -> String {
    format!(
        "<AssumeRoleWithWebIdentityResponse xmlns=\"{ns}\"><Error><Type>Sender</Type><Code>NotImplemented</Code><Message>AssumeRoleWithWebIdentity is not implemented in this phase</Message></Error><RequestId>{request_id}</RequestId></AssumeRoleWithWebIdentityResponse>",
        ns = STS_XML_NAMESPACE,
        request_id = STS_REQUEST_ID
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sts::error::{StsError, escape_xml};

    fn form(
        version: &str,
        action: &str,
        web_identity_token: &str,
        duration: Option<&str>,
        policy: Option<&str>,
    ) -> AssumeRoleWithWebIdentityForm {
        AssumeRoleWithWebIdentityForm {
            version: if version.is_empty() {
                None
            } else {
                Some(version.to_string())
            },
            action: if action.is_empty() {
                None
            } else {
                Some(action.to_string())
            },
            web_identity_token: if web_identity_token.is_empty() {
                None
            } else {
                Some(web_identity_token.to_string())
            },
            duration_seconds: duration.map(ToString::to_string),
            policy: policy.map(ToString::to_string),
        }
    }

    #[test]
    fn parse_rejects_missing_version() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form("", STS_WEB_IDENTITY_ACTION, "token", None, None),
        );

        assert!(matches!(
            request,
            Err(StsError::MissingParameter {
                parameter: "Version"
            })
        ));
    }

    #[test]
    fn parse_rejects_invalid_version() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form("2010-01-01", STS_WEB_IDENTITY_ACTION, "token", None, None),
        );

        assert!(matches!(
            request,
            Err(StsError::InvalidParameterValue {
                parameter: "Version"
            })
        ));
    }

    #[test]
    fn parse_rejects_missing_action() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(STS_API_VERSION, "", "token", None, None),
        );

        assert!(matches!(
            request,
            Err(StsError::MissingParameter {
                parameter: "Action"
            })
        ));
    }

    #[test]
    fn parse_rejects_wrong_action() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(STS_API_VERSION, "ListBuckets", "token", None, None),
        );

        assert!(matches!(
            request,
            Err(StsError::InvalidParameterValue {
                parameter: "Action"
            })
        ));
    }

    #[test]
    fn parse_rejects_missing_web_identity_token() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(STS_API_VERSION, STS_WEB_IDENTITY_ACTION, "", None, None),
        );

        assert!(matches!(
            request,
            Err(StsError::MissingParameter {
                parameter: "WebIdentityToken"
            })
        ));
    }

    #[test]
    fn parse_rejects_non_integer_duration() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token",
                Some("not-a-number"),
                None,
            ),
        );

        assert!(matches!(
            request,
            Err(StsError::InvalidParameterValue {
                parameter: "DurationSeconds"
            })
        ));
    }

    #[test]
    fn parse_rejects_duration_below_minimum() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token",
                Some("899"),
                None,
            ),
        );

        assert!(matches!(
            request,
            Err(StsError::InvalidParameterValue {
                parameter: "DurationSeconds"
            })
        ));
    }

    #[test]
    fn parse_rejects_duration_above_maximum() {
        let request = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token",
                Some("31536001"),
                None,
            ),
        );

        assert!(matches!(
            request,
            Err(StsError::InvalidParameterValue {
                parameter: "DurationSeconds"
            })
        ));
    }

    #[test]
    fn parse_accepts_minimum_and_maximum_duration_bounds() {
        let minimum = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token",
                Some("900"),
                None,
            ),
        )
        .expect("minimum duration is valid");
        let maximum = parse_sts_form(
            "tenant-a".to_string(),
            "rustfs-a".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token",
                Some("31536000"),
                None,
            ),
        )
        .expect("maximum duration is valid");

        assert_eq!(minimum.duration_seconds, STS_MIN_DURATION_SECONDS);
        assert_eq!(maximum.duration_seconds, STS_MAX_DURATION_SECONDS);
    }

    #[test]
    fn parse_defaults_duration_to_3600_and_preserves_policy() {
        let policy = r#"{\"Statement\": [{\"Action\": \"s3:GetObject\", \"Effect\": \"Allow\"}]}"#;
        let request = parse_sts_form(
            "tenant-b".to_string(),
            "tenant-b".to_string(),
            form(
                STS_API_VERSION,
                STS_WEB_IDENTITY_ACTION,
                "token-value",
                None,
                Some(policy),
            ),
        )
        .expect("valid request");

        assert_eq!(request.tenant_namespace, "tenant-b");
        assert_eq!(request.tenant_name, "tenant-b");
        assert_eq!(request.duration_seconds, STS_DEFAULT_DURATION_SECONDS);
        assert_eq!(request.policy.as_deref(), Some(policy));
    }

    #[test]
    fn error_xml_escapes_and_omit_payload_values() {
        let raw = "<x> & token";
        let escaped = escape_xml(raw);
        assert_eq!(escaped, "&lt;x&gt; &amp; token");

        let err = StsError::InvalidParameterValue {
            parameter: "DurationSeconds",
        }
        .as_xml();

        assert!(err.contains("<Code>InvalidParameterValue</Code>"));
        assert!(!err.contains("<x> & token"));
    }

    #[test]
    fn render_success_xml_matches_aws_shape() {
        let credentials = StsAssumeRoleCredentials {
            access_key_id: "[REDACTED]".to_string(),
            secret_access_key: "[REDACTED]".to_string(),
            session_token: "[REDACTED]".to_string(),
            expiration: "2026-06-20T00:00:00Z".to_string(),
        };
        let context = StsWebIdentityResponseContext {
            subject: "system:serviceaccount:apps:workload".to_string(),
            audience: "sts.rustfs.com".to_string(),
            provider: "kubernetes".to_string(),
            assumed_role_arn: "arn:rustfs:sts::tenant-a:assumed-role/tenant-a/workload".to_string(),
            assumed_role_id: "[REDACTED]:workload".to_string(),
            packed_policy_size: 0,
        };

        let xml = render_assume_role_with_web_identity_response(&credentials, &context);

        assert!(xml.contains("<AssumeRoleWithWebIdentityResponse"));
        assert!(xml.contains("<AssumeRoleWithWebIdentityResult>"));
        assert!(xml.contains("<SubjectFromWebIdentityToken>system:serviceaccount:apps:workload</SubjectFromWebIdentityToken>"));
        assert!(xml.contains("<Audience>sts.rustfs.com</Audience>"));
        assert!(xml.contains("<AssumedRoleUser>"));
        assert!(xml.contains("<Credentials>"));
        assert!(xml.contains("<AccessKeyId>[REDACTED]</AccessKeyId>"));
        assert!(xml.contains("<PackedPolicySize>0</PackedPolicySize>"));
        assert!(xml.contains("<ResponseMetadata><RequestId>"));
    }

    #[test]
    fn render_not_implemented_xml_shape_is_stable() {
        let xml = render_not_implemented_response();

        assert!(xml.contains("<AssumeRoleWithWebIdentityResponse"));
        assert!(xml.contains("<Code>NotImplemented</Code>"));
        assert!(xml.contains(&format!("<RequestId>{}</RequestId>", STS_REQUEST_ID)));
    }
}
