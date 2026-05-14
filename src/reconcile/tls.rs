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

use super::{Error, patch_status_and_record, patch_status_error};
use crate::context::{self, Context};
use crate::status::{StatusBuilder, StatusError};
use crate::types::v1alpha1::status::Reason;
use crate::types::v1alpha1::status::certificate::{
    CertificateObjectRef, SecretStatusRef, TlsCertificateStatus,
};
use crate::types::v1alpha1::tenant::Tenant;
use crate::types::v1alpha1::tls::{
    CaTrustSource, CertManagerIssuerRef, CertManagerTlsConfig, SecretKeyReference, TlsConfig,
    TlsMode, TlsPlan, TlsRotationStrategy,
};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, Patch, PatchParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Cursor;

const TLS_CERT_KEY: &str = "tls.crt";
const TLS_KEY_KEY: &str = "tls.key";
const CA_CERT_KEY: &str = "ca.crt";
const KUBERNETES_TLS_SECRET_TYPE: &str = "kubernetes.io/tls";
const CERT_MANAGER_V1_SECRET_TYPE: &str = "cert-manager.io/v1";
const CERT_MANAGER_V1ALPHA2_SECRET_TYPE: &str = "cert-manager.io/v1alpha2";
const CERT_MANAGER_GROUP: &str = "cert-manager.io";
const CERT_MANAGER_VERSION: &str = "v1";
const CERT_MANAGER_CERTIFICATE_KIND: &str = "Certificate";
const CERT_MANAGER_CERTIFICATE_PLURAL: &str = "certificates";
const CERT_MANAGER_CERTIFICATE_CRD: &str = "certificates.cert-manager.io";
const CERT_MANAGER_ISSUER_KIND: &str = "Issuer";
const CERT_MANAGER_ISSUER_PLURAL: &str = "issuers";
const CERT_MANAGER_CLUSTER_ISSUER_KIND: &str = "ClusterIssuer";
const CERT_MANAGER_CLUSTER_ISSUER_PLURAL: &str = "clusterissuers";
const STATUS_MESSAGE_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CertManagerPrerequisite {
    CertificateCrd,
    Issuer,
    ClusterIssuer,
}

#[derive(Debug, PartialEq)]
struct TlsValidationFailure {
    reason: Reason,
    message: String,
}

