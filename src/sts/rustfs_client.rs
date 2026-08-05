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

use std::ops::{Deref, DerefMut};

use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client};

use crate::Tenant;
use crate::cluster_dns;

pub use rustfs_admin::{
    CreateBucketResult, RustfsClientError, RustfsCredentials, RustfsErasureBackend,
    RustfsErasureSetInfo, RustfsPoolDecommissionInfo, RustfsPoolListItem, RustfsPoolStatus,
    RustfsServerInfo, RustfsServerUsage, RustfsUserInfo,
};

/// Tenant-aware wrapper around the kube-agnostic RustFS admin client.
pub struct RustfsAdminClient(pub rustfs_admin::RustfsAdminClient);

impl Deref for RustfsAdminClient {
    type Target = rustfs_admin::RustfsAdminClient;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RustfsAdminClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl RustfsAdminClient {
    pub const STS_VERSION: &'static str = rustfs_admin::RustfsAdminClient::STS_VERSION;
    pub const STS_ACTION: &'static str = rustfs_admin::RustfsAdminClient::STS_ACTION;

    pub fn new_with_base_url(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        Self(rustfs_admin::RustfsAdminClient::new_with_base_url(
            base_url, access_key, secret_key,
        ))
    }

    pub fn new_with_base_url_and_ca_pem(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        ca_pem: &[u8],
    ) -> Result<Self, RustfsClientError> {
        rustfs_admin::RustfsAdminClient::new_with_base_url_and_ca_pem(
            base_url, access_key, secret_key, ca_pem,
        )
        .map(Self)
    }

    pub fn new_with_base_url_and_http_client(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        http_client: reqwest::Client,
    ) -> Self {
        Self(
            rustfs_admin::RustfsAdminClient::new_with_base_url_and_http_client(
                base_url,
                access_key,
                secret_key,
                http_client,
            ),
        )
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
        cluster_domain: &str,
    ) -> Result<Self, RustfsClientError> {
        if !tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
            return Err(RustfsClientError::TenantTlsRequired);
        }
        if tenant
            .status
            .as_ref()
            .and_then(|status| status.certificates.tls.as_ref())
            .and_then(|tls| tls.client_ca_secret_ref.as_ref())
            .is_some()
        {
            return Err(RustfsClientError::TenantTlsClientCertificateRequired);
        }

        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let service_fqdn =
            cluster_dns::service_fqdn(&tenant.headless_service_name(), &namespace, cluster_domain);
        let base_url = format!("https://{service_fqdn}:9000");
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
        if !tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
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
        let data = secret
            .data
            .as_ref()
            .ok_or(RustfsClientError::TenantSecretLookupFailed)?;
        let credential = |key: &'static str| -> Result<String, RustfsClientError> {
            let value = data
                .get(key)
                .ok_or(RustfsClientError::MissingCredentialKey { key })?;
            let value = String::from_utf8(value.0.clone())
                .map_err(|_| RustfsClientError::InvalidCredentialValue { key })?;
            if value.is_empty() {
                return Err(RustfsClientError::EmptyCredentialValue { key });
            }
            Ok(value)
        };
        Ok(RustfsCredentials {
            access_key: credential("accesskey")?,
            secret_key: credential("secretkey")?,
        })
    }

    pub async fn assume_role(
        &self,
        policy: Option<&str>,
        duration_seconds: u64,
    ) -> Result<crate::sts::types::StsAssumeRoleCredentials, RustfsClientError> {
        let credentials = self.0.assume_role(policy, duration_seconds).await?;
        Ok(crate::sts::types::StsAssumeRoleCredentials {
            access_key_id: credentials.access_key_id,
            secret_access_key: credentials.secret_access_key,
            session_token: credentials.session_token,
            expiration: credentials.expiration,
        })
    }
}
