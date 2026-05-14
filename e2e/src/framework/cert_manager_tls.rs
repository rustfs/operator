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

use anyhow::{Context, Result};
use k8s_openapi::ByteString;
use k8s_openapi::api::{apps::v1::StatefulSet, core::v1 as corev1};
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::{Api, Client};
use operator::types::v1alpha1::tenant::Tenant;
use operator::types::v1alpha1::tls::{
    CaTrustConfig, CaTrustSource, CertManagerIssuerRef, CertManagerTlsConfig, SecretKeyReference,
    TlsConfig, TlsMode, TlsPlan, TlsRotationStrategy,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

use crate::framework::{
    assertions, command::CommandSpec, config::E2eConfig, kubectl::Kubectl, resources, storage,
    tenant_factory::TenantTemplate, wait,
};

const CERT_MANAGER_GROUP: &str = "cert-manager.io";
const CERT_MANAGER_VERSION: &str = "v1";
const CERT_MANAGER_CERTIFICATE_KIND: &str = "Certificate";
const CERT_MANAGER_CERTIFICATE_PLURAL: &str = "certificates";
const SELF_SIGNED_ISSUER_NAME: &str = "rustfs-e2e-selfsigned";
const PENDING_ISSUER_NAME: &str = "rustfs-e2e-pending-issuer";
const MISSING_ISSUER_NAME: &str = "rustfs-e2e-missing-issuer";
const KUBERNETES_TLS_SECRET_TYPE: &str = "kubernetes.io/tls";
const OPAQUE_SECRET_TYPE: &str = "Opaque";
const REDACTED_FIXTURE_BYTES: &[u8] = b"redacted-test-fixture";
const MANAGED_CERTIFICATE_CASE_SUFFIX: &str = "cert-manager-managed";
const EXTERNAL_SECRET_CASE_SUFFIX: &str = "cert-manager-external";
const RUSTFS_TENANT_LABEL: &str = "rustfs.tenant";
pub const POSITIVE_CERT_MANAGER_TLS_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativeTlsCase {
    MissingTlsCrt,
    MissingTlsKey,
    MissingCaForInternodeHttps,
    MissingIssuer,
    PendingCertificate,
    HotReloadUnsupported,
}

impl NegativeTlsCase {
    pub fn case_name(self) -> &'static str {
        match self {
            Self::MissingTlsCrt => "cert_manager_rejects_secret_missing_tls_crt",
            Self::MissingTlsKey => "cert_manager_rejects_secret_missing_tls_key",
            Self::MissingCaForInternodeHttps => {
                "cert_manager_rejects_secret_missing_ca_for_internode_https"
            }
            Self::MissingIssuer => "cert_manager_rejects_missing_issuer_for_managed_certificate",
            Self::PendingCertificate => "cert_manager_reports_pending_certificate_not_ready",
            Self::HotReloadUnsupported => "cert_manager_rejects_hot_reload",
        }
    }
}

pub fn managed_certificate_case_config(config: &E2eConfig) -> E2eConfig {
    positive_tls_case_config(config, MANAGED_CERTIFICATE_CASE_SUFFIX)
}

pub fn external_secret_case_config(config: &E2eConfig) -> E2eConfig {
    positive_tls_case_config(config, EXTERNAL_SECRET_CASE_SUFFIX)
}

pub fn positive_cert_manager_tls_timeout(config: &E2eConfig) -> Duration {
    std::cmp::max(config.timeout, POSITIVE_CERT_MANAGER_TLS_TIMEOUT)
}

fn tenant_watch_labels(config: &E2eConfig) -> BTreeMap<String, String> {
    BTreeMap::from([(RUSTFS_TENANT_LABEL.to_string(), config.tenant_name.clone())])
}

pub fn managed_certificate_storage_layout(config: &E2eConfig) -> storage::LocalStorageLayout {
    positive_tls_storage_layout(config, MANAGED_CERTIFICATE_CASE_SUFFIX)
}

pub fn external_secret_storage_layout(config: &E2eConfig) -> storage::LocalStorageLayout {
    positive_tls_storage_layout(config, EXTERNAL_SECRET_CASE_SUFFIX)
}

pub fn managed_secret_name(config: &E2eConfig) -> String {
    format!("{}-managed-tls", config.tenant_name)
}

pub fn managed_certificate_name(config: &E2eConfig) -> String {
    format!("{}-managed-cert", config.tenant_name)
}

pub fn external_secret_name(config: &E2eConfig) -> String {
    format!("{}-external-tls", config.tenant_name)
}

pub fn external_ca_secret_name(config: &E2eConfig) -> String {
    format!("{}-external-ca", config.tenant_name)
}