#[derive(Debug, PartialEq)]
struct ServerCaMaterial {
    key: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
struct CertManagerCertificateObservation {
    name: String,
    observed_generation: Option<i64>,
    ready: bool,
    reason: Option<String>,
    message: Option<String>,
}

impl CertManagerCertificateObservation {
    fn status_ref(&self) -> CertificateObjectRef {
        CertificateObjectRef {
            api_version: format!("{CERT_MANAGER_GROUP}/{CERT_MANAGER_VERSION}"),
            kind: CERT_MANAGER_CERTIFICATE_KIND.to_string(),
            name: self.name.clone(),
            observed_generation: self.observed_generation,
            ready: Some(self.ready),
            reason: self.reason.clone(),
        }
    }
}

pub(super) async fn reconcile_tls(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<TlsPlan, Error> {
    let Some(config) = tenant.spec.tls.as_ref() else {
        return Ok(TlsPlan::disabled());
    };
    if !config.is_enabled() {
        return Ok(TlsPlan::disabled());
    }

    if !config.mount_path.starts_with('/') {
        return tls_blocked(
            ctx,
            tenant,
            config,
            Reason::CertificateInvalid,
            format!(
                "spec.tls.mountPath must be an absolute path (got '{}')",
                config.mount_path
            ),
        )
        .await;
    }

    if config.rotation_strategy == TlsRotationStrategy::HotReload {
        return tls_blocked(
            ctx,
            tenant,
            config,
            Reason::TlsHotReloadUnsupported,
            "TLS hot reload is not supported until RustFS clean-directory reload support is implemented; use rotationStrategy=Rollout".to_string(),
        )
        .await;
    }

    match config.mode {
        TlsMode::Disabled => Ok(TlsPlan::disabled()),
        TlsMode::External => {
            tls_blocked(
                ctx,
                tenant,
                config,
                Reason::CertificateSecretNotFound,
                "spec.tls.mode=external is reserved for the external TLS Secret API and is not wired in this phase".to_string(),
            )
            .await
        }
        TlsMode::CertManager => reconcile_cert_manager_tls(ctx, tenant, namespace, config).await,
    }
}

async fn reconcile_cert_manager_tls(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    config: &TlsConfig,
) -> Result<TlsPlan, Error> {
    let Some(cert_manager) = config.cert_manager.as_ref() else {
        return tls_blocked(
            ctx,
            tenant,
            config,
            Reason::CertificateSecretNotFound,
            "spec.tls.certManager.secretName is required for certManager TLS mode".to_string(),
        )
        .await;
    };

    let Some(secret_name) = cert_manager
        .secret_name
        .as_deref()
        .filter(|name| !name.is_empty())
    else {
        return tls_blocked(
            ctx,
            tenant,
            config,
            Reason::CertificateSecretNotFound,
            "spec.tls.certManager.secretName is required for certManager TLS mode".to_string(),
        )
        .await;
    };

    let mut certificate_ref = None;
    if cert_manager.manage_certificate {
        let Some(issuer_ref) = cert_manager.issuer_ref.as_ref() else {
            return tls_blocked(
                ctx,
                tenant,
                config,
                Reason::CertManagerIssuerNotFound,
                "spec.tls.certManager.issuerRef is required when manageCertificate=true"
                    .to_string(),
            )
            .await;
        };
        let certificate_name = certificate_name(tenant, cert_manager);

        if let Err(error) = ensure_cert_manager_certificate_crd(ctx).await {
            return cert_manager_prerequisite_failed(
                ctx,
                tenant,
                config,
                CertManagerPrerequisite::CertificateCrd,
                error,
                format!(
                    "cert-manager Certificate CRD '{}' is not installed",
                    CERT_MANAGER_CERTIFICATE_CRD
                ),
            )
            .await;
        }

        if let Err(error) = ensure_cert_manager_issuer(ctx, namespace, issuer_ref).await {
            return cert_manager_prerequisite_failed(
                ctx,
                tenant,
                config,
                issuer_prerequisite(issuer_ref),
                error,
                format!(
                    "cert-manager {} '{}' was not found",
                    issuer_ref.kind, issuer_ref.name
                ),
            )
            .await;
        }

        let desired_certificate = build_cert_manager_certificate(
            tenant,
            namespace,
            config,
            cert_manager,
            secret_name,
            &certificate_name,
        );
        let observed_certificate = match apply_cert_manager_certificate(
            ctx,
            namespace,
            &certificate_name,
            &desired_certificate,
        )
        .await
        {
            Ok(certificate) => certificate,
            Err(error) if context::is_kube_not_found(&error) => {
                return tls_blocked(
                    ctx,
                    tenant,
                    config,
                    Reason::CertManagerCrdMissing,
                    format!(
                        "cert-manager Certificate API was not found while applying '{}'",
                        certificate_name
                    ),
                )
                .await;
            }
            Err(error) => {
                return tls_blocked(
                    ctx,
                    tenant,
                    config,
                    Reason::CertManagerCertificateApplyFailed,
                    format!(
                        "failed to apply cert-manager Certificate '{}': {}",
                        certificate_name,
                        sanitize_status_message(&error.to_string())
                    ),
                )
                .await;
            }
        };
        let observation = observe_cert_manager_certificate(&observed_certificate);
        certificate_ref = Some(observation.status_ref());
        if !observation.ready {
            return tls_pending_with_certificate_ref(
                ctx,
                tenant,
                config,
                Reason::CertManagerCertificateNotReady,
                certificate_not_ready_message(&certificate_name, &observation),
                certificate_ref.clone(),
            )
            .await;
        }
    }

    let secret = get_server_secret_or_tls_error(
        ctx,
        tenant,
        config,
        namespace,
        secret_name,
        cert_manager.manage_certificate,
        certificate_ref.clone(),
    )
    .await?;

    if let Err(failure) = validate_tls_secret_type(
        &secret,
        secret_name,
        cert_manager
            .secret_type
            .as_deref()
            .filter(|secret_type| !secret_type.is_empty()),
    ) {
        return tls_validation_blocked(ctx, tenant, config, failure).await;
    }

    let cert_bytes = require_secret_key(
        ctx,
        tenant,
        config,
        &secret,
        secret_name,
        TLS_CERT_KEY,
        Reason::CertificateSecretMissingKey,
    )
    .await?;
    require_secret_key(
        ctx,
        tenant,
        config,
        &secret,
        secret_name,
        TLS_KEY_KEY,
        Reason::CertificateSecretMissingKey,
    )
    .await?;

    if config.require_san_match && config.enable_internode_https {
        let expected_dns_names = certificate_dns_names(tenant, namespace, cert_manager);
        if let Err(failure) =
            validate_tls_secret_san_match(secret_name, &cert_bytes, &expected_dns_names)
        {
            return tls_validation_blocked(ctx, tenant, config, failure).await;
        }
    }

    let ca_trust = config.ca_trust();
    let trust_system_ca = ca_trust.trust_system_ca || ca_trust.source == CaTrustSource::SystemCa;
    let mut server_ca_key = None;
    let mut explicit_ca = None;
    let mut explicit_ca_secret = None;
    let mut explicit_ca_bytes: Option<Vec<u8>> = None;

    match ca_trust.source {
        CaTrustSource::CertificateSecretCa => match certificate_secret_ca_material(
            &secret,
            secret_name,
            config.enable_internode_https,
            trust_system_ca,
        ) {
            Ok(Some(material)) => {
                server_ca_key = Some(material.key);
                explicit_ca_bytes = Some(material.bytes);
            }
            Ok(None) => {}
            Err(failure) => return tls_validation_blocked(ctx, tenant, config, failure).await,
        },
        CaTrustSource::SecretRef => {
            let Some(ca_secret_ref) = ca_trust.ca_secret_ref.clone() else {
                return tls_blocked(
                    ctx,
                    tenant,
                    config,
                    Reason::CaBundleMissing,
                    "spec.tls.certManager.caTrust.caSecretRef is required when caTrust.source=SecretRef".to_string(),
                )
                .await;
            };
            let ca_secret = get_secret_or_tls_blocked(
                ctx,
                tenant,
                config,
                namespace,
                &ca_secret_ref.name,
                Reason::CaBundleMissing,
                format!("CA Secret '{}' was not found", ca_secret_ref.name),
            )
            .await?;
            let ca_bytes = require_secret_key(
                ctx,
                tenant,
                config,
                &ca_secret,
                &ca_secret_ref.name,
                &ca_secret_ref.key,
                Reason::CaBundleMissing,
            )
            .await?;
            if let Err(failure) = validate_ca_bundle_bytes(
                &ca_secret_ref.name,
                &ca_secret_ref.key,
                ca_bytes.as_slice(),
            ) {
                return tls_validation_blocked(ctx, tenant, config, failure).await;
            }
            explicit_ca_bytes = Some(ca_bytes);
            explicit_ca = Some(ca_secret_ref);
            explicit_ca_secret = Some(ca_secret);
        }
        CaTrustSource::SystemCa => {}
    }

    let mut client_ca = None;
    let mut client_ca_secret = None;
    let mut client_ca_bytes: Option<Vec<u8>> = None;
    if let Some(client_ca_secret_ref) = ca_trust.client_ca_secret_ref.clone() {
        let ca_secret = get_secret_or_tls_blocked(
            ctx,
            tenant,
            config,
            namespace,
            &client_ca_secret_ref.name,
            Reason::CaBundleMissing,
            format!(
                "Client CA Secret '{}' was not found",
                client_ca_secret_ref.name
            ),
        )
        .await?;
        client_ca_bytes = Some(
            require_secret_key(
                ctx,
                tenant,
                config,
                &ca_secret,
                &client_ca_secret_ref.name,
                &client_ca_secret_ref.key,
                Reason::CaBundleMissing,
            )
            .await?,
        );
        if let Err(failure) = validate_ca_bundle_bytes(
            &client_ca_secret_ref.name,
            &client_ca_secret_ref.key,
            client_ca_bytes.as_deref().unwrap_or_default(),
        ) {
            return tls_validation_blocked(ctx, tenant, config, failure).await;
        }
        client_ca = Some(client_ca_secret_ref);
        client_ca_secret = Some(ca_secret);
    }

    let hash = tls_hash(
        config,
        &secret,
        explicit_ca.as_ref(),
        explicit_ca_bytes.as_deref(),
        client_ca.as_ref(),
        client_ca_bytes.as_deref(),
        trust_system_ca,
    );
    let status = cert_manager_tls_status(
        config,
        secret_name,
        &secret,
        explicit_ca.as_ref().zip(explicit_ca_secret.as_ref()),
        client_ca.as_ref().zip(client_ca_secret.as_ref()),
        &hash,
        certificate_ref,
    );

    Ok(TlsPlan::rollout(
        config.mount_path.clone(),
        hash,
        secret_name.to_string(),
        server_ca_key,
        explicit_ca,
        client_ca,
        config.enable_internode_https,
        trust_system_ca,
        ca_trust.trust_leaf_certificate_as_ca,
        Some(status),
    ))
}

async fn get_server_secret_or_tls_error(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    namespace: &str,
    secret_name: &str,
    managed_certificate: bool,
    certificate_ref: Option<CertificateObjectRef>,
) -> Result<Secret, Error> {
    match ctx.get::<Secret>(secret_name, namespace).await {
        Ok(secret) => Ok(secret),
        Err(error) if context::is_kube_not_found(&error) => {
            let reason = secret_missing_reason(managed_certificate);
            let message = if managed_certificate {
                format!(
                    "TLS Secret '{}' has not been created by cert-manager yet",
                    secret_name
                )
            } else {
                format!("TLS Secret '{}' was not found", secret_name)
            };
            if managed_certificate {
                tls_pending_with_certificate_ref(
                    ctx,
                    tenant,
                    config,
                    reason,
                    message,
                    certificate_ref,
                )
                .await
            } else {
                tls_blocked(ctx, tenant, config, reason, message).await
            }
        }
        Err(error) => {
            let status_error = StatusError::from_context_error(&error);
            patch_status_error(ctx, tenant, &status_error).await;
            Err(error.into())
        }
    }
}

async fn ensure_cert_manager_certificate_crd(ctx: &Context) -> Result<(), context::Error> {
    let api: Api<CustomResourceDefinition> = Api::all(ctx.client.clone());
    api.get(CERT_MANAGER_CERTIFICATE_CRD)
        .await
        .map(|_| ())
        .map_err(|source| context::Error::Kube { source })
}

async fn ensure_cert_manager_issuer(
    ctx: &Context,
    namespace: &str,
    issuer_ref: &CertManagerIssuerRef,
) -> Result<(), context::Error> {
    let resource = issuer_api_resource(issuer_ref);
    if issuer_is_cluster_scoped(issuer_ref) {
        let api: Api<DynamicObject> = Api::all_with(ctx.client.clone(), &resource);
        api.get(&issuer_ref.name)
            .await
            .map(|_| ())
            .map_err(|source| context::Error::Kube { source })
    } else {
        let api: Api<DynamicObject> =
            Api::namespaced_with(ctx.client.clone(), namespace, &resource);
        api.get(&issuer_ref.name)
            .await
            .map(|_| ())
            .map_err(|source| context::Error::Kube { source })
    }
}

async fn apply_cert_manager_certificate(
    ctx: &Context,
    namespace: &str,
    certificate_name: &str,
    certificate: &DynamicObject,
) -> Result<DynamicObject, context::Error> {
    let resource = certificate_api_resource();
    let api: Api<DynamicObject> = Api::namespaced_with(ctx.client.clone(), namespace, &resource);
    api.patch(
        certificate_name,
        &PatchParams::apply("rustfs-operator"),
        &Patch::Apply(certificate),
    )
    .await
    .map_err(|source| context::Error::Kube { source })
}

async fn cert_manager_prerequisite_failed<T>(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    prerequisite: CertManagerPrerequisite,
    error: context::Error,
    missing_message: String,
) -> Result<T, Error> {
    if context::is_kube_not_found(&error) {
        return tls_blocked(
            ctx,
            tenant,
            config,
            missing_cert_manager_prerequisite_reason(prerequisite),
            missing_message,
        )
        .await;
    }

    let status_error = StatusError::from_context_error(&error);
    patch_status_error(ctx, tenant, &status_error).await;
    Err(error.into())
}

fn build_cert_manager_certificate(
    tenant: &Tenant,
    namespace: &str,
    _config: &TlsConfig,
    cert_manager: &CertManagerTlsConfig,
    secret_name: &str,
    certificate_name: &str,
) -> DynamicObject {
    let mut spec = Map::new();
    spec.insert("secretName".to_string(), json!(secret_name));
    if let Some(issuer_ref) = cert_manager.issuer_ref.as_ref() {
        spec.insert("issuerRef".to_string(), issuer_ref_value(issuer_ref));
    }
    if let Some(common_name) = cert_manager
        .common_name
        .as_deref()
        .filter(|common_name| !common_name.is_empty())
    {
        spec.insert("commonName".to_string(), json!(common_name));
    }
    spec.insert(
        "dnsNames".to_string(),
        json!(certificate_dns_names(tenant, namespace, cert_manager)),
    );
    spec.insert(
        "usages".to_string(),
        json!(certificate_usages(cert_manager)),
    );
    if let Some(duration) = cert_manager
        .duration
        .as_deref()
        .filter(|duration| !duration.is_empty())
    {
        spec.insert("duration".to_string(), json!(duration));
    }
    if let Some(renew_before) = cert_manager
        .renew_before
        .as_deref()
        .filter(|renew_before| !renew_before.is_empty())
    {
        spec.insert("renewBefore".to_string(), json!(renew_before));
    }
    if let Some(private_key) = cert_manager.private_key.as_ref() {
        spec.insert("privateKey".to_string(), json!(private_key));
    }
    spec.insert(
        "secretTemplate".to_string(),
        json!({ "labels": tenant.common_labels() }),
    );

    let resource = certificate_api_resource();
    let mut certificate = DynamicObject::new(certificate_name, &resource)
        .within(namespace)
        .data(json!({ "spec": Value::Object(spec) }));
    certificate.metadata.labels = Some(tenant.common_labels());
    certificate.metadata.owner_references = Some(vec![tenant.new_owner_ref()]);
    certificate
}

fn observe_cert_manager_certificate(
    certificate: &DynamicObject,
) -> CertManagerCertificateObservation {
    let ready_condition = certificate
        .data
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|conditions| {
            conditions.iter().find(|condition| {
                condition
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|type_| type_ == "Ready")
            })
        });
    let observed_generation = ready_condition
        .and_then(|condition| condition.get("observedGeneration"))
        .and_then(Value::as_i64)
        .or_else(|| {
            certificate
                .data
                .pointer("/status/observedGeneration")
                .and_then(Value::as_i64)
        });
    let condition_ready = ready_condition
        .and_then(|condition| condition.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| status == "True");
    let generation_current =
        observed_generation_matches(certificate.metadata.generation, observed_generation);
    let ready = condition_ready && generation_current;
    let reason = if condition_ready && !generation_current {
        Some(Reason::ObservedGenerationStale.as_str().to_string())
    } else {
        ready_condition
            .and_then(|condition| condition.get("reason"))
            .and_then(Value::as_str)
            .map(sanitize_status_message)
    };
    let message = if condition_ready && !generation_current {
        Some(format!(
            "Certificate observedGeneration {} is older than metadata.generation {}",
            observed_generation
                .map(|generation| generation.to_string())
                .unwrap_or_else(|| "<missing>".to_string()),
            certificate
                .metadata
                .generation
                .map(|generation| generation.to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        ))
    } else {
        ready_condition
            .and_then(|condition| condition.get("message"))
            .and_then(Value::as_str)
            .map(sanitize_status_message)
    };

    CertManagerCertificateObservation {
        name: certificate
            .metadata
            .name
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string()),
        observed_generation,
        ready,
        reason,
        message,
    }
}

