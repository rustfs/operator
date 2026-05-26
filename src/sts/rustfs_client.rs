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

use std::collections::BTreeMap;
use std::time::Duration;

use hmac::{Hmac, Mac};
use k8s_openapi::{ByteString, api::core::v1 as corev1};
use kube::{Api, Client};
use reqwest::{Certificate, Client as HttpClient, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;
use url::form_urlencoded;

use crate::Tenant;
use crate::sts::types::StsAssumeRoleCredentials;

const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";
const JSON_CONTENT_TYPE: &str = "application/json";
const ASSUME_ROLE_PATH: &str = "/";
const ADD_USER_PATH: &str = "/rustfs/admin/v3/add-user";
const USER_INFO_PATH: &str = "/rustfs/admin/v3/user-info";
const SET_POLICY_PATH: &str = "/rustfs/admin/v3/set-policy";
const LIST_CANNED_POLICIES_PATH: &str = "/rustfs/admin/v3/list-canned-policies";
const ADD_CANNED_POLICY_PATH: &str = "/rustfs/admin/v3/add-canned-policy";
const INFO_CANNED_POLICY_PATH: &str = "/rustfs/admin/v3/info-canned-policy";
const POOLS_LIST_PATH: &str = "/rustfs/admin/v3/pools/list";
const POOLS_STATUS_PATH: &str = "/rustfs/admin/v3/pools/status";
const POOLS_DECOMMISSION_PATH: &str = "/rustfs/admin/v3/pools/decommission";
const POOLS_CANCEL_PATH: &str = "/rustfs/admin/v3/pools/cancel";
const ADMIN_SIGNING_SERVICE: &str = "s3";
const STS_SIGNING_SERVICE: &str = "sts";
const ADMIN_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ADMIN_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Error type for RustFS admin/STS client operations.
#[derive(Debug)]
pub enum RustfsClientError {
    MissingTenantNamespace,
    MissingCredsSecret,
    MissingCredentialKey { key: &'static str },
    EmptyCredentialValue { key: &'static str },
    InvalidCredentialValue { key: &'static str },
    TenantSecretLookupFailed,
    InvalidPolicyName,
    InvalidPolicyDocument,
    TenantTlsRequired,
    TenantTlsNotReady,
    TenantTlsClientCertificateRequired,
    MissingTenantTlsCaKey { secret: String, key: String },
    TenantTlsCaSecretLookupFailed { secret: String },
    InvalidTenantTlsCa,
    TlsClientBuildFailed,
    RequestBuildFailed,
    RequestFailed,
    UnexpectedStatus(StatusCode),
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
            Self::UnexpectedStatus(status) => write!(f, "upstream returned {status}"),
            Self::ParseResponseFailed => write!(f, "failed to parse AssumeRole response"),
            Self::SigningFailed => write!(f, "failed to compute request signature"),
        }
    }
}

impl std::error::Error for RustfsClientError {}

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
        if !tenant_tls_enabled(tenant) {
            return Err(RustfsClientError::TenantTlsRequired);
        }
        if tenant_tls_client_certificate_required(tenant) {
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
        if !tenant_tls_enabled(tenant) {
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

        extract_credentials(secret.data.as_ref())
    }

    /// Query RustFS admin policy endpoint.
    pub async fn get_canned_policy(&self, policy_name: &str) -> Result<String, RustfsClientError> {
        if policy_name.trim().is_empty() {
            return Err(RustfsClientError::InvalidPolicyName);
        }

        let query = build_query_pairs(&[("name", policy_name)]);
        let path = INFO_CANNED_POLICY_PATH;
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let url = if query.is_empty() {
            url
        } else {
            format!("{url}?{query}")
        };

        let signed = self.sign_request("GET", path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::UnexpectedStatus(response.status()));
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        extract_canned_policy_document(&body)
    }

    /// Add or replace a RustFS canned policy through the admin API.
    pub async fn add_canned_policy(
        &self,
        policy_name: &str,
        policy_document: &str,
    ) -> Result<(), RustfsClientError> {
        if policy_name.trim().is_empty() {
            return Err(RustfsClientError::InvalidPolicyName);
        }
        serde_json::from_str::<Value>(policy_document)
            .map_err(|_| RustfsClientError::InvalidPolicyDocument)?;

        let query = build_query_pairs(&[("name", policy_name)]);
        let path = ADD_CANNED_POLICY_PATH;
        let url = format!("{}{}?{query}", self.base_url.trim_end_matches('/'), path);

        let signed = self.sign_request(
            "PUT",
            path,
            &query,
            policy_document,
            Some(JSON_CONTENT_TYPE),
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let response = self
            .http_client
            .put(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", JSON_CONTENT_TYPE)
            .body(policy_document.to_string())
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::UnexpectedStatus(response.status()));
        }

        Ok(())
    }

    pub async fn list_canned_policies(
        &self,
    ) -> Result<BTreeMap<String, String>, RustfsClientError> {
        let body = self
            .send_admin_request("GET", LIST_CANNED_POLICIES_PATH, "", "", None)
            .await?;
        let policies = serde_json::from_str::<BTreeMap<String, Value>>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)?;

        policies
            .into_iter()
            .map(|(name, policy)| {
                serde_json::to_string(&policy)
                    .map(|document| (name, document))
                    .map_err(|_| RustfsClientError::ParseResponseFailed)
            })
            .collect()
    }

    pub async fn user_exists(&self, access_key: &str) -> Result<bool, RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }

        let query = build_query_pairs(&[("accessKey", access_key)]);
        let path = USER_INFO_PATH;
        let url = format!("{}{}?{query}", self.base_url.trim_end_matches('/'), path);
        let signed = self.sign_request("GET", path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(true);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
            return Ok(false);
        }

        Err(RustfsClientError::UnexpectedStatus(status))
    }

    pub async fn add_user(
        &self,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }
        if secret_key.is_empty() {
            return Err(RustfsClientError::EmptyCredentialValue { key: "secretkey" });
        }

        let body = serde_json::json!({
            "secretKey": secret_key,
            "status": "enabled",
        })
        .to_string();
        let query = build_query_pairs(&[("accessKey", access_key)]);

        self.send_admin_request("PUT", ADD_USER_PATH, &query, &body, Some(JSON_CONTENT_TYPE))
            .await
            .map(|_| ())
    }

    pub async fn set_user_policy(
        &self,
        access_key: &str,
        policies: &[String],
    ) -> Result<(), RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }

        let policy_names = policies.join(",");
        let query = build_query_pairs(&[
            ("isGroup", "false"),
            ("policyName", policy_names.as_str()),
            ("userOrGroup", access_key),
        ]);

        self.send_admin_request("PUT", SET_POLICY_PATH, &query, "", None)
            .await
            .map(|_| ())
    }

    pub async fn create_bucket(
        &self,
        bucket: &str,
        region: Option<&str>,
        object_lock: bool,
    ) -> Result<CreateBucketResult, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let body = create_bucket_body(region);
        let content_type = (!body.is_empty()).then_some("application/xml");
        let mut extra_headers = Vec::new();
        if let Some(content_type) = content_type {
            extra_headers.push(("content-type", content_type));
        }
        if object_lock {
            extra_headers.push(("x-amz-bucket-object-lock-enabled", "true"));
        }
        let signed = self.sign_request_with_extra_headers(
            "PUT",
            &path,
            "",
            &body,
            ADMIN_SIGNING_SERVICE,
            &extra_headers,
        )?;
        let host = self.host()?;

        let mut request = self
            .http_client
            .put(format!("{}{}", self.base_url.trim_end_matches('/'), path))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host);

        for (name, value) in &extra_headers {
            request = request.header(*name, *value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(CreateBucketResult::Created);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if bucket_already_exists(status, &body) {
            return Ok(CreateBucketResult::AlreadyExists);
        }

        Err(RustfsClientError::UnexpectedStatus(status))
    }

    pub async fn bucket_object_lock_enabled(
        &self,
        bucket: &str,
    ) -> Result<bool, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_query_pairs(&[("object-lock", "")]);
        let signed = self.sign_request("GET", &path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(format!(
                "{}{}?{query}",
                self.base_url.trim_end_matches('/'),
                path
            ))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
                return Ok(false);
            }
            return Err(RustfsClientError::UnexpectedStatus(status));
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;
        Ok(body.contains("<ObjectLockEnabled>Enabled</ObjectLockEnabled>"))
    }

    pub async fn list_pools(&self) -> Result<Vec<RustfsPoolListItem>, RustfsClientError> {
        let body = self
            .send_admin_request("GET", POOLS_LIST_PATH, "", "", None)
            .await?;

        serde_json::from_str::<Vec<RustfsPoolListItem>>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    pub async fn pool_status_by_id(
        &self,
        pool_id: &str,
    ) -> Result<RustfsPoolStatus, RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        let body = self
            .send_admin_request("GET", POOLS_STATUS_PATH, &query, "", None)
            .await?;

        serde_json::from_str::<RustfsPoolStatus>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    pub async fn start_pool_decommission_by_id(
        &self,
        pool_id: &str,
    ) -> Result<(), RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        self.send_admin_request("POST", POOLS_DECOMMISSION_PATH, &query, "", None)
            .await?;
        Ok(())
    }

    pub async fn cancel_pool_decommission_by_id(
        &self,
        pool_id: &str,
    ) -> Result<(), RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        self.send_admin_request("POST", POOLS_CANCEL_PATH, &query, "", None)
            .await?;
        Ok(())
    }

    /// Send AssumeRole request to RustFS admin STS endpoint (`/`).
    pub async fn assume_role(
        &self,
        policy: Option<&str>,
        duration_seconds: u64,
    ) -> Result<StsAssumeRoleCredentials, RustfsClientError> {
        let mut params = vec![
            ("Version", Self::STS_VERSION.to_string()),
            ("Action", Self::STS_ACTION.to_string()),
            ("DurationSeconds", duration_seconds.to_string()),
        ];

        if let Some(policy) = policy {
            params.push(("Policy", policy.to_string()));
        }

        let body = build_query_pairs(
            &params
                .iter()
                .map(|(k, v)| (&k[..], &v[..]))
                .collect::<Vec<_>>(),
        );

        let path = ASSUME_ROLE_PATH;
        let signed = self.sign_request(
            "POST",
            path,
            "",
            &body,
            Some(FORM_CONTENT_TYPE),
            STS_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let response = self
            .http_client
            .post(format!("{}/", self.base_url.trim_end_matches('/')))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", FORM_CONTENT_TYPE)
            .body(body)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::UnexpectedStatus(response.status()));
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        parse_assume_role_response(&body).ok_or(RustfsClientError::ParseResponseFailed)
    }

    async fn send_admin_request(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &str,
        content_type: Option<&str>,
    ) -> Result<String, RustfsClientError> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let url = if query.is_empty() {
            url
        } else {
            format!("{url}?{query}")
        };

        let signed = self.sign_request(
            method,
            path,
            query,
            body,
            content_type,
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let builder = match method {
            "GET" => self.http_client.get(url),
            "POST" => self.http_client.post(url),
            "PUT" => self.http_client.put(url),
            _ => return Err(RustfsClientError::RequestBuildFailed),
        }
        .header("x-amz-date", &signed.amz_date)
        .header("x-amz-content-sha256", &signed.payload_hash)
        .header("authorization", &signed.authorization)
        .header("host", host);

        let builder = if let Some(content_type) = content_type {
            builder.header("content-type", content_type)
        } else {
            builder
        };
        let builder = if body.is_empty() {
            builder
        } else {
            builder.body(body.to_string())
        };

        let response = builder
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::UnexpectedStatus(response.status()));
        }

        response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)
    }

    fn sign_request(
        &self,
        method: &str,
        path: &str,
        canonical_query: &str,
        payload: &str,
        content_type: Option<&str>,
        service: &str,
    ) -> Result<SignedRequest, RustfsClientError> {
        let extra_headers = content_type
            .map(|content_type| vec![("content-type", content_type)])
            .unwrap_or_default();
        self.sign_request_with_extra_headers(
            method,
            path,
            canonical_query,
            payload,
            service,
            &extra_headers,
        )
    }

    fn sign_request_with_extra_headers(
        &self,
        method: &str,
        path: &str,
        canonical_query: &str,
        payload: &str,
        service: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<SignedRequest, RustfsClientError> {
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let payload_hash = sha256_hex(payload.as_bytes());

        let host = self.host()?;
        let mut signed_headers = vec![
            ("host", host.as_str()),
            ("x-amz-content-sha256", payload_hash.as_str()),
            ("x-amz-date", amz_date.as_str()),
        ];

        signed_headers.extend(extra_headers.iter().copied());
        signed_headers.sort_by_key(|(name, _)| *name);

        let canonical_headers: String = signed_headers
            .iter()
            .map(|(key, value)| format!("{}:{}\n", key.to_ascii_lowercase(), value.trim()))
            .collect();
        let mut signed_header_names = String::new();
        for (index, (name, _)) in signed_headers.iter().enumerate() {
            if index > 0 {
                signed_header_names.push(';');
            }
            signed_header_names.push_str(&name.to_ascii_lowercase());
        }

        let canonical_request = format!(
            "{method}\n{path}\n{canonical_query}\n{canonical_headers}\n{signed_header_names}\n{payload_hash}",
        );

        let credential_scope = format!("{date_stamp}/{}/{service}/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );

        let signing_key = derive_signing_key(&self.secret_key, &date_stamp, &self.region, service)?;
        let signature = hmac_sha256_hex(&signing_key, &string_to_sign)?;
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_header_names, signature
        );

        Ok(SignedRequest {
            amz_date,
            payload_hash,
            authorization,
        })
    }

    fn host(&self) -> Result<String, RustfsClientError> {
        let parsed =
            Url::parse(&self.base_url).map_err(|_| RustfsClientError::RequestBuildFailed)?;
        let mut host = parsed
            .host_str()
            .map(str::to_string)
            .or_else(|| parsed.host().map(|h| h.to_string()))
            .ok_or(RustfsClientError::RequestBuildFailed)?;
        if let Some(port) = parsed.port() {
            host.push(':');
            host.push_str(&port.to_string());
        }
        Ok(host)
    }
}

