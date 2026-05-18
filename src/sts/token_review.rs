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

use k8s_openapi::api::authentication::v1::{TokenReviewStatus, UserInfo};

/// Prefix used by Kubernetes for service account usernames.
const SERVICE_ACCOUNT_USERNAME_PREFIX: &str = "system:serviceaccount:";

/// Service account identity extracted from a TokenReview response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountIdentity {
    pub namespace: String,
    pub service_account: String,
}

/// Errors that can happen while processing Kubernetes TokenReview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReviewError {
    MissingTokenReview,
    NotAuthenticated,
    MissingAudience,
    InvalidAudience,
    MissingUsername,
    InvalidUsername,
    InvalidUsernameFormat,
}

impl TokenReviewError {
    pub fn as_message(&self) -> String {
        match self {
            Self::MissingTokenReview => "TokenReview response is incomplete.".to_string(),
            Self::NotAuthenticated => "TokenReview failed to authenticate token.".to_string(),
            Self::MissingAudience => "TokenReview response has no accepted audience.".to_string(),
            Self::InvalidAudience => {
                "TokenReview response did not accept the expected audience.".to_string()
            }
            Self::MissingUsername => "TokenReview response has no user identity.".to_string(),
            Self::InvalidUsername => {
                "TokenReview returned an invalid service account user name.".to_string()
            }
            Self::InvalidUsernameFormat => {
                "TokenReview service account user name format is invalid.".to_string()
            }
        }
    }
}

/// Parse `system:serviceaccount:<namespace>:<serviceaccount>` into namespace/SA fields.
pub fn parse_service_account_username(
    raw_username: &str,
) -> Result<ServiceAccountIdentity, TokenReviewError> {
    let raw = raw_username.trim();
    let remaining = raw
        .strip_prefix(SERVICE_ACCOUNT_USERNAME_PREFIX)
        .ok_or(TokenReviewError::InvalidUsername)?;

    let mut parts = remaining.split(':');
    let namespace = parts
        .next()
        .ok_or(TokenReviewError::InvalidUsernameFormat)?;
    let service_account = parts
        .next()
        .ok_or(TokenReviewError::InvalidUsernameFormat)?;
    if parts.next().is_some() {
        return Err(TokenReviewError::InvalidUsernameFormat);
    }

    if namespace.is_empty() || service_account.is_empty() {
        return Err(TokenReviewError::InvalidUsernameFormat);
    }

    Ok(ServiceAccountIdentity {
        namespace: namespace.to_string(),
        service_account: service_account.to_string(),
    })
}

/// Convert TokenReview status payload to service account identity.
pub fn extract_service_account_identity(
    status: &TokenReviewStatus,
) -> Result<ServiceAccountIdentity, TokenReviewError> {
    extract_service_account_identity_for_audience(status, None)
}

/// Convert TokenReview status payload to service account identity and validate accepted audience.
pub fn extract_service_account_identity_for_audience(
    status: &TokenReviewStatus,
    expected_audience: Option<&str>,
) -> Result<ServiceAccountIdentity, TokenReviewError> {
    if !status.authenticated.unwrap_or(false) {
        return Err(TokenReviewError::NotAuthenticated);
    }

    if let Some(expected_audience) = expected_audience {
        let audiences = status
            .audiences
            .as_ref()
            .ok_or(TokenReviewError::MissingAudience)?;
        if !audiences
            .iter()
            .any(|audience| audience == expected_audience)
        {
            return Err(TokenReviewError::InvalidAudience);
        }
    }

    let user = status
        .user
        .as_ref()
        .ok_or(TokenReviewError::MissingUsername)?;
    let username = user
        .username
        .as_ref()
        .ok_or(TokenReviewError::MissingUsername)?;
    parse_service_account_username(username)
}

/// Build a synthetic TokenReviewStatus for unit tests.
///
/// The parser helpers above are intentionally transport-agnostic and can be reused by both
/// mock and live flows.
pub fn token_review_status(authenticated: bool, username: Option<&str>) -> TokenReviewStatus {
    TokenReviewStatus {
        audiences: None,
        authenticated: Some(authenticated),
        error: None,
        user: username.map(|value| UserInfo {
            extra: None,
            groups: None,
            uid: None,
            username: Some(value.to_string()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenReviewError, parse_service_account_username, token_review_status};
    use k8s_openapi::api::authentication::v1::TokenReviewStatus;

    #[test]
    fn parse_service_account_username_success() {
        let identity = parse_service_account_username("system:serviceaccount:tenant-ns:tenant-sa")
            .expect("service account identity should parse");

        assert_eq!(identity.namespace, "tenant-ns");
        assert_eq!(identity.service_account, "tenant-sa");
    }

    #[test]
    fn parse_service_account_username_requires_service_account_prefix() {
        let error = parse_service_account_username("kube-system:tenant-sa")
            .expect_err("identity should require serviceaccount prefix");

        assert!(matches!(error, TokenReviewError::InvalidUsername));
    }

    #[test]
    fn parse_service_account_username_rejects_non_two_parts() {
        let error = parse_service_account_username("system:serviceaccount:tenant-sa")
            .expect_err("identity should require namespace and serviceaccount");

        assert!(matches!(error, TokenReviewError::InvalidUsernameFormat));
    }

    #[test]
    fn extract_service_account_identity_rejects_unauthenticated_token() {
        let status = token_review_status(false, Some("system:serviceaccount:tenant-ns:tenant-sa"));

        let err = super::extract_service_account_identity(&status)
            .expect_err("unauthenticated token should fail");

        assert!(matches!(err, TokenReviewError::NotAuthenticated));
    }

    #[test]
    fn extract_service_account_identity_rejects_non_service_account_usernames() {
        let status = token_review_status(true, Some("invalid-user"));

        let err = super::extract_service_account_identity(&status)
            .expect_err("only system:serviceaccount users are supported");

        assert!(matches!(err, TokenReviewError::InvalidUsername));
    }

    #[test]
    fn extract_service_account_identity_rejects_missing_user_field() {
        let status = token_review_status(true, None);

        let err = super::extract_service_account_identity(&status)
            .expect_err("missing user payload should fail");

        assert!(matches!(err, TokenReviewError::MissingUsername));
    }

    #[test]
    fn extract_service_account_identity_requires_expected_audience() {
        let status = TokenReviewStatus {
            audiences: Some(vec!["sts.rustfs.com".to_string()]),
            ..token_review_status(true, Some("system:serviceaccount:tenant-ns:tenant-sa"))
        };

        let identity =
            super::extract_service_account_identity_for_audience(&status, Some("sts.rustfs.com"))
                .expect("matching audience should be accepted");

        assert_eq!(identity.namespace, "tenant-ns");
        assert_eq!(identity.service_account, "tenant-sa");
    }

    #[test]
    fn extract_service_account_identity_rejects_wrong_audience() {
        let status = TokenReviewStatus {
            audiences: Some(vec!["https://kubernetes.default.svc".to_string()]),
            ..token_review_status(true, Some("system:serviceaccount:tenant-ns:tenant-sa"))
        };

        let err =
            super::extract_service_account_identity_for_audience(&status, Some("sts.rustfs.com"))
                .expect_err("wrong audience should fail");

        assert!(matches!(err, TokenReviewError::InvalidAudience));
    }
}
