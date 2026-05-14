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

use anyhow::{Context, Result, ensure};
use k8s_openapi::api::{apps::v1::StatefulSet, core::v1 as corev1};
use operator::types::v1alpha1::{
    status::certificate::SecretStatusRef,
    tenant::Tenant,
    tls::{
        CaTrustSource, DEFAULT_TLS_MOUNT_PATH, RUSTFS_CA_FILE, RUSTFS_TLS_CERT_FILE,
        RUSTFS_TLS_KEY_FILE, SecretKeyReference, TlsPlan,
    },
};
use rustfs_operator_e2e::{
    cases::cert_manager_tls,
    framework::{
        artifacts::ArtifactCollector,
        assertions, cert_manager_tls as tls_e2e,
        command::CommandSpec,
        config::{E2eConfig, KIND_WORKER_COUNT},
        kube_client, live,
    },
};
use std::collections::BTreeSet;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn cert_manager_case_inventory_matches_executable_tests() {
    let names = cert_manager_tls::cases()
        .into_iter()
        .map(|case| case.name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "cert_manager_managed_certificate_reaches_tls_ready_and_https_wiring",
            "cert_manager_external_secret_reaches_tls_ready_and_rolls_on_secret_hash",
            "cert_manager_rejects_secret_missing_tls_crt",
            "cert_manager_rejects_secret_missing_tls_key",
            "cert_manager_rejects_secret_missing_ca_for_internode_https",
            "cert_manager_rejects_missing_issuer_for_managed_certificate",
            "cert_manager_reports_pending_certificate_not_ready",
            "cert_manager_rejects_hot_reload",
            "cert_manager_artifacts_do_not_expose_secret_material",
        ]
    );
}

#[test]
fn managed_certificate_tenant_manifest_wires_ca_trust_without_secret_material() -> Result<()> {
    let config = E2eConfig::defaults();
    let manifest = tls_e2e::managed_certificate_tenant_manifest(&config)?;

    ensure!(manifest.contains("mode: certManager"));
    ensure!(manifest.contains("manageCertificate: true"));
    ensure!(manifest.contains("issuerRef:"));
    ensure!(manifest.contains("source: CertificateSecretCa"));
    ensure!(manifest.contains("enableInternodeHttps: true"));
    assertions::require_no_secret_material("managed cert-manager Tenant manifest", &manifest)?;

    Ok(())
}

#[test]
fn external_secret_tenant_manifest_uses_shared_secret_and_rollout_strategy() -> Result<()> {
    let config = E2eConfig::defaults();
    let manifest = tls_e2e::external_secret_tenant_manifest(&config)?;

    ensure!(manifest.contains("mode: certManager"));
    ensure!(manifest.contains("manageCertificate: false"));
    ensure!(manifest.contains("secretName: e2e-tenant-external-tls"));
    ensure!(manifest.contains("source: SecretRef"));
    ensure!(manifest.contains("caSecretRef:"));
    ensure!(manifest.contains("rotationStrategy: Rollout"));
    assertions::require_no_secret_material("external Secret Tenant manifest", &manifest)?;

    Ok(())
}

#[test]
fn external_secret_manifests_carry_tenant_watch_label_for_initial_create_and_rotation() -> Result<()>
{
    let config = tls_e2e::external_secret_case_config(&E2eConfig::defaults());

    let initial = tls_e2e::external_tls_secret_manifests(&config)?;
    assert_secret_manifest_tenant_watch_label(
        "initial external TLS Secret",
        &initial.tls_secret_manifest,
        &config.tenant_name,
    )?;
    assert_secret_manifest_tenant_watch_label(
        "initial external CA Secret",
        &initial.ca_secret_manifest,
        &config.tenant_name,
    )?;

    let rotated = tls_e2e::external_tls_secret_manifests(&config)?;
    assert_secret_manifest_tenant_watch_label(
        "rotated external TLS Secret",
        &rotated.tls_secret_manifest,
        &config.tenant_name,
    )?;
    assert_secret_manifest_tenant_watch_label(
        "rotated external CA Secret",
        &rotated.ca_secret_manifest,
        &config.tenant_name,
    )?;

    Ok(())
}

#[test]
fn positive_cert_manager_tls_timeout_uses_named_longer_readiness_window() -> Result<()> {
    let default_config = E2eConfig::defaults();
    ensure!(default_config.timeout < Duration::from_secs(600));
    ensure!(
        tls_e2e::positive_cert_manager_tls_timeout(&default_config) >= Duration::from_secs(600),
        "positive cert-manager TLS waits should use at least a 600s readiness window"
    );

    let overridden = E2eConfig::from_env_with(|name| match name {
        "RUSTFS_E2E_TIMEOUT_SECONDS" => Some("900".to_string()),
        _ => None,
    });
    ensure!(
        tls_e2e::positive_cert_manager_tls_timeout(&overridden) == Duration::from_secs(900),
        "positive cert-manager TLS waits should preserve explicit longer timeouts"
    );

    Ok(())
}

#[test]
fn positive_tls_live_case_configs_are_isolated_from_smoke_namespace_tenant_and_storage()
-> Result<()> {
    let smoke = E2eConfig::defaults();
    let managed = tls_e2e::managed_certificate_case_config(&smoke);
    let external = tls_e2e::external_secret_case_config(&smoke);

    for (case_name, config, manifest) in [
        (
            "managed cert-manager Tenant",
            &managed,
            tls_e2e::managed_certificate_tenant_manifest(&managed)?,
        ),
        (
            "external Secret Tenant",
            &external,
            tls_e2e::external_secret_tenant_manifest(&external)?,
        ),
    ] {
        ensure!(
            config.test_namespace != smoke.test_namespace,
            "{case_name} should not reuse the smoke namespace {}",
            smoke.test_namespace
        );
        ensure!(
            config.tenant_name != smoke.tenant_name,
            "{case_name} should not mutate the smoke Tenant {}",
            smoke.tenant_name
        );
        ensure!(
            config.storage_class != smoke.storage_class,
            "{case_name} should not bind the smoke storage class {}",
            smoke.storage_class
        );
        ensure!(
            config.pv_count > 0,
            "{case_name} should request positive isolated PV capacity, got {}",
            config.pv_count
        );
        ensure!(
            manifest.contains(&format!("namespace: {}", config.test_namespace)),
            "{case_name} manifest should use isolated namespace {}, got:\n{manifest}",
            config.test_namespace
        );
        ensure!(
            manifest.contains(&format!("name: {}", config.tenant_name)),
            "{case_name} manifest should use isolated Tenant {}, got:\n{manifest}",
            config.tenant_name
        );
        ensure!(
            manifest.contains(&format!("storageClassName: {}", config.storage_class)),
            "{case_name} manifest should use isolated StorageClass {}, got:\n{manifest}",
            config.storage_class
        );
    }

    ensure!(managed.test_namespace != external.test_namespace);
    ensure!(managed.tenant_name != external.tenant_name);
    ensure!(managed.storage_class != external.storage_class);

    Ok(())
}

