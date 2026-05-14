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

use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use k8s_openapi::schemars::JsonSchema;
use kube::KubeSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_TLS_MOUNT_PATH: &str = "/var/run/rustfs/tls";
pub const TLS_HASH_ANNOTATION: &str = "operator.rustfs.com/tls-hash";
pub const RUSTFS_TLS_CERT_FILE: &str = "rustfs_cert.pem";
pub const RUSTFS_TLS_KEY_FILE: &str = "rustfs_key.pem";
pub const RUSTFS_CA_FILE: &str = "ca.crt";
pub const RUSTFS_CLIENT_CA_FILE: &str = "client_ca.crt";

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub enum TlsMode {
    #[default]
    Disabled,
    External,
    CertManager,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum TlsRotationStrategy {
    #[default]
    Rollout,
    HotReload,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum CaTrustSource {
    #[default]
    CertificateSecretCa,
    SecretRef,
    SystemCa,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeyReference {
    pub name: String,

    #[serde(default = "default_ca_key")]
    pub key: String,
}

fn default_ca_key() -> String {
    "ca.crt".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaTrustConfig {
    #[serde(default)]
    pub source: CaTrustSource,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_secret_ref: Option<SecretKeyReference>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ca_secret_ref: Option<SecretKeyReference>,

    #[serde(default)]
    pub trust_system_ca: bool,

    #[serde(default)]
    pub trust_leaf_certificate_as_ca: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertManagerIssuerRef {
    #[serde(default = "default_cert_manager_group")]
    pub group: String,

    #[serde(default = "default_cert_manager_issuer_kind")]
    pub kind: String,

    pub name: String,
}

fn default_cert_manager_group() -> String {
    "cert-manager.io".to_string()
}

fn default_cert_manager_issuer_kind() -> String {
    "Issuer".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertManagerPrivateKeyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_policy: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertManagerTlsConfig {
    #[serde(default)]
    pub manage_certificate: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_ref: Option<CertManagerIssuerRef>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_name: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_names: Vec<String>,

    #[serde(default = "default_include_generated_dns_names")]
    pub include_generated_dns_names: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renew_before: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<CertManagerPrivateKeyConfig>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usages: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_trust: Option<CaTrustConfig>,
}

fn default_include_generated_dns_names() -> bool {
    true
}

impl Default for CertManagerTlsConfig {
    fn default() -> Self {
        Self {
            manage_certificate: false,
            secret_name: None,
            secret_type: None,
            certificate_name: None,
            issuer_ref: None,
            common_name: None,
            dns_names: Vec::new(),
            include_generated_dns_names: default_include_generated_dns_names(),
            duration: None,
            renew_before: None,
            private_key: None,
            usages: Vec::new(),
            ca_trust: None,
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TlsConfig {
    #[serde(default)]
    pub mode: TlsMode,

    #[serde(default = "default_tls_mount_path")]
    pub mount_path: String,

    #[serde(default)]
    pub rotation_strategy: TlsRotationStrategy,

    #[serde(default)]
    pub enable_internode_https: bool,

    #[serde(default = "default_require_san_match")]
    pub require_san_match: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_manager: Option<CertManagerTlsConfig>,
}

fn default_tls_mount_path() -> String {
    DEFAULT_TLS_MOUNT_PATH.to_string()
}

fn default_require_san_match() -> bool {
    true
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            mode: TlsMode::default(),
            mount_path: default_tls_mount_path(),
            rotation_strategy: TlsRotationStrategy::default(),
            enable_internode_https: false,
            require_san_match: default_require_san_match(),
            cert_manager: None,
        }
    }
}

impl TlsConfig {
    pub fn is_enabled(&self) -> bool {
        self.mode != TlsMode::Disabled
    }

    pub fn ca_trust(&self) -> CaTrustConfig {
        self.cert_manager
            .as_ref()
            .and_then(|cert_manager| cert_manager.ca_trust.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_default_matches_serde_defaults() {
        let config = TlsConfig::default();

        assert_eq!(config.mode, TlsMode::Disabled);
        assert_eq!(config.mount_path, DEFAULT_TLS_MOUNT_PATH);
        assert_eq!(config.rotation_strategy, TlsRotationStrategy::Rollout);
        assert!(!config.enable_internode_https);
        assert!(config.require_san_match);
        assert!(config.cert_manager.is_none());
    }
}

#[derive(Clone, Debug, Default)]
pub struct TlsPlan {
    pub enabled: bool,
    pub mount_path: String,
    pub internode_scheme: &'static str,
    pub probe_scheme: &'static str,
    pub pod_template_annotations: BTreeMap<String, String>,
    pub env: Vec<corev1::EnvVar>,
    pub volumes: Vec<corev1::Volume>,
    pub volume_mounts: Vec<corev1::VolumeMount>,
    pub status: Option<crate::types::v1alpha1::status::certificate::TlsCertificateStatus>,
}

impl TlsPlan {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            mount_path: DEFAULT_TLS_MOUNT_PATH.to_string(),
            internode_scheme: "http",
            probe_scheme: "HTTP",
            pod_template_annotations: BTreeMap::new(),
            env: Vec::new(),
            volumes: Vec::new(),
            volume_mounts: Vec::new(),
            status: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rollout(
        mount_path: String,
        hash: String,
        server_secret_name: String,
        server_ca_key: Option<String>,
        explicit_ca: Option<SecretKeyReference>,
        client_ca: Option<SecretKeyReference>,
        enable_internode_https: bool,
        trust_system_ca: bool,
        trust_leaf_certificate_as_ca: bool,
        status: Option<crate::types::v1alpha1::status::certificate::TlsCertificateStatus>,
    ) -> Self {
        let mut annotations = BTreeMap::new();
        annotations.insert(TLS_HASH_ANNOTATION.to_string(), hash);

        let mut env = vec![corev1::EnvVar {
            name: "RUSTFS_TLS_PATH".to_string(),
            value: Some(mount_path.clone()),
            ..Default::default()
        }];
        if trust_system_ca {
            env.push(corev1::EnvVar {
                name: "RUSTFS_TRUST_SYSTEM_CA".to_string(),
                value: Some("true".to_string()),
                ..Default::default()
            });
        }
        if trust_leaf_certificate_as_ca {
            env.push(corev1::EnvVar {
                name: "RUSTFS_TRUST_LEAF_CERT_AS_CA".to_string(),
                value: Some("true".to_string()),
                ..Default::default()
            });
        }
        if client_ca.is_some() {
            env.push(corev1::EnvVar {
                name: "RUSTFS_SERVER_MTLS_ENABLE".to_string(),
                value: Some("true".to_string()),
                ..Default::default()
            });
        }

        let has_server_ca = server_ca_key.is_some();
        let mut server_items = vec![
            key_to_path("tls.crt", RUSTFS_TLS_CERT_FILE),
            key_to_path("tls.key", RUSTFS_TLS_KEY_FILE),
        ];
        if let Some(ca_key) = server_ca_key.as_deref() {
            server_items.push(key_to_path(ca_key, RUSTFS_CA_FILE));
        }

        let (mut volumes, mut volume_mounts) = if let Some(explicit_ca) = &explicit_ca {
            (
                vec![projected_volume(
                    "rustfs-tls-server",
                    vec![
                        secret_projection(&server_secret_name, server_items),
                        secret_projection(
                            &explicit_ca.name,
                            vec![key_to_path(&explicit_ca.key, RUSTFS_CA_FILE)],
                        ),
                    ],
                )],
                vec![directory_mount("rustfs-tls-server", &mount_path)],
            )
        } else {
            let mut volume_mounts = vec![
                file_mount("rustfs-tls-server", &mount_path, RUSTFS_TLS_CERT_FILE),
                file_mount("rustfs-tls-server", &mount_path, RUSTFS_TLS_KEY_FILE),
            ];
            if has_server_ca {
                volume_mounts.push(file_mount("rustfs-tls-server", &mount_path, RUSTFS_CA_FILE));
            }
            (
                vec![secret_volume(
                    "rustfs-tls-server",
                    &server_secret_name,
                    server_items,
                )],
                volume_mounts,
            )
        };

        if let Some(client_ca) = &client_ca {
            volumes.push(secret_volume(
                "rustfs-tls-client-ca",
                &client_ca.name,
                vec![key_to_path(&client_ca.key, RUSTFS_CLIENT_CA_FILE)],
            ));
            volume_mounts.push(file_mount(
                "rustfs-tls-client-ca",
                &mount_path,
                RUSTFS_CLIENT_CA_FILE,
            ));
        }

        Self {
            enabled: true,
            mount_path,
            internode_scheme: if enable_internode_https {
                "https"
            } else {
                "http"
            },
            probe_scheme: "HTTPS",
            pod_template_annotations: annotations,
            env,
            volumes,
            volume_mounts,
            status,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(server_secret_name: &str, hash: &str) -> Self {
        Self::rollout(
            DEFAULT_TLS_MOUNT_PATH.to_string(),
            hash.to_string(),
            server_secret_name.to_string(),
            Some("ca.crt".to_string()),
            None,
            None,
            true,
            false,
            false,
            None,
        )
    }
}

fn secret_volume(name: &str, secret_name: &str, items: Vec<corev1::KeyToPath>) -> corev1::Volume {
    corev1::Volume {
        name: name.to_string(),
        secret: Some(corev1::SecretVolumeSource {
            secret_name: Some(secret_name.to_string()),
            items: Some(items),
            optional: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn projected_volume(name: &str, sources: Vec<corev1::VolumeProjection>) -> corev1::Volume {
    corev1::Volume {
        name: name.to_string(),
        projected: Some(corev1::ProjectedVolumeSource {
            sources: Some(sources),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn secret_projection(secret_name: &str, items: Vec<corev1::KeyToPath>) -> corev1::VolumeProjection {
    corev1::VolumeProjection {
        secret: Some(corev1::SecretProjection {
            name: secret_name.to_string(),
            items: Some(items),
            optional: Some(false),
        }),
        ..Default::default()
    }
}

fn key_to_path(key: &str, path: &str) -> corev1::KeyToPath {
    corev1::KeyToPath {
        key: key.to_string(),
        path: path.to_string(),
        ..Default::default()
    }
}

fn file_mount(volume_name: &str, mount_path: &str, file_name: &str) -> corev1::VolumeMount {
    corev1::VolumeMount {
        name: volume_name.to_string(),
        mount_path: format!("{}/{}", mount_path.trim_end_matches('/'), file_name),
        sub_path: Some(file_name.to_string()),
        read_only: Some(true),
        ..Default::default()
    }
}

fn directory_mount(volume_name: &str, mount_path: &str) -> corev1::VolumeMount {
    corev1::VolumeMount {
        name: volume_name.to_string(),
        mount_path: mount_path.to_string(),
        read_only: Some(true),
        ..Default::default()
    }
}

pub fn http_probe(path: &str, scheme: &'static str) -> corev1::Probe {
    corev1::Probe {
        http_get: Some(corev1::HTTPGetAction {
            path: Some(path.to_string()),
            port: IntOrString::Int(9000),
            scheme: Some(scheme.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
