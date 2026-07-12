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

use std::error::Error;
use std::fmt;

pub(crate) const DEFAULT_CLUSTER_DOMAIN: &str = "cluster.local";
pub(crate) const OPERATOR_CLUSTER_DOMAIN_ENV: &str = "OPERATOR_CLUSTER_DOMAIN";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClusterDomain(String);

impl ClusterDomain {
    pub(crate) fn from_env() -> Result<Self, ClusterDomainError> {
        match std::env::var(OPERATOR_CLUSTER_DOMAIN_ENV) {
            Ok(value) if value.trim().is_empty() => Ok(Self::default()),
            Ok(value) => Self::parse(&value),
            Err(_) => Ok(Self::default()),
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, ClusterDomainError> {
        normalize_cluster_domain(value)
            .map(Self)
            .ok_or_else(|| ClusterDomainError::new(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ClusterDomain {
    fn default() -> Self {
        Self(DEFAULT_CLUSTER_DOMAIN.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClusterDomainError {
    value: String,
}

impl ClusterDomainError {
    fn new(value: &str) -> Self {
        Self {
            value: value.to_string(),
        }
    }
}

impl fmt::Display for ClusterDomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{OPERATOR_CLUSTER_DOMAIN_ENV} must be a valid DNS domain, for example 'cluster.local' or 'k8s.mse.cloud' (got {:?})",
            self.value
        )
    }
}

impl Error for ClusterDomainError {}

fn normalize_cluster_domain(value: &str) -> Option<String> {
    let domain = value.trim().trim_matches('.').to_ascii_lowercase();
    if domain.is_empty() || !domain.split('.').all(valid_dns_label) {
        return None;
    }
    Some(domain)
}

pub(crate) fn service_fqdn(service_name: &str, namespace: &str, cluster_domain: &str) -> String {
    format!("{service_name}.{namespace}.svc.{cluster_domain}")
}

pub(crate) fn pod_fqdn(
    pod_name: &str,
    headless_service: &str,
    namespace: &str,
    cluster_domain: &str,
) -> String {
    format!("{pod_name}.{headless_service}.{namespace}.svc.{cluster_domain}")
}

fn valid_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && label
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cluster_domain_trims_lowercases_and_removes_trailing_dot() {
        assert_eq!(
            normalize_cluster_domain(" K8S.MSE.Cloud. "),
            Some("k8s.mse.cloud".to_string())
        );
    }

    #[test]
    fn normalize_cluster_domain_rejects_invalid_values() {
        assert_eq!(normalize_cluster_domain(""), None);
        assert_eq!(normalize_cluster_domain("."), None);
        assert_eq!(normalize_cluster_domain("bad..domain"), None);
        assert_eq!(normalize_cluster_domain("-bad.domain"), None);
        assert_eq!(normalize_cluster_domain("bad_domain"), None);
    }

    #[test]
    fn service_and_pod_fqdns_use_given_cluster_domain() {
        assert_eq!(
            service_fqdn("tenant-a-io", "storage", "k8s.mse.cloud"),
            "tenant-a-io.storage.svc.k8s.mse.cloud"
        );
        assert_eq!(
            pod_fqdn(
                "tenant-a-pool-0-0",
                "tenant-a-hl",
                "storage",
                "k8s.mse.cloud"
            ),
            "tenant-a-pool-0-0.tenant-a-hl.storage.svc.k8s.mse.cloud"
        );
    }

    #[test]
    fn parse_cluster_domain_returns_normalized_value() {
        let domain = ClusterDomain::parse(" K8S.MSE.Cloud. ").unwrap();

        assert_eq!(domain.as_str(), "k8s.mse.cloud");
    }

    #[test]
    fn parse_cluster_domain_rejects_invalid_values() {
        let error = ClusterDomain::parse("bad_domain").unwrap_err();

        assert!(error.to_string().contains(OPERATOR_CLUSTER_DOMAIN_ENV));
    }
}