#[test]
fn positive_tls_storage_layouts_use_dedicated_pv_names_and_paths() -> Result<()> {
    let smoke = E2eConfig::defaults();
    let managed_config = tls_e2e::managed_certificate_case_config(&smoke);
    let managed_layout = tls_e2e::managed_certificate_storage_layout(&managed_config);
    let external_config = tls_e2e::external_secret_case_config(&smoke);
    let external_layout = tls_e2e::external_secret_storage_layout(&external_config);

    for (case_name, config, layout, tenant, path_fragment, pv_name_fragment) in [
        (
            "managed cert-manager Tenant",
            &managed_config,
            managed_layout,
            tls_e2e::managed_certificate_tenant(&managed_config),
            "/mnt/data/cert-manager-managed/vol1",
            "name: rustfs-e2e-cert-manager-managed-pv-1",
        ),
        (
            "external Secret Tenant",
            &external_config,
            external_layout,
            tls_e2e::external_secret_tenant(&external_config),
            "/mnt/data/cert-manager-external/vol1",
            "name: rustfs-e2e-cert-manager-external-pv-1",
        ),
    ] {
        let manifest = rustfs_operator_e2e::framework::storage::local_storage_manifest_for_layout(
            &smoke, &layout,
        );
        let first_command =
            rustfs_operator_e2e::framework::storage::volume_directory_commands_for_layout(
                &smoke, &layout,
            )
            .into_iter()
            .next()
            .expect("layout should create volume directory commands")
            .display();
        let required_pvc_count = tenant
            .spec
            .pools
            .iter()
            .map(|pool| {
                assert!(
                    pool.servers > 0,
                    "{case_name} pool server count should be positive"
                );
                assert!(
                    pool.persistence.volumes_per_server > 0,
                    "{case_name} volumes_per_server should be positive"
                );
                pool.servers as usize * pool.persistence.volumes_per_server as usize
            })
            .sum::<usize>();
        let rendered_pv_count = manifest.matches("kind: PersistentVolume").count();

        ensure!(
            layout.storage_class == config.storage_class,
            "{case_name} layout StorageClass should match Tenant config"
        );
        ensure!(
            layout.pv_count == config.pv_count,
            "{case_name} layout PV count should match isolated config PV count"
        );
        ensure!(
            config.pv_count >= required_pvc_count,
            "{case_name} isolated config should create at least one PV per Tenant PVC: pv_count={} required_pvc_count={required_pvc_count}",
            config.pv_count
        );
        ensure!(
            rendered_pv_count >= required_pvc_count,
            "{case_name} rendered PV manifest should cover Tenant PVCs: rendered_pv_count={rendered_pv_count} required_pvc_count={required_pvc_count}"
        );
        ensure!(
            rendered_pv_count == layout.pv_count,
            "{case_name} rendered PV count should match layout PV count"
        );

        let per_worker_pv_counts = (1..=KIND_WORKER_COUNT)
            .map(|worker_group| {
                manifest
                    .matches(&format!("                - storage-{worker_group}\n"))
                    .count()
            })
            .collect::<Vec<_>>();
        ensure!(
            per_worker_pv_counts.iter().sum::<usize>() == rendered_pv_count,
            "{case_name} should assign every PV to a storage worker: per_worker_pv_counts={per_worker_pv_counts:?} rendered_pv_count={rendered_pv_count}"
        );
        for pool in &tenant.spec.pools {
            let servers = pool.servers as usize;
            let volumes_per_server = pool.persistence.volumes_per_server as usize;
            let schedulable_pods = per_worker_pv_counts
                .iter()
                .map(|pv_count| pv_count / volumes_per_server)
                .sum::<usize>();

            ensure!(
                per_worker_pv_counts
                    .iter()
                    .all(|pv_count| *pv_count >= volumes_per_server),
                "{case_name} should allocate at least {volumes_per_server} PVs on every storage worker: per_worker_pv_counts={per_worker_pv_counts:?}"
            );
            ensure!(
                schedulable_pods >= servers,
                "{case_name} PV topology should schedule {servers} pods with {volumes_per_server} PVCs each across {KIND_WORKER_COUNT} workers: per_worker_pv_counts={per_worker_pv_counts:?} schedulable_pods={schedulable_pods}"
            );
        }

        ensure!(
            manifest.contains(&format!("storageClassName: {}", config.storage_class)),
            "{case_name} PV manifest should use isolated StorageClass {}, got:\n{manifest}",
            config.storage_class
        );
        ensure!(
            manifest.contains(path_fragment),
            "{case_name} PV manifest should use isolated local path {path_fragment}, got:\n{manifest}"
        );
        ensure!(
            manifest.contains(pv_name_fragment),
            "{case_name} PV manifest should use isolated PV name {pv_name_fragment}, got:\n{manifest}"
        );
        ensure!(
            !manifest.contains("path: /mnt/data/vol1"),
            "{case_name} PV manifest should not reuse smoke local volume paths, got:\n{manifest}"
        );
        ensure!(
            !manifest.contains("name: rustfs-e2e-pv-1"),
            "{case_name} PV manifest should not reuse smoke PV names, got:\n{manifest}"
        );
        ensure!(
            first_command.contains(path_fragment),
            "{case_name} docker directory setup should prepare isolated path {path_fragment}, got {first_command}"
        );
    }

    Ok(())
}