fn certificate_not_ready_message(
    certificate_name: &str,
    observation: &CertManagerCertificateObservation,
) -> String {
    let detail = observation
        .message
        .as_deref()
        .or(observation.reason.as_deref())
        .unwrap_or("Ready condition is not True");
    format!(
        "cert-manager Certificate '{}' is not Ready: {}",
        certificate_name, detail
    )
}

fn observed_generation_matches(generation: Option<i64>, observed_generation: Option<i64>) -> bool {
    match (generation, observed_generation) {
        (Some(generation), Some(observed_generation)) => observed_generation >= generation,
        (Some(_), None) => false,
        _ => true,
    }
}

#[cfg(test)]
fn tls_reason_for_certificate_observation(
    observation: &CertManagerCertificateObservation,
) -> Reason {
    if observation.ready {
        Reason::TlsConfigured
    } else {
        Reason::CertManagerCertificateNotReady
    }
}

fn secret_missing_reason(managed_certificate: bool) -> Reason {
    if managed_certificate {
        Reason::CertificateSecretPending
    } else {
        Reason::CertificateSecretNotFound
    }
}

fn missing_cert_manager_prerequisite_reason(prerequisite: CertManagerPrerequisite) -> Reason {
    match prerequisite {
        CertManagerPrerequisite::CertificateCrd => Reason::CertManagerCrdMissing,
        CertManagerPrerequisite::Issuer | CertManagerPrerequisite::ClusterIssuer => {
            Reason::CertManagerIssuerNotFound
        }
    }
}

fn certificate_name(tenant: &Tenant, cert_manager: &CertManagerTlsConfig) -> String {
    cert_manager
        .certificate_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{}-server-tls", tenant.name()))
}

fn issuer_ref_value(issuer_ref: &CertManagerIssuerRef) -> Value {
    json!({
        "group": if issuer_ref.group.is_empty() { CERT_MANAGER_GROUP } else { issuer_ref.group.as_str() },
        "kind": if issuer_ref.kind.is_empty() { CERT_MANAGER_ISSUER_KIND } else { issuer_ref.kind.as_str() },
        "name": issuer_ref.name,
    })
}

fn certificate_dns_names(
    tenant: &Tenant,
    namespace: &str,
    cert_manager: &CertManagerTlsConfig,
) -> Vec<String> {
    let mut names = BTreeSet::new();
    names.extend(
        cert_manager
            .dns_names
            .iter()
            .filter(|name| !name.is_empty())
            .cloned(),
    );
    if cert_manager.include_generated_dns_names {
        let tenant_name = tenant.name();
        let io_service = format!("{tenant_name}-io");
        let headless_service = tenant.headless_service_name();
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
    names.into_iter().collect()
}

fn certificate_usages(cert_manager: &CertManagerTlsConfig) -> Vec<String> {
    if cert_manager.usages.is_empty() {
        vec!["server auth".to_string()]
    } else {
        cert_manager.usages.clone()
    }
}

fn issuer_prerequisite(issuer_ref: &CertManagerIssuerRef) -> CertManagerPrerequisite {
    if issuer_is_cluster_scoped(issuer_ref) {
        CertManagerPrerequisite::ClusterIssuer
    } else {
        CertManagerPrerequisite::Issuer
    }
}

fn issuer_is_cluster_scoped(issuer_ref: &CertManagerIssuerRef) -> bool {
    issuer_ref.kind == CERT_MANAGER_CLUSTER_ISSUER_KIND
}

fn certificate_api_resource() -> ApiResource {
    ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk(
            CERT_MANAGER_GROUP,
            CERT_MANAGER_VERSION,
            CERT_MANAGER_CERTIFICATE_KIND,
        ),
        CERT_MANAGER_CERTIFICATE_PLURAL,
    )
}

