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
use serde_json::Value;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::framework::{
    command::{CommandOutput, CommandSpec},
    config::ClusterTestConfig,
    kubectl::Kubectl,
    tenant_factory::TenantTemplate,
};
use operator::types::v1alpha1::k8s::PodManagementPolicy;

const TEST_ACCESS_KEY: &str = "testaccess";
const TEST_SECRET_KEY: &str = "testsecret";
const RESOURCE_RESET_TIMEOUT: Duration = Duration::from_secs(120);
const RESOURCE_RESET_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const FAULT_TEST_MANAGER: &str = "rustfs-operator-fault-test";
const FAULT_TEST_TENANT_ANNOTATION: &str = "rustfs.com/fault-test-tenant";

pub fn credential_secret_name(config: &ClusterTestConfig) -> String {
    format!("{}-credentials", config.tenant_name)
}

pub fn test_credentials() -> (&'static str, &'static str) {
    (TEST_ACCESS_KEY, TEST_SECRET_KEY)
}

pub fn namespace_manifest(namespace: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
"#
    )
}

pub fn fault_namespace_manifest(config: &ClusterTestConfig) -> String {
    format!(
        r#"apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
  labels:
    {managed_by_label}: {manager}
  annotations:
    {tenant_annotation}: {tenant_name}
"#,
        namespace = config.test_namespace,
        managed_by_label = MANAGED_BY_LABEL,
        manager = FAULT_TEST_MANAGER,
        tenant_annotation = FAULT_TEST_TENANT_ANNOTATION,
        tenant_name = config.tenant_name,
    )
}

pub fn credential_secret_manifest(config: &ClusterTestConfig) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {secret_name}
  namespace: {namespace}
type: Opaque
stringData:
  accesskey: {access_key}
  secretkey: {secret_key}
"#,
        secret_name = credential_secret_name(config),
        namespace = config.test_namespace,
        access_key = TEST_ACCESS_KEY,
        secret_key = TEST_SECRET_KEY
    )
}

pub fn smoke_tenant_template(config: &ClusterTestConfig) -> TenantTemplate {
    let mut template = TenantTemplate::kind_local(
        &config.test_namespace,
        &config.tenant_name,
        &config.rustfs_image,
        &config.storage_class,
        credential_secret_name(config),
    );

    template.pod_management_policy = Some(
        config
            .pod_management_policy
            .clone()
            .unwrap_or(PodManagementPolicy::Parallel),
    );

    template
}

pub fn smoke_tenant_manifest(config: &ClusterTestConfig) -> Result<String> {
    Ok(serde_yaml_ng::to_string(
        &smoke_tenant_template(config).build(),
    )?)
}

pub fn fault_tenant_manifest(config: &ClusterTestConfig) -> Result<String> {
    let template = TenantTemplate::real_cluster(
        &config.test_namespace,
        &config.tenant_name,
        &config.rustfs_image,
        &config.storage_class,
        credential_secret_name(config),
    );
    Ok(serde_yaml_ng::to_string(&template.build())?)
}

pub fn apply_smoke_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);
    kubectl
        .apply_yaml_command(namespace_manifest(&config.test_namespace))
        .run_checked()?;
    kubectl
        .apply_yaml_command(credential_secret_manifest(config))
        .run_checked()?;
    kubectl
        .apply_yaml_command(smoke_tenant_manifest(config)?)
        .run_checked()?;
    Ok(())
}

pub fn apply_fault_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);
    if !ensure_fault_namespace_owned_or_absent(config)? {
        kubectl
            .create_yaml_command(fault_namespace_manifest(config))
            .run_checked()
            .with_context(|| {
                format!(
                    "create dedicated fault-test namespace {:?}",
                    config.test_namespace
                )
            })?;
    }
    kubectl
        .apply_yaml_command(credential_secret_manifest(config))
        .run_checked()?;
    kubectl
        .apply_yaml_command(fault_tenant_manifest(config)?)
        .run_checked()?;
    Ok(())
}

pub fn reset_fault_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    if !ensure_fault_namespace_owned_or_absent(config)? {
        return Ok(());
    }
    reset_tenant_resources(config)
}

pub fn reset_and_apply_smoke_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    reset_tenant_resources(config)?;
    apply_smoke_tenant_resources(config)
}

