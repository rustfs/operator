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

//! Kube/Tenant wrappers around the kube-agnostic RustFS admin/S3/STS client.
//!
//! The wire-protocol client implementation (request signing, HTTP dispatch,
//! response parsing) lives in the `rustfs-admin` crate and is re-exported
//! here. This module only adds the Tenant/kube-specific constructors that
//! need access to `kube::Client` and the `Tenant` CRD type.

use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client};

use crate::Tenant;
use crate::cluster_dns;

/// helpers: Tenant/kube credential and TLS status parsing.
#[path = "helpers.rs"]
mod helpers;

pub use rustfs_admin::{
    CreateBucketResult, RustfsAdminClient, RustfsClientError, RustfsCredentials,
    RustfsErasureBackend, RustfsErasureSetInfo, RustfsPoolDecommissionInfo, RustfsPoolListItem,
    RustfsPoolStatus, RustfsServerInfo, RustfsServerUsage, StsAssumeRoleCredentials,
};

pub(super) fn tls_tenant_base_url(
    tenant: &Tenant,
    cluster_domain: &str,
) -> Result<String, RustfsClientError> {
    let namespace = tenant
        .namespace()
        .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
    let service_fqdn =
        cluster_dns::service_fqdn(&tenant.headless_service_name(), &namespace, cluster_domain);
    Ok(format!("https://{service_fqdn}:9000"))
}

/// Build a RustFS admin client using the tenant's in-cluster (plain HTTP) service address.
pub fn client_from_tenant(
    tenant: &Tenant,
    credentials: RustfsCredentials,
) -> Result<RustfsAdminClient, RustfsClientError> {
    let namespace = tenant
        .namespace()
        .map_err(|_| RustfsClientError::MissingTenantNamespace)?;
    let service_name = tenant
        .new_io_service()
        .metadata
        .name
        .unwrap_or_else(|| format!("{}-io", tenant.name()));

    Ok(RustfsAdminClient::new_with_base_url(
        format!("http://{service_name}.{namespace}.svc:9000"),
        credentials.access_key,
        credentials.secret_key,
    ))
}

/// Build a RustFS admin client against the tenant's TLS-enabled headless service,
/// trusting the tenant's CA if one is published. Requires TLS to be enabled.
pub async fn client_from_tls_tenant_for_sts(
    kube_client: &Client,
    tenant: &Tenant,
    credentials: RustfsCredentials,
    cluster_domain: &str,
) -> Result<RustfsAdminClient, RustfsClientError> {
    if !helpers::tenant_tls_enabled(tenant) {
        return Err(RustfsClientError::TenantTlsRequired);
    }
    if helpers::tenant_tls_client_certificate_required(tenant) {
        return Err(RustfsClientError::TenantTlsClientCertificateRequired);
    }

    let base_url = tls_tenant_base_url(tenant, cluster_domain)?;

    match load_tenant_tls_ca(kube_client, tenant).await? {
        Some(ca_pem) => RustfsAdminClient::new_with_base_url_and_ca_pem(
            base_url,
            credentials.access_key,
            credentials.secret_key,
            &ca_pem,
        ),
        None => Ok(RustfsAdminClient::new_with_base_url(
            base_url,
            credentials.access_key,
            credentials.secret_key,
        )),
    }
}

/// Load the tenant's TLS CA bundle, if the tenant publishes one.
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

/// Read the Tenant credential Secret and return an access/secret key pair.
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
