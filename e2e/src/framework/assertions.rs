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

use anyhow::{Context, Result, bail, ensure};
use k8s_openapi::api::{apps::v1::StatefulSet, core::v1 as corev1};
use operator::types::v1alpha1::status::certificate::SecretStatusRef;
use operator::types::v1alpha1::tenant::Tenant;
use operator::types::v1alpha1::tls::{
    RUSTFS_CA_FILE, RUSTFS_TLS_CERT_FILE, RUSTFS_TLS_KEY_FILE, TLS_HASH_ANNOTATION,
};

pub fn current_state(tenant: &Tenant) -> Option<&str> {
    tenant
        .status
        .as_ref()
        .map(|status| status.current_state.as_str())
}

pub fn condition_status<'a>(tenant: &'a Tenant, condition_type: &str) -> Option<&'a str> {
    tenant
        .status
        .as_ref()?
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_type)
        .map(|condition| condition.status.as_str())
}

pub fn require_condition(
    tenant: &Tenant,
    condition_type: &str,
    expected_status: &str,
) -> Result<()> {
    match condition_status(tenant, condition_type) {
        Some(actual) if actual == expected_status => Ok(()),
        Some(actual) => {
            bail!("condition {condition_type} expected {expected_status}, got {actual}")
        }
        None => bail!("condition {condition_type} not found"),
    }
}

pub fn require_observed_generation_current(tenant: &Tenant) -> Result<()> {
    let generation = tenant.metadata.generation;
    let observed = tenant
        .status
        .as_ref()
        .and_then(|status| status.observed_generation);

    ensure!(
        generation.is_some(),
        "tenant metadata.generation is missing"
    );
    ensure!(
        observed == generation,
        "tenant observedGeneration {observed:?} does not match generation {generation:?}"
    );
    Ok(())
}

pub fn tenant_tls_observed_hash(tenant: &Tenant) -> Result<String> {
    tenant
        .status
        .as_ref()
        .and_then(|status| status.certificates.tls.as_ref())
        .and_then(|tls| tls.observed_hash.clone())
        .context("Tenant status.certificates.tls.observedHash is missing")
}

pub fn require_tls_service_https_wiring(service: &corev1::Service) -> Result<()> {
    let ports = service
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_ref())
        .context("Service spec.ports is missing")?;
    ensure!(
        ports
            .iter()
            .any(|port| port.name.as_deref() == Some("https-rustfs") && port.port == 9000),
        "Service {} does not expose the https-rustfs port on 9000",
        service
            .metadata
            .name
            .as_deref()
            .unwrap_or("<unnamed-service>")
    );
    Ok(())
}

pub fn require_tls_statefulset_https_wiring(
    statefulset: &StatefulSet,
    expected_hash: &str,
    expected_secret_name: &str,
    expected_ca_secret_ref: &SecretStatusRef,
) -> Result<()> {
    let template = &statefulset
        .spec
        .as_ref()
        .context("StatefulSet spec is missing")?
        .template;
    let annotations = template
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.annotations.as_ref())
        .context("StatefulSet pod template annotations are missing")?;
    ensure!(
        annotations.get(TLS_HASH_ANNOTATION).map(String::as_str) == Some(expected_hash),
        "StatefulSet pod template TLS hash mismatch: expected {expected_hash}, got {:?}",
        annotations.get(TLS_HASH_ANNOTATION)
    );

    let pod_spec = template
        .spec
        .as_ref()
        .context("StatefulSet pod template spec is missing")?;
    let volumes = pod_spec.volumes.as_deref().unwrap_or_default();
    let server_volume = volumes
        .iter()
        .find(|volume| {
            volume.name == "rustfs-tls-server"
                && volume_references_secret(volume, expected_secret_name)
        })
        .with_context(|| {
            format!(
                "StatefulSet pod volumes do not mount expected TLS Secret {expected_secret_name}"
            )
        })?;
    ensure!(
        volume_has_secret_item(
            server_volume,
            expected_secret_name,
            "tls.crt",
            RUSTFS_TLS_CERT_FILE,
        ),
        "StatefulSet rustfs-tls-server volume does not map tls.crt to {RUSTFS_TLS_CERT_FILE}"
    );
    ensure!(
        volume_has_secret_item(
            server_volume,
            expected_secret_name,
            "tls.key",
            RUSTFS_TLS_KEY_FILE,
        ),
        "StatefulSet rustfs-tls-server volume does not map tls.key to {RUSTFS_TLS_KEY_FILE}"
    );

    let container = pod_spec
        .containers
        .iter()
        .find(|container| container.name == "rustfs")
        .context("rustfs container not found")?;
    let tls_path = env_value(container, "RUSTFS_TLS_PATH")
        .context("rustfs container missing RUSTFS_TLS_PATH")?;
    ensure!(
        env_value(container, "RUSTFS_VOLUMES").is_some_and(|value| value.contains("https://")),
        "rustfs container RUSTFS_VOLUMES does not use https://"
    );
    ensure!(
        probe_scheme(container.readiness_probe.as_ref()) == Some("HTTPS"),
        "rustfs readiness probe is not HTTPS"
    );
    ensure!(
        probe_scheme(container.liveness_probe.as_ref()) == Some("HTTPS"),
        "rustfs liveness probe is not HTTPS"
    );

    let mounts = container.volume_mounts.as_deref().unwrap_or_default();
    for file_name in [RUSTFS_TLS_CERT_FILE, RUSTFS_TLS_KEY_FILE] {
        require_read_only_tls_material_mount(mounts, "rustfs-tls-server", tls_path, file_name)?;
    }

    let ca_key = expected_ca_secret_ref.key.as_deref().unwrap_or("ca.crt");
    if volume_has_secret_item(
        server_volume,
        &expected_ca_secret_ref.name,
        ca_key,
        RUSTFS_CA_FILE,
    ) {
        require_read_only_tls_material_mount(
            mounts,
            "rustfs-tls-server",
            tls_path,
            RUSTFS_CA_FILE,
        )?;
    } else {
        let ca_volume = volumes
            .iter()
            .find(|volume| {
                volume.name == "rustfs-tls-ca"
                    && secret_volume_name(volume) == Some(expected_ca_secret_ref.name.as_str())
            })
            .with_context(|| {
                format!(
                    "StatefulSet pod volumes do not mount expected CA Secret {}",
                    expected_ca_secret_ref.name
                )
            })?;
        ensure!(
            secret_volume_has_item(ca_volume, ca_key, RUSTFS_CA_FILE),
            "StatefulSet rustfs-tls-ca volume does not map CA key {ca_key} to {RUSTFS_CA_FILE}"
        );
        require_read_only_tls_material_mount(mounts, "rustfs-tls-ca", tls_path, RUSTFS_CA_FILE)?;
    }

    Ok(())
}

