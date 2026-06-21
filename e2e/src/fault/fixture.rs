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

use crate::framework::{
    command::CommandOutput,
    config::ClusterTestConfig,
    kubectl::Kubectl,
    resources::{
        credential_secret_manifest, credential_secret_name,
        reset_tenant_resources as reset_generic_tenant_resources,
    },
    tenant_factory::TenantTemplate,
};

const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const FAULT_TEST_MANAGER: &str = "rustfs-operator-fault-test";
const FAULT_TEST_TENANT_ANNOTATION: &str = "rustfs.com/fault-test-tenant";

pub fn namespace_manifest(config: &ClusterTestConfig) -> String {
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

pub fn tenant_manifest(config: &ClusterTestConfig) -> Result<String> {
    let template = TenantTemplate::real_cluster(
        &config.test_namespace,
        &config.tenant_name,
        &config.rustfs_image,
        &config.storage_class,
        credential_secret_name(config),
    );
    Ok(serde_yaml_ng::to_string(&template.build())?)
}

pub fn apply_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);
    if !ensure_namespace_owned_or_absent(config)? {
        kubectl
            .create_yaml_command(namespace_manifest(config))
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
        .apply_yaml_command(tenant_manifest(config)?)
        .run_checked()?;
    Ok(())
}

pub fn reset_tenant_resources(config: &ClusterTestConfig) -> Result<()> {
    if !ensure_namespace_owned_or_absent(config)? {
        return Ok(());
    }
    reset_generic_tenant_resources(config)
}

fn ensure_namespace_owned_or_absent(config: &ClusterTestConfig) -> Result<bool> {
    let output = Kubectl::new(config)
        .command(["get", "namespace", &config.test_namespace, "-o", "json"])
        .run()?;

    match output.code {
        Some(0) => {
            validate_namespace_ownership(
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

fn validate_namespace_ownership(raw: &str, namespace: &str, tenant_name: &str) -> Result<()> {
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

fn is_not_found(output: &CommandOutput) -> bool {
    output.stderr.contains("NotFound")
        || output.stderr.contains("not found")
        || output.stdout.contains("NotFound")
        || output.stdout.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::{namespace_manifest, tenant_manifest, validate_namespace_ownership};
    use crate::fault::config::FaultTestConfig;

    #[test]
    fn fault_tenant_manifest_uses_real_cluster_defaults() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let manifest = tenant_manifest(&config.cluster).expect("fault tenant manifest");

        assert!(manifest.contains("namespace: rustfs-fault-test"));
        assert!(manifest.contains("storageClassName: fast-csi"));
        assert!(manifest.contains("storage: 100Gi"));
        assert!(!manifest.contains("rustfs-storage"));
        assert!(!manifest.contains("RUSTFS_UNSAFE_BYPASS_DISK_CHECK"));
    }

    #[test]
    fn fault_namespace_manifest_records_destructive_test_ownership() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let manifest = namespace_manifest(&config.cluster);

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
            validate_namespace_ownership(owned, "rustfs-fault-test", "fault-test-tenant").is_ok()
        );

        let unowned = r#"{"metadata":{"labels":{},"annotations":{}}}"#;
        assert!(
            validate_namespace_ownership(unowned, "rustfs-fault-test", "fault-test-tenant")
                .is_err()
        );

        assert!(
            validate_namespace_ownership(owned, "rustfs-fault-test", "another-tenant").is_err()
        );
    }
}