#[test]
fn positive_tls_fixtures_use_minimal_four_volume_https_erasure_sets() -> Result<()> {
    let smoke = E2eConfig::defaults();

    let managed_config = tls_e2e::managed_certificate_case_config(&smoke);
    let managed_tenant = tls_e2e::managed_certificate_tenant(&managed_config);
    let managed_tls_plan = tls_e2e::sample_tls_plan(
        "sha256:e2e-test",
        tls_e2e::managed_secret_name(&managed_config),
    );
    assert_positive_tls_fixture_uses_minimal_four_volume_https_erasure_set(
        "managed cert-manager Tenant",
        &managed_config,
        &managed_tenant,
        &managed_tls_plan,
    )?;

    let external_config = tls_e2e::external_secret_case_config(&smoke);
    let external_tenant = tls_e2e::external_secret_tenant(&external_config);
    let external_tls_plan = TlsPlan::rollout(
        DEFAULT_TLS_MOUNT_PATH.to_string(),
        "sha256:e2e-test".to_string(),
        tls_e2e::external_secret_name(&external_config),
        None,
        Some(SecretKeyReference {
            name: tls_e2e::external_ca_secret_name(&external_config),
            key: "ca.crt".to_string(),
        }),
        None,
        true,
        false,
        false,
        None,
    );
    assert_positive_tls_fixture_uses_minimal_four_volume_https_erasure_set(
        "external Secret Tenant",
        &external_config,
        &external_tenant,
        &external_tls_plan,
    )?;

    Ok(())
}

#[test]
fn cert_manager_tenant_fixtures_use_default_tls_mount_path() -> Result<()> {
    let config = E2eConfig::defaults();

    for (case_name, tenant, manifest) in [
        (
            "managed cert-manager Tenant",
            tls_e2e::managed_certificate_tenant(&config),
            tls_e2e::managed_certificate_tenant_manifest(&config)?,
        ),
        (
            "external Secret Tenant",
            tls_e2e::external_secret_tenant(&config),
            tls_e2e::external_secret_tenant_manifest(&config)?,
        ),
    ] {
        let tls = tenant
            .spec
            .tls
            .as_ref()
            .expect("cert-manager fixture should enable TLS");

        ensure!(
            tls.mount_path == DEFAULT_TLS_MOUNT_PATH,
            "{case_name} should use default TLS mount path {DEFAULT_TLS_MOUNT_PATH}, got {:?}",
            tls.mount_path
        );
        ensure!(
            manifest.contains(&format!("mountPath: {DEFAULT_TLS_MOUNT_PATH}")),
            "{case_name} manifest should contain the default TLS mount path, got:\n{manifest}"
        );
        ensure!(
            !manifest.contains("mountPath: ''") && !manifest.contains("mountPath: \"\""),
            "{case_name} manifest should not contain an empty TLS mountPath, got:\n{manifest}"
        );
    }

    Ok(())
}

#[test]
fn generated_tls_workload_assertions_cover_https_mounts_services_and_rollout_hash() -> Result<()> {
    let config = E2eConfig::defaults();
    let tenant = tls_e2e::managed_certificate_tenant(&config);
    let pool = tenant
        .spec
        .pools
        .first()
        .expect("managed certificate fixture should have a pool");
    let tls_plan =
        tls_e2e::sample_tls_plan("sha256:e2e-test", tls_e2e::managed_secret_name(&config));
    let managed_ca_secret_ref = SecretStatusRef {
        name: tls_e2e::managed_secret_name(&config),
        key: Some("ca.crt".to_string()),
        resource_version: None,
    };

    let statefulset = tenant.new_statefulset_with_tls_plan(pool, &tls_plan)?;
    let io_service = tenant.new_io_service_with_tls_plan(&tls_plan);
    let headless_service = tenant.new_headless_service_with_tls_plan(&tls_plan);

    assertions::require_tls_statefulset_https_wiring(
        &statefulset,
        "sha256:e2e-test",
        &tls_e2e::managed_secret_name(&config),
        &managed_ca_secret_ref,
    )?;
    assertions::require_tls_service_https_wiring(&io_service)?;
    assertions::require_tls_service_https_wiring(&headless_service)?;

    Ok(())
}

#[test]
fn generated_tls_workload_assertions_accept_explicit_ca_secret_ref() -> Result<()> {
    let config = E2eConfig::defaults();
    let tenant = tls_e2e::external_secret_tenant(&config);
    let pool = tenant
        .spec
        .pools
        .first()
        .expect("external Secret fixture should have a pool");
    let ca_secret_ref = SecretStatusRef {
        name: tls_e2e::external_ca_secret_name(&config),
        key: Some("ca.crt".to_string()),
        resource_version: None,
    };
    let tls_plan = TlsPlan::rollout(
        DEFAULT_TLS_MOUNT_PATH.to_string(),
        "sha256:e2e-test".to_string(),
        tls_e2e::external_secret_name(&config),
        None,
        Some(SecretKeyReference {
            name: ca_secret_ref.name.clone(),
            key: ca_secret_ref
                .key
                .clone()
                .expect("test CA status ref should include a key"),
        }),
        None,
        true,
        false,
        false,
        None,
    );

    let statefulset = tenant.new_statefulset_with_tls_plan(pool, &tls_plan)?;

    assertions::require_tls_statefulset_https_wiring(
        &statefulset,
        "sha256:e2e-test",
        &tls_e2e::external_secret_name(&config),
        &ca_secret_ref,
    )?;

    Ok(())
}