pub fn reset_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);
    if !namespace_exists(&kubectl, &config.test_namespace)? {
        return Ok(());
    }

    let kubectl = kubectl.namespaced(&config.test_namespace);
    let selector = format!("rustfs.tenant={}", config.tenant_name);

    run_delete(kubectl.command([
        "delete",
        "tenant",
        &config.tenant_name,
        "--ignore-not-found",
        "--wait=false",
    ]))?;
    run_delete(kubectl.command([
        "delete",
        "statefulset",
        "-l",
        &selector,
        "--ignore-not-found",
        "--wait=false",
    ]))?;
    run_delete(kubectl.command([
        "delete",
        "pod",
        "-l",
        &selector,
        "--ignore-not-found",
        "--wait=false",
    ]))?;
    run_delete(kubectl.command([
        "delete",
        "pvc",
        "-l",
        &selector,
        "--ignore-not-found",
        "--wait=false",
    ]))?;
    run_delete(kubectl.command([
        "delete",
        "svc",
        "-l",
        &selector,
        "--ignore-not-found",
        "--wait=false",
    ]))?;

    wait_for_named_resource_deleted(
        &kubectl,
        "tenant",
        &config.tenant_name,
        RESOURCE_RESET_TIMEOUT,
    )?;
    wait_for_selector_empty(&kubectl, "statefulset", &selector, RESOURCE_RESET_TIMEOUT)?;
    wait_for_selector_empty(&kubectl, "pod", &selector, RESOURCE_RESET_TIMEOUT)?;
    wait_for_selector_empty(&kubectl, "pvc", &selector, RESOURCE_RESET_TIMEOUT)?;
    wait_for_selector_empty(&kubectl, "svc", &selector, RESOURCE_RESET_TIMEOUT)?;

    Ok(())
}

pub fn cleanup_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    let kubectl = Kubectl::new(config).namespaced(&config.test_namespace);
    let selector = format!("rustfs.tenant={}", config.tenant_name);

    run_best_effort(
        kubectl.command([
            "delete",
            "tenant",
            &config.tenant_name,
            "--ignore-not-found",
        ]),
        "tenant",
    );
    run_best_effort(
        kubectl.command([
            "delete",
            "statefulset",
            "-l",
            &selector,
            "--ignore-not-found",
        ]),
        "statefulsets",
    );
    run_best_effort(
        kubectl.command(["delete", "pod", "-l", &selector, "--ignore-not-found"]),
        "pods",
    );
    run_best_effort(
        kubectl.command(["delete", "pvc", "-l", &selector, "--ignore-not-found"]),
        "PVCs",
    );
    run_best_effort(
        kubectl.command(["delete", "svc", "-l", &selector, "--ignore-not-found"]),
        "services",
    );

    Ok(())
}

fn run_best_effort(command: crate::framework::command::CommandSpec, resource_desc: &str) {
    if let Err(error) = command.run() {
        println!("best-effort cleanup for {resource_desc} skipped: {error}");
    }
}

fn namespace_exists(kubectl: &Kubectl, namespace: &str) -> Result<bool> {
    let output = kubectl.command(["get", "namespace", namespace]).run()?;
    Ok(output.code == Some(0))
}

fn ensure_fault_namespace_owned_or_absent(config: &ClusterTestConfig) -> Result<bool> {
    let output = Kubectl::new(config)
        .command(["get", "namespace", &config.test_namespace, "-o", "json"])
        .run()?;

    match output.code {
        Some(0) => {
            validate_fault_namespace_ownership(
                &output.stdout,
                &config.test_namespace,
                &config.tenant_name,
            )?;
            Ok(true)
        }
        _ if is_not_found(&output) => Ok(false),
        _ => bail!(
            "failed to inspect fault-test namespace {:?} before destructive operation\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
            config.test_namespace,
            output.code,
            output.stdout,
            output.stderr
        ),
    }
}

fn validate_fault_namespace_ownership(raw: &str, namespace: &str, tenant_name: &str) -> Result<()> {
    let value = serde_json::from_str::<Value>(raw)
        .with_context(|| format!("parse namespace {namespace:?} json"))?;
    let manager = value
        .pointer("/metadata/labels/app.kubernetes.io~1managed-by")
        .and_then(Value::as_str);
    let owned_tenant = value
        .pointer("/metadata/annotations/rustfs.com~1fault-test-tenant")
        .and_then(Value::as_str);

    ensure!(
        manager == Some(FAULT_TEST_MANAGER) && owned_tenant == Some(tenant_name),
        "refusing destructive fault-test operation in namespace {namespace:?}: expected label \
         {MANAGED_BY_LABEL}={FAULT_TEST_MANAGER:?} and annotation \
         {FAULT_TEST_TENANT_ANNOTATION}={tenant_name:?}, got manager={manager:?}, \
         tenant={owned_tenant:?}; use a dedicated namespace or explicitly label and annotate it \
         only after verifying that it contains no non-test workloads"
    );
    Ok(())
}

fn run_delete(command: CommandSpec) -> Result<()> {
    command.run_checked()?;
    Ok(())
}

