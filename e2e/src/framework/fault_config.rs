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
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

use crate::framework::{command::CommandSpec, config::ClusterTestConfig, kubectl::Kubectl};

#[derive(Debug, Clone)]
pub struct FaultTestConfig {
    pub cluster: ClusterTestConfig,
    pub destructive_enabled: bool,
    pub scenario: String,
    pub duration: Duration,
    pub percent: u8,
    pub workload_objects: usize,
    pub request_timeout: Duration,
    pub require_client_disruption: bool,
    pub chaos_namespace: String,
}

impl FaultTestConfig {
    pub fn from_env() -> Result<Self> {
        let context = current_context()?;
        Self::from_env_with(|name| std::env::var(name).ok(), context)
    }

    fn from_env_with<F>(get_env: F, context: String) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        ensure!(
            !context.starts_with("kind-"),
            "fault tests require a real Kubernetes cluster; current context {context:?} is a Kind context"
        );

        let storage_class = required_env(&get_env, "RUSTFS_FAULT_TEST_STORAGE_CLASS")?;
        let namespace = env_or(&get_env, "RUSTFS_FAULT_TEST_NAMESPACE", "rustfs-fault-test");
        let cluster = ClusterTestConfig {
            context,
            operator_namespace: env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE",
                "rustfs-system",
            ),
            test_namespace_prefix: namespace.clone(),
            test_namespace: namespace,
            tenant_name: env_or(&get_env, "RUSTFS_FAULT_TEST_TENANT", "fault-test-tenant"),
            storage_class,
            rustfs_image: env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_SERVER_IMAGE",
                "rustfs/rustfs:latest",
            ),
            artifacts_dir: PathBuf::from(env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_ARTIFACTS",
                "target/fault-tests/artifacts",
            )),
            pod_management_policy: None,
            timeout: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_TIMEOUT_SECONDS",
                300,
            )),
        };

        Ok(Self {
            cluster,
            destructive_enabled: env_bool(&get_env, "RUSTFS_FAULT_TEST_DESTRUCTIVE"),
            scenario: env_or(&get_env, "RUSTFS_FAULT_TEST_SCENARIO", "io-eio"),
            duration: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_DURATION_SECONDS",
                180,
            )),
            percent: env_u8(&get_env, "RUSTFS_FAULT_TEST_PERCENT", 20),
            workload_objects: env_usize(&get_env, "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS", 40),
            request_timeout: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS",
                3,
            )),
            require_client_disruption: env_bool(
                &get_env,
                "RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION",
            ),
            chaos_namespace: env_or(&get_env, "RUSTFS_FAULT_TEST_CHAOS_NAMESPACE", "chaos-mesh"),
        })
    }

    pub fn require_destructive_enabled(&self) -> Result<()> {
        ensure!(
            self.destructive_enabled,
            "destructive fault tests are disabled; run through `make fault-test` or set RUSTFS_FAULT_TEST_DESTRUCTIVE=1 explicitly"
        );
        Ok(())
    }

    pub fn validate_cluster(&self) -> Result<()> {
        Kubectl::new(&self.cluster)
            .command(["get", "crd", "tenants.rustfs.com"])
            .run_checked()
            .context("RustFS Tenant CRD tenants.rustfs.com is required")?;

        let output = Kubectl::new(&self.cluster)
            .command([
                "get",
                "storageclass",
                &self.cluster.storage_class,
                "-o",
                "json",
            ])
            .run_checked()
            .with_context(|| {
                format!(
                    "fault-test StorageClass {:?} is required",
                    self.cluster.storage_class
                )
            })?;
        validate_storage_class(&output.stdout)
    }

    #[cfg(test)]
    pub(crate) fn for_test(context: &str, storage_class: &str) -> Self {
        Self::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some(storage_class.to_string()),
                _ => None,
            },
            context.to_string(),
        )
        .expect("fault test config")
    }
}

fn validate_storage_class(raw: &str) -> Result<()> {
    let value = serde_json::from_str::<Value>(raw).context("parse StorageClass json")?;
    let provisioner = value
        .get("provisioner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    ensure!(
        !provisioner.is_empty(),
        "StorageClass provisioner is missing"
    );
    ensure!(
        provisioner != "kubernetes.io/no-provisioner",
        "fault tests require a dynamically provisioned StorageClass, got {provisioner}"
    );
    Ok(())
}

fn current_context() -> Result<String> {
    let output = CommandSpec::new("kubectl")
        .args(["config", "current-context"])
        .run_checked()?;
    Ok(output.stdout.trim().to_string())
}

fn required_env<F>(get_env: &F, name: &str) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    let value = get_env(name).unwrap_or_default();
    ensure!(!value.trim().is_empty(), "{name} is required");
    Ok(value)
}

fn env_or<F>(get_env: &F, name: &str, default: &str) -> String
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name).unwrap_or_else(|| default.to_string())
}

fn env_bool<F>(get_env: &F, name: &str) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn env_u64<F>(get_env: &F, name: &str, default: u64) -> u64
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize<F>(get_env: &F, name: &str, default: usize) -> usize
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u8<F>(get_env: &F, name: &str, default: u8) -> u8
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::{FaultTestConfig, validate_storage_class};

    #[test]
    fn real_cluster_fault_defaults_are_isolated() {
        let config = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        )
        .expect("fault config");

        assert_eq!(config.cluster.context, "production-test-cluster");
        assert_eq!(config.cluster.test_namespace, "rustfs-fault-test");
        assert_eq!(config.cluster.tenant_name, "fault-test-tenant");
        assert_eq!(config.cluster.storage_class, "fast-csi");
        assert_eq!(
            config.cluster.artifacts_dir,
            std::path::PathBuf::from("target/fault-tests/artifacts")
        );
        assert!(!config.destructive_enabled);
        assert!(config.require_destructive_enabled().is_err());
    }

    #[test]
    fn kind_context_is_rejected_for_fault_tests() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("local-storage".to_string()),
                _ => None,
            },
            "kind-rustfs-e2e".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn dynamic_storage_class_is_required() {
        assert!(validate_storage_class(r#"{"provisioner":"ebs.csi.aws.com"}"#).is_ok());
        assert!(
            validate_storage_class(r#"{"provisioner":"kubernetes.io/no-provisioner"}"#).is_err()
        );
    }
}