#[test]
fn external_secret_tls_plan_projects_server_and_explicit_ca_into_single_tls_directory() -> Result<()>
{
    let config = E2eConfig::defaults();
    let tenant = tls_e2e::external_secret_tenant(&config);
    let pool = tenant
        .spec
        .pools
        .first()
        .expect("external Secret fixture should have a pool");
    let tls_plan = TlsPlan::rollout(
        DEFAULT_TLS_MOUNT_PATH.to_string(),
        "sha256:e2e-test".to_string(),
        tls_e2e::external_secret_name(&config),
        None,
        Some(SecretKeyReference {
            name: tls_e2e::external_ca_secret_name(&config),
            key: "ca.crt".to_string(),
        }),
        None,
        true,
        false,
        false,
        None,
    );

    let statefulset = tenant.new_statefulset_with_tls_plan(pool, &tls_plan)?;
    let pod_spec = statefulset
        .spec
        .as_ref()
        .context("StatefulSet should have spec")?
        .template
        .spec
        .as_ref()
        .context("StatefulSet pod template should have spec")?;
    let tls_volume = pod_spec
        .volumes
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|volume| volume.name == "rustfs-tls-server")
        .context("TLS material should use the rustfs-tls-server volume")?;
    let rustfs_container = pod_spec
        .containers
        .iter()
        .find(|container| container.name == "rustfs")
        .context("rustfs container should exist")?;
    let tls_mount = rustfs_container
        .volume_mounts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|mount| mount.name == "rustfs-tls-server")
        .context("rustfs container should mount the projected TLS material volume")?;

    ensure!(
        projected_secret_item(
            tls_volume,
            &tls_e2e::external_secret_name(&config),
            "tls.crt",
            RUSTFS_TLS_CERT_FILE,
        ),
        "projected TLS material volume should include server tls.crt"
    );
    ensure!(
        projected_secret_item(
            tls_volume,
            &tls_e2e::external_secret_name(&config),
            "tls.key",
            RUSTFS_TLS_KEY_FILE,
        ),
        "projected TLS material volume should include server tls.key"
    );
    ensure!(
        projected_secret_item(
            tls_volume,
            &tls_e2e::external_ca_secret_name(&config),
            "ca.crt",
            RUSTFS_CA_FILE,
        ),
        "projected TLS material volume should include explicit CA SecretRef ca.crt"
    );
    ensure!(
        tls_mount.mount_path == DEFAULT_TLS_MOUNT_PATH,
        "projected TLS material should mount the whole TLS directory, got {}",
        tls_mount.mount_path
    );
    ensure!(
        tls_mount.sub_path.is_none(),
        "projected TLS material should not rely on subPath file mounts"
    );
    ensure!(tls_mount.read_only == Some(true));
    ensure!(
        !pod_spec
            .volumes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|volume| volume.name == "rustfs-tls-ca"),
        "explicit CA SecretRef should be projected into the TLS directory instead of mounted as a separate subPath volume"
    );

    Ok(())
}

#[test]
fn external_secret_tls_sans_cover_rendered_https_peer_hosts() -> Result<()> {
    let smoke = E2eConfig::defaults();
    let config = tls_e2e::external_secret_case_config(&smoke);
    let subject_alt_name = tls_e2e::external_tls_subject_alt_name(&config);
    let rotated_subject_alt_name = tls_e2e::external_tls_rotation_subject_alt_name(&config);
    let san_names = dns_names_from_subject_alt_name(&subject_alt_name)?;
    let rendered_peer_hosts = rendered_rustfs_volume_hosts(&config)?;

    ensure!(
        subject_alt_name == rotated_subject_alt_name,
        "external TLS Secret rotation should preserve the same SAN profile: initial={subject_alt_name} rotated={rotated_subject_alt_name}"
    );
    ensure!(
        san_names.contains(&config.tenant_name),
        "external TLS SANs should retain the Tenant DNS name {}, got {san_names:?}",
        config.tenant_name
    );
    ensure!(
        san_names.contains("localhost"),
        "external TLS SANs should retain localhost compatibility, got {san_names:?}"
    );
    ensure!(
        san_names.contains("rustfs-e2e.local"),
        "external TLS SANs should retain the configured service DNS rustfs-e2e.local, got {san_names:?}"
    );
    ensure!(
        !san_names
            .iter()
            .any(|name| name.contains("cert-manager-external-rotated")),
        "external TLS rotation must not switch to a rotated-only DNS identity, got {san_names:?}"
    );

    for host in &rendered_peer_hosts {
        ensure!(
            san_names.contains(host),
            "external TLS SANs should cover rendered RUSTFS_VOLUMES host {host}; sans={san_names:?} rendered_peer_hosts={rendered_peer_hosts:?}"
        );
    }

    Ok(())
}

#[test]
fn external_secret_tls_material_uses_real_ca_chain_and_preserves_tls_guards() -> Result<()> {
    let smoke = E2eConfig::defaults();
    let config = tls_e2e::external_secret_case_config(&smoke);
    let manifests = tls_e2e::external_tls_secret_manifests(&config)?;
    let tls_secret: corev1::Secret = serde_yaml_ng::from_str(&manifests.tls_secret_manifest)?;
    let ca_secret: corev1::Secret = serde_yaml_ng::from_str(&manifests.ca_secret_manifest)?;
    let tls_data = tls_secret
        .data
        .as_ref()
        .context("external TLS Secret should contain data")?;
    let ca_data = ca_secret
        .data
        .as_ref()
        .context("external CA Secret should contain data")?;
    let leaf_cert = secret_data_value(&tls_secret, "tls.crt")?;
    let leaf_key = secret_data_value(&tls_secret, "tls.key")?;
    let bundled_ca = secret_data_value(&tls_secret, "ca.crt")?;
    let external_ca = secret_data_value(&ca_secret, "ca.crt")?;

    ensure!(tls_secret.type_.as_deref() == Some("kubernetes.io/tls"));
    ensure!(ca_secret.type_.as_deref() == Some("Opaque"));
    ensure!(tls_data.contains_key("tls.crt"));
    ensure!(tls_data.contains_key("tls.key"));
    ensure!(tls_data.contains_key("ca.crt"));
    ensure!(ca_data.contains_key("ca.crt"));
    ensure!(
        !leaf_key.is_empty(),
        "external TLS Secret should include a private key"
    );
    ensure!(
        leaf_cert != external_ca,
        "external positive TLS fixture should not reuse the leaf certificate as ca.crt"
    );
    ensure!(
        bundled_ca == external_ca,
        "server TLS Secret ca.crt and explicit external CA Secret ca.crt should carry the same CA bundle"
    );

    require_certificate_is_ca(external_ca)?;
    require_certificate_verifies_with_ca(leaf_cert, external_ca)?;

    let san_names = certificate_dns_sans(leaf_cert)?;
    for host in rendered_rustfs_volume_hosts(&config)? {
        ensure!(
            san_names.contains(&host),
            "external TLS leaf SANs should cover rendered RUSTFS_VOLUMES peer host {host}; sans={san_names:?}"
        );
    }

    let tenant = tls_e2e::external_secret_tenant(&config);
    let tls = tenant
        .spec
        .tls
        .as_ref()
        .context("external Secret Tenant should enable TLS")?;
    let cert_manager = tls
        .cert_manager
        .as_ref()
        .context("external Secret Tenant should use cert-manager TLS mode")?;
    let ca_trust = cert_manager
        .ca_trust
        .as_ref()
        .context("external Secret Tenant should configure CA trust")?;

    ensure!(
        !cert_manager.manage_certificate,
        "external Secret fixture should keep using user-provided TLS Secrets"
    );
    ensure!(
        tls.enable_internode_https,
        "external Secret fixture should keep internode HTTPS enabled"
    );
    ensure!(
        tls.require_san_match,
        "external Secret fixture should keep SAN matching enabled"
    );
    ensure!(
        ca_trust.source == CaTrustSource::SecretRef,
        "external Secret fixture should trust the explicit CA SecretRef, got {:?}",
        ca_trust.source
    );
    ensure!(
        ca_trust
            .ca_secret_ref
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.key.as_str()))
            == Some((tls_e2e::external_ca_secret_name(&config).as_str(), "ca.crt")),
        "external Secret fixture should point CA trust at the external CA Secret ca.crt"
    );
    ensure!(
        !ca_trust.trust_leaf_certificate_as_ca,
        "external Secret fixture should not bypass CA-chain validation with trustLeafCertificateAsCa"
    );

    Ok(())
}

