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

use kube::KubeSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Kubernetes Service IP family policy values.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
pub enum IpFamilyPolicy {
    SingleStack,
    PreferDualStack,
    RequireDualStack,
}

impl IpFamilyPolicy {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::SingleStack => "SingleStack",
            Self::PreferDualStack => "PreferDualStack",
            Self::RequireDualStack => "RequireDualStack",
        }
    }
}

/// Kubernetes Service IP family values.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, PartialEq, Eq)]
pub enum IpFamily {
    #[serde(rename = "IPv4")]
    #[schemars(rename = "IPv4")]
    IPv4,
    #[serde(rename = "IPv6")]
    #[schemars(rename = "IPv6")]
    IPv6,
}

impl IpFamily {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::IPv4 => "IPv4",
            Self::IPv6 => "IPv6",
        }
    }
}

/// Tenant Service and listen-address networking.
///
/// When omitted, generated Services inherit the cluster default IP family policy and RustFS
/// listens on `0.0.0.0`. Set `ipFamilies: [IPv6]` or a dual-stack policy for IPv6-only and
/// dual-stack clusters.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, ToSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_family_policy: Option<IpFamilyPolicy>,

    #[schemars(length(max = 2), extend("x-kubernetes-list-type" = "atomic"))]
    #[x_kube(validation = Rule::new("self.size() != 2 || self[0] != self[1]").message("ipFamilies must not contain duplicates"))]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_families: Vec<IpFamily>,
}

impl NetworkConfig {
    pub(crate) fn uses_ipv6(&self) -> bool {
        self.ip_families
            .iter()
            .any(|family| matches!(family, IpFamily::IPv6))
            || matches!(
                self.ip_family_policy,
                Some(IpFamilyPolicy::PreferDualStack | IpFamilyPolicy::RequireDualStack)
            )
    }

    pub(crate) fn rustfs_listen_address(&self, port: u16) -> String {
        rustfs_listen_address(Some(self), port)
    }

    pub(crate) fn service_ip_family_policy(&self) -> Option<String> {
        self.ip_family_policy
            .as_ref()
            .map(|policy| policy.as_str().to_string())
    }

    pub(crate) fn service_ip_families(&self) -> Option<Vec<String>> {
        if self.ip_families.is_empty() {
            None
        } else {
            Some(
                self.ip_families
                    .iter()
                    .map(|family| family.as_str().to_string())
                    .collect(),
            )
        }
    }
}

pub(crate) fn rustfs_listen_address(network: Option<&NetworkConfig>, port: u16) -> String {
    if network.is_some_and(NetworkConfig::uses_ipv6) {
        format!("[::]:{port}")
    } else {
        format!("0.0.0.0:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::{IpFamily, IpFamilyPolicy, NetworkConfig, rustfs_listen_address};

    #[test]
    fn omitted_network_keeps_ipv4_listen_addresses() {
        assert_eq!(rustfs_listen_address(None, 9000), "0.0.0.0:9000");
        assert_eq!(
            NetworkConfig::default().rustfs_listen_address(9001),
            "0.0.0.0:9001"
        );
    }

    #[test]
    fn ipv6_family_and_dual_stack_policy_listen_on_unspecified_v6() {
        let ipv6 = NetworkConfig {
            ip_family_policy: Some(IpFamilyPolicy::SingleStack),
            ip_families: vec![IpFamily::IPv6],
        };
        assert_eq!(ipv6.rustfs_listen_address(9000), "[::]:9000");

        let dual = NetworkConfig {
            ip_family_policy: Some(IpFamilyPolicy::PreferDualStack),
            ip_families: vec![IpFamily::IPv4, IpFamily::IPv6],
        };
        assert_eq!(dual.rustfs_listen_address(9001), "[::]:9001");
        assert_eq!(
            dual.service_ip_families().as_deref(),
            Some(["IPv4".to_string(), "IPv6".to_string()].as_slice())
        );
        assert_eq!(
            dual.service_ip_family_policy().as_deref(),
            Some("PreferDualStack")
        );
    }

    #[test]
    fn ipv4_single_stack_keeps_ipv4_listen_address() {
        let ipv4 = NetworkConfig {
            ip_family_policy: Some(IpFamilyPolicy::SingleStack),
            ip_families: vec![IpFamily::IPv4],
        };
        assert!(!ipv4.uses_ipv6());
        assert_eq!(ipv4.rustfs_listen_address(9000), "0.0.0.0:9000");
    }

    #[test]
    fn ipv6_first_dual_stack_preserves_family_order_and_listens_on_v6() {
        let dual = NetworkConfig {
            ip_family_policy: Some(IpFamilyPolicy::RequireDualStack),
            ip_families: vec![IpFamily::IPv6, IpFamily::IPv4],
        };
        assert!(dual.uses_ipv6());
        assert_eq!(dual.rustfs_listen_address(9000), "[::]:9000");
        assert_eq!(
            dual.service_ip_families().as_deref(),
            Some(["IPv6".to_string(), "IPv4".to_string()].as_slice())
        );
    }
}
