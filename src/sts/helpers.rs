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

//! Internal helper duties: shared credential parsing, signature/hash utilities, and parsers.
use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use k8s_openapi::ByteString;
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::form_urlencoded;

use crate::Tenant;
use crate::sts::types::StsAssumeRoleCredentials;

use super::{RustfsClientError, RustfsCredentials};

pub(super) fn extract_credentials(
    data: Option<&BTreeMap<String, ByteString>>,
) -> Result<RustfsCredentials, RustfsClientError> {
    let secret_data = data.ok_or(RustfsClientError::TenantSecretLookupFailed)?;

    Ok(RustfsCredentials {
        access_key: get_secret_value(secret_data, "accesskey")?,
        secret_key: get_secret_value(secret_data, "secretkey")?,
    })
}

pub(super) fn tenant_tls_enabled(tenant: &Tenant) -> bool {
    tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled())
}

pub(super) fn tenant_tls_client_certificate_required(tenant: &Tenant) -> bool {
    tenant
        .status
        .as_ref()
        .and_then(|status| status.certificates.tls.as_ref())
        .and_then(|tls| tls.client_ca_secret_ref.as_ref())
        .is_some()
}

pub(super) fn get_secret_value(
    data: &BTreeMap<String, ByteString>,
    field: &'static str,
) -> Result<String, RustfsClientError> {
    let raw = data
        .get(field)
        .ok_or(RustfsClientError::MissingCredentialKey { key: field })?;

    let value = String::from_utf8(raw.0.clone())
        .map_err(|_| RustfsClientError::InvalidCredentialValue { key: field })?;

    if value.is_empty() {
        return Err(RustfsClientError::EmptyCredentialValue { key: field });
    }

    Ok(value)
}

pub(super) fn build_query_pairs(params: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    pairs.sort_by(|(k1, v1), (k2, v2)| k1.cmp(k2).then(v1.cmp(v2)));

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(&key, &value);
    }

    serializer.finish()
}

pub(super) fn create_bucket_body(region: Option<&str>) -> String {
    let Some(region) = region.map(str::trim).filter(|region| !region.is_empty()) else {
        return String::new();
    };

    if region == "us-east-1" {
        return String::new();
    }

    format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
        escape_xml(region)
    )
}

pub(super) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn body_mentions_not_found(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("nosuchuser")
        || body.contains("no such user")
        || body.contains("user not exist")
        || body.contains("nosuchpolicy")
        || body.contains("no such policy")
        || body.contains("objectlockconfigurationnotfound")
        || body.contains("not found")
}

pub(super) fn bucket_already_exists(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::CONFLICT {
        let body = body.to_ascii_lowercase();
        return body.contains("bucketalreadyexists") || body.contains("bucketalreadyownedbyyou");
    }

    false
}

pub(super) fn extract_canned_policy_document(body: &str) -> Result<String, RustfsClientError> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|_| RustfsClientError::InvalidPolicyDocument)?;
    let policy = value.get("policy").unwrap_or(&value);

    serde_json::to_string(policy).map_err(|_| RustfsClientError::InvalidPolicyDocument)
}

pub(super) fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

pub(super) fn hmac_sha256(key: &[u8], message: &str) -> Result<Vec<u8>, RustfsClientError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| RustfsClientError::SigningFailed)?;
    mac.update(message.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(super) fn hmac_sha256_hex(key: &[u8], message: &str) -> Result<String, RustfsClientError> {
    let bytes = hmac_sha256(key, message)?;
    Ok(hex::encode(bytes))
}

pub(super) fn derive_signing_key(
    secret_key: &str,
    date_stamp: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, RustfsClientError> {
    let k_secret = format!("AWS4{secret_key}").into_bytes();
    let k_date = hmac_sha256(&k_secret, date_stamp)?;
    let k_region = hmac_sha256(&k_date, region)?;
    let k_service = hmac_sha256(&k_region, service)?;
    hmac_sha256(&k_service, "aws4_request")
}

pub(super) fn parse_assume_role_response(body: &str) -> Option<StsAssumeRoleCredentials> {
    let access_key_id = extract_xml_tag(body, "AccessKeyId")?;
    let secret_access_key = extract_xml_tag(body, "SecretAccessKey")?;
    let session_token = extract_xml_tag(body, "SessionToken")?;
    let expiration = extract_xml_tag(body, "Expiration")?;

    Some(StsAssumeRoleCredentials {
        access_key_id,
        secret_access_key,
        session_token,
        expiration,
    })
}

pub(super) fn extract_xml_tag(document: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    let open_idx = document.find(&open)?;
    let start = open_idx + open.len();
    let rest = &document[start..];
    let end = rest.find(&close)?;

    Some(rest[..end].trim().to_string())
}
