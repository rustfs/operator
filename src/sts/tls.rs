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
use std::io::Cursor;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::cluster_dns;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::api::PostParams;
use kube::{Api, Client};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::CertificateDer;
use snafu::{OptionExt, ResultExt, Snafu};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::sync::watch;
use tokio::time::sleep;
use tracing::{info, warn};
use x509_parser::parse_x509_certificate;

const STS_TLS_SECRET_NAME: &str = "sts-tls";
const DEFAULT_STS_SERVICE_NAME: &str = "rustfs-operator-sts";
const DEFAULT_OPERATOR_NAMESPACE: &str = "rustfs-system";
const SERVICE_ACCOUNT_NAMESPACE_PATH: &str =
    "/var/run/secrets/kubernetes.io/serviceaccount/namespace";
const TLS_CERT_KEY: &str = "tls.crt";
const TLS_KEY_KEY: &str = "tls.key";
const CA_CERT_KEY: &str = "ca.crt";
const MANAGED_LABEL: &str = "operator.rustfs.com/managed-sts-tls";
const POLICY_VERSION_ANNOTATION: &str = "operator.rustfs.com/sts-tls-policy-version";
const POLICY_VERSION: &str = "v1";
const KUBERNETES_TLS_SECRET_TYPE: &str = "kubernetes.io/tls";
const SECRET_WAIT_ATTEMPTS: usize = 30;
const SECRET_WAIT_INTERVAL: Duration = Duration::from_secs(2);
const TLS_RELOAD_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CERTIFICATE_VALIDITY: TimeDuration = TimeDuration::days(365);
const CERTIFICATE_RENEWAL_WINDOW: TimeDuration = TimeDuration::days(30);
const CERTIFICATE_CLOCK_SKEW: TimeDuration = TimeDuration::minutes(5);

pub type TlsResult<T> = Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display(
        "operator STS TLS Secret {namespace}/{secret} was not found and OPERATOR_STS_TLS_AUTO=false"
    ))]
    SecretNotFound { namespace: String, secret: String },

    #[snafu(display("timed out waiting for operator STS TLS Secret {namespace}/{secret}"))]
    SecretWaitTimedOut { namespace: String, secret: String },

    #[snafu(display("failed to {action} operator STS TLS Secret {namespace}/{secret}: {source}"))]
    Kube {
        source: Box<kube::Error>,
        action: &'static str,
        namespace: String,
        secret: String,
    },

    #[snafu(display("operator STS TLS Secret {namespace}/{secret} has no data"))]
    SecretNoData { namespace: String, secret: String },

    #[snafu(display("operator STS TLS Secret {namespace}/{secret} is missing non-empty {key}"))]
    SecretMissingKey {
        namespace: String,
        secret: String,
        key: &'static str,
    },

    #[snafu(display("operator STS TLS Secret {namespace}/{secret} is missing ca.crt or tls.crt"))]
    SecretMissingCa { namespace: String, secret: String },

    #[snafu(display("failed to generate operator STS TLS certificate: {source}"))]
    GenerateCertificate { source: rcgen::Error },

    #[snafu(display("failed to parse STS TLS certificate: {source}"))]
    ParseCertificate { source: std::io::Error },

    #[snafu(display("failed to inspect STS TLS {key}: {reason}"))]
    InspectCertificate { key: &'static str, reason: String },

    #[snafu(display("STS TLS certificate bundle is empty"))]
    EmptyCertificateBundle,

    #[snafu(display("failed to parse STS TLS private key: {source}"))]
    ParsePrivateKey { source: std::io::Error },

    #[snafu(display("STS TLS private key is missing"))]
    MissingPrivateKey,

    #[snafu(display("failed to build STS TLS server config: {source}"))]
    BuildServerConfig { source: rustls::Error },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorStsTlsConfig {
    pub enabled: bool,
    pub auto_generate: bool,
    pub namespace: String,
    pub service_name: String,
    pub cluster_domain: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorStsTlsMaterial {
    pub secret_name: String,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub ca_pem: Vec<u8>,
}

impl OperatorStsTlsConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: env_bool("OPERATOR_STS_TLS_ENABLED", true),
            auto_generate: env_bool("OPERATOR_STS_TLS_AUTO", true),
            namespace: operator_namespace(),
            service_name: env_string("OPERATOR_STS_SERVICE_NAME", DEFAULT_STS_SERVICE_NAME),
            cluster_domain: cluster_dns::DEFAULT_CLUSTER_DOMAIN.to_string(),
        }
    }

    pub fn from_env_with_cluster_domain(cluster_domain: &str) -> Self {
        Self {
            cluster_domain: cluster_domain.to_string(),
            ..Self::from_env()
        }
    }
}