pub fn external_tls_subject_alt_name(config: &E2eConfig) -> String {
    subject_alt_name(&external_tls_certificate_dns_names(config))
}

pub fn external_tls_rotation_subject_alt_name(config: &E2eConfig) -> String {
    external_tls_subject_alt_name(config)
}

pub fn external_tls_certificate_dns_names(config: &E2eConfig) -> Vec<String> {
    tls_certificate_dns_names(config, &external_secret_tenant(config))
}

fn tls_certificate_dns_names(config: &E2eConfig, tenant: &Tenant) -> Vec<String> {
    let mut names = BTreeSet::from([config.tenant_name.clone(), "localhost".to_string()]);

    if let Some(cert_manager) = tenant
        .spec
        .tls
        .as_ref()
        .and_then(|tls| tls.cert_manager.as_ref())
    {
        if let Some(common_name) = cert_manager
            .common_name
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            names.insert(common_name.to_string());
        }
        names.extend(
            cert_manager
                .dns_names
                .iter()
                .filter(|name| !name.is_empty())
                .cloned(),
        );

        if cert_manager.include_generated_dns_names {
            let tenant_name = &config.tenant_name;
            let namespace = &config.test_namespace;
            let io_service = format!("{tenant_name}-io");
            let headless_service = format!("{tenant_name}-hl");
            names.insert(format!("{io_service}.{namespace}.svc"));
            names.insert(format!("{io_service}.{namespace}.svc.cluster.local"));
            names.insert(format!("{headless_service}.{namespace}.svc"));
            names.insert(format!("{headless_service}.{namespace}.svc.cluster.local"));
            for pool in &tenant.spec.pools {
                for ordinal in 0..pool.servers.max(0) {
                    names.insert(format!(
                        "{tenant_name}-{}-{ordinal}.{headless_service}.{namespace}.svc.cluster.local",
                        pool.name
                    ));
                }
            }
        }
    }

    names.into_iter().collect()
}

pub fn managed_certificate_tenant(config: &E2eConfig) -> Tenant {
    let secret_name = managed_secret_name(config);
    let certificate_name = managed_certificate_name(config);
    positive_tenant_with_tls(
        config,
        cert_manager_tls_config(
            true,
            secret_name,
            Some(certificate_name),
            Some(issuer_ref(SELF_SIGNED_ISSUER_NAME)),
            true,
            TlsRotationStrategy::Rollout,
            CaTrustConfig {
                source: CaTrustSource::CertificateSecretCa,
                ..Default::default()
            },
        ),
    )
}

pub fn managed_certificate_tenant_manifest(config: &E2eConfig) -> Result<String> {
    tenant_manifest(&managed_certificate_tenant(config))
}

pub fn external_secret_tenant(config: &E2eConfig) -> Tenant {
    positive_tenant_with_tls(
        config,
        cert_manager_tls_config(
            false,
            external_secret_name(config),
            None,
            None,
            true,
            TlsRotationStrategy::Rollout,
            CaTrustConfig {
                source: CaTrustSource::SecretRef,
                ca_secret_ref: Some(SecretKeyReference {
                    name: external_ca_secret_name(config),
                    key: "ca.crt".to_string(),
                }),
                ..Default::default()
            },
        ),
    )
}

pub fn external_secret_tenant_manifest(config: &E2eConfig) -> Result<String> {
    tenant_manifest(&external_secret_tenant(config))
}

fn positive_tls_case_config(config: &E2eConfig, suffix: &str) -> E2eConfig {
    let mut isolated = config.clone();
    isolated.test_namespace = format!("{}-{suffix}", config.test_namespace_prefix);
    isolated.tenant_name = format!("{}-{suffix}", config.tenant_name);
    isolated.storage_class = format!("{}-{suffix}", config.storage_class);
    isolated.pv_count = topology_safe_pv_count_for_tls_tenant(&isolated);
    isolated
}

fn topology_safe_pv_count_for_tls_tenant(config: &E2eConfig) -> usize {
    let template = positive_tls_tenant_template(config);
    let servers = usize::try_from(template.servers)
        .expect("TLS e2e Tenant template server count must be non-negative");
    let volumes_per_server = usize::try_from(template.volumes_per_server)
        .expect("TLS e2e Tenant template volumes_per_server must be non-negative");
    servers * volumes_per_server
}

fn positive_tls_tenant_template(config: &E2eConfig) -> TenantTemplate {
    let mut template = tls_tenant_template(config);
    template.volumes_per_server = 1;
    template
}

fn tls_tenant_template(config: &E2eConfig) -> TenantTemplate {
    TenantTemplate::kind_local(
        &config.test_namespace,
        &config.tenant_name,
        &config.rustfs_image,
        &config.storage_class,
        resources::credential_secret_name(config),
    )
}

