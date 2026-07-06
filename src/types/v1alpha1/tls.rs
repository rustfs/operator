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

#[derive(Deserialize, Serialize, Clone, Debug, Default, KubeSchema, PartialEq)]
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_generated_dns_names: Option<bool>,

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

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TlsCertificateConfig {
    pub name: String,

    #[serde(default)]
    pub default: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,

    pub cert_manager: CertManagerTlsConfig,
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

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub certificates: Vec<TlsCertificateConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_trust: Option<CaTrustConfig>,
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
            certificates: Vec::new(),
            ca_trust: None,
        }
    }
}

impl TlsConfig {
    pub fn is_enabled(&self) -> bool {
        self.mode != TlsMode::Disabled
    }

    pub fn ca_trust(&self) -> CaTrustConfig {
        if let Some(ca_trust) = self.ca_trust.clone() {
            return ca_trust;
        }

        if self.certificates.is_empty() {
            return self
                .cert_manager
                .as_ref()
                .and_then(|cert_manager| cert_manager.ca_trust.clone())
                .unwrap_or_default();
        }

        self.certificates
            .iter()
            .find(|certificate| certificate.default)
            .and_then(|certificate| certificate.cert_manager.ca_trust.clone())
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
        assert!(config.certificates.is_empty());
        assert!(config.ca_trust.is_none());
    }

    #[test]
    fn multi_certificate_ca_trust_ignores_legacy_cert_manager() {
        let config = TlsConfig {
            cert_manager: Some(CertManagerTlsConfig {
                ca_trust: Some(CaTrustConfig {
                    source: CaTrustSource::SystemCa,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            certificates: vec![TlsCertificateConfig {
                name: "internal".to_string(),
                default: true,
                hosts: Vec::new(),
                cert_manager: CertManagerTlsConfig {
                    ca_trust: Some(CaTrustConfig {
                        source: CaTrustSource::SecretRef,
                        ca_secret_ref: Some(SecretKeyReference {
                            name: "internal-ca".to_string(),
                            key: "bundle.pem".to_string(),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let ca_trust = config.ca_trust();

        assert_eq!(ca_trust.source, CaTrustSource::SecretRef);
        assert_eq!(
            ca_trust
                .ca_secret_ref
                .as_ref()
                .map(|secret| { (secret.name.as_str(), secret.key.as_str()) }),
            Some(("internal-ca", "bundle.pem"))
        );
    }

    #[test]
    fn tls_plan_projects_multiple_certificates_into_rustfs_sni_layout() {
        let plan = TlsPlan::rollout_certificates(
            DEFAULT_TLS_MOUNT_PATH.to_string(),
            "sha256:multi".to_string(),
            vec![
                TlsServerCertificateMount {
                    secret_name: "internal-tls".to_string(),
                    domains: vec![None, Some("rustfs.internal.example.local".to_string())],
                    ca_key: Some("ca.crt".to_string()),
                },
                TlsServerCertificateMount {
                    secret_name: "public-tls".to_string(),
                    domains: vec![Some("s3.example.com".to_string())],
                    ca_key: None,
                },
            ],
            None,
            None,
            true,
            false,
            false,
            None,
        );

        let volume = plan
            .volumes
            .iter()
            .find(|volume| volume.name == "rustfs-tls-server")
            .expect("TLS volume should exist");
        let paths = volume
            .projected
            .as_ref()
            .and_then(|projected| projected.sources.as_ref())
            .expect("TLS volume should use projected sources")
            .iter()
            .flat_map(|source| {
                source
                    .secret
                    .as_ref()
                    .and_then(|secret| secret.items.as_ref())
                    .into_iter()
                    .flatten()
            })
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"rustfs_cert.pem"));
        assert!(paths.contains(&"rustfs_key.pem"));
        assert!(paths.contains(&"ca.crt"));
        assert!(paths.contains(&"rustfs.internal.example.local/rustfs_cert.pem"));
        assert!(paths.contains(&"rustfs.internal.example.local/rustfs_key.pem"));
        assert!(paths.contains(&"s3.example.com/rustfs_cert.pem"));
        assert!(paths.contains(&"s3.example.com/rustfs_key.pem"));
        assert_eq!(
            plan.volume_mounts
                .iter()
                .find(|mount| mount.name == "rustfs-tls-server")
                .map(|mount| (mount.mount_path.as_str(), mount.sub_path.as_deref())),
            Some((DEFAULT_TLS_MOUNT_PATH, None))
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct TlsServerCertificateMount {
    pub secret_name: String,
    pub domains: Vec<Option<String>>,
    pub ca_key: Option<String>,
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
        Self::rollout_certificates(
            mount_path,
            hash,
            vec![TlsServerCertificateMount {
                secret_name: server_secret_name,
                domains: vec![None],
                ca_key: server_ca_key,
            }],
            explicit_ca,
            client_ca,
            enable_internode_https,
            trust_system_ca,
            trust_leaf_certificate_as_ca,
            status,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rollout_certificates(
        mount_path: String,
        hash: String,
        server_certificates: Vec<TlsServerCertificateMount>,
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

        let mut sources = server_certificates
            .iter()
            .map(|certificate| {
                secret_projection(
                    &certificate.secret_name,
                    server_certificate_items(certificate),
                )
            })
            .collect::<Vec<_>>();

        if let Some(explicit_ca) = &explicit_ca {
            sources.push(secret_projection(
                &explicit_ca.name,
                vec![key_to_path(&explicit_ca.key, RUSTFS_CA_FILE)],
            ));
        }
        if let Some(client_ca) = &client_ca {
            sources.push(secret_projection(
                &client_ca.name,
                vec![key_to_path(&client_ca.key, RUSTFS_CLIENT_CA_FILE)],
            ));
        }

        Self {
            enabled: true,
            mount_path: mount_path.clone(),
            internode_scheme: if enable_internode_https {
                "https"
            } else {
                "http"
            },
            probe_scheme: "HTTPS",
            pod_template_annotations: annotations,
            env,
            volumes: vec![projected_volume("rustfs-tls-server", sources)],
            volume_mounts: vec![directory_mount("rustfs-tls-server", &mount_path)],
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

fn server_certificate_items(certificate: &TlsServerCertificateMount) -> Vec<corev1::KeyToPath> {
    let mut items = Vec::new();
    for domain in &certificate.domains {
        items.push(key_to_path("tls.crt", &server_cert_path(domain.as_deref())));
        items.push(key_to_path("tls.key", &server_key_path(domain.as_deref())));
    }
    if let Some(ca_key) = certificate.ca_key.as_deref() {
        items.push(key_to_path(ca_key, RUSTFS_CA_FILE));
    }
    items
}

fn server_cert_path(domain: Option<&str>) -> String {
    rustfs_tls_path(domain, RUSTFS_TLS_CERT_FILE)
}

fn server_key_path(domain: Option<&str>) -> String {
    rustfs_tls_path(domain, RUSTFS_TLS_KEY_FILE)
}

fn rustfs_tls_path(domain: Option<&str>, file: &str) -> String {
    domain
        .filter(|domain| !domain.is_empty())
        .map(|domain| format!("{domain}/{file}"))
        .unwrap_or_else(|| file.to_string())
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