pub async fn load_or_create_sts_tls_material(
    client: &Client,
    config: &OperatorStsTlsConfig,
) -> TlsResult<OperatorStsTlsMaterial> {
    let api: Api<corev1::Secret> = Api::namespaced(client.clone(), &config.namespace);

    match api.get(STS_TLS_SECRET_NAME).await {
        Ok(secret) => load_material_from_secret_or_regenerate(&api, config, secret).await,
        Err(kube::Error::Api(error)) if error.code == 404 && config.auto_generate => {
            create_or_get_generated_secret(&api, config).await
        }
        Err(kube::Error::Api(error)) if error.code == 404 => SecretNotFoundSnafu {
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
        }
        .fail(),
        Err(source) => Err(Error::Kube {
            source: Box::new(source),
            action: "load",
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
        }),
    }
}

pub fn build_tls_server_config(
    material: &OperatorStsTlsMaterial,
) -> TlsResult<rustls::ServerConfig> {
    crate::install_rustls_crypto_provider();

    let certs = rustls_pemfile::certs(&mut Cursor::new(&material.cert_pem))
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .context(ParseCertificateSnafu)?;
    if certs.is_empty() {
        return EmptyCertificateBundleSnafu.fail();
    }

    let key = rustls_pemfile::private_key(&mut Cursor::new(&material.key_pem))
        .context(ParsePrivateKeySnafu)?
        .context(MissingPrivateKeySnafu)?;

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context(BuildServerConfigSnafu)
}

pub async fn reload_sts_tls_config(
    client: Client,
    config: OperatorStsTlsConfig,
    mut active_material: OperatorStsTlsMaterial,
    sender: watch::Sender<std::sync::Arc<rustls::ServerConfig>>,
) {
    loop {
        sleep(TLS_RELOAD_INTERVAL).await;
        if sender.is_closed() {
            return;
        }

        match load_or_create_sts_tls_material(&client, &config).await {
            Ok(material) if material == active_material => {}
            Ok(material) => match build_tls_server_config(&material) {
                Ok(server_config) => {
                    if sender.send(std::sync::Arc::new(server_config)).is_err() {
                        return;
                    }
                    active_material = material;
                    info!(
                        secret = STS_TLS_SECRET_NAME,
                        namespace = %config.namespace,
                        "reloaded operator STS TLS certificate"
                    );
                }
                Err(error) => warn!(
                    secret = STS_TLS_SECRET_NAME,
                    namespace = %config.namespace,
                    %error,
                    "keeping last valid operator STS TLS configuration"
                ),
            },
            Err(error) => warn!(
                secret = STS_TLS_SECRET_NAME,
                namespace = %config.namespace,
                %error,
                "failed to refresh operator STS TLS Secret; keeping last valid configuration"
            ),
        }
    }
}

async fn load_material_from_secret_or_regenerate(
    api: &Api<corev1::Secret>,
    config: &OperatorStsTlsConfig,
    secret: corev1::Secret,
) -> TlsResult<OperatorStsTlsMaterial> {
    match validated_material_from_secret(config, &secret) {
        Ok(material) => {
            if should_rotate_managed_secret(config, &secret, &material, OffsetDateTime::now_utc())?
            {
                info!(
                    secret = STS_TLS_SECRET_NAME,
                    namespace = %config.namespace,
                    "rotating managed operator STS TLS certificate"
                );
                replace_generated_secret(api, config, &secret).await
            } else {
                Ok(material)
            }
        }
        Err(error) if config.auto_generate && is_operator_managed(&secret) => {
            warn!(
                secret = STS_TLS_SECRET_NAME,
                %error,
                "regenerating invalid managed operator STS TLS Secret"
            );
            replace_generated_secret(api, config, &secret).await
        }
        Err(error) => Err(error),
    }
}

