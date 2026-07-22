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

use super::Tenant;
use crate::cluster_dns;
use crate::types;
use crate::types::v1alpha1::encryption::KmsBackendType;
use crate::types::v1alpha1::persistence::{
    DEFAULT_PERSISTENCE_PATH, LEGACY_LOCAL_KMS_KEY_DIR, data_volume_mount_path,
    default_local_kms_key_directory,
};
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::security_context::{
    MAX_KUBERNETES_ID, PodSecurityContextOverride, effective_run_as_non_root,
};
use crate::types::v1alpha1::tls::{TlsPlan, http_probe};
use k8s_openapi::DeepMerge;
use k8s_openapi::api::apps::v1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

const LOCAL_KMS_KEY_DIR_ENV: &str = "RUSTFS_KMS_KEY_DIR";
const LOCAL_KMS_LOCAL_KEY_DIR_ENV: &str = "RUSTFS_KMS_LOCAL_KEY_DIR";
const LOCAL_KMS_MASTER_KEY_ENV: &str = "RUSTFS_KMS_LOCAL_MASTER_KEY";
const KMS_ALLOW_INSECURE_DEV_DEFAULTS_ENV: &str = "RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS";
const RPC_SECRET_ENV: &str = "RUSTFS_RPC_SECRET";
const VOLUME_CLAIM_TEMPLATE_PREFIX: &str = "vol";
const DEFAULT_RUN_AS_USER: i64 = 10001;
const DEFAULT_RUN_AS_GROUP: i64 = 10001;
const DEFAULT_FS_GROUP: i64 = 10001;
const LAST_TOKIO_IO_URING_BETA: u32 = 8;
pub const RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION: &str =
    "operator.rustfs.com/runtime-default-image-ack";
// Kubernetes caps Localhost AppArmor names at PATH_MAX - 1 bytes.
const MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH: usize = 4095;

pub(crate) fn uses_unpartitioned_rolling_update(
    strategy: Option<&v1::StatefulSetUpdateStrategy>,
) -> bool {
    let strategy_type = strategy
        .and_then(|strategy| strategy.type_.as_deref())
        .unwrap_or("RollingUpdate");
    let partition = strategy
        .and_then(|strategy| strategy.rolling_update.as_ref())
        .and_then(|rolling_update| rolling_update.partition)
        .unwrap_or(0);

    strategy_type == "RollingUpdate" && partition == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageUnverifiableReason {
    CustomRepository,
    DigestQualified,
    MissingTag,
    MutableTag,
    UnknownTag,
}

impl ImageUnverifiableReason {
    fn description(self) -> &'static str {
        match self {
            Self::CustomRepository => "reference from a custom repository",
            Self::DigestQualified => {
                "digest-qualified reference (Kubernetes pulls by digest, not tag)"
            }
            Self::MissingTag => "reference without an explicit version tag",
            Self::MutableTag => "mutable 'latest' tag",
            Self::UnknownTag => "unrecognized version tag",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RustfsImageSeccompCompatibility<'a> {
    KnownCompatible,
    KnownIncompatible { tag: &'a str },
    Unverifiable { reason: ImageUnverifiableReason },
}

fn split_image_repository_and_tag(reference: &str) -> (&str, Option<&str>) {
    let last_path_separator = reference.rfind('/').map_or(0, |index| index + 1);
    let tag_separator = reference
        .rfind(':')
        .filter(|index| *index >= last_path_separator);

    match tag_separator {
        Some(index) => (&reference[..index], Some(&reference[index + 1..])),
        None => (reference, None),
    }
}

fn is_official_rustfs_repository(repository: &str) -> bool {
    matches!(
        repository,
        "rustfs/rustfs"
            | "docker.io/rustfs/rustfs"
            | "index.docker.io/rustfs/rustfs"
            | "registry-1.docker.io/rustfs/rustfs"
            | "ghcr.io/rustfs/rustfs"
            | "quay.io/rustfs/rustfs"
    )
}

fn parse_canonical_u32(part: &str) -> Option<u32> {
    let value = part.parse::<u32>().ok()?;
    (part == value.to_string()).then_some(value)
}

fn is_known_compatible_release_tag(tag: &str) -> bool {
    let (release, suffix) = tag
        .split_once('-')
        .map_or((tag, None), |(release, suffix)| (release, Some(suffix)));
    let mut components = release.split('.');
    let version = match (
        components.next().and_then(parse_canonical_u32),
        components.next().and_then(parse_canonical_u32),
        components.next().and_then(parse_canonical_u32),
        components.next(),
    ) {
        (Some(major), Some(minor), Some(patch), None) => (major, minor, patch),
        _ => return false,
    };

    if suffix.is_some_and(|suffix| suffix != "glibc") {
        return false;
    }

    version >= (1, 0, 0)
}

fn classify_rustfs_image_for_seccomp(image: &str) -> RustfsImageSeccompCompatibility<'_> {
    let image = image.trim();
    let (tagged_reference, digest) = image
        .split_once('@')
        .map_or((image, None), |(reference, digest)| {
            (reference, Some(digest))
        });
    let (repository, tag) = split_image_repository_and_tag(tagged_reference);
    if !is_official_rustfs_repository(repository) {
        return RustfsImageSeccompCompatibility::Unverifiable {
            reason: ImageUnverifiableReason::CustomRepository,
        };
    }

    if digest.is_some() {
        return RustfsImageSeccompCompatibility::Unverifiable {
            reason: ImageUnverifiableReason::DigestQualified,
        };
    }

    let Some(tag) = tag else {
        return RustfsImageSeccompCompatibility::Unverifiable {
            reason: ImageUnverifiableReason::MissingTag,
        };
    };
    if tag == "latest" {
        return RustfsImageSeccompCompatibility::Unverifiable {
            reason: ImageUnverifiableReason::MutableTag,
        };
    }

    let tag = tag.strip_prefix('v').unwrap_or(tag);
    if tag.starts_with("1.0.0-alpha.") {
        return RustfsImageSeccompCompatibility::KnownIncompatible { tag };
    }

    if let Some(beta) = tag.strip_prefix("1.0.0-beta.") {
        let (number, suffix) = beta
            .split_once('-')
            .map_or((beta, None), |(number, suffix)| (number, Some(suffix)));
        if let Some(beta) = parse_canonical_u32(number)
            && (beta <= LAST_TOKIO_IO_URING_BETA || suffix.is_none_or(|suffix| suffix == "glibc"))
        {
            return if beta <= LAST_TOKIO_IO_URING_BETA {
                RustfsImageSeccompCompatibility::KnownIncompatible { tag }
            } else {
                RustfsImageSeccompCompatibility::KnownCompatible
            };
        }
    }

    if is_known_compatible_release_tag(tag) {
        RustfsImageSeccompCompatibility::KnownCompatible
    } else {
        RustfsImageSeccompCompatibility::Unverifiable {
            reason: ImageUnverifiableReason::UnknownTag,
        }
    }
}

fn validate_declared_security_profile_shape<'a>(
    field_path: &str,
    type_: &str,
    localhost_profile: Option<&'a str>,
) -> Result<Option<&'a str>, String> {
    if !matches!(type_, "RuntimeDefault" | "Localhost" | "Unconfined") {
        return Err(format!(
            "{field_path}.type must be RuntimeDefault, Localhost, or Unconfined; got '{type_}'"
        ));
    }

    match (type_, localhost_profile) {
        ("Localhost", Some(profile)) => Ok(Some(profile)),
        ("Localhost", None) => Err(format!(
            "{field_path}.localhostProfile must be nonblank when type is Localhost"
        )),
        (_, Some(_)) => Err(format!(
            "{field_path}.localhostProfile must be omitted when type is {type_}"
        )),
        _ => Ok(None),
    }
}

fn validate_declared_seccomp_profile(
    field_path: &str,
    type_: &str,
    localhost_profile: Option<&str>,
) -> Result<(), String> {
    let Some(profile) =
        validate_declared_security_profile_shape(field_path, type_, localhost_profile)?
    else {
        return Ok(());
    };

    if profile.trim().is_empty() {
        return Err(format!(
            "{field_path}.localhostProfile must be nonblank when type is Localhost"
        ));
    }
    // Match kube-apiserver's validateLocalDescendingPath checks.
    if profile.starts_with('/') {
        return Err(format!(
            "{field_path}.localhostProfile must be a relative path"
        ));
    }
    if profile.split('/').any(|component| component == "..") {
        return Err(format!(
            "{field_path}.localhostProfile must not contain '..'"
        ));
    }

    Ok(())
}

fn validate_declared_app_armor_profile(
    field_path: &str,
    type_: &str,
    localhost_profile: Option<&str>,
) -> Result<(), String> {
    let Some(profile) =
        validate_declared_security_profile_shape(field_path, type_, localhost_profile)?
    else {
        return Ok(());
    };

    if profile.trim() != profile {
        return Err(format!(
            "{field_path}.localhostProfile must not be padded with whitespace"
        ));
    }
    if profile.is_empty() {
        return Err(format!(
            "{field_path}.localhostProfile must be nonblank when type is Localhost"
        ));
    }
    if profile.len() > MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH {
        return Err(format!(
            "{field_path}.localhostProfile must be at most {MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH} bytes"
        ));
    }

    Ok(())
}

fn validate_kubernetes_id(field_path: &str, value: Option<i64>) -> Result<(), String> {
    if value.is_some_and(|value| !(0..=MAX_KUBERNETES_ID).contains(&value)) {
        return Err(format!(
            "{field_path} must be between 0 and {MAX_KUBERNETES_ID}, inclusive"
        ));
    }

    Ok(())
}

fn validate_declared_security_context_ids(
    pod_field_path: &str,
    pod: Option<&PodSecurityContextOverride>,
    container_field_path: &str,
    container: Option<&corev1::SecurityContext>,
) -> Result<(), String> {
    validate_kubernetes_id(
        &format!("{pod_field_path}.runAsUser"),
        pod.and_then(|context| context.run_as_user),
    )?;
    validate_kubernetes_id(
        &format!("{pod_field_path}.runAsGroup"),
        pod.and_then(|context| context.run_as_group),
    )?;
    validate_kubernetes_id(
        &format!("{pod_field_path}.fsGroup"),
        pod.and_then(|context| context.fs_group),
    )?;
    validate_kubernetes_id(
        &format!("{container_field_path}.runAsUser"),
        container.and_then(|context| context.run_as_user),
    )?;
    validate_kubernetes_id(
        &format!("{container_field_path}.runAsGroup"),
        container.and_then(|context| context.run_as_group),
    )?;

    Ok(())
}

fn apply_pod_security_context_override(
    context: &mut corev1::PodSecurityContext,
    overrides: &PodSecurityContextOverride,
) {
    context.run_as_user = overrides.run_as_user.or(context.run_as_user);
    context.run_as_group = overrides.run_as_group.or(context.run_as_group);
    context.fs_group = overrides.fs_group.or(context.fs_group);
    context.run_as_non_root = overrides.run_as_non_root.or(context.run_as_non_root);
    context.seccomp_profile = overrides
        .seccomp_profile
        .clone()
        .or_else(|| context.seccomp_profile.clone());
}

fn explicit_pod_run_as_non_root(
    tenant: Option<&PodSecurityContextOverride>,
    pool: Option<&PodSecurityContextOverride>,
) -> Option<bool> {
    pool.and_then(|overrides| overrides.run_as_non_root)
        .or_else(|| tenant.and_then(|overrides| overrides.run_as_non_root))
}

