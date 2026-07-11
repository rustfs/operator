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

use std::{collections::BTreeMap, time::Duration};

use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client};
use reqwest::{Certificate, Client as HttpClient, Response, StatusCode};

use crate::Tenant;
use crate::utils::sanitize::redact_sensitive_pairs;

/// admin_ops: tenant admin operations (user/policy APIs).
#[path = "admin_ops.rs"]
mod admin_ops;
/// core_ops: shared request signing/dispatch internals.
#[path = "core_ops.rs"]
mod core_ops;
/// helpers: credential parsing, signing/hash utilities and parsers.
#[path = "helpers.rs"]
mod helpers;
/// pool_ops: pool lifecycle and status operations.
#[path = "pool_ops.rs"]
mod pool_ops;
/// s3_ops: bucket/object-lock operations for S3-compatible endpoints.
#[path = "s3_ops.rs"]
mod s3_ops;
/// sts_ops: temporary credential flows, AssumeRole request/response.
#[path = "sts_ops.rs"]
mod sts_ops;

const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";
const JSON_CONTENT_TYPE: &str = "application/json";
const ASSUME_ROLE_PATH: &str = "/";
const ADD_USER_PATH: &str = "/rustfs/admin/v3/add-user";
const USER_INFO_PATH: &str = "/rustfs/admin/v3/user-info";
const SET_POLICY_PATH: &str = "/rustfs/admin/v3/set-policy";
const LIST_CANNED_POLICIES_PATH: &str = "/rustfs/admin/v3/list-canned-policies";
const ADD_CANNED_POLICY_PATH: &str = "/rustfs/admin/v3/add-canned-policy";
const INFO_CANNED_POLICY_PATH: &str = "/rustfs/admin/v3/info-canned-policy";
const SERVER_INFO_PATH: &str = "/rustfs/admin/v3/info";
const POOLS_LIST_PATH: &str = "/rustfs/admin/v3/pools/list";
const POOLS_STATUS_PATH: &str = "/rustfs/admin/v3/pools/status";
const POOLS_DECOMMISSION_PATH: &str = "/rustfs/admin/v3/pools/decommission";
const POOLS_CANCEL_PATH: &str = "/rustfs/admin/v3/pools/cancel";
const ADMIN_SIGNING_SERVICE: &str = "s3";
const STS_SIGNING_SERVICE: &str = "sts";
const ADMIN_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ADMIN_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_UPSTREAM_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_UPSTREAM_ERROR_DETAIL_CHARS: usize = 512;

/// Credentials read from Tenant `.spec.credsSecret`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustfsCredentials {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct RustfsPoolListItem {
    pub id: usize,
    #[serde(rename = "cmdline")]
    pub cmd_line: String,
    #[serde(rename = "lastUpdate")]
    pub last_update: String,
    #[serde(rename = "totalSize")]
    pub total_size: Option<u64>,
    #[serde(rename = "currentSize")]
    pub current_size: Option<u64>,
    #[serde(rename = "usedSize")]
    pub used_size: Option<u64>,
    pub used: Option<f64>,
    pub status: String,
    #[serde(rename = "decommissionInfo")]
    pub decommission: Option<RustfsPoolDecommissionInfo>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct RustfsPoolStatus {
    pub id: usize,
    #[serde(rename = "cmdline")]
    pub cmd_line: String,
    #[serde(rename = "lastUpdate")]
    pub last_update: String,
    #[serde(rename = "decommissionInfo")]
    pub decommission: Option<RustfsPoolDecommissionInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateBucketResult {
    Created,
    AlreadyExists,
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq)]
pub struct RustfsPoolDecommissionInfo {
    #[serde(rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(rename = "startSize")]
    pub start_size: Option<u64>,
    #[serde(rename = "totalSize")]
    pub total_size: Option<u64>,
    #[serde(rename = "currentSize")]
    pub current_size: Option<u64>,
    pub complete: Option<bool>,
    pub failed: Option<bool>,
    pub canceled: Option<bool>,
    #[serde(rename = "objectsDecommissioned")]
    pub objects_decommissioned: Option<u64>,
    #[serde(rename = "objectsDecommissionedFailed")]
    pub objects_decommissioned_failed: Option<u64>,
    #[serde(rename = "bytesDecommissioned")]
    pub bytes_decommissioned: Option<u64>,
    #[serde(rename = "bytesDecommissionedFailed")]
    pub bytes_decommissioned_failed: Option<u64>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq)]