#[test]
fn negative_secret_fixtures_are_api_admissible_and_still_test_missing_tls_keys() -> Result<()> {
    let config = E2eConfig::defaults();

    for (case, present_key, missing_key) in [
        (
            tls_e2e::NegativeTlsCase::MissingTlsCrt,
            "tls.key",
            "tls.crt",
        ),
        (
            tls_e2e::NegativeTlsCase::MissingTlsKey,
            "tls.crt",
            "tls.key",
        ),
    ] {
        let secret_manifest = tls_e2e::negative_tls_secret_manifest(&config, case)?;
        let tenant_manifest = tls_e2e::negative_case_tenant_manifest(&config, case)?;

        ensure!(
            secret_manifest.contains("type: Opaque"),
            "negative Secret fixture should use an API-admissible Opaque Secret, got:\n{secret_manifest}"
        );
        ensure!(
            secret_manifest.contains(present_key),
            "negative Secret fixture should include {present_key}, got:\n{secret_manifest}"
        );
        ensure!(
            !secret_manifest.contains(missing_key),
            "negative Secret fixture should omit {missing_key}, got:\n{secret_manifest}"
        );
        ensure!(
            tenant_manifest.contains("secretType: Opaque"),
            "negative Tenant fixture should explicitly expect Opaque so Operator validation reaches missing key checks, got:\n{tenant_manifest}"
        );
        ensure!(
            !secret_manifest.contains("redacted-test-fixture")
                && !secret_manifest.contains("-----BEGIN")
                && !secret_manifest.contains("PRIVATE KEY"),
            "negative Secret fixture should not contain raw secret material, got:\n{secret_manifest}"
        );
    }

    Ok(())
}

#[test]
fn missing_ca_fixture_uses_valid_server_cert_key_and_omits_ca_bundle() -> Result<()> {
    let config = E2eConfig::defaults();
    let secret_manifest = tls_e2e::negative_tls_secret_manifest(
        &config,
        tls_e2e::NegativeTlsCase::MissingCaForInternodeHttps,
    )?;
    let tenant_manifest = tls_e2e::negative_case_tenant_manifest(
        &config,
        tls_e2e::NegativeTlsCase::MissingCaForInternodeHttps,
    )?;
    let secret: corev1::Secret = serde_yaml_ng::from_str(&secret_manifest)?;
    let data = secret
        .data
        .as_ref()
        .context("missing-ca Secret fixture should include data")?;
    let cert = data
        .get("tls.crt")
        .context("missing-ca Secret fixture should include tls.crt")?
        .0
        .as_slice();
    let key = data
        .get("tls.key")
        .context("missing-ca Secret fixture should include tls.key")?
        .0
        .as_slice();

    ensure!(secret.type_.as_deref() == Some("kubernetes.io/tls"));
    ensure!(
        pem_contains_label(cert, "CERTIFICATE"),
        "missing-ca Secret fixture should use a parseable-looking PEM certificate so Operator validation reaches CaBundleMissing"
    );
    ensure!(
        pem_contains_label(key, "PRIVATE KEY"),
        "missing-ca Secret fixture should use a parseable-looking PEM private key so Operator validation reaches CaBundleMissing"
    );
    ensure!(
        !data.contains_key("ca.crt"),
        "missing-ca Secret fixture should omit ca.crt to exercise CaBundleMissing"
    );
    ensure!(
        tenant_manifest.contains("enableInternodeHttps: true"),
        "missing-ca Tenant fixture should keep internode HTTPS enabled, got:\n{tenant_manifest}"
    );
    ensure!(
        tenant_manifest.contains("source: CertificateSecretCa"),
        "missing-ca Tenant fixture should use the server Secret CA trust source, got:\n{tenant_manifest}"
    );
    ensure!(
        !secret_manifest.contains("redacted-test-fixture")
            && !secret_manifest.contains("-----BEGIN")
            && !secret_manifest.contains("PRIVATE KEY"),
        "missing-ca Secret fixture manifest should not print raw secret material, got:\n{secret_manifest}"
    );

    Ok(())
}