fn issuer_api_resource(issuer_ref: &CertManagerIssuerRef) -> ApiResource {
    if issuer_is_cluster_scoped(issuer_ref) {
        ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk(
                CERT_MANAGER_GROUP,
                CERT_MANAGER_VERSION,
                CERT_MANAGER_CLUSTER_ISSUER_KIND,
            ),
            CERT_MANAGER_CLUSTER_ISSUER_PLURAL,
        )
    } else {
        ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk(
                CERT_MANAGER_GROUP,
                CERT_MANAGER_VERSION,
                CERT_MANAGER_ISSUER_KIND,
            ),
            CERT_MANAGER_ISSUER_PLURAL,
        )
    }
}

fn sanitize_status_message(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let truncated = chars
        .by_ref()
        .take(STATUS_MESSAGE_LIMIT)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

async fn get_secret_or_tls_blocked(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    namespace: &str,
    secret_name: &str,
    missing_reason: Reason,
    missing_message: String,
) -> Result<Secret, Error> {
    match ctx.get::<Secret>(secret_name, namespace).await {
        Ok(secret) => Ok(secret),
        Err(error) if context::is_kube_not_found(&error) => {
            tls_blocked(ctx, tenant, config, missing_reason, missing_message).await
        }
        Err(error) => {
            let status_error = StatusError::from_context_error(&error);
            patch_status_error(ctx, tenant, &status_error).await;
            Err(error.into())
        }
    }
}

async fn require_secret_key(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    secret: &Secret,
    secret_name: &str,
    key: &str,
    missing_reason: Reason,
) -> Result<Vec<u8>, Error> {
    match require_secret_key_bytes(secret, secret_name, key, missing_reason) {
        Ok(bytes) => Ok(bytes.to_vec()),
        Err(failure) => tls_validation_blocked(ctx, tenant, config, failure).await,
    }
}

fn require_secret_key_bytes<'a>(
    secret: &'a Secret,
    secret_name: &str,
    key: &str,
    missing_reason: Reason,
) -> Result<&'a [u8], TlsValidationFailure> {
    secret_bytes(secret, key).ok_or_else(|| TlsValidationFailure {
        reason: missing_reason,
        message: format!(
            "TLS Secret '{}' is missing required key '{}'",
            secret_name, key
        ),
    })
}

fn validate_tls_secret_type(
    secret: &Secret,
    secret_name: &str,
    expected_type: Option<&str>,
) -> Result<(), TlsValidationFailure> {
    let actual_type = secret.type_.as_deref().unwrap_or("");
    if let Some(expected_type) = expected_type {
        if actual_type == expected_type {
            return Ok(());
        }
        return Err(TlsValidationFailure {
            reason: Reason::CertificateSecretInvalidType,
            message: format!(
                "TLS Secret '{}' has type '{}', expected '{}'",
                secret_name, actual_type, expected_type
            ),
        });
    }

    if supported_tls_secret_type(actual_type) {
        return Ok(());
    }

    Err(TlsValidationFailure {
        reason: Reason::CertificateSecretInvalidType,
        message: format!(
            "TLS Secret '{}' has type '{}', expected one of: {}, {}, {}",
            secret_name,
            actual_type,
            KUBERNETES_TLS_SECRET_TYPE,
            CERT_MANAGER_V1_SECRET_TYPE,
            CERT_MANAGER_V1ALPHA2_SECRET_TYPE
        ),
    })
}

fn supported_tls_secret_type(secret_type: &str) -> bool {
    matches!(
        secret_type,
        KUBERNETES_TLS_SECRET_TYPE
            | CERT_MANAGER_V1_SECRET_TYPE
            | CERT_MANAGER_V1ALPHA2_SECRET_TYPE
    )
}