fn extract_credentials(
    data: Option<&BTreeMap<String, ByteString>>,
) -> Result<RustfsCredentials, RustfsClientError> {
    let secret_data = data.ok_or(RustfsClientError::TenantSecretLookupFailed)?;

    Ok(RustfsCredentials {
        access_key: get_secret_value(secret_data, "accesskey")?,
        secret_key: get_secret_value(secret_data, "secretkey")?,
    })
}

fn tenant_tls_enabled(tenant: &Tenant) -> bool {
    tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled())
}

fn tenant_tls_client_certificate_required(tenant: &Tenant) -> bool {
    tenant
        .status
        .as_ref()
        .and_then(|status| status.certificates.tls.as_ref())
        .and_then(|tls| tls.client_ca_secret_ref.as_ref())
        .is_some()
}

fn get_secret_value(
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

fn build_query_pairs(params: &[(&str, &str)]) -> String {
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

fn create_bucket_body(region: Option<&str>) -> String {
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

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn body_mentions_not_found(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("nosuchuser")
        || body.contains("no such user")
        || body.contains("user not exist")
        || body.contains("nosuchpolicy")
        || body.contains("no such policy")
        || body.contains("objectlockconfigurationnotfound")
        || body.contains("not found")
}

fn bucket_already_exists(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::CONFLICT {
        let body = body.to_ascii_lowercase();
        return body.contains("bucketalreadyexists") || body.contains("bucketalreadyownedbyyou");
    }

    false
}

fn extract_canned_policy_document(body: &str) -> Result<String, RustfsClientError> {
    let value = serde_json::from_str::<Value>(body)
        .map_err(|_| RustfsClientError::InvalidPolicyDocument)?;
    let policy = value.get("policy").unwrap_or(&value);

    serde_json::to_string(policy).map_err(|_| RustfsClientError::InvalidPolicyDocument)
}

fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], message: &str) -> Result<Vec<u8>, RustfsClientError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| RustfsClientError::SigningFailed)?;
    mac.update(message.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hmac_sha256_hex(key: &[u8], message: &str) -> Result<String, RustfsClientError> {
    let bytes = hmac_sha256(key, message)?;
    Ok(hex::encode(bytes))
}

fn derive_signing_key(
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

fn parse_assume_role_response(body: &str) -> Option<StsAssumeRoleCredentials> {
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

fn extract_xml_tag(document: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    let open_idx = document.find(&open)?;
    let start = open_idx + open.len();
    let rest = &document[start..];
    let end = rest.find(&close)?;

    Some(rest[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{Request, StatusCode},
        routing::{get, post, put},
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn secret_with_fields(fields: Vec<(&str, &[u8])>) -> corev1::Secret {
        let mut data = BTreeMap::new();
        for (key, value) in fields {
            data.insert(key.to_string(), ByteString(value.to_vec()));
        }

        corev1::Secret {
            data: Some(data),
            ..Default::default()
        }
    }

    #[test]
    fn extract_credentials_reports_missing_access_key() {
        let secret = secret_with_fields(vec![("secretkey", b"sekret")]);

        let err =
            extract_credentials(secret.data.as_ref()).expect_err("expected missing access key");
        assert!(matches!(
            err,
            RustfsClientError::MissingCredentialKey { key: "accesskey" }
        ));
    }

    #[test]
    fn extract_credentials_reports_non_utf8_access_key() {
        let secret =
            secret_with_fields(vec![("accesskey", &[0xff, 0xfe]), ("secretkey", b"sekret")]);

        let err = extract_credentials(secret.data.as_ref()).expect_err("expected invalid utf8");
        assert!(matches!(
            err,
            RustfsClientError::InvalidCredentialValue { key: "accesskey" }
        ));
    }

    #[test]
    fn extract_credentials_reports_missing_secret_key() {
        let secret = secret_with_fields(vec![("accesskey", b"access")]);

        let err =
            extract_credentials(secret.data.as_ref()).expect_err("expected missing secret key");
        assert!(matches!(
            err,
            RustfsClientError::MissingCredentialKey { key: "secretkey" }
        ));
    }

    #[test]
    fn extract_credentials_reports_non_utf8_secret_key() {
        let secret =
            secret_with_fields(vec![("accesskey", b"access"), ("secretkey", &[0xff, 0xfe])]);

        let err = extract_credentials(secret.data.as_ref()).expect_err("expected invalid utf8");
        assert!(matches!(
            err,
            RustfsClientError::InvalidCredentialValue { key: "secretkey" }
        ));
    }

    #[test]
    fn extract_credentials_reports_empty_secret_key() {
        let secret = secret_with_fields(vec![("accesskey", b"abc"), ("secretkey", b"")]);

        let err = extract_credentials(secret.data.as_ref()).expect_err("expected empty secret key");
        assert!(matches!(
            err,
            RustfsClientError::EmptyCredentialValue { key: "secretkey" }
        ));
    }

    #[test]
    fn parse_assume_role_xml_success_and_failure() {
        let body_ok = "<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleResult><Credentials><AccessKeyId>AKI</AccessKeyId><SecretAccessKey>SEC</SecretAccessKey><SessionToken>TOKEN</SessionToken><Expiration>2026-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
        let parsed =
            parse_assume_role_response(body_ok).expect("valid assume role response should parse");

        assert_eq!(parsed.access_key_id, "AKI");
        assert_eq!(parsed.secret_access_key, "SEC");
        assert_eq!(parsed.session_token, "TOKEN");
        assert_eq!(parsed.expiration, "2026-01-01T00:00:00Z");

        assert!(parse_assume_role_response("<NotFound />").is_none());
    }

    #[derive(Clone, Default)]
    struct Capture {
        path: Arc<Mutex<String>>,
        query: Arc<Mutex<String>>,
        body: Arc<Mutex<String>>,
        authorization: Arc<Mutex<String>>,
        object_lock_header: Arc<Mutex<String>>,
    }

    #[tokio::test]
    async fn assume_role_request_targets_root_path_and_action_is_assume_role() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new().route(
            "/",
            post(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    let path = req.uri().path().to_string();
                    let query = req.uri().query().unwrap_or("").to_string();
                    let authorization = req
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body = String::from_utf8(body_bytes.to_vec()).unwrap();

                    *c.path.lock().await = path;
                    *c.query.lock().await = query;
                    *c.body.lock().await = body;
                    *c.authorization.lock().await = authorization;

                    let response =
                        "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>AKI</AccessKeyId><SecretAccessKey>SEC</SecretAccessKey><SessionToken>TOKEN</SessionToken><Expiration>2026-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
                    (StatusCode::OK, response)
                },
            ),
        )
        .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let creds = client
            .assume_role(Some("{\"Statement\": []}"), 3600)
            .await
            .unwrap();
        assert_eq!(creds.access_key_id, "AKI");

        assert_eq!(&*capture.path.lock().await, "/");
        assert!(capture.body.lock().await.contains("Action=AssumeRole"));
        assert!(capture.body.lock().await.contains("Version=2011-06-15"));
        assert!(capture.body.lock().await.contains("DurationSeconds=3600"));
        assert!(capture.query.lock().await.is_empty());
        assert!(
            capture
                .authorization
                .lock()
                .await
                .contains("/sts/aws4_request")
        );

        server.abort();
    }

    #[tokio::test]
    async fn info_canned_policy_uses_expected_path_and_query() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                "/rustfs/admin/v3/info-canned-policy",
                get(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        let path = req.uri().path().to_string();
                        let query = req.uri().query().unwrap_or("").to_string();
                        let authorization = req
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        *c.path.lock().await = path;
                        *c.query.lock().await = query;
                        *c.authorization.lock().await = authorization;

                        (
                            StatusCode::OK,
                            "{\"policy_name\":\"tenant-policy\",\"policy\":{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"allow\",\"Effect\":\"Allow\"}]}}",
                        )
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let policy = client.get_canned_policy("tenant-policy").await.unwrap();
        let policy_value = serde_json::from_str::<Value>(&policy).unwrap();
        assert_eq!(policy_value["Version"], "2012-10-17");
        assert_eq!(policy_value["Statement"][0]["Sid"], "allow");

        assert_eq!(
            &*capture.path.lock().await,
            "/rustfs/admin/v3/info-canned-policy"
        );
        assert!(capture.query.lock().await.contains("name=tenant-policy"));
        assert!(
            capture
                .authorization
                .lock()
                .await
                .contains("/s3/aws4_request")
        );

        server.abort();
    }

    #[tokio::test]
    async fn add_canned_policy_uses_expected_path_query_body_and_admin_signing() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                "/rustfs/admin/v3/add-canned-policy",
                put(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        let path = req.uri().path().to_string();
                        let query = req.uri().query().unwrap_or("").to_string();
                        let authorization = req
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                            .await
                            .unwrap();
                        let body = String::from_utf8(body_bytes.to_vec()).unwrap();

                        *c.path.lock().await = path;
                        *c.query.lock().await = query;
                        *c.authorization.lock().await = authorization;
                        *c.body.lock().await = body;

                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;

        client
            .add_canned_policy("tenant-policy", policy)
            .await
            .unwrap();

        assert_eq!(
            &*capture.path.lock().await,
            "/rustfs/admin/v3/add-canned-policy"
        );
        assert!(capture.query.lock().await.contains("name=tenant-policy"));
        assert_eq!(&*capture.body.lock().await, policy);
        assert!(
            capture
                .authorization
                .lock()
                .await
                .contains("/s3/aws4_request")
        );

        server.abort();
    }

    #[tokio::test]
    async fn list_pools_parses_current_rustfs_pool_shape() {
        let router = Router::new().route(
            POOLS_LIST_PATH,
            get(|| async {
                (
                    StatusCode::OK,
                    r#"[{"id":1,"cmdline":"http://tenant-pool-a-{0...3}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...3}","lastUpdate":"2026-05-20T00:00:00Z","totalSize":100,"currentSize":50,"usedSize":25,"used":25.0,"status":"running","decommissionInfo":{"startTime":"2026-05-20T00:00:00Z","complete":false,"failed":false,"canceled":false,"objectsDecommissioned":7,"objectsDecommissionedFailed":1,"bytesDecommissioned":9,"bytesDecommissionedFailed":2}}]"#,
                )
            }),
        );

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let pools = client.list_pools().await.unwrap();

        assert_eq!(pools[0].id, 1);
        assert_eq!(pools[0].status, "running");
        assert_eq!(
            pools[0]
                .decommission
                .as_ref()
                .and_then(|info| info.objects_decommissioned),
            Some(7)
        );

        server.abort();
    }

    #[tokio::test]
    async fn pool_decommission_start_uses_by_id_query_and_admin_signing() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                POOLS_DECOMMISSION_PATH,
                post(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.query.lock().await = req.uri().query().unwrap_or("").to_string();
                        *c.authorization.lock().await = req
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        client.start_pool_decommission_by_id("1").await.unwrap();

        assert_eq!(&*capture.path.lock().await, POOLS_DECOMMISSION_PATH);
        assert_eq!(&*capture.query.lock().await, "by-id=true&pool=1");
        assert!(
            capture
                .authorization
                .lock()
                .await
                .contains("/s3/aws4_request")
        );

        server.abort();
    }

    #[tokio::test]
    async fn pool_status_uses_by_id_query_and_parses_decommission_info() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                POOLS_STATUS_PATH,
                get(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.query.lock().await = req.uri().query().unwrap_or("").to_string();

                        (
                            StatusCode::OK,
                            r#"{"id":1,"cmdline":"http://tenant-pool-a-{0...3}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...3}","lastUpdate":"2026-05-20T00:00:00Z","decommissionInfo":{"startTime":"2026-05-20T00:00:00Z","complete":true,"failed":false,"canceled":false,"objectsDecommissioned":10,"objectsDecommissionedFailed":0,"bytesDecommissioned":20,"bytesDecommissionedFailed":0}}"#,
                        )
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

        let status = client.pool_status_by_id("1").await.unwrap();

        assert_eq!(status.id, 1);
        assert_eq!(&*capture.path.lock().await, POOLS_STATUS_PATH);
        assert_eq!(&*capture.query.lock().await, "by-id=true&pool=1");
        assert_eq!(
            status.decommission.and_then(|info| info.complete),
            Some(true)
        );

        server.abort();
    }

    #[tokio::test]
    async fn add_user_uses_expected_path_query_and_body() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                ADD_USER_PATH,
                put(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.query.lock().await = req.uri().query().unwrap_or("").to_string();
                        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                            .await
                            .unwrap();
                        *c.body.lock().await = String::from_utf8(body_bytes.to_vec()).unwrap();
                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        client.add_user("app-user", "secret123").await.unwrap();

        assert_eq!(&*capture.path.lock().await, ADD_USER_PATH);
        assert_eq!(&*capture.query.lock().await, "accessKey=app-user");
        assert_eq!(
            &*capture.body.lock().await,
            r#"{"secretKey":"secret123","status":"enabled"}"#
        );

        server.abort();
    }

    #[tokio::test]
    async fn set_user_policy_uses_single_authoritative_mapping_call() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                SET_POLICY_PATH,
                put(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.query.lock().await = req.uri().query().unwrap_or("").to_string();
                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        client
            .set_user_policy(
                "app-user",
                &["app-readwrite".to_string(), "diagnostics".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(&*capture.path.lock().await, SET_POLICY_PATH);
        assert_eq!(
            &*capture.query.lock().await,
            "isGroup=false&policyName=app-readwrite%2Cdiagnostics&userOrGroup=app-user"
        );

        server.abort();
    }

    #[tokio::test]
    async fn create_bucket_sends_object_lock_header_and_region_body() {
        let capture = Capture::default();
        let route_capture = capture.clone();

        let router = Router::new()
            .route(
                "/app-data",
                put(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.object_lock_header.lock().await = req
                            .headers()
                            .get("x-amz-bucket-object-lock-enabled")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                            .await
                            .unwrap();
                        *c.body.lock().await = String::from_utf8(body_bytes.to_vec()).unwrap();
                        StatusCode::OK
                    },
                ),
            )
            .with_state(route_capture.clone());

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client =
            RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
        let result = client
            .create_bucket("app-data", Some("us-west-2"), true)
            .await
            .unwrap();

        assert_eq!(result, CreateBucketResult::Created);
        assert_eq!(&*capture.path.lock().await, "/app-data");
        assert_eq!(&*capture.object_lock_header.lock().await, "true");
        assert!(
            capture
                .body
                .lock()
                .await
                .contains("<LocationConstraint>us-west-2</LocationConstraint>")
        );

        server.abort();
    }

    #[test]
    fn extract_canned_policy_document_accepts_raw_policy_document() {
        let raw_policy =
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"raw\",\"Effect\":\"Allow\"}]}";

        let policy = extract_canned_policy_document(raw_policy).unwrap();

        let policy_value = serde_json::from_str::<Value>(&policy).unwrap();
        assert_eq!(policy_value["Version"], "2012-10-17");
        assert_eq!(policy_value["Statement"][0]["Sid"], "raw");
    }
}