pub struct RustfsServerInfo {
    #[serde(default)]
    pub usage: Option<RustfsServerUsage>,
    #[serde(default)]
    pub backend: Option<RustfsErasureBackend>,
    #[serde(default)]
    pub pools: Option<BTreeMap<String, BTreeMap<String, RustfsErasureSetInfo>>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq)]
pub struct RustfsServerUsage {
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq)]
pub struct RustfsErasureBackend {
    #[serde(default, rename = "onlineDisks")]
    pub online_disks: u64,
    #[serde(default, rename = "offlineDisks")]
    pub offline_disks: u64,
    #[serde(default, rename = "standardSCParity", alias = "StandardSCParity")]
    pub standard_sc_parity: Option<u64>,
    #[serde(default, rename = "totalSets")]
    pub total_sets: Vec<u64>,
    #[serde(default, rename = "totalDrivesPerSet", alias = "drivesPerSet")]
    pub drives_per_set: Vec<u64>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq)]
pub struct RustfsErasureSetInfo {
    #[serde(default, rename = "rawUsage")]
    pub raw_usage: u64,
    #[serde(default, rename = "rawCapacity")]
    pub raw_capacity: u64,
    #[serde(default)]
    pub usage: u64,
    #[serde(default, rename = "objectsCount")]
    pub objects_count: u64,
    #[serde(default, rename = "healDisks")]
    pub heal_disks: u64,
}

/// Error type for RustFS admin/STS client operations.
#[derive(Debug)]
pub enum RustfsClientError {
    MissingTenantNamespace,
    MissingCredsSecret,
    MissingCredentialKey {
        key: &'static str,
    },
    EmptyCredentialValue {
        key: &'static str,
    },
    InvalidCredentialValue {
        key: &'static str,
    },
    TenantSecretLookupFailed,
    InvalidPolicyName,
    InvalidPolicyDocument,
    TenantTlsRequired,
    TenantTlsNotReady,
    TenantTlsClientCertificateRequired,
    MissingTenantTlsCaKey {
        secret: String,
        key: String,
    },
    TenantTlsCaSecretLookupFailed {
        secret: String,
    },
    InvalidTenantTlsCa,
    TlsClientBuildFailed,
    RequestBuildFailed,
    RequestFailed,
    UnexpectedStatus {
        status: StatusCode,
        detail: Option<String>,
    },
    ParseResponseFailed,
    SigningFailed,
}

impl std::fmt::Display for RustfsClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTenantNamespace => write!(f, "tenant namespace is missing"),
            Self::MissingCredsSecret => write!(f, "tenant credsSecret is missing"),
            Self::MissingCredentialKey { key } => write!(f, "secret key missing: {key}"),
            Self::EmptyCredentialValue { key } => write!(f, "secret key empty: {key}"),
            Self::InvalidCredentialValue { key } => {
                write!(f, "secret key is not valid utf8: {key}")
            }
            Self::TenantSecretLookupFailed => {
                write!(f, "failed to load tenant credential secret")
            }
            Self::InvalidPolicyName => write!(f, "invalid policy name"),
            Self::InvalidPolicyDocument => write!(f, "failed to parse canned policy response"),
            Self::TenantTlsRequired => write!(f, "STS requires a TLS-enabled tenant"),
            Self::TenantTlsNotReady => write!(f, "tenant TLS status is not ready"),
            Self::TenantTlsClientCertificateRequired => {
                write!(f, "tenant TLS requires a client certificate")
            }
            Self::MissingTenantTlsCaKey { secret, key } => {
                write!(f, "tenant TLS CA secret {secret} missing key {key}")
            }
            Self::TenantTlsCaSecretLookupFailed { secret } => {
                write!(f, "failed to load tenant TLS CA secret {secret}")
            }
            Self::InvalidTenantTlsCa => write!(f, "tenant TLS CA is not a valid PEM bundle"),
            Self::TlsClientBuildFailed => write!(f, "failed to build TLS HTTP client"),
            Self::RequestBuildFailed => write!(f, "failed to construct request"),
            Self::RequestFailed => write!(f, "request failed"),
            Self::UnexpectedStatus { status, detail } => {
                write!(f, "upstream returned {status}")?;
                if let Some(detail) = detail {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            Self::ParseResponseFailed => write!(f, "failed to parse AssumeRole response"),
            Self::SigningFailed => write!(f, "failed to compute request signature"),
        }
    }
}