pub fn require_no_secret_material(label: &str, content: &str) -> Result<()> {
    for forbidden in [
        "-----BEGIN",
        "PRIVATE KEY",
        "tls.key:",
        "tls.crt:",
        "accesskey:",
        "secretkey:",
    ] {
        ensure!(
            !content.contains(forbidden),
            "{label} exposes forbidden secret material marker {forbidden}"
        );
    }
    Ok(())
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

fn secret_volume_name(volume: &corev1::Volume) -> Option<&str> {
    volume
        .secret
        .as_ref()
        .and_then(|secret| secret.secret_name.as_deref())
}

fn secret_volume_has_item(volume: &corev1::Volume, key: &str, path: &str) -> bool {
    volume
        .secret
        .as_ref()
        .and_then(|secret| secret.items.as_ref())
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.key == key && item.path == path)
        })
}

fn volume_references_secret(volume: &corev1::Volume, secret_name: &str) -> bool {
    secret_volume_name(volume) == Some(secret_name)
        || projected_volume_references_secret(volume, secret_name)
}

fn volume_has_secret_item(
    volume: &corev1::Volume,
    secret_name: &str,
    key: &str,
    path: &str,
) -> bool {
    if secret_volume_name(volume) == Some(secret_name) && secret_volume_has_item(volume, key, path)
    {
        return true;
    }
    projected_volume_has_secret_item(volume, secret_name, key, path)
}

fn projected_volume_references_secret(volume: &corev1::Volume, secret_name: &str) -> bool {
    volume
        .projected
        .as_ref()
        .and_then(|projected| projected.sources.as_ref())
        .is_some_and(|sources| {
            sources.iter().any(|source| {
                source
                    .secret
                    .as_ref()
                    .is_some_and(|secret| secret.name == secret_name)
            })
        })
}

fn projected_volume_has_secret_item(
    volume: &corev1::Volume,
    secret_name: &str,
    key: &str,
    path: &str,
) -> bool {
    volume
        .projected
        .as_ref()
        .and_then(|projected| projected.sources.as_ref())
        .is_some_and(|sources| {
            sources.iter().any(|source| {
                source
                    .secret
                    .as_ref()
                    .filter(|secret| secret.name == secret_name)
                    .and_then(|secret| secret.items.as_ref())
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item.key == key && item.path == path)
                    })
            })
        })
}

fn require_read_only_tls_material_mount(
    mounts: &[corev1::VolumeMount],
    volume_name: &str,
    tls_path: &str,
    file_name: &str,
) -> Result<()> {
    ensure!(
        mounts.iter().any(|mount| {
            mount.name == volume_name
                && mount.read_only == Some(true)
                && ((mount.mount_path.ends_with(file_name)
                    && mount.sub_path.as_deref() == Some(file_name))
                    || (mount.mount_path == tls_path && mount.sub_path.is_none()))
        }),
        "rustfs container does not expose {file_name} read-only from volume {volume_name}"
    );
    Ok(())
}

fn probe_scheme(probe: Option<&corev1::Probe>) -> Option<&str> {
    probe?.http_get.as_ref()?.scheme.as_deref()
}

#[cfg(test)]
mod tests {
    use super::{condition_status, current_state, require_condition};
    use operator::types::v1alpha1::status::{Condition, Status};
    use operator::types::v1alpha1::tenant::{Tenant, TenantSpec};

    #[test]
    fn tenant_condition_helpers_find_status_by_type() {
        let mut tenant = Tenant::new("tenant-a", TenantSpec::default());
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            conditions: vec![Condition {
                type_: "Ready".to_string(),
                status: "True".to_string(),
                last_transition_time: None,
                observed_generation: Some(1),
                reason: "ReconcileSucceeded".to_string(),
                message: "ready".to_string(),
            }],
            ..Status::default()
        });

        assert_eq!(current_state(&tenant), Some("Ready"));
        assert_eq!(condition_status(&tenant, "Ready"), Some("True"));
        assert!(require_condition(&tenant, "Ready", "True").is_ok());
        assert!(require_condition(&tenant, "Ready", "False").is_err());
    }
}
