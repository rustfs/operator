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

//! Internal helper duties: Tenant/kube credential and TLS status parsing.
//! Wire-protocol helpers (signing, hashing, response parsing) live in the
//! kube-agnostic `rustfs-admin` crate.
use std::collections::BTreeMap;

use k8s_openapi::ByteString;

use crate::Tenant;
use crate::sts::rustfs_client::{RustfsClientError, RustfsCredentials};

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

#[cfg(test)]
mod tests {
    use k8s_openapi::{ByteString, api::core::v1 as corev1};
    use std::collections::BTreeMap;

    use super::extract_credentials;
    use crate::sts::rustfs_client::RustfsClientError;

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
}