fn positive_tls_storage_layout(config: &E2eConfig, suffix: &str) -> storage::LocalStorageLayout {
    storage::LocalStorageLayout::new(
        config.storage_class.clone(),
        format!("{}-{suffix}-pv", config.cluster_name),
        format!("/mnt/data/{suffix}"),
        config.pv_count,
    )
}

pub fn negative_case_tenant_manifest(config: &E2eConfig, case: NegativeTlsCase) -> Result<String> {
    tenant_manifest(&negative_case_tenant(config, case))
}

pub fn negative_tls_secret_manifest(config: &E2eConfig, case: NegativeTlsCase) -> Result<String> {
    let (secret_type, data) = match case {
        NegativeTlsCase::MissingCaForInternodeHttps => {
            let material = generate_missing_ca_tls_material(config)?;
            (
                KUBERNETES_TLS_SECRET_TYPE,
                secret_data(Some(&material.cert), Some(&material.key), None),
            )
        }
        _ => negative_tls_secret_fixture(case).with_context(|| {
            format!(
                "case {} does not apply a TLS Secret fixture",
                case.case_name()
            )
        })?,
    };
    tls_secret_manifest(
        &config.test_namespace,
        &negative_secret_name(config, case),
        secret_type,
        data,
    )
}

pub fn sample_tls_plan(hash: &str, server_secret_name: String) -> TlsPlan {
    TlsPlan::rollout(
        "/var/run/rustfs/tls".to_string(),
        hash.to_string(),
        server_secret_name,
        Some("ca.crt".to_string()),
        None,
        None,
        true,
        false,
        false,
        None,
    )
}

