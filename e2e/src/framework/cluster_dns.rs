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

pub const DEFAULT_CLUSTER_DOMAIN: &str = "cluster.local";
const E2E_CLUSTER_DOMAIN_ENV: &str = "RUSTFS_E2E_CLUSTER_DOMAIN";

pub fn configured_cluster_domain(value: &str) -> String {
    if value.trim().is_empty() {
        return DEFAULT_CLUSTER_DOMAIN.to_string();
    }
    normalize_cluster_domain(value).unwrap_or_else(|| {
        panic!(
            "{E2E_CLUSTER_DOMAIN_ENV} must be a valid DNS domain, for example 'cluster.local' or 'k8s.mse.cloud'"
        )
    })
}

pub fn service_fqdn(service_name: &str, namespace: &str, cluster_domain: &str) -> String {
    format!("{service_name}.{namespace}.svc.{cluster_domain}")
}

pub fn pod_fqdn(
    pod_name: &str,
    headless_service: &str,
    namespace: &str,
    cluster_domain: &str,
) -> String {
    format!("{pod_name}.{headless_service}.{namespace}.svc.{cluster_domain}")
}

fn normalize_cluster_domain(value: &str) -> Option<String> {
    let domain = value.trim().trim_matches('.').to_ascii_lowercase();
    if domain.is_empty() || !domain.split('.').all(valid_dns_label) {
        return None;
    }
    Some(domain)
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