#[test]
fn cert_manager_artifacts_do_not_expose_secret_material() -> Result<()> {
    let command = tls_e2e::external_tls_secret_apply_command(
        &E2eConfig::defaults(),
        tls_e2e::external_secret_name(&E2eConfig::defaults()),
    )?;

    assertions::require_no_secret_material(
        "external TLS Secret apply command",
        &command.display(),
    )?;
    ensure!(
        !command.display().contains("tls.crt") && !command.display().contains("tls.key"),
        "kubectl apply display should hide Secret stdin payload"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires cert-manager installed in the dedicated Kind cluster; run after `make e2e-live-run`"]
async fn cert_manager_managed_certificate_reaches_tls_ready_and_https_wiring() -> Result<()> {
    let base_config = E2eConfig::from_env();
    live::require_live_enabled(&base_config)?;
    live::ensure_dedicated_context(&base_config)?;
    let config = tls_e2e::managed_certificate_case_config(&base_config);
    let positive_timeout = tls_e2e::positive_cert_manager_tls_timeout(&config);

    let result = async {
        tls_e2e::apply_managed_certificate_case_resources(&config)?;
        let client = kube_client::default_client().await?;
        let tenant = tls_e2e::wait_for_tenant_tls_ready(
            client.clone(),
            &config.test_namespace,
            &config.tenant_name,
            positive_timeout,
        )
        .await?;
        tls_e2e::wait_for_certificate_ready(
            client.clone(),
            &config.test_namespace,
            &tls_e2e::managed_certificate_name(&config),
            positive_timeout,
        )
        .await?;
        tls_e2e::assert_live_workload_tls_wiring(client, &config, &tenant).await?;
        Ok(())
    }
    .await;

    collect_tls_artifacts_on_error(
        &config,
        "cert_manager_managed_certificate_reaches_tls_ready_and_https_wiring",
        &result,
    );
    result
}

#[tokio::test]
#[ignore = "creates an external TLS Secret and waits for rollout; run after `make e2e-live-run`"]
async fn cert_manager_external_secret_reaches_tls_ready_and_rolls_on_secret_hash() -> Result<()> {
    let base_config = E2eConfig::from_env();
    live::require_live_enabled(&base_config)?;
    live::ensure_dedicated_context(&base_config)?;
    let config = tls_e2e::external_secret_case_config(&base_config);
    let positive_timeout = tls_e2e::positive_cert_manager_tls_timeout(&config);

    let result = async {
        tls_e2e::apply_external_secret_case_resources(&config)?;
        let client = kube_client::default_client().await?;
        let tenant = tls_e2e::wait_for_tenant_tls_ready(
            client.clone(),
            &config.test_namespace,
            &config.tenant_name,
            positive_timeout,
        )
        .await?;
        let initial_hash = assertions::tenant_tls_observed_hash(&tenant)?;
        tls_e2e::rotate_external_tls_secret(&config)?;
        let rotated = tls_e2e::wait_for_tenant_tls_hash_change(
            client.clone(),
            &config.test_namespace,
            &config.tenant_name,
            &initial_hash,
            positive_timeout,
        )
        .await?;
        tls_e2e::assert_live_workload_tls_wiring(client, &config, &rotated).await?;
        Ok(())
    }
    .await;

    collect_tls_artifacts_on_error(
        &config,
        "cert_manager_external_secret_reaches_tls_ready_and_rolls_on_secret_hash",
        &result,
    );
    result
}

#[tokio::test]
#[ignore = "mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_rejects_secret_missing_tls_crt() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::MissingTlsCrt,
        "CertificateSecretMissingKey",
    )
    .await
}

#[tokio::test]
#[ignore = "mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_rejects_secret_missing_tls_key() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::MissingTlsKey,
        "CertificateSecretMissingKey",
    )
    .await
}

#[tokio::test]
#[ignore = "mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_rejects_secret_missing_ca_for_internode_https() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::MissingCaForInternodeHttps,
        "CaBundleMissing",
    )
    .await
}

#[tokio::test]
#[ignore = "requires cert-manager API and mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_rejects_missing_issuer_for_managed_certificate() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::MissingIssuer,
        "CertManagerIssuerNotFound",
    )
    .await
}

#[tokio::test]
#[ignore = "requires cert-manager API and mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_reports_pending_certificate_not_ready() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::PendingCertificate,
        "CertManagerCertificateNotReady",
    )
    .await
}

#[tokio::test]
#[ignore = "mutates live Tenant fixtures; run after `make e2e-live-run`"]
async fn cert_manager_rejects_hot_reload() -> Result<()> {
    assert_negative_case_tls_reason(
        tls_e2e::NegativeTlsCase::HotReloadUnsupported,
        "TlsHotReloadUnsupported",
    )
    .await
}

async fn assert_negative_case_tls_reason(
    case: tls_e2e::NegativeTlsCase,
    reason: &str,
) -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let result = async {
        tls_e2e::apply_negative_case_resources(&config, case)?;
        let client = kube_client::default_client().await?;
        let tenant = tls_e2e::wait_for_tenant_tls_reason(
            client,
            &config.test_namespace,
            &config.tenant_name,
            reason,
            config.timeout,
        )
        .await?;
        assertions::require_no_secret_material(
            "Tenant TLS status",
            &format!("{:?}", tenant.status),
        )?;
        Ok(())
    }
    .await;

    collect_tls_artifacts_on_error(&config, case.case_name(), &result);
    result
}

fn collect_tls_artifacts_on_error(config: &E2eConfig, case_name: &str, result: &Result<()>) {
    if let Err(error) = result {
        let collector = ArtifactCollector::new(&config.artifacts_dir);
        if let Err(artifact_error) = collector.collect_kubernetes_snapshot(case_name, config) {
            eprintln!("failed to collect e2e artifacts after {error}: {artifact_error}");
        }
    }
}

fn assert_secret_manifest_tenant_watch_label(
    context: &str,
    manifest: &str,
    expected_tenant: &str,
) -> Result<()> {
    let secret: corev1::Secret = serde_yaml_ng::from_str(manifest)
        .with_context(|| format!("parse {context} Secret manifest"))?;
    let labels = secret
        .metadata
        .labels
        .as_ref()
        .with_context(|| format!("{context} missing labels"))?;
    ensure!(
        labels.get("rustfs.tenant").map(String::as_str) == Some(expected_tenant),
        "{context} should carry rustfs.tenant label for Tenant {expected_tenant}"
    );
    Ok(())
}