fn validate_tls_secret_san_match(
    secret_name: &str,
    cert_bytes: &[u8],
    expected_dns_names: &[String],
) -> Result<(), TlsValidationFailure> {
    if expected_dns_names.is_empty() {
        return Ok(());
    }

    let certs = rustls_pemfile::certs(&mut Cursor::new(cert_bytes))
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .map_err(|_| TlsValidationFailure {
            reason: Reason::CertificateInvalid,
            message: format!(
                "TLS certificate in Secret '{}' key '{}' must contain a valid PEM certificate",
                secret_name, TLS_CERT_KEY
            ),
        })?;
    let cert_der = certs.first().ok_or_else(|| TlsValidationFailure {
        reason: Reason::CertificateInvalid,
        message: format!(
            "TLS certificate in Secret '{}' key '{}' must contain at least one valid PEM certificate",
            secret_name, TLS_CERT_KEY
        ),
    })?;
    let cert = webpki::EndEntityCert::try_from(cert_der).map_err(|_| TlsValidationFailure {
        reason: Reason::CertificateInvalid,
        message: format!(
            "TLS certificate in Secret '{}' key '{}' must be a valid X.509 end-entity certificate",
            secret_name, TLS_CERT_KEY
        ),
    })?;

    let mut missing = Vec::new();
    for dns_name in expected_dns_names {
        let server_name =
            ServerName::try_from(dns_name.as_str()).map_err(|_| TlsValidationFailure {
                reason: Reason::CertificateSanMismatch,
                message: format!("required TLS DNS name '{dns_name}' is invalid"),
            })?;
        if cert.verify_is_valid_for_subject_name(&server_name).is_err() {
            missing.push(dns_name.clone());
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(TlsValidationFailure {
            reason: Reason::CertificateSanMismatch,
            message: format!(
                "TLS certificate in Secret '{}' key '{}' does not cover required DNS names: {}",
                secret_name,
                TLS_CERT_KEY,
                missing.join(", ")
            ),
        })
    }
}

fn certificate_secret_ca_material(
    secret: &Secret,
    secret_name: &str,
    enable_internode_https: bool,
    trust_system_ca: bool,
) -> Result<Option<ServerCaMaterial>, TlsValidationFailure> {
    if let Some(ca_bytes) = secret_bytes(secret, CA_CERT_KEY) {
        validate_ca_bundle_bytes(secret_name, CA_CERT_KEY, ca_bytes)?;
        return Ok(Some(ServerCaMaterial {
            key: CA_CERT_KEY.to_string(),
            bytes: ca_bytes.to_vec(),
        }));
    }

    if enable_internode_https && !trust_system_ca {
        return Err(TlsValidationFailure {
            reason: Reason::CaBundleMissing,
            message: format!(
                "TLS Secret '{}' is missing '{}' while spec.tls.enableInternodeHttps=true and trustSystemCa is false",
                secret_name, CA_CERT_KEY
            ),
        });
    }

    Ok(None)
}

fn validate_ca_bundle_bytes(
    secret_name: &str,
    key: &str,
    bytes: &[u8],
) -> Result<(), TlsValidationFailure> {
    let parsed = rustls_pemfile::certs(&mut Cursor::new(bytes)).collect::<Result<Vec<_>, _>>();
    match parsed {
        Ok(certs) if !certs.is_empty() => Ok(()),
        Ok(_) | Err(_) => Err(TlsValidationFailure {
            reason: Reason::CaBundleInvalid,
            message: format!(
                "CA bundle in Secret '{}' key '{}' must contain at least one valid PEM certificate",
                secret_name, key
            ),
        }),
    }
}

async fn tls_validation_blocked<T>(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    failure: TlsValidationFailure,
) -> Result<T, Error> {
    tls_blocked(ctx, tenant, config, failure.reason, failure.message).await
}

async fn tls_blocked<T>(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    reason: Reason,
    message: String,
) -> Result<T, Error> {
    patch_tls_error(ctx, tenant, config, reason, &message, true).await?;
    Err(Error::TlsBlocked {
        reason: reason.as_str().to_string(),
        message,
    })
}

async fn tls_pending_with_certificate_ref<T>(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    reason: Reason,
    message: String,
    certificate_ref: Option<CertificateObjectRef>,
) -> Result<T, Error> {
    patch_tls_error_with_certificate_ref(
        ctx,
        tenant,
        config,
        reason,
        &message,
        false,
        certificate_ref,
    )
    .await?;
    Err(Error::TlsPending {
        reason: reason.as_str().to_string(),
        message,
    })
}

async fn patch_tls_error(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    reason: Reason,
    message: &str,
    blocked: bool,
) -> Result<(), Error> {
    patch_tls_error_with_certificate_ref(ctx, tenant, config, reason, message, blocked, None).await
}

async fn patch_tls_error_with_certificate_ref(
    ctx: &Context,
    tenant: &Tenant,
    config: &TlsConfig,
    reason: Reason,
    message: &str,
    blocked: bool,
    certificate_ref: Option<CertificateObjectRef>,
) -> Result<(), Error> {
    let status_error = if blocked {
        StatusError::tls_blocked(reason, message.to_string())
    } else {
        StatusError::tls_reconciling(reason, message.to_string())
    };
    let mut builder = StatusBuilder::from_tenant(tenant);
    builder.set_tls_status(error_tls_status_with_certificate_ref(
        config,
        reason,
        message,
        certificate_ref,
    ));
    builder.mark_error(&status_error);
    let status = builder.build();
    patch_status_and_record(
        ctx,
        tenant,
        status,
        status_error.condition_type,
        status_error.reason,
        status_error.event_type,
        &status_error.safe_message,
    )
    .await
}

fn cert_manager_tls_status(
    config: &TlsConfig,
    secret_name: &str,
    secret: &Secret,
    explicit_ca: Option<(&SecretKeyReference, &Secret)>,
    client_ca: Option<(&SecretKeyReference, &Secret)>,
    hash: &str,
    certificate_ref: Option<CertificateObjectRef>,
) -> TlsCertificateStatus {
    let ca_trust = config.ca_trust();
    TlsCertificateStatus {
        mode: tls_mode_name(config.mode).to_string(),
        ready: true,
        managed_certificate: config
            .cert_manager
            .as_ref()
            .map(|cert_manager| cert_manager.manage_certificate),
        rotation_strategy: Some(rotation_strategy_name(config.rotation_strategy).to_string()),
        mount_path: Some(config.mount_path.clone()),
        certificate_ref,
        server_secret_ref: Some(SecretStatusRef {
            name: secret_name.to_string(),
            key: None,
            resource_version: secret.metadata.resource_version.clone(),
        }),
        ca_secret_ref: ca_status_ref(secret_name, secret, explicit_ca),
        client_ca_secret_ref: client_ca.map(|(secret_ref, ca_secret)| SecretStatusRef {
            name: secret_ref.name.clone(),
            key: Some(secret_ref.key.clone()),
            resource_version: ca_secret.metadata.resource_version.clone(),
        }),
        observed_hash: Some(hash.to_string()),
        dns_names: config
            .cert_manager
            .as_ref()
            .map(|cert_manager| cert_manager.dns_names.clone())
            .unwrap_or_default(),
        trust_source: Some(ca_trust_source_name(ca_trust.source).to_string()),
        last_validated_time: Some(
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ),
        ..Default::default()
    }
}

fn ca_status_ref(
    secret_name: &str,
    secret: &Secret,
    explicit_ca: Option<(&SecretKeyReference, &Secret)>,
) -> Option<SecretStatusRef> {
    if let Some((secret_ref, ca_secret)) = explicit_ca {
        return Some(SecretStatusRef {
            name: secret_ref.name.clone(),
            key: Some(secret_ref.key.clone()),
            resource_version: ca_secret.metadata.resource_version.clone(),
        });
    }
    secret_bytes(secret, CA_CERT_KEY).map(|_| SecretStatusRef {
        name: secret_name.to_string(),
        key: Some(CA_CERT_KEY.to_string()),
        resource_version: secret.metadata.resource_version.clone(),
    })
}

#[cfg(test)]
fn error_tls_status(config: &TlsConfig, reason: Reason, message: &str) -> TlsCertificateStatus {
    error_tls_status_with_certificate_ref(config, reason, message, None)
}

fn error_tls_status_with_certificate_ref(
    config: &TlsConfig,
    reason: Reason,
    message: &str,
    certificate_ref: Option<CertificateObjectRef>,
) -> TlsCertificateStatus {
    TlsCertificateStatus {
        mode: tls_mode_name(config.mode).to_string(),
        ready: false,
        managed_certificate: config
            .cert_manager
            .as_ref()
            .map(|cert_manager| cert_manager.manage_certificate),
        rotation_strategy: Some(rotation_strategy_name(config.rotation_strategy).to_string()),
        mount_path: Some(config.mount_path.clone()),
        certificate_ref,
        trust_source: Some(ca_trust_source_name(config.ca_trust().source).to_string()),
        last_error_reason: Some(reason.as_str().to_string()),
        last_error_message: Some(message.to_string()),
        ..Default::default()
    }
}

fn tls_hash(
    config: &TlsConfig,
    secret: &Secret,
    explicit_ca: Option<&SecretKeyReference>,
    explicit_ca_bytes: Option<&[u8]>,
    client_ca: Option<&SecretKeyReference>,
    client_ca_bytes: Option<&[u8]>,
    trust_system_ca: bool,
) -> String {
    let mut hasher = Sha256::new();
    hash_str(&mut hasher, "mountPath", &config.mount_path);
    hash_str(
        &mut hasher,
        "rotationStrategy",
        rotation_strategy_name(config.rotation_strategy),
    );
    hash_str(
        &mut hasher,
        "enableInternodeHttps",
        if config.enable_internode_https {
            "true"
        } else {
            "false"
        },
    );
    hash_str(
        &mut hasher,
        "trustSystemCa",
        if trust_system_ca { "true" } else { "false" },
    );
    hash_str(
        &mut hasher,
        "serverSecret.resourceVersion",
        secret.metadata.resource_version.as_deref().unwrap_or(""),
    );
    hash_bytes(&mut hasher, "tls.crt", secret_bytes(secret, TLS_CERT_KEY));
    hash_bytes(
        &mut hasher,
        "secret.ca.crt",
        secret_bytes(secret, CA_CERT_KEY),
    );
    if let Some(secret_ref) = explicit_ca {
        hash_str(&mut hasher, "explicitCa.name", &secret_ref.name);
        hash_str(&mut hasher, "explicitCa.key", &secret_ref.key);
    }
    hash_bytes(&mut hasher, "explicitCa.bytes", explicit_ca_bytes);
    if let Some(secret_ref) = client_ca {
        hash_str(&mut hasher, "clientCa.name", &secret_ref.name);
        hash_str(&mut hasher, "clientCa.key", &secret_ref.key);
    }
    hash_bytes(&mut hasher, "clientCa.bytes", client_ca_bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn hash_str(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(value.len().to_le_bytes());
    hasher.update(value.as_bytes());
    hasher.update([0]);
}

fn hash_bytes(hasher: &mut Sha256, label: &str, value: Option<&[u8]>) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    match value {
        Some(bytes) => {
            hasher.update(bytes.len().to_le_bytes());
            hasher.update(bytes);
        }
        None => hasher.update(0usize.to_le_bytes()),
    }
    hasher.update([0]);
}

fn secret_bytes<'a>(secret: &'a Secret, key: &str) -> Option<&'a [u8]> {
    secret
        .data
        .as_ref()?
        .get(key)
        .map(|bytes| bytes.0.as_slice())
}

const fn tls_mode_name(mode: TlsMode) -> &'static str {
    match mode {
        TlsMode::Disabled => "Disabled",
        TlsMode::External => "External",
        TlsMode::CertManager => "CertManager",
    }
}

const fn rotation_strategy_name(strategy: TlsRotationStrategy) -> &'static str {
    match strategy {
        TlsRotationStrategy::Rollout => "Rollout",
        TlsRotationStrategy::HotReload => "HotReload",
    }
}