impl std::error::Error for RustfsClientError {}

impl RustfsClientError {
    pub(super) async fn unexpected_response(response: Response) -> Self {
        let status = response.status();
        let (body, truncated) = read_limited_response_body(response).await;
        Self::unexpected_status_with_limited_body(status, &body, truncated)
    }

    pub(super) async fn limited_response_body(response: Response) -> (String, bool) {
        read_limited_response_body(response).await
    }

    fn unexpected_status_with_limited_body(
        status: StatusCode,
        body: &str,
        body_truncated: bool,
    ) -> Self {
        Self::UnexpectedStatus {
            status,
            detail: summarize_upstream_error_body(body, body_truncated),
        }
    }

    #[cfg(test)]
    pub(super) fn unexpected_status_with_body(status: StatusCode, body: &str) -> Self {
        Self::unexpected_status_with_limited_body(status, body, false)
    }
}

async fn read_limited_response_body(mut response: Response) -> (String, bool) {
    let mut body = Vec::new();
    let read_limit = MAX_UPSTREAM_ERROR_BODY_BYTES.saturating_add(1);

    loop {
        let remaining = read_limit.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }

        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => break,
        };
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            break;
        }
        body.extend_from_slice(&chunk);
    }

    let truncated = body.len() > MAX_UPSTREAM_ERROR_BODY_BYTES;
    if truncated {
        body.truncate(MAX_UPSTREAM_ERROR_BODY_BYTES);
    }

    (String::from_utf8_lossy(&body).into_owned(), truncated)
}

fn summarize_upstream_error_body(body: &str, body_truncated: bool) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }

    if let Some(message) = helpers::extract_xml_tag(body, "Message") {
        let message = decode_basic_xml_entities(&message);
        let detail = match helpers::extract_xml_tag(body, "Code") {
            Some(code) if !code.trim().is_empty() => {
                format!("{}: {message}", decode_basic_xml_entities(&code))
            }
            _ => message,
        };
        return Some(sanitize_error_detail(&detail));
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(detail) = summarize_json_error(&value)
    {
        return Some(sanitize_error_detail(&detail));
    }

    if body_truncated {
        return Some(format!(
            "response body exceeded {MAX_UPSTREAM_ERROR_BODY_BYTES} bytes"
        ));
    }

    Some(sanitize_error_detail(body))
}

fn summarize_json_error(value: &serde_json::Value) -> Option<String> {
    if let Some(message) = value.as_str() {
        return Some(message.to_string());
    }

    let object = value.as_object()?;
    let message = ["message", "Message", "error", "Error"]
        .iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))?;
    let code = ["code", "Code"]
        .iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str));

    Some(match code {
        Some(code) if !code.trim().is_empty() => format!("{code}: {message}"),
        _ => message.to_string(),
    })
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_error_detail(value: &str) -> String {
    let detail = collapse_whitespace(value);
    let detail = redact_sensitive_pairs(&detail);
    truncate_error_detail(detail)
}

fn truncate_error_detail(value: String) -> String {
    let mut truncated = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= MAX_UPSTREAM_ERROR_DETAIL_CHARS {
            truncated.push_str("...");
            return truncated;
        }
        truncated.push(ch);
    }
    truncated
}

fn decode_basic_xml_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[derive(Debug)]
struct SignedRequest {
    amz_date: String,
    payload_hash: String,
    authorization: String,
}

/// RustFS admin/STS client.
pub struct RustfsAdminClient {
    base_url: String,
    access_key: String,
    secret_key: String,
    region: String,
    http_client: HttpClient,
}

fn default_http_client() -> HttpClient {
    HttpClient::builder()
        .connect_timeout(ADMIN_HTTP_CONNECT_TIMEOUT)
        .timeout(ADMIN_HTTP_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| HttpClient::new())
}

impl RustfsAdminClient {
    pub const STS_VERSION: &'static str = "2011-06-15";
    pub const STS_ACTION: &'static str = "AssumeRole";

    pub fn new_with_base_url(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        Self::new_with_base_url_and_http_client(
            base_url,
            access_key,
            secret_key,
            default_http_client(),
        )
    }