async fn create_or_get_generated_secret(
    api: &Api<corev1::Secret>,
    config: &OperatorStsTlsConfig,
) -> TlsResult<OperatorStsTlsMaterial> {
    let generated = generated_sts_tls_secret(config)?;
    match api.create(&PostParams::default(), &generated).await {
        Ok(secret) => {
            info!(
                secret = STS_TLS_SECRET_NAME,
                namespace = %config.namespace,
                "created operator STS TLS Secret"
            );
            validated_material_from_secret(config, &secret)
        }
        Err(kube::Error::Api(error)) if error.code == 409 => {
            wait_for_secret_material(api, config).await
        }
        Err(source) => Err(Error::Kube {
            source: Box::new(source),
            action: "create",
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
        }),
    }
}

async fn replace_generated_secret(
    api: &Api<corev1::Secret>,
    config: &OperatorStsTlsConfig,
    existing: &corev1::Secret,
) -> TlsResult<OperatorStsTlsMaterial> {
    let mut generated = generated_sts_tls_secret(config)?;
    generated.metadata.resource_version = existing.metadata.resource_version.clone();
    match api
        .replace(STS_TLS_SECRET_NAME, &PostParams::default(), &generated)
        .await
    {
        Ok(secret) => validated_material_from_secret(config, &secret),
        Err(source) if matches!(&source, kube::Error::Api(error) if error.code == 409) => {
            let latest = api
                .get(STS_TLS_SECRET_NAME)
                .await
                .map_err(|get_error| Error::Kube {
                    source: Box::new(get_error),
                    action: "load after replace conflict",
                    namespace: config.namespace.clone(),
                    secret: STS_TLS_SECRET_NAME.to_string(),
                })?;
            let material = validated_material_from_secret(config, &latest)?;
            if managed_secret_needs_rotation(&latest, &material, OffsetDateTime::now_utc())? {
                return Err(Error::Kube {
                    source: Box::new(source),
                    action: "replace managed",
                    namespace: config.namespace.clone(),
                    secret: STS_TLS_SECRET_NAME.to_string(),
                });
            }
            Ok(material)
        }
        Err(source) => Err(Error::Kube {
            source: Box::new(source),
            action: "replace managed",
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
        }),
    }
}

async fn wait_for_secret_material(
    api: &Api<corev1::Secret>,
    config: &OperatorStsTlsConfig,
) -> TlsResult<OperatorStsTlsMaterial> {
    for _ in 0..SECRET_WAIT_ATTEMPTS {
        match api.get(STS_TLS_SECRET_NAME).await {
            Ok(secret) => return validated_material_from_secret(config, &secret),
            Err(kube::Error::Api(error)) if error.code == 404 => {
                sleep(SECRET_WAIT_INTERVAL).await;
            }
            Err(source) => {
                return Err(Error::Kube {
                    source: Box::new(source),
                    action: "wait for",
                    namespace: config.namespace.clone(),
                    secret: STS_TLS_SECRET_NAME.to_string(),
                });
            }
        }
    }

    SecretWaitTimedOutSnafu {
        namespace: config.namespace.clone(),
        secret: STS_TLS_SECRET_NAME.to_string(),
    }
    .fail()
}