const fn ca_trust_source_name(source: CaTrustSource) -> &'static str {
    match source {
        CaTrustSource::CertificateSecretCa => "CertificateSecretCa",
        CaTrustSource::SecretRef => "SecretRef",
        CaTrustSource::SystemCa => "SystemCa",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::tls::{
        CaTrustConfig, CertManagerPrivateKeyConfig, CertManagerTlsConfig,
    };
    use k8s_openapi::ByteString;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::CustomResourceExt;
    use std::collections::BTreeMap;

    const PUBLIC_CERT_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUD4D7ObFcJ5PEZwq2t/cmrTbzcU0wDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI1MTExMDA3NDQwNVoXDTI2MTEx\nMDA3NDQwNVowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAsnrreaQGztdaTppY7p1ExoDU7FpYjk8MalWs9xIioHTe\ndpDlZmEWak0Q80qTvc+x6GT8VD/pLYqg6B2mot8I+Uv44GUmpPD/+WDxVbjvwL2b\nfvcNGEniqKJUOy2za98WcmI8EoILwbmYy7cZslf6b3D0xuDsmovYJgtjNeziV6ie\nLQfbWWXhAipYhUwaBAdUSQS+BWPPdYFG4LEE/8+BqmYdGU7ujIFlqSU89ZMfpZS4\npVRoEy16fs5O0UkbP1l63Q0qBLrLXjWw874dV8wC2p9iuVwofpDZRGhfYFaviZHb\nMHdUBRUughU4vvTknAGwMzbrIH+eTp7aKrGKWb7ozQIDAQABo1MwUTAdBgNVHQ4E\nFgQUGSE2L3XLbuxlA1Q0iX65aVGKzl4wHwYDVR0jBBgwFoAUGSE2L3XLbuxlA1Q0\niX65aVGKzl4wDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAGHwM\nSYFN1/9ZlriVaJEpSvGlfeDvN5ipXqf0s1Ykux9rsTYchn7tcA6zhWqZUimwy/jO\nI7jLfBNa3r5HT1uX3/RlMs6dMIO4h3vkSWjQ3QaGiuXh6U+erbkaeETtrw9b40ta\nDsj2rruE3Z11JV0y5fGcvXjXMFV7XsFQjNXF5TlXu4OUvfMeo9h4IbPmNQtq+g+t\nnx0ZBloqo+punQVjHjovoQUWlrOOL5ZRZl1vLqqhHfw54a9weCXY8XJNnxWN0l0C\nKzht0TgbidDlWKBsk/CMTY8zpYrfVyPhnjNCeFGFG0DzrsehCgpEiEZ6vlylei7c\nRfKUdp4DXmUZBDzeQw==\n-----END CERTIFICATE-----\n";
    const CERT_WITH_PEER_SANS_PEM: &[u8] = br#"-----BEGIN CERTIFICATE-----
MIIDoDCCAoigAwIBAgIUeB45TQucDL0Dm5Jn7CyeIWTRkQUwDQYJKoZIhvcNAQEL
BQAwEzERMA8GA1UEAwwIdGVuYW50LWEwHhcNMjYwNTEzMDkyODA4WhcNMjYwNTE0
MDkyODA4WjATMREwDwYDVQQDDAh0ZW5hbnQtYTCCASIwDQYJKoZIhvcNAQEBBQAD
ggEPADCCAQoCggEBAKPvXLnfHwjzz1EsnINmuJfBGcUf6dFgw+seTNXbBDEfQ/+R
tpmTa1TO5Eqo9utDk7TZx9GTGr1vFArOP8MBEJ0qdx5YCvoWVoexVc1FhsFSe9Mv
+EGpV9RGniIfMmkVj8BHR+rmTopRoHEYnsDL9wm9D47GNbSuHuMHG4qlkLY4270a
QDMaTGgaH0iLN63ISl6mf/ca55kWqrcCmERNvpfA7EywYm8wwyPf8fURjfg+nKGL
CJ2roZrpXJnUhQmAMF0RDx+Q02RAgkJXClO59Qk9vm7QpnIKwglUPuYK/LJ3bSA7
4COHbYZxDatBedyBZFDlUZw0kNnQo5+JJSPfKAUCAwEAAaOB6zCB6DAdBgNVHQ4E
FgQUUGaN5hB6CJ9Ds0s9zlG1R/YhiQ4wHwYDVR0jBBgwFoAUUGaN5hB6CJ9Ds0s9
zlG1R/YhiQ4wDwYDVR0TAQH/BAUwAwEB/zCBlAYDVR0RBIGMMIGJggh0ZW5hbnQt
YYIJbG9jYWxob3N0gjh0ZW5hbnQtYS1wcmltYXJ5LTAudGVuYW50LWEtaGwuc3Rv
cmFnZS5zdmMuY2x1c3Rlci5sb2NhbII4dGVuYW50LWEtcHJpbWFyeS0xLnRlbmFu
dC1hLWhsLnN0b3JhZ2Uuc3ZjLmNsdXN0ZXIubG9jYWwwDQYJKoZIhvcNAQELBQAD
ggEBAAqx762x484bIVcdQXE1dO6GhFPS8OoZWBxFAURnfep8H9lwVgcoXLgglpjM
dfD9EaPNjXpixDX/SK6nI/rCVnbHXFk1nEBpWBHC+XBPIj/J3nUeuhEGJPjif0KX
wjIUfC3RADGlA7AdgLeFJ21FOwtmjdxUsD2aZ1gqOm3flsyBxuIFozZEi1ZTlBes
90l8P6bqksl/3t9ssTdIF5O/mtKJqy8fBXsE2yazKO6dl1Mt7Zn4Lw6OQraaxNWT
S2+cuFyHX+xgTPNxiG9zUDrgtXds/63ePISjIADAUvsmI97k96E6jdcgB9MmWdJj
84SYe6DQkgSslVKrEZIaVd/q8t8=
-----END CERTIFICATE-----
"#;

    #[test]
    fn tenant_crd_schema_types_cert_manager_private_key() {
        let crd = serde_json::to_value(Tenant::crd()).expect("tenant CRD serializes to JSON");
        let private_key_schema = crd
            .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/tls/properties/certManager/properties/privateKey")
            .expect("spec.tls.certManager.privateKey schema exists");
        let properties = private_key_schema
            .pointer("/properties")
            .and_then(Value::as_object)
            .expect("privateKey schema has typed properties");

        assert_eq!(
            private_key_schema.pointer("/type").and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            properties
                .get("algorithm")
                .and_then(|schema| schema.pointer("/type"))
                .and_then(Value::as_str),
            Some("string")
        );
        assert_eq!(
            properties
                .get("encoding")
                .and_then(|schema| schema.pointer("/type"))
                .and_then(Value::as_str),
            Some("string")
        );
        assert_eq!(
            properties
                .get("rotationPolicy")
                .and_then(|schema| schema.pointer("/type"))
                .and_then(Value::as_str),
            Some("string")
        );
        assert_eq!(
            properties
                .get("size")
                .and_then(|schema| schema.pointer("/type"))
                .and_then(Value::as_str),
            Some("integer")
        );
    }

    #[test]
    fn default_secret_type_rejects_unconventional_server_secret() {
        let secret = tls_secret("server-tls", "7", Some("Opaque"), true, true, None);

        let error = validate_tls_secret_type(&secret, "server-tls", None);

        assert_validation_reason(error, Reason::CertificateSecretInvalidType);
    }

    #[test]
    fn default_secret_type_accepts_kubernetes_and_cert_manager_tls_types() {
        for secret_type in [
            "kubernetes.io/tls",
            "cert-manager.io/v1",
            "cert-manager.io/v1alpha2",
        ] {
            let secret = tls_secret("server-tls", "7", Some(secret_type), true, true, None);

            assert!(validate_tls_secret_type(&secret, "server-tls", None).is_ok());
        }
    }

    #[test]
    fn explicit_secret_type_rejects_supported_but_wrong_type() {
        let secret = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            None,
        );

        let error = validate_tls_secret_type(&secret, "server-tls", Some("cert-manager.io/v1"));

        assert_validation_reason(error, Reason::CertificateSecretInvalidType);
    }

    #[test]
    fn missing_server_tls_key_maps_to_certificate_secret_missing_key() {
        let secret = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            false,
            true,
            None,
        );

        let error = require_secret_key_bytes(
            &secret,
            "server-tls",
            TLS_KEY_KEY,
            Reason::CertificateSecretMissingKey,
        );

        assert_validation_reason(error, Reason::CertificateSecretMissingKey);
    }

    #[test]
    fn missing_ca_key_maps_to_ca_bundle_missing() {
        let secret = tls_secret("server-ca", "7", Some("Opaque"), false, true, None);

        let error =
            require_secret_key_bytes(&secret, "server-ca", CA_CERT_KEY, Reason::CaBundleMissing);

        assert_validation_reason(error, Reason::CaBundleMissing);
    }

    #[test]
    fn internode_https_allows_system_ca_without_secret_ca_bundle() {
        let secret = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            None,
        );

        let ca = certificate_secret_ca_material(&secret, "server-tls", true, true);

        assert!(ca.is_ok());
        assert!(matches!(ca, Ok(None)));
    }

    #[test]
    fn internode_https_requires_ca_bundle_without_system_ca() {
        let secret = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            None,
        );

        let error = certificate_secret_ca_material(&secret, "server-tls", true, false);

        assert_validation_reason(error, Reason::CaBundleMissing);
    }

    #[test]
    fn invalid_ca_bundle_maps_to_ca_bundle_invalid() {
        let secret = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            Some(b"not a pem certificate"),
        );

        let error = certificate_secret_ca_material(&secret, "server-tls", false, false);

        assert_validation_reason(error, Reason::CaBundleInvalid);
    }

    #[test]
    fn hot_reload_remains_explicitly_unsupported_in_tls_status() {
        let config = TlsConfig {
            mode: TlsMode::CertManager,
            rotation_strategy: TlsRotationStrategy::HotReload,
            ..Default::default()
        };

        let status = error_tls_status(
            &config,
            Reason::TlsHotReloadUnsupported,
            "hot reload unsupported",
        );

        assert!(!status.ready);
        assert_eq!(
            status.last_error_reason.as_deref(),
            Some("TlsHotReloadUnsupported")
        );
        assert_eq!(status.rotation_strategy.as_deref(), Some("HotReload"));
    }

    #[test]
    fn tls_hash_uses_resource_version_public_cert_and_ca_not_private_key() {
        let config = TlsConfig::default();
        let first = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            Some(PUBLIC_CERT_PEM),
        );
        let changed_private_key = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            false,
            Some(PUBLIC_CERT_PEM),
        );
        let changed_resource_version = tls_secret(
            "server-tls",
            "8",
            Some("kubernetes.io/tls"),
            true,
            false,
            Some(PUBLIC_CERT_PEM),
        );
        let changed_ca = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            Some(b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----\n"),
        );

        let baseline = tls_hash(&config, &first, None, None, None, None, false);
        let private_key_changed =
            tls_hash(&config, &changed_private_key, None, None, None, None, false);
        let resource_version_changed = tls_hash(
            &config,
            &changed_resource_version,
            None,
            None,
            None,
            None,
            false,
        );
        let ca_changed = tls_hash(&config, &changed_ca, None, None, None, None, false);

        assert_eq!(baseline, private_key_changed);
        assert_ne!(baseline, resource_version_changed);
        assert_ne!(baseline, ca_changed);
    }

    #[test]
    fn require_san_match_accepts_certificate_covering_required_peer_dns_names() {
        let expected_dns_names = vec![
            "tenant-a-primary-0.tenant-a-hl.storage.svc.cluster.local".to_string(),
            "tenant-a-primary-1.tenant-a-hl.storage.svc.cluster.local".to_string(),
            "localhost".to_string(),
        ];

        assert_eq!(
            validate_tls_secret_san_match(
                "server-tls",
                CERT_WITH_PEER_SANS_PEM,
                &expected_dns_names
            ),
            Ok(())
        );
    }

    #[test]
    fn require_san_match_rejects_certificate_missing_required_peer_dns_names() {
        let expected_dns_names = vec![
            "tenant-a-primary-0.tenant-a-hl.storage.svc.cluster.local".to_string(),
            "tenant-a-primary-2.tenant-a-hl.storage.svc.cluster.local".to_string(),
        ];

        let failure = validate_tls_secret_san_match(
            "server-tls",
            CERT_WITH_PEER_SANS_PEM,
            &expected_dns_names,
        )
        .expect_err("missing peer DNS should fail SAN validation");

        assert_eq!(failure.reason, Reason::CertificateSanMismatch);
        assert!(
            failure
                .message
                .contains("tenant-a-primary-2.tenant-a-hl.storage.svc.cluster.local"),
            "message should name missing peer DNS: {}",
            failure.message
        );
        assert!(
            !failure.message.contains("BEGIN CERTIFICATE"),
            "SAN mismatch message must not expose certificate material: {}",
            failure.message
        );
    }

    #[test]
    fn tls_status_records_explicit_ca_and_client_ca_resource_versions() {
        let config = TlsConfig {
            mode: TlsMode::CertManager,
            cert_manager: Some(CertManagerTlsConfig {
                secret_name: Some("server-tls".to_string()),
                ca_trust: Some(CaTrustConfig {
                    source: CaTrustSource::SecretRef,
                    ca_secret_ref: Some(secret_ref("server-ca", "ca.crt")),
                    client_ca_secret_ref: Some(secret_ref("client-ca", "client_ca.crt")),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let server = tls_secret(
            "server-tls",
            "7",
            Some("kubernetes.io/tls"),
            true,
            true,
            None,
        );
        let ca = tls_secret(
            "server-ca",
            "11",
            Some("Opaque"),
            false,
            false,
            Some(PUBLIC_CERT_PEM),
        );
        let client_ca = tls_secret(
            "client-ca",
            "13",
            Some("Opaque"),
            false,
            false,
            Some(PUBLIC_CERT_PEM),
        );

        let status = cert_manager_tls_status(
            &config,
            "server-tls",
            &server,
            Some((&secret_ref("server-ca", "ca.crt"), &ca)),
            Some((&secret_ref("client-ca", "client_ca.crt"), &client_ca)),
            "sha256:test",
            None,
        );

        assert_eq!(
            status
                .ca_secret_ref
                .as_ref()
                .and_then(|secret| secret.resource_version.as_deref()),
            Some("11")
        );
        assert_eq!(
            status
                .client_ca_secret_ref
                .as_ref()
                .and_then(|secret| secret.resource_version.as_deref()),
            Some("13")
        );
    }

    #[test]
    fn managed_certificate_manifest_renders_spec_owner_and_generated_dns() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.name = Some("tenant-a".to_string());
        tenant.metadata.namespace = Some("storage".to_string());
        tenant.spec.pools[0].servers = 2;
        let config = TlsConfig {
            mode: TlsMode::CertManager,
            cert_manager: Some(CertManagerTlsConfig {
                manage_certificate: true,
                certificate_name: Some("tenant-a-server".to_string()),
                secret_name: Some("tenant-a-server-tls".to_string()),
                issuer_ref: Some(CertManagerIssuerRef {
                    group: "cert-manager.io".to_string(),
                    kind: "Issuer".to_string(),
                    name: "rustfs-issuer".to_string(),
                }),
                common_name: Some("tenant-a-io.storage.svc".to_string()),
                dns_names: vec!["custom.storage.svc".to_string()],
                duration: Some("2160h".to_string()),
                renew_before: Some("360h".to_string()),
                private_key: Some(CertManagerPrivateKeyConfig {
                    algorithm: Some("RSA".to_string()),
                    size: Some(2048),
                    ..Default::default()
                }),
                usages: vec!["server auth".to_string(), "client auth".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let Some(cert_manager) = config.cert_manager.as_ref() else {
            panic!("test config must include cert-manager settings");
        };

        let certificate = build_cert_manager_certificate(
            &tenant,
            "storage",
            &config,
            cert_manager,
            "tenant-a-server-tls",
            "tenant-a-server",
        );
        let dns_names = certificate
            .data
            .pointer("/spec/dnsNames")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            });

        assert_eq!(
            certificate.metadata.name.as_deref(),
            Some("tenant-a-server")
        );
        assert_eq!(certificate.metadata.namespace.as_deref(), Some("storage"));
        assert_eq!(
            certificate
                .metadata
                .owner_references
                .as_ref()
                .and_then(|owners| owners.first())
                .map(|owner| (owner.kind.as_str(), owner.name.as_str(), owner.controller)),
            Some(("Tenant", "tenant-a", Some(true)))
        );
        assert_eq!(
            certificate.data.pointer("/spec/secretName"),
            Some(&serde_json::json!("tenant-a-server-tls"))
        );
        assert_eq!(
            certificate
                .data
                .pointer("/spec/secretTemplate/labels/rustfs.tenant"),
            Some(&serde_json::json!("tenant-a"))
        );
        assert_eq!(
            certificate
                .data
                .pointer("/spec/secretTemplate/labels/app.kubernetes.io~1managed-by"),
            Some(&serde_json::json!("rustfs-operator"))
        );
        assert_eq!(
            certificate.data.pointer("/spec/issuerRef/name"),
            Some(&serde_json::json!("rustfs-issuer"))
        );
        assert_eq!(
            certificate.data.pointer("/spec/duration"),
            Some(&serde_json::json!("2160h"))
        );
        assert_eq!(
            certificate.data.pointer("/spec/renewBefore"),
            Some(&serde_json::json!("360h"))
        );
        assert_eq!(
            certificate.data.pointer("/spec/privateKey/algorithm"),
            Some(&serde_json::json!("RSA"))
        );
        assert_eq!(
            dns_names,
            Some(vec![
                "custom.storage.svc".to_string(),
                "tenant-a-hl.storage.svc".to_string(),
                "tenant-a-hl.storage.svc.cluster.local".to_string(),
                "tenant-a-io.storage.svc".to_string(),
                "tenant-a-io.storage.svc.cluster.local".to_string(),
                "tenant-a-pool-0-0.tenant-a-hl.storage.svc.cluster.local".to_string(),
                "tenant-a-pool-0-1.tenant-a-hl.storage.svc.cluster.local".to_string(),
            ])
        );
    }

    #[test]
    fn certificate_observation_requires_ready_condition_for_current_generation() {
        let ready = certificate_object(
            "tenant-a-server",
            Some(3),
            serde_json::json!({
                "status": {
                    "observedGeneration": 3,
                    "conditions": [{"type": "Ready", "status": "True", "reason": "Ready", "message": "Certificate is up to date"}]
                }
            }),
        );
        let stale = certificate_object(
            "tenant-a-server",
            Some(4),
            serde_json::json!({
                "status": {
                    "observedGeneration": 3,
                    "conditions": [{"type": "Ready", "status": "True", "reason": "Ready", "message": "Old revision"}]
                }
            }),
        );

        let ready_observation = observe_cert_manager_certificate(&ready);
        let stale_observation = observe_cert_manager_certificate(&stale);

        assert!(ready_observation.ready);
        assert_eq!(ready_observation.observed_generation, Some(3));
        assert_eq!(ready_observation.reason.as_deref(), Some("Ready"));
        assert!(!stale_observation.ready);
        assert_eq!(stale_observation.observed_generation, Some(3));
        assert_eq!(
            stale_observation.reason.as_deref(),
            Some("ObservedGenerationStale")
        );
    }

    #[test]
    fn certificate_observation_uses_ready_condition_observed_generation() {
        let ready = certificate_object(
            "tenant-a-server",
            Some(3),
            serde_json::json!({
                "status": {
                    "conditions": [{
                        "type": "Ready",
                        "status": "True",
                        "observedGeneration": 3,
                        "reason": "Ready",
                        "message": "Certificate is up to date"
                    }]
                }
            }),
        );

        let observation = observe_cert_manager_certificate(&ready);

        assert!(observation.ready);
        assert_eq!(observation.observed_generation, Some(3));
        assert_eq!(observation.reason.as_deref(), Some("Ready"));
    }

    #[test]
    fn certificate_observation_marks_stale_ready_condition_observed_generation() {
        let stale = certificate_object(
            "tenant-a-server",
            Some(4),
            serde_json::json!({
                "status": {
                    "conditions": [{
                        "type": "Ready",
                        "status": "True",
                        "observedGeneration": 3,
                        "reason": "Ready",
                        "message": "Old revision"
                    }]
                }
            }),
        );

        let observation = observe_cert_manager_certificate(&stale);

        assert!(!observation.ready);
        assert_eq!(observation.observed_generation, Some(3));
        assert_eq!(
            observation.reason.as_deref(),
            Some("ObservedGenerationStale")
        );
    }

    #[test]
    fn pending_certificate_and_managed_secret_missing_map_to_reconciling_reasons() {
        let pending = certificate_object(
            "tenant-a-server",
            Some(3),
            serde_json::json!({
                "status": {
                    "observedGeneration": 3,
                    "conditions": [{"type": "Ready", "status": "False", "reason": "DoesNotExist", "message": "Secret is not available\nretrying"}]
                }
            }),
        );

        let observation = observe_cert_manager_certificate(&pending);

        assert!(!observation.ready);
        assert_eq!(
            tls_reason_for_certificate_observation(&observation),
            Reason::CertManagerCertificateNotReady
        );
        assert_eq!(
            observation.message.as_deref(),
            Some("Secret is not available retrying")
        );
        assert_eq!(
            secret_missing_reason(true),
            Reason::CertificateSecretPending
        );
        assert_eq!(
            secret_missing_reason(false),
            Reason::CertificateSecretNotFound
        );
    }

    #[test]
    fn cert_manager_prerequisite_missing_resources_map_to_stable_reasons() {
        assert_eq!(
            missing_cert_manager_prerequisite_reason(CertManagerPrerequisite::CertificateCrd),
            Reason::CertManagerCrdMissing
        );
        assert_eq!(
            missing_cert_manager_prerequisite_reason(CertManagerPrerequisite::Issuer),
            Reason::CertManagerIssuerNotFound
        );
        assert_eq!(
            missing_cert_manager_prerequisite_reason(CertManagerPrerequisite::ClusterIssuer),
            Reason::CertManagerIssuerNotFound
        );
    }

    fn assert_validation_reason<T>(result: Result<T, TlsValidationFailure>, reason: Reason) {
        assert!(
            matches!(result, Err(TlsValidationFailure { reason: actual, .. }) if actual == reason)
        );
    }

    fn secret_ref(name: &str, key: &str) -> SecretKeyReference {
        SecretKeyReference {
            name: name.to_string(),
            key: key.to_string(),
        }
    }

    fn certificate_object(name: &str, generation: Option<i64>, data: Value) -> DynamicObject {
        let mut object = DynamicObject::new(name, &certificate_api_resource()).data(data);
        object.metadata.generation = generation;
        object
    }

    fn tls_secret(
        name: &str,
        resource_version: &str,
        type_: Option<&str>,
        include_tls_keys: bool,
        first_key: bool,
        ca: Option<&[u8]>,
    ) -> Secret {
        let mut data = BTreeMap::new();
        if include_tls_keys {
            data.insert(
                TLS_CERT_KEY.to_string(),
                ByteString(PUBLIC_CERT_PEM.to_vec()),
            );
            data.insert(
                TLS_KEY_KEY.to_string(),
                ByteString(
                    if first_key {
                        b"private-a"
                    } else {
                        b"private-b"
                    }
                    .to_vec(),
                ),
            );
        }
        if let Some(ca) = ca {
            data.insert(CA_CERT_KEY.to_string(), ByteString(ca.to_vec()));
        }

        Secret {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                resource_version: Some(resource_version.to_string()),
                ..Default::default()
            },
            type_: type_.map(ToString::to_string),
            data: Some(data),
            ..Default::default()
        }
    }
}