fn assert_positive_tls_fixture_uses_minimal_four_volume_https_erasure_set(
    case_name: &str,
    config: &E2eConfig,
    tenant: &Tenant,
    tls_plan: &TlsPlan,
) -> Result<()> {
    let pool = tenant
        .spec
        .pools
        .first()
        .with_context(|| format!("{case_name} should include one pool"))?;
    ensure!(
        pool.servers == 4,
        "{case_name} should keep the 4-server erasure set, got {}",
        pool.servers
    );
    ensure!(
        pool.persistence.volumes_per_server == 1,
        "{case_name} should use one volume per server for a 4-volume erasure set, got {}",
        pool.persistence.volumes_per_server
    );
    let required_pvc_count = usize::try_from(pool.servers)
        .context("positive TLS pool server count should be non-negative")?
        * usize::try_from(pool.persistence.volumes_per_server)
            .context("positive TLS pool volumes_per_server should be non-negative")?;
    ensure!(
        required_pvc_count == 4,
        "{case_name} should render exactly four RustFS data PVCs, got {required_pvc_count}"
    );
    ensure!(
        config.pv_count == required_pvc_count,
        "{case_name} isolated PV count should match its 4x1 PVC count: pv_count={} required_pvc_count={required_pvc_count}",
        config.pv_count
    );

    let statefulset = tenant.new_statefulset_with_tls_plan(pool, tls_plan)?;
    let statefulset_spec = statefulset
        .spec
        .as_ref()
        .context("StatefulSet should have spec")?;
    let expected_headless_service = format!("{}-hl", config.tenant_name);
    ensure!(
        statefulset_spec.service_name.as_deref() == Some(expected_headless_service.as_str()),
        "{case_name} StatefulSet should target the headless Service {expected_headless_service}, got {:?}",
        statefulset_spec.service_name
    );
    ensure!(
        statefulset_spec
            .volume_claim_templates
            .as_ref()
            .map(Vec::len)
            == Some(1),
        "{case_name} StatefulSet should render exactly one PVC template for the single-volume fixture"
    );

    let headless_service = tenant.new_headless_service_with_tls_plan(tls_plan);
    let headless_service_spec = headless_service
        .spec
        .as_ref()
        .context("headless Service should have spec")?;
    ensure!(
        headless_service.metadata.name.as_deref() == Some(expected_headless_service.as_str()),
        "{case_name} headless Service should be named {expected_headless_service}, got {:?}",
        headless_service.metadata.name
    );
    ensure!(
        headless_service_spec.publish_not_ready_addresses == Some(true),
        "{case_name} headless Service should publish not-ready pod DNS records for TLS bootstrap"
    );

    let pod_spec = statefulset_pod_spec(&statefulset)?;
    let rustfs_container = rustfs_container(pod_spec)?;
    let rustfs_volumes = env_value(rustfs_container, "RUSTFS_VOLUMES")?;
    require_single_volume_https_rustfs_volumes(case_name, rustfs_volumes)?;

    let expected_sans = expected_tls_dns_names(config, tenant);
    for host in expand_rustfs_volume_hosts(rustfs_volumes)? {
        ensure!(
            expected_sans.contains(&host),
            "{case_name} expected SANs should cover rendered RUSTFS_VOLUMES host {host}; expected_sans={expected_sans:?} rustfs_volumes={rustfs_volumes}"
        );
    }

    ensure!(
        env_value(rustfs_container, "RUSTFS_TLS_PATH")? == DEFAULT_TLS_MOUNT_PATH,
        "{case_name} should set RUSTFS_TLS_PATH={DEFAULT_TLS_MOUNT_PATH}"
    );
    require_tls_material_files_share_runtime_dir(case_name, pod_spec, rustfs_container)?;

    Ok(())
}

fn statefulset_pod_spec(statefulset: &StatefulSet) -> Result<&corev1::PodSpec> {
    statefulset
        .spec
        .as_ref()
        .context("StatefulSet should have spec")?
        .template
        .spec
        .as_ref()
        .context("StatefulSet pod template should have spec")
}

fn rustfs_container(pod_spec: &corev1::PodSpec) -> Result<&corev1::Container> {
    pod_spec
        .containers
        .iter()
        .find(|container| container.name == "rustfs")
        .context("rustfs container should exist")
}

fn env_value<'a>(container: &'a corev1::Container, name: &str) -> Result<&'a str> {
    container
        .env
        .as_ref()
        .and_then(|vars| vars.iter().find(|var| var.name == name))
        .and_then(|var| var.value.as_deref())
        .with_context(|| format!("rustfs container should set {name}"))
}

fn require_single_volume_https_rustfs_volumes(case_name: &str, volumes_value: &str) -> Result<()> {
    let specs = volumes_value.split_whitespace().collect::<Vec<_>>();
    ensure!(
        specs.len() == 1,
        "{case_name} should render one RUSTFS_VOLUMES entry for the single positive TLS pool, got {volumes_value}"
    );

    for spec in specs {
        let (host_pattern, path_expression) = spec
            .strip_prefix("https://")
            .with_context(|| {
                format!("{case_name} RUSTFS_VOLUMES entry should use https://: {spec}")
            })?
            .split_once(":9000")
            .with_context(|| {
                format!("{case_name} RUSTFS_VOLUMES entry should include :9000: {spec}")
            })?;
        ensure!(
            host_pattern.contains(".svc.cluster.local"),
            "{case_name} RUSTFS_VOLUMES should use peer FQDNs, got host pattern {host_pattern}"
        );
        ensure!(
            path_expression == "/data/rustfs{0...0}" || path_expression == "/data/rustfs0",
            "{case_name} RUSTFS_VOLUMES should use a single-volume path expression, got {path_expression} in {volumes_value}"
        );
    }

    Ok(())
}

fn expected_tls_dns_names(config: &E2eConfig, tenant: &Tenant) -> BTreeSet<String> {
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

    names
}

fn require_tls_material_files_share_runtime_dir(
    case_name: &str,
    pod_spec: &corev1::PodSpec,
    rustfs_container: &corev1::Container,
) -> Result<()> {
    let tls_volume = pod_spec
        .volumes
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|volume| volume.name == "rustfs-tls-server")
        .with_context(|| format!("{case_name} should render rustfs-tls-server volume"))?;
    for file in [RUSTFS_TLS_CERT_FILE, RUSTFS_TLS_KEY_FILE, RUSTFS_CA_FILE] {
        ensure!(
            tls_volume_has_item_path(tls_volume, file),
            "{case_name} TLS volume should render {file} into the runtime TLS directory"
        );
    }

    let mounts = rustfs_container
        .volume_mounts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|mount| mount.name == "rustfs-tls-server")
        .collect::<Vec<_>>();
    ensure!(
        !mounts.is_empty(),
        "{case_name} rustfs container should mount rustfs-tls-server"
    );

    let has_directory_mount = mounts
        .iter()
        .any(|mount| mount.mount_path == DEFAULT_TLS_MOUNT_PATH && mount.sub_path.is_none());
    if !has_directory_mount {
        for file in [RUSTFS_TLS_CERT_FILE, RUSTFS_TLS_KEY_FILE, RUSTFS_CA_FILE] {
            let expected_mount_path = format!("{DEFAULT_TLS_MOUNT_PATH}/{file}");
            ensure!(
                mounts
                    .iter()
                    .any(|mount| mount.mount_path == expected_mount_path
                        && mount.sub_path.as_deref() == Some(file)),
                "{case_name} should mount {file} under {DEFAULT_TLS_MOUNT_PATH}, mounts={mounts:?}"
            );
        }
    }

    Ok(())
}

fn tls_volume_has_item_path(volume: &corev1::Volume, path: &str) -> bool {
    volume
        .secret
        .as_ref()
        .and_then(|secret| secret.items.as_ref())
        .is_some_and(|items| items.iter().any(|item| item.path == path))
        || volume
            .projected
            .as_ref()
            .and_then(|projected| projected.sources.as_ref())
            .is_some_and(|sources| {
                sources.iter().any(|source| {
                    source
                        .secret
                        .as_ref()
                        .and_then(|secret| secret.items.as_ref())
                        .is_some_and(|items| items.iter().any(|item| item.path == path))
                })
            })
}

