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

use anyhow::{Context, Result, bail};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::framework::{
    command::{CommandOutput, CommandSpec},
    config::E2eConfig,
    kubectl::Kubectl,
    tenant_factory::TenantTemplate,
};
use operator::types::v1alpha1::k8s::PodManagementPolicy;

const E2E_ACCESS_KEY: &str = "e2eaccess";
const E2E_SECRET_KEY: &str = "e2esecret";
const RESOURCE_RESET_TIMEOUT: Duration = Duration::from_secs(120);
const RESOURCE_RESET_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub fn credential_secret_name(config: &E2eConfig) -> String {
    format!("{}-credentials", config.tenant_name)
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

pub fn credential_secret_manifest(config: &E2eConfig) -> String {
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
        access_key = E2E_ACCESS_KEY,
        secret_key = E2E_SECRET_KEY
    )
}

pub fn smoke_tenant_template(config: &E2eConfig) -> TenantTemplate {
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

pub fn smoke_tenant_manifest(config: &E2eConfig) -> Result<String> {
    Ok(serde_yaml_ng::to_string(
        &smoke_tenant_template(config).build(),
    )?)
}

pub fn apply_smoke_tenant_resources(config: &E2eConfig) -> Result<()> {
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

pub fn reset_and_apply_smoke_tenant_resources(config: &E2eConfig) -> Result<()> {
    reset_smoke_tenant_resources(config)?;
    apply_smoke_tenant_resources(config)
}

pub fn reset_smoke_tenant_resources(config: &E2eConfig) -> Result<()> {
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

pub fn cleanup_smoke_tenant_resources(config: &E2eConfig) -> Result<()> {
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
    use super::{credential_secret_manifest, credential_secret_name, smoke_tenant_manifest};
    use crate::framework::config::E2eConfig;

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
}