fn generated_sts_tls_secret(config: &OperatorStsTlsConfig) -> TlsResult<corev1::Secret> {
    let generated = generate_sts_tls_material(
        &config.namespace,
        &config.service_name,
        &config.cluster_domain,
    )?;
    let mut data = BTreeMap::new();
    data.insert(TLS_CERT_KEY.to_string(), ByteString(generated.cert_pem));
    data.insert(TLS_KEY_KEY.to_string(), ByteString(generated.key_pem));
    data.insert(CA_CERT_KEY.to_string(), ByteString(generated.ca_pem));

    let mut labels = BTreeMap::new();
    labels.insert(MANAGED_LABEL.to_string(), "true".to_string());
    labels.insert(
        "app.kubernetes.io/name".to_string(),
        "rustfs-operator".to_string(),
    );
    labels.insert(
        "app.kubernetes.io/component".to_string(),
        "operator".to_string(),
    );
    let annotations = BTreeMap::from([(
        POLICY_VERSION_ANNOTATION.to_string(),
        POLICY_VERSION.to_string(),
    )]);

    Ok(corev1::Secret {
        metadata: metav1::ObjectMeta {
            name: Some(STS_TLS_SECRET_NAME.to_string()),
            namespace: Some(config.namespace.clone()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        type_: Some(KUBERNETES_TLS_SECRET_TYPE.to_string()),
        data: Some(data),
        ..Default::default()
    })
}

fn generate_sts_tls_material(
    namespace: &str,
    service_name: &str,
    cluster_domain: &str,
) -> TlsResult<OperatorStsTlsMaterial> {
    generate_sts_tls_material_at(
        namespace,
        service_name,
        cluster_domain,
        OffsetDateTime::now_utc(),
    )
}

fn generate_sts_tls_material_at(
    namespace: &str,
    service_name: &str,
    cluster_domain: &str,
    now: OffsetDateTime,
) -> TlsResult<OperatorStsTlsMaterial> {
    let ca_key = KeyPair::generate().context(GenerateCertificateSnafu)?;
    let mut ca_params = CertificateParams::default();
    ca_params.not_before = now - CERTIFICATE_CLOCK_SKEW;
    ca_params.not_after = now + CERTIFICATE_VALIDITY;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .context(GenerateCertificateSnafu)?;

    let server_key = KeyPair::generate().context(GenerateCertificateSnafu)?;
    let mut server_names = service_dns_names(namespace, service_name, cluster_domain);
    server_names.push("localhost".to_string());
    server_names.push(Ipv4Addr::LOCALHOST.to_string());
    let mut server_params =
        CertificateParams::new(server_names).context(GenerateCertificateSnafu)?;
    server_params.not_before = now - CERTIFICATE_CLOCK_SKEW;
    server_params.not_after = now + CERTIFICATE_VALIDITY;
    server_params.is_ca = IsCa::NoCa;
    server_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .context(GenerateCertificateSnafu)?;

    Ok(OperatorStsTlsMaterial {
        secret_name: STS_TLS_SECRET_NAME.to_string(),
        cert_pem: server_cert.pem().into_bytes(),
        key_pem: server_key.serialize_pem().into_bytes(),
        ca_pem: ca_cert.pem().into_bytes(),
    })
}

fn material_from_secret(
    config: &OperatorStsTlsConfig,
    secret: &corev1::Secret,
) -> TlsResult<OperatorStsTlsMaterial> {
    let data = secret.data.as_ref().context(SecretNoDataSnafu {
        namespace: config.namespace.clone(),
        secret: STS_TLS_SECRET_NAME.to_string(),
    })?;

    let cert_pem = secret_data(data, TLS_CERT_KEY, config)?;
    let key_pem = secret_data(data, TLS_KEY_KEY, config)?;
    let ca_pem = data
        .get(CA_CERT_KEY)
        .or_else(|| data.get(TLS_CERT_KEY))
        .map(|bytes| bytes.0.clone())
        .context(SecretMissingCaSnafu {
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
        })?;

    Ok(OperatorStsTlsMaterial {
        secret_name: STS_TLS_SECRET_NAME.to_string(),
        cert_pem,
        key_pem,
        ca_pem,
    })
}

fn validated_material_from_secret(
    config: &OperatorStsTlsConfig,
    secret: &corev1::Secret,
) -> TlsResult<OperatorStsTlsMaterial> {
    let material = material_from_secret(config, secret)?;
    build_tls_server_config(&material)?;
    validate_material_at(&material, OffsetDateTime::now_utc())?;
    record_expiry_metrics(&material)?;
    Ok(material)
}

fn validate_material_at(material: &OperatorStsTlsMaterial, now: OffsetDateTime) -> TlsResult<()> {
    let now = now.unix_timestamp();
    for (pem, key) in [
        (material.cert_pem.as_slice(), TLS_CERT_KEY),
        (material.ca_pem.as_slice(), CA_CERT_KEY),
    ] {
        let (not_before, not_after) = certificate_validity_timestamps(pem, key)?;
        if now < not_before || now > not_after {
            return Err(Error::InspectCertificate {
                key,
                reason: format!(
                    "certificate is not valid at timestamp {now} (valid from {not_before} to {not_after})"
                ),
            });
        }
    }
    Ok(())
}

fn managed_secret_needs_rotation(
    secret: &corev1::Secret,
    material: &OperatorStsTlsMaterial,
    now: OffsetDateTime,
) -> TlsResult<bool> {
    let policy_is_current = secret
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(POLICY_VERSION_ANNOTATION))
        .is_some_and(|version| version == POLICY_VERSION);
    if !policy_is_current {
        return Ok(true);
    }

    let (certificate_expiry, ca_expiry) = material_expiry_timestamps(material)?;
    let renewal_deadline = (now + CERTIFICATE_RENEWAL_WINDOW).unix_timestamp();
    Ok(certificate_expiry <= renewal_deadline || ca_expiry <= renewal_deadline)
}

fn should_rotate_managed_secret(
    config: &OperatorStsTlsConfig,
    secret: &corev1::Secret,
    material: &OperatorStsTlsMaterial,
    now: OffsetDateTime,
) -> TlsResult<bool> {
    if !config.auto_generate || !is_operator_managed(secret) {
        return Ok(false);
    }
    managed_secret_needs_rotation(secret, material, now)
}

fn record_expiry_metrics(material: &OperatorStsTlsMaterial) -> TlsResult<()> {
    let (certificate_expiry, ca_expiry) = material_expiry_timestamps(material)?;
    crate::metrics::set_sts_tls_expiry_timestamps(certificate_expiry, ca_expiry);
    Ok(())
}

fn material_expiry_timestamps(material: &OperatorStsTlsMaterial) -> TlsResult<(i64, i64)> {
    Ok((
        certificate_expiry_timestamp(&material.cert_pem, TLS_CERT_KEY)?,
        certificate_expiry_timestamp(&material.ca_pem, CA_CERT_KEY)?,
    ))
}

fn certificate_expiry_timestamp(pem: &[u8], key: &'static str) -> TlsResult<i64> {
    certificate_validity_timestamps(pem, key).map(|(_, not_after)| not_after)
}

fn certificate_validity_timestamps(pem: &[u8], key: &'static str) -> TlsResult<(i64, i64)> {
    let certificate = rustls_pemfile::certs(&mut Cursor::new(pem))
        .next()
        .transpose()
        .context(ParseCertificateSnafu)?
        .context(InspectCertificateSnafu {
            key,
            reason: "certificate bundle is empty".to_string(),
        })?;
    let (_, certificate) = parse_x509_certificate(certificate.as_ref()).map_err(|source| {
        Error::InspectCertificate {
            key,
            reason: source.to_string(),
        }
    })?;
    Ok((
        certificate.validity().not_before.timestamp(),
        certificate.validity().not_after.timestamp(),
    ))
}

fn secret_data(
    data: &BTreeMap<String, ByteString>,
    key: &'static str,
    config: &OperatorStsTlsConfig,
) -> TlsResult<Vec<u8>> {
    data.get(key)
        .map(|bytes| bytes.0.clone())
        .filter(|bytes| !bytes.is_empty())
        .context(SecretMissingKeySnafu {
            namespace: config.namespace.clone(),
            secret: STS_TLS_SECRET_NAME.to_string(),
            key,
        })
}

fn service_dns_names(namespace: &str, service_name: &str, cluster_domain: &str) -> Vec<String> {
    vec![
        service_name.to_string(),
        format!("{service_name}.{namespace}"),
        format!("{service_name}.{namespace}.svc"),
        cluster_dns::service_fqdn(service_name, namespace, cluster_domain),
    ]
}

fn is_operator_managed(secret: &corev1::Secret) -> bool {
    secret
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(MANAGED_LABEL))
        .is_some_and(|value| value == "true")
}