fn wait_for_named_resource_deleted(
    kubectl: &Kubectl,
    resource: &str,
    name: &str,
    timeout: Duration,
) -> Result<()> {
    wait_until(&format!("{resource}/{name} to be deleted"), timeout, || {
        let output = kubectl
            .command(["get", resource, name, "-o", "name"])
            .run()?;
        match output.code {
            Some(0) => Ok(false),
            _ if is_not_found(&output) => Ok(true),
            _ => bail!(
                "command failed while waiting for {resource}/{name} deletion\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
                output.code,
                output.stdout,
                output.stderr
            ),
        }
    })
}

fn wait_for_selector_empty(
    kubectl: &Kubectl,
    resource: &str,
    selector: &str,
    timeout: Duration,
) -> Result<()> {
    wait_until(
        &format!("{resource} selector {selector} to be empty"),
        timeout,
        || {
            let output = kubectl
                .command([
                    "get",
                    resource,
                    "-l",
                    selector,
                    "-o",
                    "name",
                    "--ignore-not-found",
                ])
                .run_checked()?;
            Ok(output.stdout.lines().all(|line| line.trim().is_empty()))
        },
    )
}

fn wait_until<F>(description: &str, timeout: Duration, mut condition: F) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if condition().with_context(|| format!("check {description}"))? {
            return Ok(());
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for {description} after {timeout:?}");
        }

        sleep(RESOURCE_RESET_POLL_INTERVAL);
    }
}

fn is_not_found(output: &CommandOutput) -> bool {
    output.stderr.contains("NotFound")
        || output.stderr.contains("not found")
        || output.stdout.contains("NotFound")
        || output.stdout.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::{
        credential_secret_manifest, credential_secret_name, fault_namespace_manifest,
        fault_tenant_manifest, smoke_tenant_manifest, validate_fault_namespace_ownership,
    };
    use crate::framework::config::E2eConfig;
    use crate::framework::fault_config::FaultTestConfig;

    #[test]
    fn smoke_tenant_manifest_wires_secret_storage_and_image() {
        let config = E2eConfig::defaults();
        let manifest = smoke_tenant_manifest(&config).expect("tenant manifest");

        assert!(manifest.contains("kind: Tenant"));
        assert!(manifest.contains("namespace: rustfs-e2e-smoke"));
        assert!(manifest.contains("image: rustfs/rustfs:latest"));
        assert!(manifest.contains("storageClassName: local-storage"));
        assert!(manifest.contains("name: e2e-tenant-credentials"));
    }

    #[test]
    fn credential_secret_uses_e2e_tenant_scope() {
        let config = E2eConfig::defaults();
        let manifest = credential_secret_manifest(&config);

        assert_eq!(credential_secret_name(&config), "e2e-tenant-credentials");
        assert!(manifest.contains("namespace: rustfs-e2e-smoke"));
        assert!(manifest.contains("accesskey:"));
        assert!(manifest.contains("secretkey:"));
    }

    #[test]
    fn fault_tenant_manifest_uses_real_cluster_defaults() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let manifest = fault_tenant_manifest(&config.cluster).expect("fault tenant manifest");

        assert!(manifest.contains("namespace: rustfs-fault-test"));
        assert!(manifest.contains("storageClassName: fast-csi"));
        assert!(!manifest.contains("rustfs-storage"));
        assert!(!manifest.contains("RUSTFS_UNSAFE_BYPASS_DISK_CHECK"));
    }

    #[test]
    fn fault_namespace_manifest_records_destructive_test_ownership() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let manifest = fault_namespace_manifest(&config.cluster);

        assert!(manifest.contains("name: rustfs-fault-test"));
        assert!(manifest.contains("app.kubernetes.io/managed-by: rustfs-operator-fault-test"));
        assert!(manifest.contains("rustfs.com/fault-test-tenant: fault-test-tenant"));
    }

    #[test]
    fn fault_namespace_ownership_requires_matching_manager_and_tenant() {
        let owned = r#"{
            "metadata": {
                "labels": {
                    "app.kubernetes.io/managed-by": "rustfs-operator-fault-test"
                },
                "annotations": {
                    "rustfs.com/fault-test-tenant": "fault-test-tenant"
                }
            }
        }"#;
        assert!(
            validate_fault_namespace_ownership(owned, "rustfs-fault-test", "fault-test-tenant")
                .is_ok()
        );

        let unowned = r#"{"metadata":{"labels":{},"annotations":{}}}"#;
        assert!(
            validate_fault_namespace_ownership(unowned, "rustfs-fault-test", "fault-test-tenant")
                .is_err()
        );

        assert!(
            validate_fault_namespace_ownership(owned, "rustfs-fault-test", "another-tenant")
                .is_err()
        );
    }
}