fn dns_names_from_subject_alt_name(subject_alt_name: &str) -> Result<BTreeSet<String>> {
    let value = subject_alt_name
        .strip_prefix("subjectAltName=")
        .context("subjectAltName addext should start with subjectAltName=")?;
    let names = value
        .split(',')
        .map(|entry| {
            entry
                .strip_prefix("DNS:")
                .with_context(|| format!("subjectAltName entry should be a DNS name: {entry}"))
                .map(ToString::to_string)
        })
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(!names.is_empty(), "subjectAltName should contain DNS SANs");
    Ok(names)
}

fn rendered_rustfs_volume_hosts(config: &E2eConfig) -> Result<BTreeSet<String>> {
    let tenant = tls_e2e::external_secret_tenant(config);
    let pool = tenant
        .spec
        .pools
        .first()
        .context("external Secret fixture should have a pool")?;
    let tls_plan =
        tls_e2e::sample_tls_plan("sha256:e2e-test", tls_e2e::external_secret_name(config));
    let statefulset = tenant.new_statefulset_with_tls_plan(pool, &tls_plan)?;
    let pod_spec = statefulset
        .spec
        .as_ref()
        .context("StatefulSet should have spec")?
        .template
        .spec
        .as_ref()
        .context("StatefulSet pod template should have spec")?;
    let rustfs_container = pod_spec
        .containers
        .iter()
        .find(|container| container.name == "rustfs")
        .context("rustfs container should exist")?;
    let volumes_value = rustfs_container
        .env
        .as_ref()
        .and_then(|vars| vars.iter().find(|var| var.name == "RUSTFS_VOLUMES"))
        .and_then(|var| var.value.as_deref())
        .context("rustfs container should render RUSTFS_VOLUMES")?;

    expand_rustfs_volume_hosts(volumes_value)
}

fn expand_rustfs_volume_hosts(volumes_value: &str) -> Result<BTreeSet<String>> {
    let mut hosts = BTreeSet::new();
    for spec in volumes_value.split_whitespace() {
        let host_pattern = spec
            .strip_prefix("https://")
            .with_context(|| format!("RUSTFS_VOLUMES entry should use https://: {spec}"))?
            .split_once(":9000")
            .with_context(|| format!("RUSTFS_VOLUMES entry should include :9000: {spec}"))?
            .0;
        if let Some(range_start) = host_pattern.find("{0...") {
            let range_end = host_pattern[range_start..]
                .find('}')
                .map(|offset| range_start + offset)
                .with_context(|| format!("host range should close with }}: {host_pattern}"))?;
            let last_ordinal = host_pattern[range_start + "{0...".len()..range_end]
                .parse::<usize>()
                .with_context(|| {
                    format!("host range should end with an ordinal: {host_pattern}")
                })?;
            let prefix = &host_pattern[..range_start];
            let suffix = &host_pattern[range_end + 1..];
            for ordinal in 0..=last_ordinal {
                hosts.insert(format!("{prefix}{ordinal}{suffix}"));
            }
        } else {
            hosts.insert(host_pattern.to_string());
        }
    }
    ensure!(
        !hosts.is_empty(),
        "RUSTFS_VOLUMES should render at least one peer host"
    );
    Ok(hosts)
}

fn secret_data_value<'a>(secret: &'a corev1::Secret, key: &str) -> Result<&'a [u8]> {
    secret
        .data
        .as_ref()
        .with_context(|| format!("Secret {:?} should contain data", secret.metadata.name))?
        .get(key)
        .with_context(|| format!("Secret {:?} should contain {key}", secret.metadata.name))
        .map(|value| value.0.as_slice())
}

fn require_certificate_is_ca(cert: &[u8]) -> Result<()> {
    let text = openssl_x509_text(cert)?;
    ensure!(
        text.contains("CA:TRUE"),
        "certificate should have BasicConstraints CA:TRUE, got:\n{text}"
    );
    Ok(())
}

fn require_certificate_verifies_with_ca(cert: &[u8], ca: &[u8]) -> Result<()> {
    let dir = TempDir::new()?;
    let cert_path = dir.path().join("tls.crt");
    let ca_path = dir.path().join("ca.crt");
    std::fs::write(&cert_path, cert)?;
    std::fs::write(&ca_path, ca)?;

    CommandSpec::new("openssl")
        .args(["verify", "-CAfile"])
        .arg(ca_path.display().to_string())
        .arg(cert_path.display().to_string())
        .run_checked()?;
    Ok(())
}

fn certificate_dns_sans(cert: &[u8]) -> Result<BTreeSet<String>> {
    let text = openssl_x509_text(cert)?;
    let names = text
        .split([',', '\n'])
        .filter_map(|entry| entry.trim().strip_prefix("DNS:"))
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    ensure!(
        !names.is_empty(),
        "certificate should contain DNS subjectAltName entries, got:\n{text}"
    );
    Ok(names)
}

fn openssl_x509_text(cert: &[u8]) -> Result<String> {
    let dir = TempDir::new()?;
    let cert_path = dir.path().join("cert.pem");
    std::fs::write(&cert_path, cert)?;
    let output = CommandSpec::new("openssl")
        .args(["x509", "-in"])
        .arg(cert_path.display().to_string())
        .args(["-noout", "-text"])
        .run_checked()?;
    Ok(output.stdout)
}

fn projected_secret_item(
    volume: &corev1::Volume,
    secret_name: &str,
    key: &str,
    path: &str,
) -> bool {
    volume
        .projected
        .as_ref()
        .and_then(|projected| projected.sources.as_ref())
        .map(|sources| {
            sources.iter().any(|source| {
                source
                    .secret
                    .as_ref()
                    .filter(|secret| secret.name == secret_name)
                    .and_then(|secret| secret.items.as_ref())
                    .map(|items| {
                        items
                            .iter()
                            .any(|item| item.key == key && item.path == path)
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn pem_contains_label(bytes: &[u8], label: &str) -> bool {
    std::str::from_utf8(bytes)
        .map(|pem| {
            pem.contains(&format!("-----BEGIN {label}-----"))
                && pem.contains(&format!("-----END {label}-----"))
        })
        .unwrap_or(false)
}