fn operator_namespace() -> String {
    if let Some(value) = std::env::var("OPERATOR_NAMESPACE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return value;
    }

    std::fs::read_to_string(SERVICE_ACCOUNT_NAMESPACE_PATH)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OPERATOR_NAMESPACE.to_string())
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_string(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_dns_names_cover_short_and_cluster_forms() {
        assert_eq!(
            service_dns_names(
                "rustfs-system",
                "rustfs-operator-sts",
                cluster_dns::DEFAULT_CLUSTER_DOMAIN
            ),
            vec![
                "rustfs-operator-sts",
                "rustfs-operator-sts.rustfs-system",
                "rustfs-operator-sts.rustfs-system.svc",
                "rustfs-operator-sts.rustfs-system.svc.cluster.local"
            ]
        );
    }

    #[test]
    fn service_dns_names_use_custom_cluster_domain() {
        assert_eq!(
            service_dns_names("rustfs-system", "rustfs-operator-sts", "k8s.mse.cloud")[3],
            "rustfs-operator-sts.rustfs-system.svc.k8s.mse.cloud"
        );
    }

    #[test]
    fn generated_material_builds_rustls_server_config() {
        let material = generate_sts_tls_material(
            "rustfs-system",
            "rustfs-operator-sts",
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
        )
        .unwrap();

        assert!(!material.cert_pem.is_empty());
        assert!(!material.key_pem.is_empty());
        assert!(!material.ca_pem.is_empty());
        build_tls_server_config(&material).unwrap();
    }

    #[test]
    fn generated_certificates_are_valid_for_one_year() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let material = generate_sts_tls_material_at(
            "rustfs-system",
            "rustfs-operator-sts",
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
            now,
        )
        .unwrap();

        let (certificate_expiry, ca_expiry) = material_expiry_timestamps(&material).unwrap();
        let expected_expiry = (now + CERTIFICATE_VALIDITY).unix_timestamp();
        assert_eq!(certificate_expiry, expected_expiry);
        assert_eq!(ca_expiry, expected_expiry);
    }

    #[test]
    fn expired_certificate_material_is_rejected() {
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let material = generate_sts_tls_material_at(
            "rustfs-system",
            "rustfs-operator-sts",
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
            now - TimeDuration::days(366),
        )
        .unwrap();

        assert!(validate_material_at(&material, now).is_err());
    }

    #[test]
    fn managed_certificate_rotation_migrates_legacy_policy_and_renews_early() {
        let config = test_config();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let current_material = generate_sts_tls_material_at(
            &config.namespace,
            &config.service_name,
            &config.cluster_domain,
            now,
        )
        .unwrap();
        let mut secret = generated_sts_tls_secret(&config).unwrap();
        set_secret_material(&mut secret, &current_material);

        assert!(!managed_secret_needs_rotation(&secret, &current_material, now).unwrap());

        secret.metadata.annotations = None;
        assert!(managed_secret_needs_rotation(&secret, &current_material, now).unwrap());
        let mut external_config = config.clone();
        external_config.auto_generate = false;
        assert!(
            !should_rotate_managed_secret(&external_config, &secret, &current_material, now)
                .unwrap()
        );

        secret.metadata.annotations = Some(BTreeMap::from([(
            POLICY_VERSION_ANNOTATION.to_string(),
            POLICY_VERSION.to_string(),
        )]));
        let expiring_material = generate_sts_tls_material_at(
            &config.namespace,
            &config.service_name,
            &config.cluster_domain,
            now - TimeDuration::days(336),
        )
        .unwrap();
        set_secret_material(&mut secret, &expiring_material);
        assert!(managed_secret_needs_rotation(&secret, &expiring_material, now).unwrap());
    }

    #[test]
    fn secret_material_uses_leaf_as_ca_fallback() {
        let config = test_config();
        let generated = generate_sts_tls_material(
            &config.namespace,
            &config.service_name,
            &config.cluster_domain,
        )
        .unwrap();
        let mut data = BTreeMap::new();
        data.insert(TLS_CERT_KEY.to_string(), ByteString(generated.cert_pem));
        data.insert(TLS_KEY_KEY.to_string(), ByteString(generated.key_pem));
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some(STS_TLS_SECRET_NAME.to_string()),
                namespace: Some(config.namespace.clone()),
                ..Default::default()
            },
            data: Some(data),
            ..Default::default()
        };

        let material = material_from_secret(&config, &secret).unwrap();
        assert_eq!(material.ca_pem, material.cert_pem);
    }

    fn test_config() -> OperatorStsTlsConfig {
        OperatorStsTlsConfig {
            enabled: true,
            auto_generate: true,
            namespace: "rustfs-system".to_string(),
            service_name: "rustfs-operator-sts".to_string(),
            cluster_domain: cluster_dns::DEFAULT_CLUSTER_DOMAIN.to_string(),
        }
    }

    fn set_secret_material(secret: &mut corev1::Secret, material: &OperatorStsTlsMaterial) {
        secret.data = Some(BTreeMap::from([
            (
                TLS_CERT_KEY.to_string(),
                ByteString(material.cert_pem.clone()),
            ),
            (
                TLS_KEY_KEY.to_string(),
                ByteString(material.key_pem.clone()),
            ),
            (CA_CERT_KEY.to_string(), ByteString(material.ca_pem.clone())),
        ]));
    }
}
