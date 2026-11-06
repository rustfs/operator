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
    BucketEncryptionAlgorithm, BucketEncryptionConfiguration,
    BucketLifecycleAbortIncompleteMultipartUpload, BucketLifecycleConfiguration,
    BucketLifecycleExpiration, BucketLifecycleNoncurrentVersionTransition, BucketLifecycleRule,
    BucketLifecycleRuleStatus, BucketLifecycleTransition, BucketObjectLockConfiguration,
    BucketObjectLockDefaultRetention, BucketObjectLockRetentionMode,
    BucketObjectLockRetentionPeriod, BucketVersioningState, BucketVersioningUpdate,
    CreateBucketResult, RustfsClientError, RustfsCredentials, RustfsDriveInfo,
    RustfsErasureBackend, RustfsErasureSetInfo, RustfsPoolDecommissionInfo, RustfsPoolListItem,
    RustfsPoolStatus, RustfsServerInfo, RustfsServerProperties, RustfsServerUsage, RustfsTier,
    RustfsTierConfig, RustfsUserInfo, canonicalize_bucket_lifecycle_xml,
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

    pub fn new_with_base_url_tls_material(
        base_url: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        ca_pem: Option<&[u8]>,
        client_cert_and_key: Option<(&[u8], &[u8])>,
    ) -> Result<Self, RustfsClientError> {
        rustfs_admin::RustfsAdminClient::new_with_base_url_tls_material(
            base_url,
            access_key,
            secret_key,
            ca_pem,
            client_cert_and_key,
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

        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let service_fqdn =
            cluster_dns::service_fqdn(&tenant.headless_service_name(), &namespace, cluster_domain);
        let base_url = format!("https://{service_fqdn}:9000");
        let ca_pem = Self::load_tenant_tls_ca(kube_client, tenant).await?;
        let client_identity = Self::load_tenant_tls_client_identity(kube_client, tenant).await?;
        match (ca_pem.as_deref(), client_identity.as_ref()) {
            (None, None) => Ok(Self::new_with_base_url(
                base_url,
                credentials.access_key,
                credentials.secret_key,
            )),
            (ca_pem, client_identity) => Self::new_with_base_url_tls_material(
                base_url,
                credentials.access_key,
                credentials.secret_key,
                ca_pem,
                client_identity
                    .map(|(cert_pem, key_pem)| (cert_pem.as_slice(), key_pem.as_slice())),
            ),
        }
    }

    pub async fn load_tenant_tls_client_identity(
        kube_client: &Client,
        tenant: &Tenant,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>, RustfsClientError> {
        if !tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
            return Ok(None);
        }
        let tls_status = tenant
            .status
            .as_ref()
            .and_then(|status| status.certificates.tls.as_ref())
            .filter(|tls| tls.ready)
            .ok_or(RustfsClientError::TenantTlsNotReady)?;
        let mtls_required = tenant
            .spec
            .tls
            .as_ref()
            .is_some_and(|tls| tls.is_mtls_enabled())
            || tls_status.client_ca_secret_ref.is_some();
        if !mtls_required {
            return Ok(None);
        }
        let Some(server_ref) = tls_status.server_secret_ref.as_ref() else {
            return Err(RustfsClientError::TenantTlsClientCertificateRequired);
        };
        let namespace = tenant
            .namespace()
            .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
        let api: Api<corev1::Secret> = Api::namespaced(kube_client.clone(), &namespace);
        let secret = api.get(&server_ref.name).await.map_err(|_| {
            RustfsClientError::TenantTlsCaSecretLookupFailed {
                secret: server_ref.name.clone(),
            }
        })?;
        let data = secret.data.as_ref();
        let cert_pem = data
            .and_then(|map| map.get("tls.crt"))
            .map(|bytes| bytes.0.clone())
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| RustfsClientError::MissingTenantTlsCaKey {
                secret: server_ref.name.clone(),
                key: "tls.crt".to_string(),
            })?;
        let key_pem = data
            .and_then(|map| map.get("tls.key"))
            .map(|bytes| bytes.0.clone())
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| RustfsClientError::MissingTenantTlsCaKey {
                secret: server_ref.name.clone(),
                key: "tls.key".to_string(),
            })?;
        Ok(Some((cert_pem, key_pem)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::status::{
        Status,
        certificate::{SecretStatusRef, Status as CertificateStatus, TlsCertificateStatus},
    };
    use crate::types::v1alpha1::tls::{TlsConfig, TlsMode};
    use http::{Method, Request, Response, StatusCode};
    use k8s_openapi::ByteString;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::client::Body;
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use tower::service_fn;

    #[tokio::test]
    async fn load_tenant_tls_client_identity_reads_server_secret_when_enable_mtls_is_set() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.namespace = Some("default".to_string());
        tenant.spec.tls = Some(TlsConfig {
            mode: TlsMode::CertManager,
            enable_mtls: true,
            ..Default::default()
        });
        tenant.status = Some(Status {
            certificates: CertificateStatus {
                tls: Some(TlsCertificateStatus {
                    mode: "CertManager".to_string(),
                    ready: true,
                    server_secret_ref: Some(SecretStatusRef {
                        name: "mtls-server-tls".to_string(),
                        key: None,
                        resource_version: Some("1".to_string()),
                    }),
                    ..Default::default()
                }),
            },
            ..Default::default()
        });

        let secret = corev1::Secret {
            metadata: ObjectMeta {
                name: Some("mtls-server-tls".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::from([
                ("tls.crt".to_string(), ByteString(b"cert-pem".to_vec())),
                ("tls.key".to_string(), ByteString(b"key-pem".to_vec())),
            ])),
            ..Default::default()
        };

        let service = service_fn(move |request: Request<Body>| {
            let secret = secret.clone();
            async move {
                assert_eq!(request.method(), Method::GET);
                assert_eq!(
                    request.uri().path(),
                    "/api/v1/namespaces/default/secrets/mtls-server-tls"
                );
                let response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(
                        serde_json::to_vec(&secret).expect("Secret should serialize"),
                    ))
                    .expect("response should build");
                Ok::<_, Infallible>(response)
            }
        });
        let client = Client::new(service, "default");

        let identity = RustfsAdminClient::load_tenant_tls_client_identity(&client, &tenant)
            .await
            .expect("client identity should load")
            .expect("client identity should be Some when enable_mtls=true");

        assert_eq!(identity.0, b"cert-pem");
        assert_eq!(identity.1, b"key-pem");
    }
}