fn effective_pod_security_context(
    tenant: Option<&PodSecurityContextOverride>,
    pool: Option<&PodSecurityContextOverride>,
) -> corev1::PodSecurityContext {
    let explicit_run_as_non_root = explicit_pod_run_as_non_root(tenant, pool);
    let mut context = corev1::PodSecurityContext {
        run_as_user: Some(DEFAULT_RUN_AS_USER),
        run_as_group: Some(DEFAULT_RUN_AS_GROUP),
        fs_group: Some(DEFAULT_FS_GROUP),
        fs_group_change_policy: Some("OnRootMismatch".to_string()),
        run_as_non_root: Some(true),
        seccomp_profile: Some(corev1::SeccompProfile {
            type_: "RuntimeDefault".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    for overrides in [tenant, pool].into_iter().flatten() {
        apply_pod_security_context_override(&mut context, overrides);
    }

    context.run_as_non_root = Some(effective_run_as_non_root(
        context.run_as_user,
        explicit_run_as_non_root,
    ));

    context
}

fn merge_container_security_context(
    context: &mut corev1::SecurityContext,
    overrides: &corev1::SecurityContext,
) {
    context.merge_from(overrides.clone());

    // These tagged profile objects are atomic Kubernetes values. Deep-merging them can retain
    // localhostProfile after changing type from Localhost to RuntimeDefault, which is invalid.
    if overrides.seccomp_profile.is_some() {
        context
            .seccomp_profile
            .clone_from(&overrides.seccomp_profile);
    }
    if overrides.app_armor_profile.is_some() {
        context
            .app_armor_profile
            .clone_from(&overrides.app_armor_profile);
    }
}

fn effective_container_security_context(
    tenant: Option<&corev1::SecurityContext>,
    pool: Option<&corev1::SecurityContext>,
    explicit_pod_run_as_non_root: Option<bool>,
) -> corev1::SecurityContext {
    let explicit_container_run_as_non_root = pool
        .and_then(|overrides| overrides.run_as_non_root)
        .or_else(|| tenant.and_then(|overrides| overrides.run_as_non_root));
    let mut context = corev1::SecurityContext {
        allow_privilege_escalation: Some(false),
        capabilities: Some(corev1::Capabilities {
            drop: Some(vec!["ALL".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    };

    for overrides in [tenant, pool].into_iter().flatten() {
        merge_container_security_context(&mut context, overrides);
    }

    // A container UID overrides the Pod UID. When neither the container nor Pod explicitly
    // chooses runAsNonRoot, materialize the matching container value so the Pod default does not
    // turn an intentional UID 0 override into the contradictory UID 0 + runAsNonRoot=true pair.
    if context.run_as_user.is_some()
        && explicit_container_run_as_non_root.is_none()
        && explicit_pod_run_as_non_root.is_none()
    {
        context.run_as_non_root = Some(effective_run_as_non_root(context.run_as_user, None));
    }

    context
}

struct EffectiveWorkloadSecurityContext {
    pod: corev1::PodSecurityContext,
    container: corev1::SecurityContext,
}

fn effective_workload_security_context(
    tenant_pod: Option<&PodSecurityContextOverride>,
    pool_pod: Option<&PodSecurityContextOverride>,
    tenant_container: Option<&corev1::SecurityContext>,
    pool_container: Option<&corev1::SecurityContext>,
) -> EffectiveWorkloadSecurityContext {
    let explicit_pod_run_as_non_root = explicit_pod_run_as_non_root(tenant_pod, pool_pod);
    EffectiveWorkloadSecurityContext {
        pod: effective_pod_security_context(tenant_pod, pool_pod),
        container: effective_container_security_context(
            tenant_container,
            pool_container,
            explicit_pod_run_as_non_root,
        ),
    }
}

const TLS_OPERATOR_MANAGED_ENV_VARS: &[&str] = &[
    "RUSTFS_VOLUMES",
    "RUSTFS_TLS_PATH",
    "RUSTFS_TRUST_SYSTEM_CA",
    "RUSTFS_TRUST_LEAF_CERT_AS_CA",
    "RUSTFS_SERVER_MTLS_ENABLE",
];

fn is_tls_operator_managed_env_var(name: &str) -> bool {
    TLS_OPERATOR_MANAGED_ENV_VARS.contains(&name)
}

fn is_kms_operator_managed_env_var(name: &str) -> bool {
    name.starts_with("RUSTFS_KMS_")
}

fn volume_claim_template_name(shard: i32) -> String {
    format!("{VOLUME_CLAIM_TEMPLATE_PREFIX}-{shard}")
}

fn container_env_values<'a>(container: &'a corev1::Container, name: &str) -> Vec<&'a str> {
    container
        .env
        .as_ref()
        .map(|env| {
            env.iter()
                .filter(|var| var.name == name)
                .filter_map(|var| var.value.as_deref())
                .collect()
        })
        .unwrap_or_default()
}

fn local_kms_key_dir_env_values(container: &corev1::Container) -> Vec<&str> {
    let mut values = container_env_values(container, LOCAL_KMS_KEY_DIR_ENV);
    values.extend(container_env_values(container, LOCAL_KMS_LOCAL_KEY_DIR_ENV));
    values
}

fn first_container(spec: &v1::StatefulSetSpec) -> Option<&corev1::Container> {
    spec.template.spec.as_ref()?.containers.first()
}

fn stateful_name(tenant: &Tenant, pool: &Pool) -> String {
    format!("{}-{}", tenant.name(), pool.name)
}

impl Tenant {
    fn validate_declared_workload_security_contexts(&self) -> Result<(), types::error::Error> {
        let invalid_profile = |message| types::error::Error::InvalidWorkloadSecurityProfile {
            name: self.name(),
            message,
        };
        let validate_seccomp = |field_path: &str, type_: &str, localhost_profile: Option<&str>| {
            validate_declared_seccomp_profile(field_path, type_, localhost_profile)
                .map_err(&invalid_profile)
        };
        let validate_app_armor =
            |field_path: &str, type_: &str, localhost_profile: Option<&str>| {
                validate_declared_app_armor_profile(field_path, type_, localhost_profile).map_err(
                    |message| types::error::Error::InvalidWorkloadSecurityProfile {
                        name: self.name(),
                        message,
                    },
                )
            };

        validate_declared_security_context_ids(
            "spec.securityContext",
            self.spec.security_context.as_ref(),
            "spec.containerSecurityContext",
            self.spec.container_security_context.as_ref(),
        )
        .map_err(&invalid_profile)?;

        if let Some(profile) = self
            .spec
            .security_context
            .as_ref()
            .and_then(|context| context.seccomp_profile.as_ref())
        {
            validate_seccomp(
                "spec.securityContext.seccompProfile",
                &profile.type_,
                profile.localhost_profile.as_deref(),
            )?;
        }

        if let Some(context) = self.spec.container_security_context.as_ref() {
            if let Some(profile) = context.seccomp_profile.as_ref() {
                validate_seccomp(
                    "spec.containerSecurityContext.seccompProfile",
                    &profile.type_,
                    profile.localhost_profile.as_deref(),
                )?;
            }
            if let Some(profile) = context.app_armor_profile.as_ref() {
                validate_app_armor(
                    "spec.containerSecurityContext.appArmorProfile",
                    &profile.type_,
                    profile.localhost_profile.as_deref(),
                )?;
            }
        }

        for pool in &self.spec.pools {
            validate_declared_security_context_ids(
                &format!("spec.pools[name={}].securityContext", pool.name),
                pool.security_context.as_ref(),
                &format!("spec.pools[name={}].containerSecurityContext", pool.name),
                pool.container_security_context.as_ref(),
            )
            .map_err(&invalid_profile)?;

            if let Some(profile) = pool
                .security_context
                .as_ref()
                .and_then(|context| context.seccomp_profile.as_ref())
            {
                validate_seccomp(
                    &format!(
                        "spec.pools[name={}].securityContext.seccompProfile",
                        pool.name
                    ),
                    &profile.type_,
                    profile.localhost_profile.as_deref(),
                )?;
            }

            if let Some(context) = pool.container_security_context.as_ref() {
                if let Some(profile) = context.seccomp_profile.as_ref() {
                    validate_seccomp(
                        &format!(
                            "spec.pools[name={}].containerSecurityContext.seccompProfile",
                            pool.name
                        ),
                        &profile.type_,
                        profile.localhost_profile.as_deref(),
                    )?;
                }
                if let Some(profile) = context.app_armor_profile.as_ref() {
                    validate_app_armor(
                        &format!(
                            "spec.pools[name={}].containerSecurityContext.appArmorProfile",
                            pool.name
                        ),
                        &profile.type_,
                        profile.localhost_profile.as_deref(),
                    )?;
                }
            }
        }

        Ok(())
    }

    fn validate_effective_workload_identity(
        &self,
        pool: &Pool,
        security: &EffectiveWorkloadSecurityContext,
    ) -> Result<(), types::error::Error> {
        let effective_run_as_user = security.container.run_as_user.or(security.pod.run_as_user);
        let effective_run_as_non_root = security
            .container
            .run_as_non_root
            .or(security.pod.run_as_non_root);

        if effective_run_as_user == Some(0) && effective_run_as_non_root == Some(true) {
            return Err(types::error::Error::InvalidWorkloadSecurityProfile {
                name: self.name(),
                message: format!(
                    "pool '{}' resolves runAsUser to UID 0 while runAsNonRoot is explicitly true; use a non-zero UID or explicitly set the effective runAsNonRoot value to false",
                    pool.name
                ),
            });
        }

        Ok(())
    }

    pub fn validate_workload_security_compatibility(&self) -> Result<(), types::error::Error> {
        self.validate_declared_workload_security_contexts()?;

        let image = super::helper::get_rustfs_image_or_default(self.spec.image.as_ref());
        let image_compatibility = classify_rustfs_image_for_seccomp(&image);

        for pool in &self.spec.pools {
            let security = effective_workload_security_context(
                self.spec.security_context.as_ref(),
                pool.security_context.as_ref(),
                self.spec.container_security_context.as_ref(),
                pool.container_security_context.as_ref(),
            );
            self.validate_effective_workload_identity(pool, &security)?;

            let seccomp_type = security
                .container
                .seccomp_profile
                .as_ref()
                .or(security.pod.seccomp_profile.as_ref())
                .map(|profile| profile.type_.as_str());

            if seccomp_type != Some("RuntimeDefault") {
                continue;
            }

            match image_compatibility {
                RustfsImageSeccompCompatibility::KnownCompatible => {}
                RustfsImageSeccompCompatibility::KnownIncompatible { tag } => {
                    return Err(types::error::Error::WorkloadSecurityIncompatible {
                        name: self.name(),
                        message: format!(
                            "image '{image}' ({tag}) enables Tokio io_uring and cannot run with RuntimeDefault seccomp; upgrade to a RustFS build containing rustfs/rustfs#4364 before reconciling pool '{}', or configure a compatible Localhost seccomp profile",
                            pool.name
                        ),
                    });
                }
                RustfsImageSeccompCompatibility::Unverifiable { reason } => {
                    let image_acknowledged = self
                        .metadata
                        .annotations
                        .as_ref()
                        .and_then(|annotations| {
                            annotations.get(RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION)
                        })
                        .is_some_and(|acknowledged_image| acknowledged_image == &image);
                    if !image_acknowledged {
                        return Err(types::error::Error::WorkloadSecurityIncompatible {
                            name: self.name(),
                            message: format!(
                                "image '{image}' uses a {} and its seccomp compatibility cannot be verified for RuntimeDefault in pool '{}'; pin a verified RustFS 1.0.0-beta.9 or later release tag (for example 1.0.0-beta.10), or verify the image and set metadata.annotations['{RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION}'] to the exact current resolved image reference; mutable references can change without changing the annotation, so a digest-qualified reference is strongly recommended",
                                reason.description(),
                                pool.name
                            ),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn rustfs_pool_volume_spec(
        &self,
        pool: &Pool,
        scheme: &str,
        namespace: &str,
        cluster_domain: &str,
    ) -> String {
        let tenant_name = self.name();
        let headless_service = self.headless_service_name();
        let base_path = pool
            .persistence
            .path
            .as_deref()
            .unwrap_or(DEFAULT_PERSISTENCE_PATH);
        let base_path = base_path.trim_end_matches('/');

        if self.spec.pools.len() == 1 && pool.is_single_node_single_disk() {
            return format!("{base_path}/rustfs0");
        }

        let pod_name = format!("{tenant_name}-{}-{{0...{}}}", pool.name, pool.servers - 1);
        let peer_host =
            cluster_dns::pod_fqdn(&pod_name, &headless_service, namespace, cluster_domain);
        format!(
            "{scheme}://{peer_host}:9000{}/rustfs{{0...{}}}",
            base_path,
            pool.persistence.volumes_per_server - 1
        )
    }

    /// Constructs the RUSTFS_VOLUMES environment variable value
    /// Distributed and multi-pool tenants use peer DNS entries, while a single-pool
    /// single-node single-disk tenant uses its local data path.
    fn rustfs_volumes_env_value(
        &self,
        scheme: &str,
        cluster_domain: &str,
    ) -> Result<String, types::error::Error> {
        let namespace = self.namespace()?;
        let volume_specs = self
            .spec
            .pools
            .iter()
            .map(|pool| self.rustfs_pool_volume_spec(pool, scheme, &namespace, cluster_domain))
            .collect::<Vec<_>>();

        Ok(volume_specs.join(" "))
    }

    /// Configure logging based on tenant.spec.logging
    /// Returns (pod_volumes, volume_mounts) tuple
    fn configure_logging(
        &self,
    ) -> Result<(Vec<corev1::Volume>, Vec<corev1::VolumeMount>), types::error::Error> {
        use crate::types::v1alpha1::logging::{LoggingConfig, LoggingMode};

        let default_logging = LoggingConfig::default();
        let logging = self.spec.logging.as_ref().unwrap_or(&default_logging);
        let mount_path = logging.mount_path.as_deref().unwrap_or("/logs");

        match &logging.mode {
            LoggingMode::Stdout => {
                // Default: no volumes, logs to stdout
                // This is cloud-native best practice
                Ok((vec![], vec![]))
            }
            LoggingMode::EmptyDir => {
                // Create emptyDir volume for temporary logs
                let volume = corev1::Volume {
                    name: "logs".to_string(),
                    empty_dir: Some(corev1::EmptyDirVolumeSource::default()),
                    ..Default::default()
                };
                let mount = corev1::VolumeMount {
                    name: "logs".to_string(),
                    mount_path: mount_path.to_string(),
                    ..Default::default()
                };
                Ok((vec![volume], vec![mount]))
            }
            LoggingMode::Persistent => {
                // Persistent logs via PVC will be handled in volume_claim_templates
                // For now, we only mount it here
                let mount = corev1::VolumeMount {
                    name: "logs".to_string(),
                    mount_path: mount_path.to_string(),
                    ..Default::default()
                };
                Ok((vec![], vec![mount]))
            }
        }
    }

    /// Creates volume claim templates for a pool
    /// Returns a vector of PersistentVolumeClaim templates for StatefulSet
    fn volume_claim_templates(
        &self,
        pool: &Pool,
    ) -> Result<Vec<corev1::PersistentVolumeClaim>, types::error::Error> {
        // Get PVC spec or create default (ReadWriteOnce, 10Gi)
        let spec = pool
            .persistence
            .volume_claim_template
            .clone()
            .unwrap_or_else(|| {
                let mut resources = std::collections::BTreeMap::new();
                resources.insert(
                    "storage".to_string(),
                    k8s_openapi::apimachinery::pkg::api::resource::Quantity("10Gi".to_string()),
                );

                corev1::PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(corev1::VolumeResourceRequirements {
                        requests: Some(resources),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            });

        // Start with operator-managed labels (follows Kubernetes recommended labels)
        let mut labels = self.pool_labels(pool);

        // Merge with user-provided labels (user labels can override)
        if let Some(user_labels) = &pool.persistence.labels {
            labels.extend(user_labels.clone());
        }

        // Get annotations from persistence config
        let annotations = pool.persistence.annotations.clone();

        // Generate volume claim templates for each volume
        let templates: Vec<_> = (0..pool.persistence.volumes_per_server)
            .map(|i| corev1::PersistentVolumeClaim {
                metadata: metav1::ObjectMeta {
                    name: Some(volume_claim_template_name(i)),
                    labels: Some(labels.clone()),
                    annotations: annotations.clone(),
                    ..Default::default()
                },
                spec: Some(spec.clone()),
                ..Default::default()
            })
            .collect();

        // Add log PVC if persistent logging is enabled
        let mut all_templates = templates;
        if let Some(logging) = &self.spec.logging {
            use crate::types::v1alpha1::logging::LoggingMode;
            if logging.mode == LoggingMode::Persistent {
                let log_pvc = self.create_log_pvc(pool, logging)?;
                all_templates.push(log_pvc);
            }
        }

        Ok(all_templates)
    }

    /// Create PVC for persistent logging
    fn create_log_pvc(
        &self,
        pool: &Pool,
        logging: &crate::types::v1alpha1::logging::LoggingConfig,
    ) -> Result<corev1::PersistentVolumeClaim, types::error::Error> {
        let labels = self.pool_labels(pool);

        let storage_size = logging.storage_size.as_deref().unwrap_or("5Gi");

        let mut resources = std::collections::BTreeMap::new();
        resources.insert(
            "storage".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity(storage_size.to_string()),
        );

        let mut spec = corev1::PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_string()]),
            resources: Some(corev1::VolumeResourceRequirements {
                requests: Some(resources),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Set storage class if specified
        if let Some(storage_class) = &logging.storage_class {
            spec.storage_class_name = Some(storage_class.clone());
        }

        Ok(corev1::PersistentVolumeClaim {
            metadata: metav1::ObjectMeta {
                name: Some("logs".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            spec: Some(spec),
            ..Default::default()
        })
    }

    /// Build KMS-related environment variables for `spec.encryption`.
    ///
    /// Matches RustFS server startup (`rustfs/src/init.rs` `build_local_kms_config` /
    /// `build_vault_kms_config`) and CLI env (`rustfs/src/config/cli.rs`): only the variables
    /// parsed into `Config` are set here.
    ///
    /// Returns `(env_vars, pod_volumes, volume_mounts)` — Local KMS uses the data PVC.
    fn configure_kms(
        &self,
        pool: &Pool,
    ) -> (
        Vec<corev1::EnvVar>,
        Vec<corev1::Volume>,
        Vec<corev1::VolumeMount>,
    ) {
        let Some(ref enc) = self.spec.encryption else {
            return (vec![], vec![], vec![]);
        };
        if !enc.enabled {
            return (vec![], vec![], vec![]);
        }

        let mut env = Vec::new();
        let volumes: Vec<corev1::Volume> = vec![];
        let mounts: Vec<corev1::VolumeMount> = vec![];

        env.push(corev1::EnvVar {
            name: "RUSTFS_KMS_ENABLE".to_owned(),
            value: Some("true".to_owned()),
            ..Default::default()
        });
        env.push(corev1::EnvVar {
            name: "RUSTFS_KMS_BACKEND".to_owned(),
            value: Some(enc.backend.to_string()),
            ..Default::default()
        });

        match enc.backend {
            KmsBackendType::Vault => {
                if let Some(ref vault) = enc.vault {
                    env.push(corev1::EnvVar {
                        name: "RUSTFS_KMS_VAULT_ADDRESS".to_owned(),
                        value: Some(vault.endpoint.clone()),
                        ..Default::default()
                    });
                }

                if let Some(ref secret_ref) = enc.kms_secret
                    && !secret_ref.name.is_empty()
                {
                    env.push(corev1::EnvVar {
                        name: "RUSTFS_KMS_VAULT_TOKEN".to_owned(),
                        value_from: Some(corev1::EnvVarSource {
                            secret_key_ref: Some(corev1::SecretKeySelector {
                                name: secret_ref.name.clone(),
                                key: "vault-token".to_string(),
                                optional: Some(false),
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }

                if let Some(ref id) = enc.default_key_id
                    && !id.is_empty()
                {
                    env.push(corev1::EnvVar {
                        name: "RUSTFS_KMS_DEFAULT_KEY_ID".to_owned(),
                        value: Some(id.clone()),
                        ..Default::default()
                    });
                }
            }
            KmsBackendType::Local => {
                let key_dir = enc
                    .local
                    .as_ref()
                    .and_then(|l| l.key_directory.as_deref())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        default_local_kms_key_directory(pool.persistence.path.as_deref())
                    });

                env.push(corev1::EnvVar {
                    name: LOCAL_KMS_KEY_DIR_ENV.to_owned(),
                    value: Some(key_dir),
                    ..Default::default()
                });

                if let Some(selector) = enc
                    .local
                    .as_ref()
                    .and_then(|l| l.master_key_secret_ref.as_ref())
                {
                    env.push(corev1::EnvVar {
                        name: LOCAL_KMS_MASTER_KEY_ENV.to_owned(),
                        value_from: Some(corev1::EnvVarSource {
                            secret_key_ref: Some(corev1::SecretKeySelector {
                                name: selector.name.clone(),
                                key: selector.key.clone(),
                                optional: Some(false),
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }

                if enc
                    .local
                    .as_ref()
                    .is_some_and(|l| l.allow_insecure_dev_defaults)
                {
                    env.push(corev1::EnvVar {
                        name: KMS_ALLOW_INSECURE_DEV_DEFAULTS_ENV.to_owned(),
                        value: Some("true".to_owned()),
                        ..Default::default()
                    });
                }

                if let Some(ref id) = enc.default_key_id
                    && !id.is_empty()
                {
                    env.push(corev1::EnvVar {
                        name: "RUSTFS_KMS_DEFAULT_KEY_ID".to_owned(),
                        value: Some(id.clone()),
                        ..Default::default()
                    });
                }
            }
        }

        (env, volumes, mounts)
    }

    pub fn new_statefulset(&self, pool: &Pool) -> Result<v1::StatefulSet, types::error::Error> {
        self.new_statefulset_with_tls_plan(pool, &TlsPlan::disabled())
    }

    pub fn new_statefulset_with_tls_plan(
        &self,
        pool: &Pool,
        tls_plan: &TlsPlan,
    ) -> Result<v1::StatefulSet, types::error::Error> {
        self.new_statefulset_with_tls_plan_and_cluster_domain(
            pool,
            tls_plan,
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
        )
    }

    pub(crate) fn new_statefulset_with_tls_plan_and_cluster_domain(
        &self,
        pool: &Pool,
        tls_plan: &TlsPlan,
        cluster_domain: &str,
    ) -> Result<v1::StatefulSet, types::error::Error> {
        self.validate_declared_workload_security_contexts()?;

        let labels = self.pool_labels(pool);
        let selector_labels = self.pool_selector_labels(pool);

        // Generate volume claim templates using helper function
        let volume_claim_templates = self.volume_claim_templates(pool)?;

        // Generate volume mounts for each volume
        // Default path is /data if not specified
        // Volume mount names must match the volume claim template names (vol-0, vol-1, etc.)
        // Mount paths follow RustFS convention: /data/rustfs0, /data/rustfs1, etc.
        let mut volume_mounts: Vec<corev1::VolumeMount> = (0..pool.persistence.volumes_per_server)
            .map(|i| corev1::VolumeMount {
                name: volume_claim_template_name(i),
                mount_path: data_volume_mount_path(pool.persistence.path.as_deref(), i),
                ..Default::default()
            })
            .collect();

        // Generate environment variables: operator-managed + user-provided
        let mut env_vars = Vec::new();

        // Add RUSTFS_VOLUMES environment variable for the inferred storage layout.
        let rustfs_volumes =
            self.rustfs_volumes_env_value(tls_plan.internode_scheme, cluster_domain)?;
        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_VOLUMES".to_owned(),
            value: Some(rustfs_volumes),
            ..Default::default()
        });
        env_vars.extend(tls_plan.env.clone());

        // Add required RustFS environment variables
        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_ADDRESS".to_owned(),
            value: Some("0.0.0.0:9000".to_owned()),
            ..Default::default()
        });

        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_CONSOLE_ADDRESS".to_owned(),
            value: Some("0.0.0.0:9001".to_owned()),
            ..Default::default()
        });

        env_vars.push(corev1::EnvVar {
            name: "RUSTFS_CONSOLE_ENABLE".to_owned(),
            value: Some("true".to_owned()),
            ..Default::default()
        });

        // Add credentials from Secret if credsSecret is specified
        if let Some(ref cfg) = self.spec.creds_secret
            && !cfg.name.is_empty()
        {
            env_vars.push(corev1::EnvVar {
                name: "RUSTFS_ACCESS_KEY".to_owned(),
                value_from: Some(corev1::EnvVarSource {
                    secret_key_ref: Some(corev1::SecretKeySelector {
                        name: cfg.name.clone(),
                        key: "accesskey".to_string(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });

            env_vars.push(corev1::EnvVar {
                name: "RUSTFS_SECRET_KEY".to_owned(),
                value_from: Some(corev1::EnvVarSource {
                    secret_key_ref: Some(corev1::SecretKeySelector {
                        name: cfg.name.clone(),
                        key: "secretkey".to_string(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        // Keep internode RPC authentication independent from admin credentials when
        // the Tenant explicitly selects a dedicated Secret key. If omitted, RustFS
        // retains ownership of RPC secret resolution.
        if let Some(ref secret_ref) = self.spec.rpc_secret {
            env_vars.push(corev1::EnvVar {
                name: RPC_SECRET_ENV.to_owned(),
                value_from: Some(corev1::EnvVarSource {
                    secret_key_ref: Some(corev1::SecretKeySelector {
                        name: secret_ref.name.clone(),
                        key: secret_ref.key.clone(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        // Merge with user-provided environment variables.
        // Preserve the legacy override behavior except for operator-managed runtime
        // values that must stay aligned with rendered mounts, probes, status, and hash.
        for user_env in &self.spec.env {
            if tls_plan.enabled && is_tls_operator_managed_env_var(&user_env.name) {
                continue;
            }
            if is_kms_operator_managed_env_var(&user_env.name) {
                continue;
            }
            if self.spec.rpc_secret.is_some() && user_env.name == RPC_SECRET_ENV {
                continue;
            }
            // Remove any existing var with the same name to allow non-reserved overrides.
            env_vars.retain(|e| e.name != user_env.name);
            env_vars.push(user_env.clone());
        }

        // Configure logging based on tenant.spec.logging
        // Default: stdout (cloud-native best practice)
        let (mut pod_volumes, mut log_volume_mounts) = self.configure_logging()?;

        // Merge log volume mounts with data volume mounts
        volume_mounts.append(&mut log_volume_mounts);

        // Configure KMS / encryption environment variables and volumes
        let (kms_env, mut kms_volumes, mut kms_mounts) = self.configure_kms(pool);
        env_vars.extend(kms_env);
        pod_volumes.append(&mut kms_volumes);
        volume_mounts.append(&mut kms_mounts);
        pod_volumes.extend(tls_plan.volumes.clone());
        volume_mounts.extend(tls_plan.volume_mounts.clone());

        let security = effective_workload_security_context(
            self.spec.security_context.as_ref(),
            pool.security_context.as_ref(),
            self.spec.container_security_context.as_ref(),
            pool.container_security_context.as_ref(),
        );
        self.validate_effective_workload_identity(pool, &security)?;
        let EffectiveWorkloadSecurityContext {
            pod: pod_security_context,
            container: container_security_context,
        } = security;

        let container = corev1::Container {
            name: "rustfs".to_owned(),
            image: Some(super::helper::get_rustfs_image_or_default(
                self.spec.image.as_ref(),
            )),
            env: if env_vars.is_empty() {
                None
            } else {
                Some(env_vars)
            },
            ports: Some(vec![
                corev1::ContainerPort {
                    container_port: 9000,
                    name: Some("http".to_owned()),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                },
                corev1::ContainerPort {
                    container_port: 9001,
                    name: Some("console".to_owned()),
                    protocol: Some("TCP".to_owned()),
                    ..Default::default()
                },
            ]),
            volume_mounts: Some(volume_mounts),
            lifecycle: self.spec.lifecycle.clone(),
            // Apply pool-level resource requirements to container
            resources: pool.scheduling.resources.clone(),
            image_pull_policy: self
                .spec
                .image_pull_policy
                .as_ref()
                .map(ToString::to_string),
            liveness_probe: Some(http_probe("/health", tls_plan.probe_scheme)),
            readiness_probe: Some(http_probe("/health/ready", tls_plan.probe_scheme)),
            startup_probe: Some(http_probe("/health", tls_plan.probe_scheme)),
            termination_message_policy: Some("FallbackToLogsOnError".to_string()),
            security_context: Some(container_security_context),
            ..Default::default()
        };

        Ok(v1::StatefulSet {
            metadata: metav1::ObjectMeta {
                name: Some(stateful_name(self, pool)),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(labels.clone()),
                ..Default::default()
            },
            spec: Some(v1::StatefulSetSpec {
                replicas: Some(pool.servers),
                service_name: Some(self.headless_service_name()),
                pod_management_policy: Some(
                    self.spec
                        .pod_management_policy
                        .as_ref()
                        .cloned()
                        .unwrap_or_default()
                        .to_string(),
                ),
                selector: metav1::LabelSelector {
                    match_labels: Some(selector_labels),
                    ..Default::default()
                },
                update_strategy: self.spec.service_account_name.is_none().then(|| {
                    v1::StatefulSetUpdateStrategy {
                        type_: Some("RollingUpdate".to_string()),
                        rolling_update: Some(v1::RollingUpdateStatefulSetStrategy {
                            partition: Some(0),
                            ..Default::default()
                        }),
                    }
                }),
                template: corev1::PodTemplateSpec {
                    metadata: Some(metav1::ObjectMeta {
                        labels: Some(labels),
                        annotations: (!tls_plan.pod_template_annotations.is_empty())
                            .then(|| tls_plan.pod_template_annotations.clone()),
                        ..Default::default()
                    }),
                    spec: Some(corev1::PodSpec {
                        service_account_name: Some(self.service_account_name()),
                        automount_service_account_token: self
                            .spec
                            .service_account_name
                            .is_none()
                            .then_some(false),
                        containers: vec![container],
                        security_context: Some(pod_security_context),
                        volumes: Some(pod_volumes),
                        scheduler_name: self.spec.scheduler.clone(),
                        // Pool-level priority class overrides tenant-level
                        priority_class_name: pool
                            .scheduling
                            .priority_class_name
                            .clone()
                            .or_else(|| self.spec.priority_class_name.clone()),
                        // Pool-level scheduling controls
                        node_selector: pool.scheduling.node_selector.clone(),
                        affinity: pool.scheduling.affinity.clone(),
                        tolerations: pool.scheduling.tolerations.clone(),
                        topology_spread_constraints: pool
                            .scheduling
                            .topology_spread_constraints
                            .clone(),
                        image_pull_secrets: self.spec.image_pull_secret.clone().map(|s| vec![s]),
                        ..Default::default()
                    }),
                },
                volume_claim_templates: Some(volume_claim_templates),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// Checks if a StatefulSet needs to be updated based on differences between
    /// the existing StatefulSet and the desired state defined in the Tenant spec.
    ///
    /// This method performs a semantic comparison of key StatefulSet fields to
    /// determine if an update is necessary, avoiding unnecessary API calls.
    ///
    /// # Returns
    /// - `Ok(true)` if the StatefulSet needs to be updated
    /// - `Ok(false)` if the StatefulSet matches the desired state
    /// - `Err` if comparison fails
    pub fn statefulset_needs_update(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
    ) -> Result<bool, types::error::Error> {
        self.statefulset_needs_update_with_tls_plan(existing, pool, &TlsPlan::disabled())
    }

    pub fn statefulset_needs_update_with_tls_plan(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
        tls_plan: &TlsPlan,
    ) -> Result<bool, types::error::Error> {
        self.statefulset_needs_update_with_tls_plan_and_cluster_domain(
            existing,
            pool,
            tls_plan,
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
        )
    }

    pub(crate) fn statefulset_needs_update_with_tls_plan_and_cluster_domain(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
        tls_plan: &TlsPlan,
        cluster_domain: &str,
    ) -> Result<bool, types::error::Error> {
        let desired =
            self.new_statefulset_with_tls_plan_and_cluster_domain(pool, tls_plan, cluster_domain)?;

        // Compare key spec fields that should trigger updates
        let existing_spec = existing
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Existing StatefulSet missing spec".to_string(),
            })?;

        let desired_spec = desired
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Desired StatefulSet missing spec".to_string(),
            })?;

        // Check replicas (server count)
        if existing_spec.replicas != desired_spec.replicas {
            return Ok(true);
        }

        // Check pod management policy
        if existing_spec.pod_management_policy != desired_spec.pod_management_policy {
            return Ok(true);
        }

        // Compare pod template spec
        let existing_template = &existing_spec.template;
        let desired_template = &desired_spec.template;

        // Check if pod template metadata labels changed
        if existing_template
            .metadata
            .as_ref()
            .and_then(|m| m.labels.as_ref())
            != desired_template
                .metadata
                .as_ref()
                .and_then(|m| m.labels.as_ref())
        {
            return Ok(true);
        }

        // Check if pod template annotations changed (TLS hash rollout lives here).
        if existing_template
            .metadata
            .as_ref()
            .and_then(|m| m.annotations.as_ref())
            != desired_template
                .metadata
                .as_ref()
                .and_then(|m| m.annotations.as_ref())
        {
            return Ok(true);
        }

        let existing_pod_spec =
            existing_template
                .spec
                .as_ref()
                .ok_or(types::error::Error::InternalError {
                    msg: "Existing pod template missing spec".to_string(),
                })?;

        let desired_pod_spec =
            desired_template
                .spec
                .as_ref()
                .ok_or(types::error::Error::InternalError {
                    msg: "Desired pod template missing spec".to_string(),
                })?;

        // Check service account
        if existing_pod_spec.service_account_name != desired_pod_spec.service_account_name {
            return Ok(true);
        }

        // Operator-created ServiceAccounts do not require Kubernetes API access. Compare this
        // field only for the default ServiceAccount so custom workload identity webhooks remain
        // free to manage token projection without causing a reconcile loop.
        if self.spec.service_account_name.is_none()
            && existing_pod_spec.automount_service_account_token
                != desired_pod_spec.automount_service_account_token
        {
            return Ok(true);
        }

        if self.spec.service_account_name.is_none()
            && !uses_unpartitioned_rolling_update(existing_spec.update_strategy.as_ref())
        {
            return Ok(true);
        }

        // Check scheduler
        if existing_pod_spec.scheduler_name != desired_pod_spec.scheduler_name {
            return Ok(true);
        }

        // Check priority class
        if existing_pod_spec.priority_class_name != desired_pod_spec.priority_class_name {
            return Ok(true);
        }

        // Check image pull secrets
        if existing_pod_spec.image_pull_secrets != desired_pod_spec.image_pull_secrets {
            return Ok(true);
        }

        // Check pod volumes (TLS Secret/CA mounts live here).
        if serde_json::to_value(&existing_pod_spec.volumes)?
            != serde_json::to_value(&desired_pod_spec.volumes)?
        {
            return Ok(true);
        }

        // Check node selector
        if existing_pod_spec.node_selector != desired_pod_spec.node_selector {
            return Ok(true);
        }

        // Check affinity (compare as JSON to handle deep equality)
        if serde_json::to_value(&existing_pod_spec.affinity)?
            != serde_json::to_value(&desired_pod_spec.affinity)?
        {
            return Ok(true);
        }

        // Check tolerations
        if serde_json::to_value(&existing_pod_spec.tolerations)?
            != serde_json::to_value(&desired_pod_spec.tolerations)?
        {
            return Ok(true);
        }

        // Check topology spread constraints
        if serde_json::to_value(&existing_pod_spec.topology_spread_constraints)?
            != serde_json::to_value(&desired_pod_spec.topology_spread_constraints)?
        {
            return Ok(true);
        }

        // Check pod security context (runAsUser, runAsGroup, fsGroup, runAsNonRoot)
        if serde_json::to_value(&existing_pod_spec.security_context)?
            != serde_json::to_value(&desired_pod_spec.security_context)?
        {
            return Ok(true);
        }

        // Compare container specs
        if existing_pod_spec.containers.is_empty() || desired_pod_spec.containers.is_empty() {
            return Err(types::error::Error::InternalError {
                msg: "Pod spec missing container".to_string(),
            });
        }

        let existing_container = &existing_pod_spec.containers[0];
        let desired_container = &desired_pod_spec.containers[0];

        // Check image
        if existing_container.image != desired_container.image {
            return Ok(true);
        }

        // Check image pull policy
        if existing_container.image_pull_policy != desired_container.image_pull_policy {
            return Ok(true);
        }

        // Check RustFS container security context.
        if serde_json::to_value(&existing_container.security_context)?
            != serde_json::to_value(&desired_container.security_context)?
        {
            return Ok(true);
        }

        // Check environment variables (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.env)?
            != serde_json::to_value(&desired_container.env)?
        {
            return Ok(true);
        }

        // Check resources (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.resources)?
            != serde_json::to_value(&desired_container.resources)?
        {
            return Ok(true);
        }

        // Check lifecycle hooks
        if serde_json::to_value(&existing_container.lifecycle)?
            != serde_json::to_value(&desired_container.lifecycle)?
        {
            return Ok(true);
        }

        // Check volume mounts (compare as JSON for deep equality)
        if serde_json::to_value(&existing_container.volume_mounts)?
            != serde_json::to_value(&desired_container.volume_mounts)?
        {
            return Ok(true);
        }

        // If we reach here, no updates are needed
        Ok(false)
    }

    fn uses_implicit_local_kms_key_directory(&self) -> bool {
        self.spec.encryption.as_ref().is_some_and(|enc| {
            enc.enabled
                && enc.backend == KmsBackendType::Local
                && enc
                    .local
                    .as_ref()
                    .and_then(|local| local.key_directory.as_deref())
                    .is_none()
        })
    }

    fn blocked_local_kms_implicit_default_migration(
        &self,
        existing_spec: &v1::StatefulSetSpec,
        desired_spec: &v1::StatefulSetSpec,
    ) -> Option<(String, String)> {
        if !self.uses_implicit_local_kms_key_directory() {
            return None;
        }

        let existing_container = first_container(existing_spec)?;
        let existing_key_dirs = local_kms_key_dir_env_values(existing_container);
        if !existing_key_dirs.contains(&LEGACY_LOCAL_KMS_KEY_DIR) {
            return None;
        }

        let desired_container = first_container(desired_spec)?;
        let desired_key_dirs = local_kms_key_dir_env_values(desired_container);
        let desired_key_dir = desired_key_dirs
            .iter()
            .copied()
            .find(|dir| *dir != LEGACY_LOCAL_KMS_KEY_DIR)
            .or_else(|| desired_key_dirs.first().copied())?;
        (desired_key_dir != LEGACY_LOCAL_KMS_KEY_DIR).then(|| {
            (
                LEGACY_LOCAL_KMS_KEY_DIR.to_string(),
                desired_key_dir.to_string(),
            )
        })
    }

    /// Validates that a StatefulSet update is safe by checking for changes to
    /// immutable fields that would cause API rejection.
    ///
    /// StatefulSet has several immutable fields that cannot be changed after creation:
    /// - spec.selector: Pod selector labels cannot be modified
    /// - spec.volumeClaimTemplates: PVC templates cannot be modified
    /// - spec.serviceName: Headless service name cannot be changed
    ///
    /// # Returns
    /// - `Ok(())` if the update is safe
    /// - `Err` if the update would modify immutable fields
    pub fn validate_statefulset_update(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
    ) -> Result<(), types::error::Error> {
        self.validate_statefulset_update_with_tls_plan(existing, pool, &TlsPlan::disabled())
    }

    pub fn validate_statefulset_update_with_tls_plan(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
        tls_plan: &TlsPlan,
    ) -> Result<(), types::error::Error> {
        self.validate_statefulset_update_with_tls_plan_and_cluster_domain(
            existing,
            pool,
            tls_plan,
            cluster_dns::DEFAULT_CLUSTER_DOMAIN,
        )
    }

    pub(crate) fn validate_statefulset_update_with_tls_plan_and_cluster_domain(
        &self,
        existing: &v1::StatefulSet,
        pool: &Pool,
        tls_plan: &TlsPlan,
        cluster_domain: &str,
    ) -> Result<(), types::error::Error> {
        let desired =
            self.new_statefulset_with_tls_plan_and_cluster_domain(pool, tls_plan, cluster_domain)?;

        let existing_spec = existing
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Existing StatefulSet missing spec".to_string(),
            })?;

        let desired_spec = desired
            .spec
            .as_ref()
            .ok_or(types::error::Error::InternalError {
                msg: "Desired StatefulSet missing spec".to_string(),
            })?;

        let ss_name = existing
            .metadata
            .name
            .as_ref()
            .unwrap_or(&"<unknown>".to_string())
            .clone();

        if let Some((existing_key_dir, desired_key_dir)) =
            self.blocked_local_kms_implicit_default_migration(existing_spec, desired_spec)
        {
            return Err(types::error::Error::KmsMigrationBlocked {
                name: self.name(),
                message: format!(
                    "Local KMS default key directory migration for StatefulSet '{}' is blocked: existing pods use legacy non-PVC path '{}', while the desired implicit default is '{}'. Copy existing key files and .master-key.salt into '{}', then set spec.encryption.local.keyDirectory explicitly to that PVC-backed path before rolling the StatefulSet.",
                    ss_name, existing_key_dir, desired_key_dir, desired_key_dir
                ),
            });
        }

        // MinIO-compatible expansion model: an existing pool's server count is
        // immutable. Horizontal capacity expansion must add a new pool.
        if existing_spec.replicas != desired_spec.replicas {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.replicas".to_string(),
                message: "Cannot change pool servers for an existing StatefulSet. Add a new pool to expand capacity.".to_string(),
            });
        }

        // Validate selector is unchanged (immutable field)
        if serde_json::to_value(&existing_spec.selector)?
            != serde_json::to_value(&desired_spec.selector)?
        {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.selector".to_string(),
                message: "StatefulSet selector cannot be modified. Pool name may have changed."
                    .to_string(),
            });
        }

        // Validate serviceName is unchanged (immutable field)
        if existing_spec.service_name != desired_spec.service_name {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.serviceName".to_string(),
                message: "StatefulSet serviceName cannot be modified.".to_string(),
            });
        }

        // Validate volumeClaimTemplates are unchanged (immutable field)
        // Note: This is a simplified check. In reality, you can only change certain fields
        // like storage size (depending on storage class), but template structure and names cannot change.
        let existing_vcts = existing_spec.volume_claim_templates.as_ref();
        let desired_vcts = desired_spec.volume_claim_templates.as_ref();

        // Check if the number of volume claim templates changed
        let existing_vct_count = existing_vcts.map(|v| v.len()).unwrap_or(0);
        let desired_vct_count = desired_vcts.map(|v| v.len()).unwrap_or(0);

        if existing_vct_count != desired_vct_count {
            return Err(types::error::Error::ImmutableFieldModified {
                name: ss_name,
                field: "spec.volumeClaimTemplates".to_string(),
                message: format!(
                    "Cannot change volumesPerServer from {} to {}. This would modify volumeClaimTemplates which is immutable.",
                    existing_vct_count, desired_vct_count
                ),
            });
        }

        // Check if volume claim template names changed (indicates structure change)
        if let (Some(existing_vcts), Some(desired_vcts)) = (existing_vcts, desired_vcts) {
            for (i, (existing_vct, desired_vct)) in
                existing_vcts.iter().zip(desired_vcts.iter()).enumerate()
            {
                let existing_name = existing_vct.metadata.name.as_deref().unwrap_or("");
                let desired_name = desired_vct.metadata.name.as_deref().unwrap_or("");

                if existing_name != desired_name {
                    return Err(types::error::Error::ImmutableFieldModified {
                        name: ss_name,
                        field: format!("spec.volumeClaimTemplates[{}].metadata.name", i),
                        message: format!(
                            "Volume claim template name changed from '{}' to '{}'. This is not allowed.",
                            existing_name, desired_name
                        ),
                    });
                }

                // Check if storage class changed (also problematic)
                let existing_sc = existing_vct
                    .spec
                    .as_ref()
                    .and_then(|s| s.storage_class_name.as_ref());
                let desired_sc = desired_vct
                    .spec
                    .as_ref()
                    .and_then(|s| s.storage_class_name.as_ref());

                if existing_sc != desired_sc {
                    return Err(types::error::Error::ImmutableFieldModified {
                        name: ss_name.clone(),
                        field: format!("spec.volumeClaimTemplates[{}].spec.storageClassName", i),
                        message: format!(
                            "Storage class changed from '{:?}' to '{:?}'. This is not allowed.",
                            existing_sc, desired_sc
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        DEFAULT_FS_GROUP, DEFAULT_RUN_AS_GROUP, DEFAULT_RUN_AS_USER,
        MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH, RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION,
        uses_unpartitioned_rolling_update, validate_declared_app_armor_profile,
        validate_declared_seccomp_profile,
    };
    use crate::types::v1alpha1::encryption::{
        EncryptionConfig, KmsBackendType, LocalKmsConfig, LocalKmsMasterKeySecretRef,
    };
    use crate::types::v1alpha1::logging::{LoggingConfig, LoggingMode};
    use crate::types::v1alpha1::security_context::{MAX_KUBERNETES_ID, PodSecurityContextOverride};
    use crate::types::v1alpha1::tenant::{RpcSecretRef, Tenant};
    use crate::types::v1alpha1::tls::{SecretKeyReference, TlsPlan};
    use k8s_openapi::api::apps::v1;
    use k8s_openapi::api::core::v1 as corev1;

    fn image_pull_secret(name: &str) -> corev1::LocalObjectReference {
        corev1::LocalObjectReference {
            name: name.to_string(),
        }
    }

    fn tls_plan(hash: &str) -> TlsPlan {
        TlsPlan::for_test("server-tls", hash)
    }

    fn runtime_default_seccomp_profile() -> corev1::SeccompProfile {
        corev1::SeccompProfile {
            type_: "RuntimeDefault".to_string(),
            ..Default::default()
        }
    }

    fn acknowledge_runtime_default_image(tenant: &mut Tenant, image: &str) {
        tenant.metadata.annotations.get_or_insert_default().insert(
            RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION.to_string(),
            image.to_string(),
        );
    }

    fn invalid_security_profile_error_message(tenant: &Tenant) -> String {
        match tenant.validate_workload_security_compatibility() {
            Err(crate::types::error::Error::InvalidWorkloadSecurityProfile { message, .. }) => {
                message
            }
            Ok(()) => panic!("workload security validation should fail"),
            Err(error) => panic!("unexpected workload security error: {error}"),
        }
    }

    type SetDeclaredId = fn(&mut Tenant, i64);

    fn declared_security_context_id_fields() -> Vec<(&'static str, SetDeclaredId)> {
        vec![
            ("spec.securityContext.runAsUser", |tenant, value| {
                tenant
                    .spec
                    .security_context
                    .get_or_insert_default()
                    .run_as_user = Some(value);
            }),
            ("spec.securityContext.runAsGroup", |tenant, value| {
                tenant
                    .spec
                    .security_context
                    .get_or_insert_default()
                    .run_as_group = Some(value);
            }),
            ("spec.securityContext.fsGroup", |tenant, value| {
                tenant
                    .spec
                    .security_context
                    .get_or_insert_default()
                    .fs_group = Some(value);
            }),
            (
                "spec.containerSecurityContext.runAsUser",
                |tenant, value| {
                    tenant
                        .spec
                        .container_security_context
                        .get_or_insert_default()
                        .run_as_user = Some(value);
                },
            ),
            (
                "spec.containerSecurityContext.runAsGroup",
                |tenant, value| {
                    tenant
                        .spec
                        .container_security_context
                        .get_or_insert_default()
                        .run_as_group = Some(value);
                },
            ),
            (
                "spec.pools[name=pool-0].securityContext.runAsUser",
                |tenant, value| {
                    tenant.spec.pools[0]
                        .security_context
                        .get_or_insert_default()
                        .run_as_user = Some(value);
                },
            ),
            (
                "spec.pools[name=pool-0].securityContext.runAsGroup",
                |tenant, value| {
                    tenant.spec.pools[0]
                        .security_context
                        .get_or_insert_default()
                        .run_as_group = Some(value);
                },
            ),
            (
                "spec.pools[name=pool-0].securityContext.fsGroup",
                |tenant, value| {
                    tenant.spec.pools[0]
                        .security_context
                        .get_or_insert_default()
                        .fs_group = Some(value);
                },
            ),
            (
                "spec.pools[name=pool-0].containerSecurityContext.runAsUser",
                |tenant, value| {
                    tenant.spec.pools[0]
                        .container_security_context
                        .get_or_insert_default()
                        .run_as_user = Some(value);
                },
            ),
            (
                "spec.pools[name=pool-0].containerSecurityContext.runAsGroup",
                |tenant, value| {
                    tenant.spec.pools[0]
                        .container_security_context
                        .get_or_insert_default()
                        .run_as_group = Some(value);
                },
            ),
        ]
    }

    #[test]
    fn declared_security_context_ids_reject_out_of_range_values_at_every_scope() {
        for (field_path, set_value) in declared_security_context_id_fields() {
            for value in [-1, MAX_KUBERNETES_ID + 1] {
                let mut tenant = crate::tests::create_test_tenant(None, None);
                set_value(&mut tenant, value);

                let message = invalid_security_profile_error_message(&tenant);
                assert!(
                    message.contains(field_path),
                    "missing field path for value {value}: {message}"
                );
                assert!(
                    message.contains("between 0 and 2147483647, inclusive"),
                    "missing valid range for value {value}: {message}"
                );

                let render_error = tenant
                    .new_statefulset(&tenant.spec.pools[0])
                    .expect_err("an out-of-range workload ID must fail before rendering");
                assert!(matches!(
                    render_error,
                    crate::types::error::Error::InvalidWorkloadSecurityProfile { message, .. }
                        if message.contains(field_path)
                ));
            }
        }
    }

    #[test]
    fn declared_security_context_ids_accept_kubernetes_boundaries_at_every_scope() {
        for (field_path, set_value) in declared_security_context_id_fields() {
            for value in [0, MAX_KUBERNETES_ID] {
                let mut tenant = crate::tests::create_test_tenant(None, None);
                set_value(&mut tenant, value);

                tenant
                    .validate_workload_security_compatibility()
                    .unwrap_or_else(|error| {
                        panic!("{field_path} should accept boundary value {value}: {error}")
                    });
                tenant
                    .new_statefulset(&tenant.spec.pools[0])
                    .unwrap_or_else(|error| {
                        panic!("{field_path} should render boundary value {value}: {error}")
                    });
            }
        }
    }

    #[test]
    fn seccomp_localhost_profile_uses_kubernetes_descending_path_rules() {
        validate_declared_seccomp_profile(
            "spec.securityContext.seccompProfile",
            "Localhost",
            Some("profiles/rustfs.json"),
        )
        .expect("relative descending path should be valid");

        for (profile, detail) in [
            ("", "must be nonblank"),
            ("/profiles/rustfs.json", "must be a relative path"),
            ("profiles/../rustfs.json", "must not contain '..'"),
            ("../profiles/rustfs.json", "must not contain '..'"),
        ] {
            let error = validate_declared_seccomp_profile(
                "spec.securityContext.seccompProfile",
                "Localhost",
                Some(profile),
            )
            .expect_err("invalid seccomp Localhost path should be rejected");
            assert!(
                error.contains(detail),
                "unexpected validation error: {error}"
            );
        }
    }

    #[test]
    fn app_armor_localhost_profile_uses_kubernetes_name_rules() {
        validate_declared_app_armor_profile(
            "spec.containerSecurityContext.appArmorProfile",
            "Localhost",
            Some("profiles/rustfs"),
        )
        .expect("unpadded AppArmor profile name should be valid");

        let maximum_length = "a".repeat(MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH);
        validate_declared_app_armor_profile(
            "spec.containerSecurityContext.appArmorProfile",
            "Localhost",
            Some(&maximum_length),
        )
        .expect("a 4095-byte AppArmor Localhost name should be valid");

        for (profile, detail) in [
            ("", "must be nonblank"),
            (" profiles/rustfs", "must not be padded with whitespace"),
            ("profiles/rustfs ", "must not be padded with whitespace"),
        ] {
            let error = validate_declared_app_armor_profile(
                "spec.containerSecurityContext.appArmorProfile",
                "Localhost",
                Some(profile),
            )
            .expect_err("invalid AppArmor Localhost name should be rejected");
            assert!(
                error.contains(detail),
                "unexpected validation error: {error}"
            );
        }

        let oversized = "a".repeat(MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH + 1);
        let error = validate_declared_app_armor_profile(
            "spec.containerSecurityContext.appArmorProfile",
            "Localhost",
            Some(&oversized),
        )
        .expect_err("oversized AppArmor Localhost name should be rejected");
        assert!(
            error.contains("must be at most 4095 bytes"),
            "unexpected validation error: {error}"
        );
    }

    fn env_value<'a>(container: &'a corev1::Container, name: &str) -> Option<&'a str> {
        container
            .env
            .as_ref()?
            .iter()
            .find(|var| var.name == name)?
            .value
            .as_deref()
    }

    fn set_env_value(container: &mut corev1::Container, name: &str, value: &str) {
        let env = container
            .env
            .as_mut()
            .expect("container should have environment variables");
        if let Some(var) = env.iter_mut().find(|var| var.name == name) {
            var.value = Some(value.to_string());
        } else {
            env.push(corev1::EnvVar {
                name: name.to_string(),
                value: Some(value.to_string()),
                ..Default::default()
            });
        }
    }

    fn local_master_key_selector() -> LocalKmsMasterKeySecretRef {
        LocalKmsMasterKeySecretRef {
            name: "local-kms-master-key".to_string(),
            key: "local-master-key".to_string(),
        }
    }

    #[test]
    fn disabled_tls_statefulset_keeps_http_and_has_no_tls_wiring() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet without TLS");

        let template = statefulset.spec.unwrap().template;
        assert!(
            template
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.annotations.as_ref())
                .is_none_or(|annotations| !annotations.contains_key("operator.rustfs.com/tls-hash"))
        );

        let pod_spec = template.spec.unwrap();
        assert!(pod_spec.volumes.as_ref().is_none_or(|volumes| {
            !volumes
                .iter()
                .any(|volume| volume.name.starts_with("rustfs-tls"))
        }));

        let container = &pod_spec.containers[0];
        assert!(
            env_value(container, "RUSTFS_VOLUMES")
                .is_some_and(|value| value.starts_with("http://"))
        );
        assert!(env_value(container, "RUSTFS_TLS_PATH").is_none());
        assert_eq!(
            container
                .liveness_probe
                .as_ref()
                .and_then(|probe| probe.http_get.as_ref())
                .and_then(|http_get| http_get.scheme.as_deref()),
            Some("HTTP")
        );
        assert_eq!(
            container
                .readiness_probe
                .as_ref()
                .and_then(|probe| probe.http_get.as_ref())
                .and_then(|http_get| http_get.path.as_deref()),
            Some("/health/ready")
        );
        assert_eq!(
            container
                .startup_probe
                .as_ref()
                .and_then(|probe| probe.http_get.as_ref())
                .and_then(|http_get| http_get.scheme.as_deref()),
            Some("HTTP")
        );
        assert_eq!(
            container.termination_message_policy.as_deref(),
            Some("FallbackToLogsOnError")
        );
        assert!(container.volume_mounts.as_ref().is_none_or(|mounts| {
            !mounts
                .iter()
                .any(|mount| mount.name.starts_with("rustfs-tls"))
        }));
    }

    #[test]
    fn omitted_rpc_secret_leaves_rpc_auth_resolution_to_rustfs() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet without an RPC Secret");
        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];

        assert!(
            container
                .env
                .as_ref()
                .is_none_or(|env| env.iter().all(|var| var.name != "RUSTFS_RPC_SECRET"))
        );
    }

    #[test]
    fn rpc_secret_maps_selected_secret_key_and_owns_the_env_var() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.rpc_secret = Some(RpcSecretRef {
            name: "tenant-rpc-auth".to_string(),
            key: "rpc-secret".to_string(),
        });
        tenant.spec.env.push(corev1::EnvVar {
            name: "RUSTFS_RPC_SECRET".to_string(),
            value: Some("raw-override-must-not-win".to_string()),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with an RPC Secret");
        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];
        let rpc_env = container
            .env
            .as_ref()
            .unwrap()
            .iter()
            .filter(|var| var.name == "RUSTFS_RPC_SECRET")
            .collect::<Vec<_>>();

        assert_eq!(rpc_env.len(), 1);
        assert_eq!(rpc_env[0].value, None);
        assert_eq!(
            rpc_env[0]
                .value_from
                .as_ref()
                .and_then(|source| source.secret_key_ref.as_ref()),
            Some(&corev1::SecretKeySelector {
                name: "tenant-rpc-auth".to_string(),
                key: "rpc-secret".to_string(),
                optional: Some(false),
            })
        );
    }

    #[test]
    fn raw_rpc_secret_env_remains_supported_without_rpc_secret_ref() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.env.push(corev1::EnvVar {
            name: "RUSTFS_RPC_SECRET".to_string(),
            value: Some("legacy-explicit-rpc-secret".to_string()),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should preserve the legacy raw RPC Secret environment variable");
        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];

        assert_eq!(
            env_value(container, "RUSTFS_RPC_SECRET"),
            Some("legacy-explicit-rpc-secret")
        );
    }

    #[test]
    fn cert_manager_tls_statefulset_maps_secret_to_rustfs_tls_files() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset_with_tls_plan(pool, &tls_plan("sha256:test"))
            .expect("Should create StatefulSet with TLS");

        let template = statefulset.spec.unwrap().template;
        let annotations = template.metadata.unwrap().annotations.unwrap();
        assert_eq!(
            annotations.get("operator.rustfs.com/tls-hash"),
            Some(&"sha256:test".to_string())
        );

        let pod_spec = template.spec.unwrap();
        let volumes = pod_spec.volumes.unwrap_or_default();
        assert!(
            volumes
                .iter()
                .any(|volume| volume.name == "rustfs-tls-server")
        );
        let server_volume = volumes
            .iter()
            .find(|volume| volume.name == "rustfs-tls-server")
            .expect("TLS server volume should exist");
        let projected_items = server_volume
            .projected
            .as_ref()
            .and_then(|projected| projected.sources.as_ref())
            .expect("TLS server volume should be projected")
            .iter()
            .flat_map(|source| {
                source
                    .secret
                    .as_ref()
                    .and_then(|secret| secret.items.as_ref())
                    .into_iter()
                    .flatten()
            })
            .map(|item| (item.key.as_str(), item.path.as_str()))
            .collect::<Vec<_>>();
        assert!(projected_items.contains(&("tls.crt", "rustfs_cert.pem")));
        assert!(projected_items.contains(&("tls.key", "rustfs_key.pem")));
        assert!(projected_items.contains(&("ca.crt", "ca.crt")));

        let container = &pod_spec.containers[0];
        let env = container.env.as_ref().expect("TLS env should be present");
        assert!(env.iter().any(|var| {
            var.name == "RUSTFS_TLS_PATH" && var.value.as_deref() == Some("/var/run/rustfs/tls")
        }));
        assert!(env.iter().any(|var| {
            var.name == "RUSTFS_VOLUMES"
                && var
                    .value
                    .as_deref()
                    .is_some_and(|value| value.starts_with("https://"))
        }));

        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("TLS volume mounts should be present");
        assert!(mounts.iter().any(|mount| {
            mount.name == "rustfs-tls-server"
                && mount.mount_path == "/var/run/rustfs/tls"
                && mount.sub_path.is_none()
        }));

        assert_eq!(
            container
                .readiness_probe
                .as_ref()
                .and_then(|probe| probe.http_get.as_ref())
                .and_then(|http_get| http_get.scheme.as_deref()),
            Some("HTTPS")
        );
        assert_eq!(
            container.termination_message_policy.as_deref(),
            Some("FallbackToLogsOnError")
        );
    }

    #[test]
    fn single_node_single_disk_statefulset_uses_local_rustfs_volume() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.pools[0].persistence.volumes_per_server = 1;
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet for single-node single-disk");

        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];
        assert_eq!(
            env_value(container, "RUSTFS_VOLUMES"),
            Some("/data/rustfs0")
        );
        assert_eq!(
            container
                .volume_mounts
                .as_ref()
                .expect("data mount should be present")
                .iter()
                .filter(|mount| mount.mount_path == "/data/rustfs0")
                .count(),
            1
        );
    }

    #[test]
    fn local_kms_default_key_directory_uses_data_pvc_subdirectory() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");

        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];
        assert_eq!(
            env_value(container, "RUSTFS_KMS_KEY_DIR"),
            Some("/data/rustfs0/.kms-keys")
        );
        assert_eq!(env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR"), None);
        assert!(
            container
                .volume_mounts
                .as_ref()
                .expect("data mount should be present")
                .iter()
                .any(|mount| mount.mount_path == "/data/rustfs0")
        );
    }

    #[test]
    fn local_kms_default_key_directory_uses_custom_persistence_path() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.pools[0].persistence.path = Some("/mnt/rustfs".to_string());
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");

        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];
        assert_eq!(
            env_value(container, "RUSTFS_KMS_KEY_DIR"),
            Some("/mnt/rustfs/rustfs0/.kms-keys")
        );
        assert!(
            container
                .volume_mounts
                .as_ref()
                .expect("data mount should be present")
                .iter()
                .any(|mount| mount.mount_path == "/mnt/rustfs/rustfs0")
        );
    }

    #[test]
    fn local_kms_custom_key_directory_is_rendered_unchanged() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            local: Some(LocalKmsConfig {
                key_directory: Some("/data/rustfs0/custom-kms".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");

        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];
        assert_eq!(
            env_value(container, "RUSTFS_KMS_KEY_DIR"),
            Some("/data/rustfs0/custom-kms")
        );
        assert_eq!(env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR"), None);
    }

    #[test]
    fn local_kms_master_key_secret_ref_is_rendered() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            local: Some(LocalKmsConfig {
                master_key_secret_ref: Some(local_master_key_selector()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");

        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];
        let master_key_env = container
            .env
            .as_ref()
            .expect("env should be rendered")
            .iter()
            .find(|var| var.name == "RUSTFS_KMS_LOCAL_MASTER_KEY")
            .expect("local master key env should be rendered");
        let selector = master_key_env
            .value_from
            .as_ref()
            .and_then(|source| source.secret_key_ref.as_ref())
            .expect("local master key should come from Secret key ref");

        assert_eq!(selector.name, "local-kms-master-key");
        assert_eq!(selector.key, "local-master-key");
        assert_eq!(selector.optional, Some(false));
    }

    #[test]
    fn local_kms_allow_insecure_dev_defaults_is_explicitly_rendered() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            local: Some(LocalKmsConfig {
                allow_insecure_dev_defaults: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");

        let container = &statefulset.spec.unwrap().template.spec.unwrap().containers[0];
        assert_eq!(
            env_value(container, "RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS"),
            Some("true")
        );
        assert_eq!(env_value(container, "RUSTFS_KMS_LOCAL_MASTER_KEY"), None);
    }

    #[test]
    fn local_kms_statefulset_keeps_operator_managed_env_when_spec_env_conflicts() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        tenant.spec.env = vec![
            corev1::EnvVar {
                name: "RUSTFS_KMS_KEY_DIR".to_string(),
                value: Some("/data/kms-keys".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_KMS_LOCAL_KEY_DIR".to_string(),
                value: Some("/data/kms-keys".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_KMS_BACKEND".to_string(),
                value: Some("vault".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "CUSTOM_USER_ENV".to_string(),
                value: Some("kept".to_string()),
                ..Default::default()
            },
        ];
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");
        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];
        let env = container.env.as_ref().expect("env should be rendered");

        assert_eq!(
            env.iter()
                .filter(|var| var.name == "RUSTFS_KMS_KEY_DIR")
                .count(),
            1
        );
        assert_eq!(
            env_value(container, "RUSTFS_KMS_KEY_DIR"),
            Some("/data/rustfs0/.kms-keys")
        );
        assert_eq!(env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR"), None);
        assert_eq!(env_value(container, "RUSTFS_KMS_BACKEND"), Some("local"));
        assert_eq!(env_value(container, "CUSTOM_USER_ENV"), Some("kept"));
    }

    #[test]
    fn statefulset_drops_reserved_kms_env_even_when_encryption_is_disabled() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.encryption = None;
        tenant.spec.env = vec![
            corev1::EnvVar {
                name: "RUSTFS_KMS_LOCAL_MASTER_KEY".to_string(),
                value: Some("secret".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "CUSTOM_USER_ENV".to_string(),
                value: Some("kept".to_string()),
                ..Default::default()
            },
        ];
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];

        assert_eq!(env_value(container, "RUSTFS_KMS_LOCAL_MASTER_KEY"), None);
        assert_eq!(env_value(container, "CUSTOM_USER_ENV"), Some("kept"));
    }

    #[test]
    fn local_kms_implicit_default_migration_from_legacy_dir_is_blocked() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];
        let mut existing = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");
        let container = existing
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .containers
            .first_mut()
            .expect("Container should exist");
        set_env_value(container, "RUSTFS_KMS_KEY_DIR", "/data/kms-keys");
        set_env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR", "/data/kms-keys");

        let err = tenant
            .validate_statefulset_update(&existing, pool)
            .expect_err("implicit Local KMS default migration should be blocked");

        assert!(
            matches!(err, crate::types::error::Error::KmsMigrationBlocked { message, .. }
                if message.contains("/data/kms-keys")
                    && message.contains("/data/rustfs0/.kms-keys")
                    && message.contains("spec.encryption.local.keyDirectory"))
        );
    }

    #[test]
    fn local_kms_implicit_default_migration_detects_duplicate_legacy_env() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];
        let mut existing = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet with Local KMS");
        let container = existing
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .containers
            .first_mut()
            .expect("Container should exist");
        container
            .env
            .as_mut()
            .expect("container should have environment variables")
            .insert(
                0,
                corev1::EnvVar {
                    name: "RUSTFS_KMS_KEY_DIR".to_string(),
                    value: Some("/data/rustfs0/custom-old-env".to_string()),
                    ..Default::default()
                },
            );
        set_env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR", "/data/kms-keys");

        let err = tenant
            .validate_statefulset_update(&existing, pool)
            .expect_err("duplicate legacy Local KMS env should still be blocked");

        assert!(matches!(
            err,
            crate::types::error::Error::KmsMigrationBlocked { .. }
        ));
    }

    #[test]
    fn local_kms_explicit_key_directory_allows_user_controlled_migration() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            ..Default::default()
        });
        let mut existing = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet with Local KMS");
        let container = existing
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .containers
            .first_mut()
            .expect("Container should exist");
        set_env_value(container, "RUSTFS_KMS_KEY_DIR", "/data/kms-keys");
        set_env_value(container, "RUSTFS_KMS_LOCAL_KEY_DIR", "/data/kms-keys");

        tenant.spec.encryption = Some(EncryptionConfig {
            enabled: true,
            backend: KmsBackendType::Local,
            local: Some(LocalKmsConfig {
                key_directory: Some("/data/rustfs0/.kms-keys".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let pool = &tenant.spec.pools[0];

        tenant
            .validate_statefulset_update(&existing, pool)
            .expect("explicit Local KMS keyDirectory should allow user-controlled rollout");
    }

    #[test]
    fn mixed_pool_single_node_single_disk_uses_peer_dns_volume() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 1;
        tenant.spec.pools[0].persistence.volumes_per_server = 1;
        let mut second_pool = tenant.spec.pools[0].clone();
        second_pool.name = "pool-1".to_string();
        second_pool.servers = 2;
        second_pool.persistence.volumes_per_server = 1;
        tenant.spec.pools.push(second_pool);
        let pool = &tenant.spec.pools[1];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet for mixed pools");

        let pod_spec = statefulset.spec.unwrap().template.spec.unwrap();
        let container = &pod_spec.containers[0];
        let rustfs_volumes =
            env_value(container, "RUSTFS_VOLUMES").expect("RUSTFS_VOLUMES should be configured");
        assert!(!rustfs_volumes.starts_with("/data/rustfs0"));
        assert!(rustfs_volumes.contains(
            "http://test-tenant-pool-0-{0...0}.test-tenant-hl.default.svc.cluster.local:9000/data/rustfs{0...0}"
        ));
        assert!(rustfs_volumes.contains(
            "http://test-tenant-pool-1-{0...1}.test-tenant-hl.default.svc.cluster.local:9000/data/rustfs{0...0}"
        ));
    }

    #[test]
    fn rustfs_pool_volume_spec_uses_custom_cluster_domain() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.metadata.name = Some("prod-rustfs".to_string());
        tenant.spec.pools[0].name = "mse-nvme-500".to_string();
        tenant.spec.pools[0].servers = 3;
        tenant.spec.pools[0].persistence.volumes_per_server = 1;
        let pool = &tenant.spec.pools[0];

        let volume_spec = tenant.rustfs_pool_volume_spec(pool, "https", "mse", "k8s.mse.cloud");

        assert_eq!(
            volume_spec,
            "https://prod-rustfs-mse-nvme-500-{0...2}.prod-rustfs-hl.mse.svc.k8s.mse.cloud:9000/data/rustfs{0...0}"
        );
    }

    #[test]
    fn tls_statefulset_keeps_operator_managed_env_when_spec_env_conflicts() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.env = vec![
            corev1::EnvVar {
                name: "RUSTFS_TLS_PATH".to_string(),
                value: Some("/wrong/tls".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_VOLUMES".to_string(),
                value: Some("http://wrong.example/rustfs0".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_TRUST_SYSTEM_CA".to_string(),
                value: Some("false".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_TRUST_LEAF_CERT_AS_CA".to_string(),
                value: Some("false".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_SERVER_MTLS_ENABLE".to_string(),
                value: Some("false".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "CUSTOM_USER_ENV".to_string(),
                value: Some("kept".to_string()),
                ..Default::default()
            },
        ];
        let pool = &tenant.spec.pools[0];
        let plan = TlsPlan::rollout(
            "/var/run/rustfs/tls".to_string(),
            "sha256:reserved-env".to_string(),
            "server-tls".to_string(),
            Some("ca.crt".to_string()),
            None,
            Some(SecretKeyReference {
                name: "client-ca".to_string(),
                key: "ca.crt".to_string(),
            }),
            true,
            true,
            true,
            None,
        );

        let statefulset = tenant
            .new_statefulset_with_tls_plan(pool, &plan)
            .expect("Should create StatefulSet with TLS");

        let container = &statefulset
            .spec
            .as_ref()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_ref()
            .expect("Pod template should have spec")
            .containers[0];
        let env = container.env.as_ref().expect("TLS env should be present");
        for name in [
            "RUSTFS_TLS_PATH",
            "RUSTFS_VOLUMES",
            "RUSTFS_TRUST_SYSTEM_CA",
            "RUSTFS_TRUST_LEAF_CERT_AS_CA",
            "RUSTFS_SERVER_MTLS_ENABLE",
        ] {
            assert_eq!(
                env.iter().filter(|var| var.name == name).count(),
                1,
                "reserved env var {name} should appear exactly once"
            );
        }
        assert_eq!(
            env_value(container, "RUSTFS_TLS_PATH"),
            Some("/var/run/rustfs/tls")
        );
        assert!(
            env_value(container, "RUSTFS_VOLUMES")
                .is_some_and(|value| value.starts_with("https://") && !value.contains("wrong"))
        );
        assert_eq!(env_value(container, "RUSTFS_TRUST_SYSTEM_CA"), Some("true"));
        assert_eq!(
            env_value(container, "RUSTFS_TRUST_LEAF_CERT_AS_CA"),
            Some("true")
        );
        assert_eq!(
            env_value(container, "RUSTFS_SERVER_MTLS_ENABLE"),
            Some("true")
        );
        assert_eq!(env_value(container, "CUSTOM_USER_ENV"), Some("kept"));
    }

    #[test]
    fn tls_hash_annotation_change_triggers_statefulset_update() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset_with_tls_plan(pool, &tls_plan("sha256:old"))
            .expect("Should create StatefulSet with TLS");

        let needs_update = tenant
            .statefulset_needs_update_with_tls_plan(&statefulset, pool, &tls_plan("sha256:new"))
            .expect("Should compare StatefulSet");

        assert!(needs_update, "TLS hash change should roll the pod template");
    }

    // Test: Pod runs as non-root with proper security context
    #[test]
    fn test_statefulset_sets_security_context() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");
        assert_eq!(
            pod_spec.containers[0].image.as_deref(),
            Some("rustfs/rustfs:1.0.0-beta.10")
        );

        let security_context = pod_spec
            .security_context
            .as_ref()
            .expect("Pod should have securityContext");

        assert_eq!(
            security_context.run_as_user,
            Some(DEFAULT_RUN_AS_USER),
            "Pod should run as RustFS user"
        );
        assert_eq!(
            security_context.run_as_group,
            Some(DEFAULT_RUN_AS_GROUP),
            "Pod should use RustFS primary group"
        );
        assert_eq!(
            security_context.fs_group,
            Some(DEFAULT_FS_GROUP),
            "Mounted volumes should be owned by RustFS group"
        );
        assert_eq!(
            security_context.fs_group_change_policy,
            Some("OnRootMismatch".to_string()),
            "fsGroup change policy should be set for PVC mounts"
        );
        assert_eq!(security_context.run_as_non_root, Some(true));
        assert_eq!(
            security_context
                .seccomp_profile
                .as_ref()
                .map(|profile| profile.type_.as_str()),
            Some("RuntimeDefault")
        );

        let container_security_context = pod_spec.containers[0]
            .security_context
            .as_ref()
            .expect("RustFS container should have securityContext");
        assert_eq!(
            container_security_context.allow_privilege_escalation,
            Some(false)
        );
        assert_eq!(
            container_security_context
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.drop.as_ref()),
            Some(&vec!["ALL".to_string()])
        );
        assert_eq!(
            container_security_context.read_only_root_filesystem, None,
            "readOnlyRootFilesystem is configurable but not required by restricted"
        );
    }

    #[test]
    fn known_tokio_io_uring_images_are_blocked_with_runtime_default_seccomp() {
        for image in [
            "rustfs/rustfs:1.0.0-alpha.99",
            "rustfs/rustfs:1.0.0-beta.8",
            "docker.io/rustfs/rustfs:1.0.0-beta.8-glibc",
            "ghcr.io/rustfs/rustfs:1.0.0-beta.8-glibc",
            "quay.io/rustfs/rustfs:1.0.0-beta.8-glibc",
            "registry-1.docker.io/rustfs/rustfs:v1.0.0-beta.7-glibc",
        ] {
            let mut tenant = crate::tests::create_test_tenant(None, None);
            tenant.spec.image = Some(image.to_string());

            let error = match tenant.validate_workload_security_compatibility() {
                Ok(()) => panic!("image {image} should be blocked"),
                Err(error) => error,
            };

            assert!(matches!(
                error,
                crate::types::error::Error::WorkloadSecurityIncompatible { message, .. }
                    if message.contains("rustfs/rustfs#4364")
                        && message.contains("pool-0")
                        && message.contains(image)
            ));
        }

        let image = "rustfs/rustfs:1.0.0-beta.8";
        let mut acknowledged_incompatible = crate::tests::create_test_tenant(None, None);
        acknowledged_incompatible.spec.image = Some(image.to_string());
        acknowledge_runtime_default_image(&mut acknowledged_incompatible, image);
        let error = acknowledged_incompatible
            .validate_workload_security_compatibility()
            .expect_err("an image acknowledgement must not override a known incompatibility");
        assert!(matches!(
            error,
            crate::types::error::Error::WorkloadSecurityIncompatible { message, .. }
                if message.contains("rustfs/rustfs#4364")
        ));
    }

    #[test]
    fn compatible_localhost_profile_allows_legacy_rustfs_image() {
        for image in [
            "docker.io/rustfs/rustfs:1.0.0-alpha.99",
            "docker.io/rustfs/rustfs:1.0.0-beta.8",
        ] {
            let mut tenant = crate::tests::create_test_tenant(None, None);
            tenant.spec.image = Some(image.to_string());
            tenant.spec.security_context = Some(PodSecurityContextOverride {
                seccomp_profile: Some(corev1::SeccompProfile {
                    localhost_profile: Some("profiles/rustfs-io-uring.json".to_string()),
                    type_: "Localhost".to_string(),
                }),
                ..Default::default()
            });

            tenant
                .validate_workload_security_compatibility()
                .unwrap_or_else(|error| {
                    panic!("image {image} should allow an explicit Localhost profile: {error}")
                });
        }
    }

    #[test]
    fn verified_compatible_rustfs_release_tags_are_accepted() {
        for image in [
            "rustfs/rustfs:1.0.0-beta.9",
            "docker.io/rustfs/rustfs:1.0.0-beta.9-glibc",
            "ghcr.io/rustfs/rustfs:1.0.0-beta.9-glibc",
            "quay.io/rustfs/rustfs:1.0.0-beta.9-glibc",
            "index.docker.io/rustfs/rustfs:v1.0.0-beta.10-glibc",
            "rustfs/rustfs:1.0.0",
            "rustfs/rustfs:1.0.1-glibc",
        ] {
            let mut tenant = crate::tests::create_test_tenant(None, None);
            tenant.spec.image = Some(image.to_string());

            tenant
                .validate_workload_security_compatibility()
                .unwrap_or_else(|error| panic!("image {image} should not be blocked: {error}"));
        }
    }

    #[test]
    fn unverifiable_images_are_blocked_with_implicit_runtime_default_seccomp() {
        for image in [
            "rustfs/rustfs:latest",
            "rustfs/rustfs",
            "rustfs/rustfs@sha256:0123456789abcdef",
            "rustfs/rustfs:1.0.0-beta.10@sha256:0123456789abcdef",
            "rustfs/rustfs:1.0.0-beta.10-preview.5",
            "rustfs/rustfs:nightly",
            "rustfs/rustfs:01.0.0",
            "rustfs/rustfs:1.00.0",
            "rustfs/rustfs:1.0.00",
            "rustfs/rustfs:1.0.0-beta.09",
            "rustfs/rustfs:1.0.0-beta.09-glibc",
            "registry.example.com/rustfs/rustfs:1.0.0-beta.10",
        ] {
            let mut tenant = crate::tests::create_test_tenant(None, None);
            tenant.spec.image = Some(image.to_string());

            let error = tenant
                .validate_workload_security_compatibility()
                .expect_err("unverifiable image should be blocked with implicit RuntimeDefault");
            assert!(matches!(
                error,
                crate::types::error::Error::WorkloadSecurityIncompatible { message, .. }
                    if message.contains(image)
                        && message.contains("cannot be verified")
                        && message.contains(RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION)
                        && message.contains("pool-0")
            ));
        }
    }

    #[test]
    fn runtime_default_image_ack_must_exactly_match_the_resolved_image() {
        let image = "rustfs/rustfs:latest";
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(image.to_string());
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(runtime_default_seccomp_profile()),
            ..Default::default()
        });

        tenant
            .validate_workload_security_compatibility()
            .expect_err("an explicit RuntimeDefault profile must not acknowledge the image");

        acknowledge_runtime_default_image(&mut tenant, "rustfs/rustfs:nightly");
        tenant
            .validate_workload_security_compatibility()
            .expect_err("a mismatched image acknowledgement must not be accepted");

        acknowledge_runtime_default_image(&mut tenant, image);
        tenant
            .validate_workload_security_compatibility()
            .expect("a matching acknowledgement should permit an unverifiable mutable image");

        tenant.spec.image = Some("rustfs/rustfs:nightly".to_string());
        tenant
            .validate_workload_security_compatibility()
            .expect_err("changing the image must invalidate its previous acknowledgement");
    }

    #[test]
    fn digest_is_authoritative_over_a_legacy_looking_tag() {
        // Kubernetes pulls tag@digest references by digest, so the tag cannot prove which build
        // runs. A matching acknowledgement may therefore permit even a beta.8-looking digest.
        let image = "rustfs/rustfs:1.0.0-beta.8@sha256:0123456789abcdef";
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(image.to_string());
        acknowledge_runtime_default_image(&mut tenant, image);

        tenant
            .validate_workload_security_compatibility()
            .expect("a matching acknowledgement should permit a digest-qualified image");
    }

    #[test]
    fn invalid_declared_security_profiles_are_rejected_before_image_compatibility() {
        let legacy_image = "rustfs/rustfs:1.0.0-beta.8".to_string();
        let mut cases = Vec::new();

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "Localhost".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.securityContext.seccompProfile.localhostProfile",
            "must be nonblank",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("profiles/invalid.json".to_string()),
                type_: "RuntimeDefault".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.containerSecurityContext.seccompProfile.localhostProfile",
            "must be omitted",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                type_: "Invalid".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.containerSecurityContext.appArmorProfile.type",
            "must be RuntimeDefault, Localhost, or Unconfined",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.pools[0].security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("   ".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.pools[name=pool-0].securityContext.seccompProfile.localhostProfile",
            "must be nonblank",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "Unknown".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.pools[name=pool-0].containerSecurityContext.seccompProfile.type",
            "must be RuntimeDefault, Localhost, or Unconfined",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("/profiles/rustfs.json".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.securityContext.seccompProfile.localhostProfile",
            "must be a relative path",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("profiles/../rustfs.json".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.pools[name=pool-0].containerSecurityContext.seccompProfile.localhostProfile",
            "must not contain '..'",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                localhost_profile: Some(" profiles/rustfs".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.containerSecurityContext.appArmorProfile.localhostProfile",
            "must not be padded with whitespace",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image.clone());
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                localhost_profile: Some("a".repeat(MAX_APP_ARMOR_LOCALHOST_PROFILE_LENGTH + 1)),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.pools[name=pool-0].containerSecurityContext.appArmorProfile.localhostProfile",
            "must be at most 4095 bytes",
        ));

        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some(legacy_image);
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                localhost_profile: Some("profiles/invalid".to_string()),
                type_: "Unconfined".to_string(),
            }),
            ..Default::default()
        });
        cases.push((
            tenant,
            "spec.pools[name=pool-0].containerSecurityContext.appArmorProfile.localhostProfile",
            "must be omitted",
        ));

        for (tenant, field, detail) in cases {
            let message = invalid_security_profile_error_message(&tenant);
            assert!(message.contains(field), "missing field path in: {message}");
            assert!(
                message.contains(detail),
                "missing validation detail in: {message}"
            );
            assert!(
                !message.contains("io_uring"),
                "profile validation must run before image compatibility: {message}"
            );
        }
    }

    #[test]
    fn valid_declared_security_profiles_are_accepted_at_every_scope() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some("rustfs/rustfs:1.0.0-beta.8-glibc".to_string());
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                type_: "RuntimeDefault".to_string(),
                ..Default::default()
            }),
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "Unconfined".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.pools[0].security_context = Some(PodSecurityContextOverride {
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "Unconfined".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                localhost_profile: Some("profiles/rustfs-apparmor".to_string()),
                type_: "Localhost".to_string(),
            }),
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("profiles/rustfs-seccomp.json".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });

        tenant
            .validate_workload_security_compatibility()
            .expect("valid Localhost container override should allow the legacy image");
    }

    #[test]
    fn security_context_overrides_merge_from_tenant_and_pool() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            run_as_user: Some(20_001),
            run_as_non_root: Some(false),
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("profiles/rustfs.json".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            capabilities: Some(corev1::Capabilities {
                add: Some(vec!["NET_BIND_SERVICE".to_string()]),
                ..Default::default()
            }),
            read_only_root_filesystem: Some(true),
            ..Default::default()
        });
        tenant.spec.pools[0].security_context = Some(PodSecurityContextOverride {
            run_as_group: Some(30_001),
            ..Default::default()
        });
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            allow_privilege_escalation: Some(true),
            capabilities: Some(corev1::Capabilities {
                drop: Some(vec![]),
                ..Default::default()
            }),
            ..Default::default()
        });

        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet");
        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");
        let pod_context = pod_spec
            .security_context
            .expect("Pod should have securityContext");

        assert_eq!(pod_context.run_as_user, Some(20_001));
        assert_eq!(pod_context.run_as_group, Some(30_001));
        assert_eq!(pod_context.fs_group, Some(DEFAULT_FS_GROUP));
        assert_eq!(pod_context.run_as_non_root, Some(false));
        assert_eq!(
            pod_context
                .seccomp_profile
                .as_ref()
                .map(|profile| profile.type_.as_str()),
            Some("Localhost")
        );

        let container_context = pod_spec.containers[0]
            .security_context
            .as_ref()
            .expect("RustFS container should have securityContext");
        assert_eq!(container_context.allow_privilege_escalation, Some(true));
        assert_eq!(container_context.read_only_root_filesystem, Some(true));
        assert_eq!(container_context.run_as_non_root, None);
        assert_eq!(container_context.seccomp_profile, None);
        assert_eq!(
            container_context
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.add.as_ref()),
            Some(&vec!["NET_BIND_SERVICE".to_string()])
        );
        assert_eq!(
            container_context
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.drop.as_ref()),
            Some(&vec![])
        );
    }

    #[test]
    fn partial_container_override_preserves_safe_defaults() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            read_only_root_filesystem: Some(true),
            ..Default::default()
        });

        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet");
        let context = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec")
            .containers[0]
            .security_context
            .clone()
            .expect("RustFS container should have securityContext");

        assert_eq!(context.allow_privilege_escalation, Some(false));
        assert_eq!(context.read_only_root_filesystem, Some(true));
        assert_eq!(
            context
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.drop.as_ref()),
            Some(&vec!["ALL".to_string()])
        );
    }

    #[test]
    fn legacy_root_override_disables_implicit_run_as_non_root() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            run_as_user: Some(0),
            ..Default::default()
        });

        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet");
        let context = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec")
            .security_context
            .expect("Pod should have securityContext");

        assert_eq!(context.run_as_user, Some(0));
        assert_eq!(context.run_as_non_root, Some(false));
    }

    #[test]
    fn container_root_override_derives_non_root_false_at_tenant_and_pool_scopes() {
        for pool_scope in [false, true] {
            let mut tenant = crate::tests::create_test_tenant(None, None);
            let root_context = corev1::SecurityContext {
                run_as_user: Some(0),
                ..Default::default()
            };
            if pool_scope {
                tenant.spec.pools[0].container_security_context = Some(root_context);
            } else {
                tenant.spec.container_security_context = Some(root_context);
            }

            tenant
                .validate_workload_security_compatibility()
                .expect("implicit container runAsNonRoot should follow the container UID");
            let statefulset = tenant
                .new_statefulset(&tenant.spec.pools[0])
                .expect("root container override should render consistently");
            let pod_spec = statefulset
                .spec
                .expect("StatefulSet should have spec")
                .template
                .spec
                .expect("Pod template should have spec");
            let container_context = pod_spec.containers[0]
                .security_context
                .as_ref()
                .expect("RustFS container should have securityContext");

            assert_eq!(
                pod_spec.security_context.unwrap().run_as_non_root,
                Some(true)
            );
            assert_eq!(container_context.run_as_user, Some(0));
            assert_eq!(container_context.run_as_non_root, Some(false));
        }
    }

    #[test]
    fn explicit_root_and_non_root_true_is_rejected_before_rendering() {
        let mut cases = Vec::new();

        let mut tenant_container = crate::tests::create_test_tenant(None, None);
        tenant_container.spec.container_security_context = Some(corev1::SecurityContext {
            run_as_user: Some(0),
            run_as_non_root: Some(true),
            ..Default::default()
        });
        cases.push(tenant_container);

        let mut pool_container = crate::tests::create_test_tenant(None, None);
        pool_container.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            run_as_user: Some(0),
            run_as_non_root: Some(true),
            ..Default::default()
        });
        cases.push(pool_container);

        let mut inherited_pod_true = crate::tests::create_test_tenant(None, None);
        inherited_pod_true.spec.security_context = Some(PodSecurityContextOverride {
            run_as_non_root: Some(true),
            ..Default::default()
        });
        inherited_pod_true.spec.pools[0].container_security_context =
            Some(corev1::SecurityContext {
                run_as_user: Some(0),
                ..Default::default()
            });
        cases.push(inherited_pod_true);

        for tenant in cases {
            let error = tenant
                .validate_workload_security_compatibility()
                .expect_err("UID 0 with explicit runAsNonRoot=true should be rejected");
            assert!(matches!(
                error,
                crate::types::error::Error::InvalidWorkloadSecurityProfile { message, .. }
                    if message.contains("pool-0")
                        && message.contains("UID 0")
                        && message.contains("explicitly true")
            ));

            let render_error = tenant
                .new_statefulset(&tenant.spec.pools[0])
                .expect_err("contradictory identity must fail before StatefulSet rendering");
            assert!(matches!(
                render_error,
                crate::types::error::Error::InvalidWorkloadSecurityProfile { .. }
            ));
        }
    }

    #[test]
    fn container_non_root_uid_overrides_implicit_pod_root_identity() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            run_as_user: Some(0),
            ..Default::default()
        });
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            run_as_user: Some(20_001),
            ..Default::default()
        });

        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("container non-root UID should override the Pod root identity");
        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");
        let container_context = pod_spec.containers[0]
            .security_context
            .as_ref()
            .expect("RustFS container should have securityContext");

        assert_eq!(
            pod_spec.security_context.unwrap().run_as_non_root,
            Some(false)
        );
        assert_eq!(container_context.run_as_user, Some(20_001));
        assert_eq!(container_context.run_as_non_root, Some(true));
    }

    #[test]
    fn pool_replaces_tagged_container_security_profiles_atomically() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                localhost_profile: Some("profiles/tenant-apparmor".to_string()),
                type_: "Localhost".to_string(),
            }),
            seccomp_profile: Some(corev1::SeccompProfile {
                localhost_profile: Some("profiles/tenant-seccomp.json".to_string()),
                type_: "Localhost".to_string(),
            }),
            ..Default::default()
        });
        tenant.spec.pools[0].container_security_context = Some(corev1::SecurityContext {
            app_armor_profile: Some(corev1::AppArmorProfile {
                type_: "RuntimeDefault".to_string(),
                ..Default::default()
            }),
            seccomp_profile: Some(corev1::SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });

        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet");
        let context = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec")
            .containers[0]
            .security_context
            .clone()
            .expect("RustFS container should have securityContext");

        let app_armor = context
            .app_armor_profile
            .expect("AppArmor profile should be set");
        assert_eq!(app_armor.type_, "RuntimeDefault");
        assert_eq!(app_armor.localhost_profile, None);

        let seccomp = context
            .seccomp_profile
            .expect("Seccomp profile should be set");
        assert_eq!(seccomp.type_, "RuntimeDefault");
        assert_eq!(seccomp.localhost_profile, None);
    }

    // Test: Default logging mode is stdout (no volumes)
    #[test]
    fn test_default_logging_is_stdout() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        // Default: no log volumes (stdout logging)
        let volumes = pod_spec.volumes.unwrap_or_default();
        let has_log_volume = volumes.iter().any(|v| v.name == "logs");
        assert!(!has_log_volume, "Default should not have log volume");

        // Should not have log volume mounts
        let container = pod_spec.containers.first().expect("Should have container");
        let empty_mounts = vec![];
        let mounts = container.volume_mounts.as_ref().unwrap_or(&empty_mounts);
        let has_log_mount = mounts.iter().any(|m| m.name == "logs");
        assert!(!has_log_mount, "Default should not have log volume mount");
    }

    // Test: EmptyDir logging mode creates volume
    #[test]
    fn test_emptydir_logging_creates_volume() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.logging = Some(LoggingConfig {
            mode: LoggingMode::EmptyDir,
            storage_size: None,
            storage_class: None,
            mount_path: None,
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        // Should have emptyDir log volume
        let volumes = pod_spec
            .volumes
            .as_ref()
            .expect("Pod should define volumes");
        let log_volume = volumes
            .iter()
            .find(|v| v.name == "logs")
            .expect("Should have logs volume");
        assert!(
            log_volume.empty_dir.is_some(),
            "Logs volume should be emptyDir"
        );

        // Should have log volume mount
        let container = pod_spec.containers.first().expect("Should have container");
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("Container should have mounts");
        let log_mount = mounts
            .iter()
            .find(|m| m.name == "logs")
            .expect("Should have logs mount");
        assert_eq!(log_mount.mount_path, "/logs", "Logs should mount at /logs");
    }

    // Test: Persistent logging mode creates PVC
    #[test]
    fn test_persistent_logging_creates_pvc() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.logging = Some(LoggingConfig {
            mode: LoggingMode::Persistent,
            storage_size: Some("10Gi".to_string()),
            storage_class: Some("fast-ssd".to_string()),
            mount_path: None,
        });
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Should have log PVC in volumeClaimTemplates
        let vcts = statefulset
            .spec
            .as_ref()
            .and_then(|s| s.volume_claim_templates.as_ref())
            .expect("Should have volumeClaimTemplates");

        let log_pvc = vcts
            .iter()
            .find(|v| v.metadata.name.as_deref() == Some("logs"))
            .expect("Should have logs PVC");

        // Verify PVC spec
        let pvc_spec = log_pvc.spec.as_ref().expect("PVC should have spec");
        assert_eq!(
            pvc_spec.storage_class_name.as_deref(),
            Some("fast-ssd"),
            "Should use specified storage class"
        );

        let storage = pvc_spec
            .resources
            .as_ref()
            .and_then(|r| r.requests.as_ref())
            .and_then(|r| r.get("storage"))
            .map(|q| q.0.as_str())
            .expect("Should have storage request");
        assert_eq!(storage, "10Gi", "Should request 10Gi storage");
    }

    // Test: StatefulSet uses correct service account
    #[test]
    fn test_statefulset_uses_default_sa() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        assert!(uses_unpartitioned_rolling_update(
            statefulset
                .spec
                .as_ref()
                .and_then(|spec| spec.update_strategy.as_ref())
        ));

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.service_account_name,
            Some("test-tenant-sa".to_string()),
            "Pod should use default service account"
        );
        assert_eq!(pod_spec.automount_service_account_token, Some(false));
    }

    // Test: StatefulSet uses custom service account
    #[test]
    fn test_statefulset_uses_custom_sa() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        assert_eq!(
            statefulset
                .spec
                .as_ref()
                .and_then(|spec| spec.update_strategy.as_ref()),
            None
        );

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.service_account_name,
            Some("my-custom-sa".to_string()),
            "Pod should use custom service account"
        );
        assert_eq!(pod_spec.automount_service_account_token, None);
    }

    #[test]
    fn default_service_account_token_hardening_triggers_statefulset_update() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];
        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        statefulset
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .automount_service_account_token = None;

        assert!(
            tenant
                .statefulset_needs_update(&statefulset, pool)
                .expect("Should compare StatefulSet"),
            "Legacy default ServiceAccount token automount should trigger a rollout"
        );
    }

    #[test]
    fn default_service_account_token_hardening_replaces_non_rolling_strategy() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];
        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        statefulset
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .update_strategy = Some(v1::StatefulSetUpdateStrategy {
            type_: Some("OnDelete".to_string()),
            ..Default::default()
        });

        assert!(
            tenant
                .statefulset_needs_update(&statefulset, pool)
                .expect("Should compare StatefulSet"),
            "OnDelete must not leave legacy Pods with mounted API tokens"
        );
    }

    #[test]
    fn default_service_account_token_hardening_removes_rolling_partition() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];
        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        statefulset
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .update_strategy = Some(v1::StatefulSetUpdateStrategy {
            type_: Some("RollingUpdate".to_string()),
            rolling_update: Some(v1::RollingUpdateStatefulSetStrategy {
                partition: Some(1),
                ..Default::default()
            }),
        });

        assert!(
            tenant
                .statefulset_needs_update(&statefulset, pool)
                .expect("Should compare StatefulSet"),
            "a rolling partition must not leave legacy Pods with mounted API tokens"
        );
    }

    #[test]
    fn custom_service_account_token_projection_does_not_reconcile_loop() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), None);
        let pool = &tenant.spec.pools[0];
        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        statefulset
            .spec
            .as_mut()
            .expect("StatefulSet should have spec")
            .template
            .spec
            .as_mut()
            .expect("Pod template should have spec")
            .automount_service_account_token = Some(true);

        assert!(
            !tenant
                .statefulset_needs_update(&statefulset, pool)
                .expect("Should compare StatefulSet"),
            "Custom ServiceAccount token projection should remain user-managed"
        );
    }

    // Test: StatefulSet renders tenant-level image pull secret
    #[test]
    fn test_statefulset_renders_image_pull_secret() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image_pull_secret = Some(image_pull_secret("registry-cred"));
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.image_pull_secrets,
            Some(vec![image_pull_secret("registry-cred")]),
            "Pod should use tenant image pull secret"
        );
    }

    // Test: StatefulSet applies pool-level node selector
    #[test]
    fn test_statefulset_applies_node_selector() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let mut node_selector = std::collections::BTreeMap::new();
        node_selector.insert("storage-type".to_string(), "nvme".to_string());
        tenant.spec.pools[0].scheduling.node_selector = Some(node_selector.clone());

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.node_selector,
            Some(node_selector),
            "Pod should use pool-level node selector"
        );
    }

    // Test: StatefulSet applies pool-level tolerations
    #[test]
    fn test_statefulset_applies_tolerations() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let tolerations = vec![corev1::Toleration {
            key: Some("spot-instance".to_string()),
            operator: Some("Equal".to_string()),
            value: Some("true".to_string()),
            effect: Some("NoSchedule".to_string()),
            ..Default::default()
        }];
        tenant.spec.pools[0].scheduling.tolerations = Some(tolerations.clone());

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.tolerations,
            Some(tolerations),
            "Pod should use pool-level tolerations"
        );
    }

    // Test: Pool-level priority class overrides tenant-level
    #[test]
    fn test_pool_priority_class_overrides_tenant() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.priority_class_name = Some("tenant-priority".to_string());
        tenant.spec.pools[0].scheduling.priority_class_name = Some("pool-priority".to_string());

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.priority_class_name,
            Some("pool-priority".to_string()),
            "Pool-level priority class should override tenant-level"
        );
    }

    // Test: Tenant-level priority class used when pool-level not set
    #[test]
    fn test_tenant_priority_class_fallback() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.priority_class_name = Some("tenant-priority".to_string());
        // pool.priority_class_name remains None

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let pod_spec = statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec");

        assert_eq!(
            pod_spec.priority_class_name,
            Some("tenant-priority".to_string()),
            "Should fall back to tenant-level priority class when pool-level not set"
        );
    }

    // Test: Pool-level resources applied to container
    #[test]
    fn test_pool_resources_applied_to_container() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let mut requests = std::collections::BTreeMap::new();
        requests.insert(
            "cpu".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("4".to_string()),
        );
        requests.insert(
            "memory".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("16Gi".to_string()),
        );

        tenant.spec.pools[0].scheduling.resources = Some(corev1::ResourceRequirements {
            requests: Some(requests.clone()),
            limits: None,
            claims: None,
        });

        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        let container = &statefulset
            .spec
            .expect("StatefulSet should have spec")
            .template
            .spec
            .expect("Pod template should have spec")
            .containers[0];

        assert!(
            container.resources.is_some(),
            "Container should have resources"
        );
        assert_eq!(
            container.resources.as_ref().unwrap().requests,
            Some(requests),
            "Container should use pool-level resource requests"
        );
    }

    // Test: StatefulSet diff detection - no changes needed
    #[test]
    fn test_statefulset_no_update_needed() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Check if update is needed comparing StatefulSet to itself
        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            !needs_update,
            "StatefulSet should not need update when comparing to itself"
        );
    }

    #[test]
    fn test_statefulset_container_security_context_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let statefulset = tenant
            .new_statefulset(&tenant.spec.pools[0])
            .expect("Should create StatefulSet");

        tenant.spec.container_security_context = Some(corev1::SecurityContext {
            read_only_root_filesystem: Some(true),
            ..Default::default()
        });

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, &tenant.spec.pools[0])
            .expect("Should check update need");

        assert!(needs_update, "Container security changes should roll Pods");
    }

    #[test]
    fn test_statefulset_without_container_security_defaults_needs_update() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];
        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");
        statefulset
            .spec
            .as_mut()
            .and_then(|spec| spec.template.spec.as_mut())
            .expect("Pod template should have spec")
            .containers[0]
            .security_context = None;

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "Legacy StatefulSets should receive safe defaults"
        );
    }

    // Test: StatefulSet diff detection - image change
    #[test]
    fn test_statefulset_image_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some("rustfs:v1".to_string());
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change image
        tenant.spec.image = Some("rustfs:v2".to_string());

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when image changes"
        );
    }

    // Test: StatefulSet diff detection - image pull secret add
    #[test]
    fn test_statefulset_image_pull_secret_add_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        tenant.spec.image_pull_secret = Some(image_pull_secret("registry-cred"));

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when image pull secret is added"
        );
    }

    // Test: StatefulSet diff detection - image pull secret change
    #[test]
    fn test_statefulset_image_pull_secret_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image_pull_secret = Some(image_pull_secret("old-registry-cred"));
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        tenant.spec.image_pull_secret = Some(image_pull_secret("new-registry-cred"));

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when image pull secret changes"
        );
    }

    // Test: StatefulSet diff detection - image pull secret removal
    #[test]
    fn test_statefulset_image_pull_secret_removal_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image_pull_secret = Some(image_pull_secret("registry-cred"));
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        tenant.spec.image_pull_secret = None;

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when image pull secret is removed"
        );
    }

    // Test: StatefulSet diff detection - replicas change
    #[test]
    fn test_statefulset_replicas_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].servers = 4;
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change replicas
        tenant.spec.pools[0].servers = 6;
        let pool = &tenant.spec.pools[0];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when replicas change"
        );
    }

    // Test: StatefulSet diff detection - environment variable change
    #[test]
    fn test_statefulset_env_change_detected() {
        use k8s_openapi::api::core::v1 as corev1;

        let mut tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Add environment variable
        tenant.spec.env = vec![corev1::EnvVar {
            name: "NEW_VAR".to_string(),
            value: Some("value".to_string()),
            ..Default::default()
        }];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when env vars change"
        );
    }

    #[test]
    fn test_statefulset_rpc_secret_change_detected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.rpc_secret = Some(RpcSecretRef {
            name: "tenant-rpc-auth".to_string(),
            key: "rpc-secret".to_string(),
        });
        let pool = &tenant.spec.pools[0];
        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        tenant.spec.rpc_secret.as_mut().unwrap().name = "rotated-rpc-auth".to_string();

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when the RPC Secret reference changes"
        );
    }

    // Test: StatefulSet diff detection - resources change
    #[test]
    fn test_statefulset_resources_change_detected() {
        use k8s_openapi::api::core::v1 as corev1;

        let mut tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Add resource requirements
        let mut requests = std::collections::BTreeMap::new();
        requests.insert(
            "cpu".to_string(),
            k8s_openapi::apimachinery::pkg::api::resource::Quantity("2".to_string()),
        );

        tenant.spec.pools[0].scheduling.resources = Some(corev1::ResourceRequirements {
            requests: Some(requests),
            limits: None,
            claims: None,
        });
        let pool = &tenant.spec.pools[0];

        let needs_update = tenant
            .statefulset_needs_update(&statefulset, pool)
            .expect("Should check update need");

        assert!(
            needs_update,
            "StatefulSet should need update when resources change"
        );
    }

    // Test: StatefulSet validation - selector change rejected
    #[test]
    fn test_statefulset_selector_change_rejected() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Modify selector (immutable field)
        if let Some(ref mut spec) = statefulset.spec
            && let Some(ref mut labels) = spec.selector.match_labels
        {
            labels.insert("modified".to_string(), "true".to_string());
        }

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when selector changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, .. } => {
                assert_eq!(
                    field, "spec.selector",
                    "Error should indicate selector field"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - serviceName change rejected
    #[test]
    fn test_statefulset_service_name_change_rejected() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let pool = &tenant.spec.pools[0];

        let mut statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Modify serviceName (immutable field)
        if let Some(ref mut spec) = statefulset.spec {
            spec.service_name = Some("different-service".to_string());
        }

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when serviceName changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, .. } => {
                assert_eq!(
                    field, "spec.serviceName",
                    "Error should indicate serviceName field"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - volumesPerServer change rejected
    #[test]
    fn test_statefulset_volumes_per_server_change_rejected() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.pools[0].persistence.volumes_per_server = 2;
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change volumesPerServer (would modify volumeClaimTemplates - immutable)
        tenant.spec.pools[0].persistence.volumes_per_server = 4;
        let pool = &tenant.spec.pools[0];

        // Validation should fail
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_err(),
            "Validation should fail when volumesPerServer changes"
        );

        let err = result.unwrap_err();
        match err {
            crate::types::error::Error::ImmutableFieldModified { field, message, .. } => {
                assert_eq!(
                    field, "spec.volumeClaimTemplates",
                    "Error should indicate volumeClaimTemplates field"
                );
                assert!(
                    message.contains("volumesPerServer"),
                    "Error message should mention volumesPerServer"
                );
            }
            _ => panic!("Expected ImmutableFieldModified error"),
        }
    }

    // Test: StatefulSet validation - safe update allowed
    #[test]
    fn test_statefulset_safe_update_allowed() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.image = Some("rustfs:v1".to_string());
        let pool = &tenant.spec.pools[0];

        let statefulset = tenant
            .new_statefulset(pool)
            .expect("Should create StatefulSet");

        // Change image (safe update)
        tenant.spec.image = Some("rustfs:v2".to_string());

        // Validation should pass
        let result = tenant.validate_statefulset_update(&statefulset, pool);

        assert!(
            result.is_ok(),
            "Validation should pass for safe updates like image changes"
        );
    }
}