    pub fn new_with_base_url_and_ca_pem(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        ca_pem: &[u8],
    ) -> Result<Self, RustfsClientError> {
        let certs = Certificate::from_pem_bundle(ca_pem)
            .map_err(|_| RustfsClientError::InvalidTenantTlsCa)?;
        let mut builder = HttpClient::builder()
            .connect_timeout(ADMIN_HTTP_CONNECT_TIMEOUT)
            .timeout(ADMIN_HTTP_REQUEST_TIMEOUT);
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
        let http_client = builder
            .build()
            .map_err(|_| RustfsClientError::TlsClientBuildFailed)?;

        Ok(Self::new_with_base_url_and_http_client(
            base_url,
            access_key,
            secret_key,
            http_client,
        ))
    }

    pub fn new_with_base_url_and_http_client(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        http_client: HttpClient,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            region: "us-east-1".to_string(),
            http_client,
        }
    }

    pub fn from_tenant(
        tenant: &Tenant,
        credentials: RustfsCredentials,
    ) -> Result<Self, RustfsClientError> {
        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let service_name = tenant
            .new_io_service()
            .metadata
            .name
            .unwrap_or_else(|| format!("{}-io", tenant.name()));

        Ok(Self::new_with_base_url(
            format!("http://{service_name}.{namespace}.svc:9000"),
            credentials.access_key,
            credentials.secret_key,
        ))
    }

    pub async fn from_tls_tenant_for_sts(
        kube_client: &Client,
        tenant: &Tenant,
        credentials: RustfsCredentials,
    ) -> Result<Self, RustfsClientError> {
        if !helpers::tenant_tls_enabled(tenant) {
            return Err(RustfsClientError::TenantTlsRequired);
        }
        if helpers::tenant_tls_client_certificate_required(tenant) {
            return Err(RustfsClientError::TenantTlsClientCertificateRequired);
        }

        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let service_name = tenant
            .new_io_service()
            .metadata
            .name
            .unwrap_or_else(|| format!("{}-io", tenant.name()));
        let base_url = format!("https://{service_name}.{namespace}.svc:9000");

        match Self::load_tenant_tls_ca(kube_client, tenant).await? {
            Some(ca_pem) => Self::new_with_base_url_and_ca_pem(
                base_url,
                credentials.access_key,
                credentials.secret_key,
                &ca_pem,
            ),
            None => Ok(Self::new_with_base_url(
                base_url,
                credentials.access_key,
                credentials.secret_key,
            )),
        }
    }

    pub async fn load_tenant_tls_ca(
        kube_client: &Client,
        tenant: &Tenant,
    ) -> Result<Option<Vec<u8>>, RustfsClientError> {
        if !helpers::tenant_tls_enabled(tenant) {
            return Ok(None);
        }

        let tls_status = tenant
            .status
            .as_ref()
            .and_then(|status| status.certificates.tls.as_ref())
            .filter(|tls| tls.ready)
            .ok_or(RustfsClientError::TenantTlsNotReady)?;

        let Some(ca_ref) = tls_status.ca_secret_ref.as_ref() else {
            return Ok(None);
        };

        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let api: Api<corev1::Secret> = Api::namespaced(kube_client.clone(), &namespace);
        let secret = api.get(&ca_ref.name).await.map_err(|_| {
            RustfsClientError::TenantTlsCaSecretLookupFailed {
                secret: ca_ref.name.clone(),
            }
        })?;
        let key = ca_ref.key.as_deref().unwrap_or("ca.crt");
        let ca_pem = secret
            .data
            .as_ref()
            .and_then(|data| data.get(key))
            .map(|bytes| bytes.0.clone())
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| RustfsClientError::MissingTenantTlsCaKey {
                secret: ca_ref.name.clone(),
                key: key.to_string(),
            })?;

        Ok(Some(ca_pem))
    }

    /// Read Tenant credential Secret and return access/secret key pair.
    pub async fn load_tenant_credentials(
        kube_client: &Client,
        tenant: &Tenant,
    ) -> Result<RustfsCredentials, RustfsClientError> {
        let reference = tenant
            .spec
            .creds_secret
            .as_ref()
            .ok_or(RustfsClientError::MissingCredsSecret)?;

        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let api: Api<corev1::Secret> = Api::namespaced(kube_client.clone(), &namespace);
        let secret = api
            .get(&reference.name)
            .await
            .map_err(|_| RustfsClientError::TenantSecretLookupFailed)?;

        helpers::extract_credentials(secret.data.as_ref())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