pub fn external_tls_secret_apply_command(
    config: &E2eConfig,
    secret_name: String,
) -> Result<CommandSpec> {
    Ok(
        Kubectl::new(config).apply_yaml_command(external_tls_secret_manifest(
            config,
            &secret_name,
            KUBERNETES_TLS_SECRET_TYPE,
            secret_data(
                Some(REDACTED_FIXTURE_BYTES),
                Some(REDACTED_FIXTURE_BYTES),
                Some(REDACTED_FIXTURE_BYTES),
            ),
        )?),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTlsSecretManifests {
    pub tls_secret_manifest: String,
    pub ca_secret_manifest: String,
}

pub fn external_tls_secret_manifests(config: &E2eConfig) -> Result<ExternalTlsSecretManifests> {
    let material = generate_external_tls_material(config)?;
    external_tls_secret_manifests_from_material(config, &material)
}

fn external_tls_secret_manifests_from_material(
    config: &E2eConfig,
    material: &GeneratedTlsMaterial,
) -> Result<ExternalTlsSecretManifests> {
    Ok(ExternalTlsSecretManifests {
        tls_secret_manifest: external_tls_secret_manifest(
            config,
            &external_secret_name(config),
            KUBERNETES_TLS_SECRET_TYPE,
            secret_data(
                Some(&material.cert),
                Some(&material.key),
                Some(&material.ca),
            ),
        )?,
        ca_secret_manifest: external_tls_secret_manifest(
            config,
            &external_ca_secret_name(config),
            OPAQUE_SECRET_TYPE,
            secret_data(None, None, Some(&material.ca)),
        )?,
    })
}

pub fn apply_managed_certificate_case_resources(config: &E2eConfig) -> Result<()> {
    apply_positive_case_base_resources(config, managed_certificate_storage_layout(config))?;
    apply_yaml(config, self_signed_issuer_manifest(config))?;
    apply_yaml(config, managed_certificate_tenant_manifest(config)?)?;
    Ok(())
}

pub fn apply_external_secret_case_resources(config: &E2eConfig) -> Result<()> {
    apply_positive_case_base_resources(config, external_secret_storage_layout(config))?;
    let material = generate_external_tls_material(config)?;
    apply_external_tls_material(config, &material)?;
    apply_yaml(config, external_secret_tenant_manifest(config)?)?;
    Ok(())
}

pub fn rotate_external_tls_secret(config: &E2eConfig) -> Result<()> {
    let material = generate_external_tls_material(config)?;
    apply_external_tls_material(config, &material)?;
    Ok(())
}

pub fn apply_negative_case_resources(config: &E2eConfig, case: NegativeTlsCase) -> Result<()> {
    apply_base_resources(config)?;

    match case {
        NegativeTlsCase::MissingTlsCrt
        | NegativeTlsCase::MissingTlsKey
        | NegativeTlsCase::MissingCaForInternodeHttps => {
            apply_yaml(config, negative_tls_secret_manifest(config, case)?)?;
        }
        NegativeTlsCase::PendingCertificate => {
            apply_yaml(config, pending_ca_issuer_manifest(config))?;
        }
        NegativeTlsCase::MissingIssuer | NegativeTlsCase::HotReloadUnsupported => {}
    }

    apply_yaml(
        config,
        tenant_manifest(&negative_case_tenant(config, case))?,
    )?;
    Ok(())
}

pub async fn wait_for_tenant_tls_ready(
    client: Client,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> Result<Tenant> {
    let tenants: Api<Tenant> = Api::namespaced(client, namespace);
    let name = name.to_string();
    wait::wait_until(
        &format!("Tenant {name} TLSReady=True and Ready=True"),
        timeout,
        Duration::from_secs(5),
        move || {
            let tenants = tenants.clone();
            let name = name.clone();
            async move {
                let tenant = tenants.get(&name).await?;
                let tls_ready = assertions::condition_status(&tenant, "TlsReady") == Some("True")
                    || assertions::condition_status(&tenant, "TLSReady") == Some("True");
                let tenant_ready = assertions::current_state(&tenant) == Some("Ready")
                    && assertions::condition_status(&tenant, "Ready") == Some("True")
                    && assertions::condition_status(&tenant, "Degraded") == Some("False");
                if tls_ready && tenant_ready {
                    Ok(Some(tenant))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await
}

pub async fn wait_for_tenant_tls_hash_change(
    client: Client,
    namespace: &str,
    name: &str,
    previous_hash: &str,
    timeout: Duration,
) -> Result<Tenant> {
    let tenants: Api<Tenant> = Api::namespaced(client, namespace);
    let name = name.to_string();
    let previous_hash = previous_hash.to_string();
    wait::wait_until(
        &format!("Tenant {name} TLS hash to change from {previous_hash}"),
        timeout,
        Duration::from_secs(5),
        move || {
            let tenants = tenants.clone();
            let name = name.clone();
            let previous_hash = previous_hash.clone();
            async move {
                let tenant = tenants.get(&name).await?;
                match assertions::tenant_tls_observed_hash(&tenant) {
                    Ok(hash) if hash != previous_hash => Ok(Some(tenant)),
                    _ => Ok(None),
                }
            }
        },
    )
    .await
}

pub async fn wait_for_tenant_tls_reason(
    client: Client,
    namespace: &str,
    name: &str,
    reason: &str,
    timeout: Duration,
) -> Result<Tenant> {
    let tenants: Api<Tenant> = Api::namespaced(client, namespace);
    let name = name.to_string();
    let reason = reason.to_string();
    wait::wait_until(
        &format!("Tenant {name} TLSReady reason {reason}"),
        timeout,
        Duration::from_secs(5),
        move || {
            let tenants = tenants.clone();
            let name = name.clone();
            let reason = reason.clone();
            async move {
                let tenant = tenants.get(&name).await?;
                if tenant_tls_reason(&tenant).as_deref() == Some(reason.as_str()) {
                    Ok(Some(tenant))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await
}

pub async fn wait_for_certificate_ready(
    client: Client,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> Result<DynamicObject> {
    let certificates: Api<DynamicObject> = Api::namespaced_with(
        client,
        namespace,
        &ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk(
                CERT_MANAGER_GROUP,
                CERT_MANAGER_VERSION,
                CERT_MANAGER_CERTIFICATE_KIND,
            ),
            CERT_MANAGER_CERTIFICATE_PLURAL,
        ),
    );
    let name = name.to_string();
    wait::wait_until(
        &format!("cert-manager Certificate {name} Ready=True"),
        timeout,
        Duration::from_secs(5),
        move || {
            let certificates = certificates.clone();
            let name = name.clone();
            async move {
                let certificate = certificates.get(&name).await?;
                if dynamic_certificate_ready(&certificate) {
                    Ok(Some(certificate))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await
}

pub async fn assert_live_workload_tls_wiring(
    client: Client,
    config: &E2eConfig,
    tenant: &Tenant,
) -> Result<()> {
    let tls_status = tenant
        .status
        .as_ref()
        .and_then(|status| status.certificates.tls.as_ref())
        .context("Tenant status.certificates.tls is missing")?;
    let hash = tls_status
        .observed_hash
        .as_deref()
        .context("Tenant TLS status missing observedHash")?;
    let secret_name = tls_status
        .server_secret_ref
        .as_ref()
        .map(|secret| secret.name.as_str())
        .context("Tenant TLS status missing serverSecretRef")?;
    let ca_secret_ref = tls_status
        .ca_secret_ref
        .as_ref()
        .context("Tenant TLS status missing caSecretRef")?;

    let statefulsets: Api<StatefulSet> = Api::namespaced(client.clone(), &config.test_namespace);
    let services: Api<corev1::Service> = Api::namespaced(client, &config.test_namespace);
    let pool = tenant
        .spec
        .pools
        .first()
        .context("Tenant fixture must include at least one pool")?;
    let statefulset = statefulsets
        .get(&format!("{}-{}", config.tenant_name, pool.name))
        .await?;
    let io_service = services.get(&format!("{}-io", config.tenant_name)).await?;
    let headless_service = services.get(&format!("{}-hl", config.tenant_name)).await?;

    assertions::require_tls_statefulset_https_wiring(
        &statefulset,
        hash,
        secret_name,
        ca_secret_ref,
    )?;
    assertions::require_tls_service_https_wiring(&io_service)?;
    assertions::require_tls_service_https_wiring(&headless_service)?;
    Ok(())
}

fn positive_tenant_with_tls(config: &E2eConfig, tls: TlsConfig) -> Tenant {
    let mut tenant = positive_tls_tenant_template(config).build();
    tenant.spec.tls = Some(tls);
    tenant
}

fn tenant_with_tls(config: &E2eConfig, tls: TlsConfig) -> Tenant {
    let mut tenant = tls_tenant_template(config).build();
    tenant.spec.tls = Some(tls);
    tenant
}

#[allow(clippy::too_many_arguments)]
fn cert_manager_tls_config(
    manage_certificate: bool,
    secret_name: String,
    certificate_name: Option<String>,
    issuer_ref: Option<CertManagerIssuerRef>,
    enable_internode_https: bool,
    rotation_strategy: TlsRotationStrategy,
    ca_trust: CaTrustConfig,
) -> TlsConfig {
    TlsConfig {
        mode: TlsMode::CertManager,
        rotation_strategy,
        enable_internode_https,
        cert_manager: Some(CertManagerTlsConfig {
            manage_certificate,
            secret_name: Some(secret_name),
            secret_type: Some(KUBERNETES_TLS_SECRET_TYPE.to_string()),
            certificate_name,
            issuer_ref,
            common_name: Some("rustfs-e2e.local".to_string()),
            dns_names: vec!["rustfs-e2e.local".to_string()],
            ca_trust: Some(ca_trust),
            ..CertManagerTlsConfig::default()
        }),
        ..TlsConfig::default()
    }
}

fn issuer_ref(name: &str) -> CertManagerIssuerRef {
    CertManagerIssuerRef {
        group: CERT_MANAGER_GROUP.to_string(),
        kind: "Issuer".to_string(),
        name: name.to_string(),
    }
}

fn tenant_manifest(tenant: &Tenant) -> Result<String> {
    Ok(serde_yaml_ng::to_string(tenant)?)
}

fn apply_base_resources(config: &E2eConfig) -> Result<()> {
    storage::prepare_local_storage(config)?;
    apply_shared_namespace_resources(config)
}

fn apply_positive_case_base_resources(
    config: &E2eConfig,
    layout: storage::LocalStorageLayout,
) -> Result<()> {
    storage::prepare_local_storage_with_layout(config, &layout)?;
    apply_shared_namespace_resources(config)
}

fn apply_shared_namespace_resources(config: &E2eConfig) -> Result<()> {
    apply_yaml(
        config,
        resources::namespace_manifest(&config.test_namespace),
    )?;
    apply_yaml(config, resources::credential_secret_manifest(config))?;
    Ok(())
}

fn apply_yaml(config: &E2eConfig, yaml: String) -> Result<()> {
    Kubectl::new(config)
        .apply_yaml_command(yaml)
        .run_checked()?;
    Ok(())
}

fn self_signed_issuer_manifest(config: &E2eConfig) -> String {
    format!(
        r#"apiVersion: cert-manager.io/v1
kind: Issuer
metadata:
  name: {SELF_SIGNED_ISSUER_NAME}
  namespace: {namespace}
spec:
  selfSigned: {{}}
"#,
        namespace = config.test_namespace
    )
}

fn pending_ca_issuer_manifest(config: &E2eConfig) -> String {
    format!(
        r#"apiVersion: cert-manager.io/v1
kind: Issuer
metadata:
  name: {PENDING_ISSUER_NAME}
  namespace: {namespace}
spec:
  ca:
    secretName: rustfs-e2e-missing-ca-secret
"#,
        namespace = config.test_namespace
    )
}

fn negative_case_tenant(config: &E2eConfig, case: NegativeTlsCase) -> Tenant {
    match case {
        NegativeTlsCase::MissingTlsCrt => tenant_with_expected_secret_type(
            tenant_with_tls(
                config,
                cert_manager_tls_config(
                    false,
                    negative_secret_name(config, case),
                    None,
                    None,
                    false,
                    TlsRotationStrategy::Rollout,
                    CaTrustConfig::default(),
                ),
            ),
            OPAQUE_SECRET_TYPE,
        ),
        NegativeTlsCase::MissingTlsKey => tenant_with_expected_secret_type(
            tenant_with_tls(
                config,
                cert_manager_tls_config(
                    false,
                    negative_secret_name(config, case),
                    None,
                    None,
                    false,
                    TlsRotationStrategy::Rollout,
                    CaTrustConfig::default(),
                ),
            ),
            OPAQUE_SECRET_TYPE,
        ),
        NegativeTlsCase::MissingCaForInternodeHttps => tenant_with_tls(
            config,
            cert_manager_tls_config(
                false,
                negative_secret_name(config, case),
                None,
                None,
                true,
                TlsRotationStrategy::Rollout,
                CaTrustConfig::default(),
            ),
        ),
        NegativeTlsCase::MissingIssuer => tenant_with_tls(
            config,
            cert_manager_tls_config(
                true,
                negative_secret_name(config, case),
                Some(format!("{}-missing-issuer-cert", config.tenant_name)),
                Some(issuer_ref(MISSING_ISSUER_NAME)),
                true,
                TlsRotationStrategy::Rollout,
                CaTrustConfig::default(),
            ),
        ),
        NegativeTlsCase::PendingCertificate => tenant_with_tls(
            config,
            cert_manager_tls_config(
                true,
                negative_secret_name(config, case),
                Some(format!("{}-pending-cert", config.tenant_name)),
                Some(issuer_ref(PENDING_ISSUER_NAME)),
                true,
                TlsRotationStrategy::Rollout,
                CaTrustConfig::default(),
            ),
        ),
        NegativeTlsCase::HotReloadUnsupported => tenant_with_tls(
            config,
            cert_manager_tls_config(
                false,
                negative_secret_name(config, case),
                None,
                None,
                false,
                TlsRotationStrategy::HotReload,
                CaTrustConfig::default(),
            ),
        ),
    }
}

fn tenant_with_expected_secret_type(mut tenant: Tenant, secret_type: &str) -> Tenant {
    let cert_manager = tenant
        .spec
        .tls
        .as_mut()
        .and_then(|tls| tls.cert_manager.as_mut())
        .expect("negative TLS fixture should configure cert-manager TLS");
    cert_manager.secret_type = Some(secret_type.to_string());
    tenant
}

fn negative_secret_name(config: &E2eConfig, case: NegativeTlsCase) -> String {
    let suffix = match case {
        NegativeTlsCase::MissingTlsCrt => "missing-crt",
        NegativeTlsCase::MissingTlsKey => "missing-key",
        NegativeTlsCase::MissingCaForInternodeHttps => "missing-ca",
        NegativeTlsCase::MissingIssuer => "missing-issuer",
        NegativeTlsCase::PendingCertificate => "pending",
        NegativeTlsCase::HotReloadUnsupported => "hot-reload",
    };
    format!("{}-{suffix}-tls", config.tenant_name)
}

fn negative_tls_secret_fixture(
    case: NegativeTlsCase,
) -> Option<(&'static str, BTreeMap<String, ByteString>)> {
    match case {
        NegativeTlsCase::MissingTlsCrt => Some((
            OPAQUE_SECRET_TYPE,
            secret_data(
                None,
                Some(REDACTED_FIXTURE_BYTES),
                Some(REDACTED_FIXTURE_BYTES),
            ),
        )),
        NegativeTlsCase::MissingTlsKey => Some((
            OPAQUE_SECRET_TYPE,
            secret_data(
                Some(REDACTED_FIXTURE_BYTES),
                None,
                Some(REDACTED_FIXTURE_BYTES),
            ),
        )),
        NegativeTlsCase::MissingCaForInternodeHttps
        | NegativeTlsCase::MissingIssuer
        | NegativeTlsCase::PendingCertificate
        | NegativeTlsCase::HotReloadUnsupported => None,
    }
}

struct GeneratedTlsMaterial {
    cert: Vec<u8>,
    key: Vec<u8>,
    ca: Vec<u8>,
    _dir: TempDir,
}

fn generate_external_tls_material(config: &E2eConfig) -> Result<GeneratedTlsMaterial> {
    let dns_names = external_tls_certificate_dns_names(config);
    generate_ca_signed_tls_material(&config.tenant_name, &dns_names)
}

fn generate_missing_ca_tls_material(config: &E2eConfig) -> Result<GeneratedTlsMaterial> {
    let tenant = negative_case_tenant(config, NegativeTlsCase::MissingCaForInternodeHttps);
    let dns_names = tls_certificate_dns_names(config, &tenant);
    generate_self_signed_tls_material(&config.tenant_name, &dns_names)
}

fn generate_ca_signed_tls_material(
    common_name: &str,
    dns_names: &[String],
) -> Result<GeneratedTlsMaterial> {
    let dir = tempfile::tempdir()?;
    let ca_key_path = dir.path().join("ca.key");
    let ca_cert_path = dir.path().join("ca.crt");
    let key_path = dir.path().join("tls.key");
    let csr_path = dir.path().join("tls.csr");
    let cert_path = dir.path().join("tls.crt");
    let leaf_ext_path = dir.path().join("leaf.ext");
    let san = subject_alt_name(dns_names);

    openssl_ca_certificate_command(dir.path(), &ca_key_path, &ca_cert_path, common_name)
        .run_checked()?;
    openssl_leaf_csr_command(dir.path(), &key_path, &csr_path, common_name).run_checked()?;
    fs::write(
        &leaf_ext_path,
        format!(
            "basicConstraints=critical,CA:FALSE\n\
             keyUsage=critical,digitalSignature,keyEncipherment\n\
             extendedKeyUsage=serverAuth,clientAuth\n\
             {san}\n"
        ),
    )
    .with_context(|| format!("write {}", leaf_ext_path.display()))?;
    openssl_sign_leaf_command(
        dir.path(),
        &csr_path,
        &ca_cert_path,
        &ca_key_path,
        &cert_path,
        &leaf_ext_path,
    )
    .run_checked()?;

    let cert = fs::read(&cert_path).with_context(|| format!("read {}", cert_path.display()))?;
    let key = fs::read(&key_path).with_context(|| format!("read {}", key_path.display()))?;
    let ca = fs::read(&ca_cert_path).with_context(|| format!("read {}", ca_cert_path.display()))?;
    Ok(GeneratedTlsMaterial {
        cert,
        key,
        ca,
        _dir: dir,
    })
}

fn generate_self_signed_tls_material(
    common_name: &str,
    dns_names: &[String],
) -> Result<GeneratedTlsMaterial> {
    let dir = tempfile::tempdir()?;
    let key_path = dir.path().join("tls.key");
    let cert_path = dir.path().join("tls.crt");
    let san = subject_alt_name(dns_names);

    openssl_self_signed_command(dir.path(), &key_path, &cert_path, common_name, &san)
        .run_checked()?;

    let cert = fs::read(&cert_path).with_context(|| format!("read {}", cert_path.display()))?;
    let key = fs::read(&key_path).with_context(|| format!("read {}", key_path.display()))?;
    Ok(GeneratedTlsMaterial {
        ca: cert.clone(),
        cert,
        key,
        _dir: dir,
    })
}

fn subject_alt_name(dns_names: &[String]) -> String {
    format!(
        "subjectAltName={}",
        dns_names
            .iter()
            .map(|name| format!("DNS:{name}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn openssl_ca_certificate_command(
    cwd: &Path,
    key_path: &Path,
    cert_path: &Path,
    common_name: &str,
) -> CommandSpec {
    CommandSpec::new("openssl")
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout"])
        .arg(key_path.display().to_string())
        .args(["-out"])
        .arg(cert_path.display().to_string())
        .args(["-days", "1", "-sha256", "-subj"])
        .arg(format!("/CN={common_name}-ca"))
        .args(["-addext", "basicConstraints=critical,CA:TRUE"])
        .args(["-addext", "keyUsage=critical,keyCertSign,cRLSign"])
        .cwd(cwd)
}

fn openssl_leaf_csr_command(
    cwd: &Path,
    key_path: &Path,
    csr_path: &Path,
    common_name: &str,
) -> CommandSpec {
    CommandSpec::new("openssl")
        .args(["req", "-newkey", "rsa:2048", "-nodes", "-keyout"])
        .arg(key_path.display().to_string())
        .args(["-out"])
        .arg(csr_path.display().to_string())
        .args(["-subj"])
        .arg(format!("/CN={common_name}"))
        .cwd(cwd)
}

fn openssl_sign_leaf_command(
    cwd: &Path,
    csr_path: &Path,
    ca_cert_path: &Path,
    ca_key_path: &Path,
    cert_path: &Path,
    leaf_ext_path: &Path,
) -> CommandSpec {
    CommandSpec::new("openssl")
        .args(["x509", "-req", "-in"])
        .arg(csr_path.display().to_string())
        .args(["-CA"])
        .arg(ca_cert_path.display().to_string())
        .args(["-CAkey"])
        .arg(ca_key_path.display().to_string())
        .args(["-CAcreateserial", "-out"])
        .arg(cert_path.display().to_string())
        .args(["-days", "1", "-sha256", "-extfile"])
        .arg(leaf_ext_path.display().to_string())
        .cwd(cwd)
}

fn openssl_self_signed_command(
    cwd: &Path,
    key_path: &Path,
    cert_path: &Path,
    common_name: &str,
    san: &str,
) -> CommandSpec {
    CommandSpec::new("openssl")
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout"])
        .arg(key_path.display().to_string())
        .args(["-out"])
        .arg(cert_path.display().to_string())
        .args(["-days", "1", "-subj"])
        .arg(format!("/CN={common_name}"))
        .args(["-addext"])
        .arg(san.to_string())
        .cwd(cwd)
}

fn apply_external_tls_material(config: &E2eConfig, material: &GeneratedTlsMaterial) -> Result<()> {
    let manifests = external_tls_secret_manifests_from_material(config, material)?;
    apply_yaml(config, manifests.tls_secret_manifest)?;
    apply_yaml(config, manifests.ca_secret_manifest)?;
    Ok(())
}

fn external_tls_secret_manifest(
    config: &E2eConfig,
    name: &str,
    secret_type: &str,
    data: BTreeMap<String, ByteString>,
) -> Result<String> {
    tls_secret_manifest_with_labels(
        &config.test_namespace,
        name,
        secret_type,
        data,
        tenant_watch_labels(config),
    )
}

fn tls_secret_manifest(
    namespace: &str,
    name: &str,
    secret_type: &str,
    data: BTreeMap<String, ByteString>,
) -> Result<String> {
    tls_secret_manifest_with_labels(namespace, name, secret_type, data, BTreeMap::new())
}

fn tls_secret_manifest_with_labels(
    namespace: &str,
    name: &str,
    secret_type: &str,
    data: BTreeMap<String, ByteString>,
    labels: BTreeMap<String, String>,
) -> Result<String> {
    let labels = (!labels.is_empty()).then_some(labels);
    let secret = corev1::Secret {
        metadata: metav1::ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels,
            ..Default::default()
        },
        type_: Some(secret_type.to_string()),
        data: Some(data),
        ..Default::default()
    };
    Ok(serde_yaml_ng::to_string(&secret)?)
}

fn secret_data(
    cert: Option<&[u8]>,
    key: Option<&[u8]>,
    ca: Option<&[u8]>,
) -> BTreeMap<String, ByteString> {
    let mut data = BTreeMap::new();
    if let Some(cert) = cert {
        data.insert("tls.crt".to_string(), ByteString(cert.to_vec()));
    }
    if let Some(key) = key {
        data.insert("tls.key".to_string(), ByteString(key.to_vec()));
    }
    if let Some(ca) = ca {
        data.insert("ca.crt".to_string(), ByteString(ca.to_vec()));
    }
    data
}

fn dynamic_certificate_ready(certificate: &DynamicObject) -> bool {
    status_conditions(&certificate.data)
        .iter()
        .any(|condition| {
            condition.get("type").and_then(Value::as_str) == Some("Ready")
                && condition.get("status").and_then(Value::as_str) == Some("True")
        })
}

fn status_conditions(data: &Value) -> Vec<&Value> {
    data.pointer("/status/conditions")
        .and_then(Value::as_array)
        .map(|conditions| conditions.iter().collect())
        .unwrap_or_default()
}

fn tenant_tls_reason(tenant: &Tenant) -> Option<String> {
    tenant
        .status
        .as_ref()
        .and_then(|status| {
            status
                .conditions
                .iter()
                .find(|condition| condition.type_ == "TlsReady" || condition.type_ == "TLSReady")
                .map(|condition| condition.reason.clone())
        })
        .or_else(|| {
            tenant
                .status
                .as_ref()
                .and_then(|status| status.certificates.tls.as_ref())
                .and_then(|tls| tls.last_error_reason.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::{
        external_secret_name, external_secret_tenant_manifest, managed_certificate_tenant_manifest,
    };
    use crate::framework::assertions;
    use crate::framework::config::E2eConfig;

    #[test]
    fn tenant_manifests_do_not_embed_pem_material() {
        let config = E2eConfig::defaults();

        assertions::require_no_secret_material(
            "managed manifest",
            &managed_certificate_tenant_manifest(&config).expect("managed manifest"),
        )
        .expect("managed manifest should not expose secrets");
        assertions::require_no_secret_material(
            "external manifest",
            &external_secret_tenant_manifest(&config).expect("external manifest"),
        )
        .expect("external manifest should not expose secrets");
    }

    #[test]
    fn external_secret_name_is_stable() {
        let config = E2eConfig::defaults();

        assert_eq!(external_secret_name(&config), "e2e-tenant-external-tls");
    }
}
