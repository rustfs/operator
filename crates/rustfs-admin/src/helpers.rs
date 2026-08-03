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

//! Internal helper duties: signature/hash utilities and wire-format parsers.
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::form_urlencoded;

use crate::client::RustfsClientError;
use crate::credentials::StsAssumeRoleCredentials;

/// Encode an `application/x-www-form-urlencoded` request body.
pub(crate) fn build_form_body(params: &[(&str, &str)]) -> String {
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

/// Encode and sort query parameters according to the AWS SigV4 rules.
pub(crate) fn build_canonical_query(params: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = params
        .iter()
        .map(|(key, value)| (uri_encode(key), uri_encode(value)))
        .collect();
    pairs.sort_unstable();

    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn uri_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

pub(crate) fn create_bucket_body(region: Option<&str>) -> String {
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

pub(crate) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(crate) fn body_mentions_not_found(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("nosuchuser")
        || body.contains("no such user")
        || body.contains("user not exist")
        || body.contains("nosuchpolicy")
        || body.contains("no such policy")
        || body.contains("objectlockconfigurationnotfound")
        || body.contains("not found")
}

/// Whether the response body indicates the target bucket does not exist.
pub(crate) fn bucket_not_found(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("nosuchbucket") || body.contains("no such bucket") || body.contains("not found")
}

pub(crate) fn bucket_already_exists(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::CONFLICT {
        let body = body.to_ascii_lowercase();
        return body.contains("bucketalreadyexists") || body.contains("bucketalreadyownedbyyou");
    }

    false
}

pub(crate) fn extract_canned_policy_document(body: &str) -> Result<String, RustfsClientError> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|_| RustfsClientError::InvalidPolicyDocument)?;
    let policy = value.get("policy").unwrap_or(&value);

    serde_json::to_string(policy).map_err(|_| RustfsClientError::InvalidPolicyDocument)
}

pub(crate) fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

pub(crate) fn hmac_sha256(key: &[u8], message: &str) -> Result<Vec<u8>, RustfsClientError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| RustfsClientError::SigningFailed)?;
    mac.update(message.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(crate) fn hmac_sha256_hex(key: &[u8], message: &str) -> Result<String, RustfsClientError> {
    let bytes = hmac_sha256(key, message)?;
    Ok(hex::encode(bytes))
}

pub(crate) fn derive_signing_key(
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

pub(crate) fn parse_assume_role_response(body: &str) -> Option<StsAssumeRoleCredentials> {
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

pub(crate) fn extract_xml_tag(document: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    let open_idx = document.find(&open)?;
    let start = open_idx + open.len();
    let rest = &document[start..];
    let end = rest.find(&close)?;

    Some(rest[..end].trim().to_string())
}
