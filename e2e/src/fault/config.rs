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
use std::path::PathBuf;
use std::time::Duration;

use crate::framework::{command::CommandSpec, config::ClusterTestConfig, kubectl::Kubectl};

pub const DEFAULT_FAULT_NAMESPACE: &str = "rustfs-fault-test";
pub const DEFAULT_FAULT_TENANT: &str = "fault-test-tenant";
pub const DEFAULT_CHAOS_NAMESPACE: &str = "chaos-mesh";
pub const DEFAULT_OPERATOR_NAMESPACE: &str = "rustfs-system";
pub const DEFAULT_WORKLOAD_OBJECTS: usize = 40_000;
pub const DEFAULT_WORKLOAD_CONCURRENCY: usize = 80;
pub const DEFAULT_PREFILL_CONCURRENCY: usize = 16;
pub const DEFAULT_FAULT_DURATION_SECONDS: u64 = 7_200;
pub const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 30;
pub const DEFAULT_CLUSTER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_WARP_DURATION_SECONDS: u64 = 60;
pub const DEFAULT_DM_HELPER_IMAGE: &str = "rancher/mirrored-library-busybox:1.37.0";
pub const MIN_WORKLOAD_OBJECTS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultWorkloadProfile {
    pub object_count: usize,
    pub concurrency: usize,
}

impl FaultWorkloadProfile {
    pub fn new(object_count: usize, concurrency: usize) -> Result<Self> {
        let profile = Self {
            object_count,
            concurrency,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(self) -> Result<()> {
        ensure!(
            self.object_count >= MIN_WORKLOAD_OBJECTS,
            "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS must be at least {MIN_WORKLOAD_OBJECTS}"
        );
        ensure!(
            (1..=self.object_count).contains(&self.concurrency),
            "RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY must be between 1 and RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS ({})",
            self.object_count
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FaultTestConfig {
    pub cluster: ClusterTestConfig,
    pub expected_context: Option<String>,
    pub destructive_enabled: bool,
    pub scenario: String,
    pub duration: Duration,
    pub percent: u8,
    pub percent_overridden: bool,
    pub workload: FaultWorkloadProfile,
    pub prefill_concurrency: usize,
    pub workload_seed: Option<u64>,
    pub request_timeout: Duration,
    pub use_cluster_ip: bool,
    pub require_client_disruption: bool,
    pub dm_name: Option<String>,
    pub dm_node: Option<String>,
    pub dm_mount_path: Option<String>,
    pub dm_fault_table: Option<String>,
    pub dm_recovery_table: Option<String>,
    pub dm_helper_image: String,
    pub warp_duration: Duration,
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
        let expected_context = env_optional(&get_env, "RUSTFS_FAULT_TEST_EXPECTED_CONTEXT");
        if let Some(expected) = expected_context.as_deref() {
            ensure!(
                context == expected,
                "current context {context:?} does not match RUSTFS_FAULT_TEST_EXPECTED_CONTEXT {expected:?}"
            );
        }
        ensure!(
            !context.starts_with("kind-"),
            "fault tests require a real Kubernetes or K3s cluster; current context {context:?} is a Kind context"
        );

        let storage_class = required_env(&get_env, "RUSTFS_FAULT_TEST_STORAGE_CLASS")?;
        let rustfs_image = required_env(&get_env, "RUSTFS_FAULT_TEST_SERVER_IMAGE")?;
        let namespace = env_or(
            &get_env,
            "RUSTFS_FAULT_TEST_NAMESPACE",
            DEFAULT_FAULT_NAMESPACE,
        );
        let scenario = env_or(&get_env, "RUSTFS_FAULT_TEST_SCENARIO", "io-eio");
        let default_percent = if scenario == "disk-full" { 100 } else { 20 };
        let workload = FaultWorkloadProfile::new(
            env_usize(
                &get_env,
                "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS",
                DEFAULT_WORKLOAD_OBJECTS,
            )?,
            env_usize(
                &get_env,
                "RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY",
                DEFAULT_WORKLOAD_CONCURRENCY,
            )?,
        )?;
        let prefill_concurrency = env_usize(
            &get_env,
            "RUSTFS_FAULT_TEST_PREFILL_CONCURRENCY",
            workload.concurrency.min(DEFAULT_PREFILL_CONCURRENCY),
        )?;
        ensure!(
            (1..=workload.object_count).contains(&prefill_concurrency),
            "RUSTFS_FAULT_TEST_PREFILL_CONCURRENCY must be between 1 and RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS ({})",
            workload.object_count
        );
        let cluster = ClusterTestConfig {
            context,
            operator_namespace: env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE",
                DEFAULT_OPERATOR_NAMESPACE,
            ),
            test_namespace_prefix: namespace.clone(),
            test_namespace: namespace,
            tenant_name: env_or(&get_env, "RUSTFS_FAULT_TEST_TENANT", DEFAULT_FAULT_TENANT),
            storage_class,
            rustfs_image,
            artifacts_dir: PathBuf::from(env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_ARTIFACTS",
                "target/fault-tests/artifacts",
            )),
            pod_management_policy: None,
            timeout: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_TIMEOUT_SECONDS",
                DEFAULT_CLUSTER_TIMEOUT_SECONDS,
            )?),
        };

        Ok(Self {
            cluster,
            expected_context,
            destructive_enabled: env_bool(&get_env, "RUSTFS_FAULT_TEST_DESTRUCTIVE")?,
            scenario,
            duration: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_DURATION_SECONDS",
                DEFAULT_FAULT_DURATION_SECONDS,
            )?),
            percent: env_u8(&get_env, "RUSTFS_FAULT_TEST_PERCENT", default_percent)?,
            percent_overridden: env_optional(&get_env, "RUSTFS_FAULT_TEST_PERCENT").is_some(),
            workload,
            prefill_concurrency,
            workload_seed: env_optional_u64(&get_env, "RUSTFS_FAULT_TEST_SEED")?,
            request_timeout: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS",
                DEFAULT_REQUEST_TIMEOUT_SECONDS,
            )?),
            use_cluster_ip: env_bool(&get_env, "RUSTFS_FAULT_TEST_USE_CLUSTER_IP")?,
            require_client_disruption: env_bool(
                &get_env,
                "RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION",
            )?,
            dm_name: env_optional(&get_env, "RUSTFS_FAULT_TEST_DM_NAME"),
            dm_node: env_optional(&get_env, "RUSTFS_FAULT_TEST_DM_NODE"),
            dm_mount_path: env_optional(&get_env, "RUSTFS_FAULT_TEST_DM_MOUNT_PATH"),
            dm_fault_table: env_optional(&get_env, "RUSTFS_FAULT_TEST_DM_FAULT_TABLE"),
            dm_recovery_table: env_optional(&get_env, "RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE"),
            dm_helper_image: env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_DM_HELPER_IMAGE",
                DEFAULT_DM_HELPER_IMAGE,
            ),
            warp_duration: Duration::from_secs(env_u64(
                &get_env,
                "RUSTFS_FAULT_TEST_WARP_DURATION_SECONDS",
                DEFAULT_WARP_DURATION_SECONDS,
            )?),
            chaos_namespace: env_or(
                &get_env,
                "RUSTFS_FAULT_TEST_CHAOS_NAMESPACE",
                DEFAULT_CHAOS_NAMESPACE,
            ),
        })
    }

    pub fn require_destructive_enabled(&self) -> Result<()> {
        ensure!(
            self.destructive_enabled,
            "destructive fault tests are disabled; run through an e2e package fault Make target or set RUSTFS_FAULT_TEST_DESTRUCTIVE=1 explicitly"
        );
        Ok(())
    }

    pub fn validate_cluster(&self, allow_static_storage: bool) -> Result<()> {
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
        validate_storage_class(&output.stdout, allow_static_storage)
    }

    #[cfg(test)]
    pub(crate) fn for_test(context: &str, storage_class: &str) -> Self {
        Self::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some(storage_class.to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                _ => None,
            },
            context.to_string(),
        )
        .expect("fault test config")
    }
}

fn validate_storage_class(raw: &str, allow_static: bool) -> Result<()> {
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
        allow_static || provisioner != "kubernetes.io/no-provisioner",
        "fault tests require a dynamically provisioned StorageClass unless the selected scenario explicitly requires dedicated static local PVs, got {provisioner}"
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

fn env_optional<F>(get_env: &F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get_env(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool<F>(get_env: &F, name: &str) -> Result<bool>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = env_optional(get_env, name) else {
        return Ok(false);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => bail!("{name} must be a boolean: 1/0, true/false, or yes/no"),
    }
}

fn env_u64<F>(get_env: &F, name: &str, default: u64) -> Result<u64>
where
    F: Fn(&str) -> Option<String>,
{
    env_optional(get_env, name)
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("{name} must be an unsigned 64-bit integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_optional_u64<F>(get_env: &F, name: &str) -> Result<Option<u64>>
where
    F: Fn(&str) -> Option<String>,
{
    env_optional(get_env, name)
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("{name} must be an unsigned 64-bit integer"))
        })
        .transpose()
}

fn env_usize<F>(get_env: &F, name: &str, default: usize) -> Result<usize>
where
    F: Fn(&str) -> Option<String>,
{
    env_optional(get_env, name)
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be an unsigned integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u8<F>(get_env: &F, name: &str, default: u8) -> Result<u8>
where
    F: Fn(&str) -> Option<String>,
{
    env_optional(get_env, name)
        .map(|value| {
            value
                .parse::<u8>()
                .with_context(|| format!("{name} must be an unsigned 8-bit integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(test)]
mod tests {
    use super::{FaultTestConfig, FaultWorkloadProfile, validate_storage_class};

    #[test]
    fn real_cluster_fault_defaults_are_isolated() {
        let config = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        )
        .expect("fault config");

        assert_eq!(config.cluster.context, "production-test-cluster");
        assert_eq!(config.expected_context, None);
        assert_eq!(config.cluster.test_namespace, "rustfs-fault-test");
        assert_eq!(config.cluster.tenant_name, "fault-test-tenant");
        assert_eq!(config.cluster.storage_class, "fast-csi");
        assert_eq!(config.cluster.rustfs_image, "rustfs/rustfs:test");
        assert_eq!(
            config.cluster.artifacts_dir,
            std::path::PathBuf::from("target/fault-tests/artifacts")
        );
        assert_eq!(config.scenario, "io-eio");
        assert_eq!(config.duration, std::time::Duration::from_secs(7200));
        assert_eq!(config.percent, 20);
        assert!(!config.percent_overridden);
        assert_eq!(config.workload.object_count, 40000);
        assert_eq!(config.workload.concurrency, 80);
        assert_eq!(config.prefill_concurrency, 16);
        assert_eq!(config.workload_seed, None);
        assert_eq!(config.request_timeout, std::time::Duration::from_secs(30));
        assert!(!config.use_cluster_ip);
        assert!(config.dm_name.is_none());
        assert!(config.dm_node.is_none());
        assert!(config.dm_mount_path.is_none());
        assert!(config.dm_fault_table.is_none());
        assert!(config.dm_recovery_table.is_none());
        assert_eq!(
            config.dm_helper_image,
            "rancher/mirrored-library-busybox:1.37.0"
        );
        assert_eq!(config.warp_duration, std::time::Duration::from_secs(60));
        assert!(!config.destructive_enabled);
        assert!(config.require_destructive_enabled().is_err());
    }

    #[test]
    fn fault_scenario_env_overrides_are_parsed() {
        let config = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" => Some("production-test-cluster".to_string()),
                "RUSTFS_FAULT_TEST_SCENARIO" => Some("dm-flakey".to_string()),
                "RUSTFS_FAULT_TEST_DURATION_SECONDS" => Some("45".to_string()),
                "RUSTFS_FAULT_TEST_PERCENT" => Some("35".to_string()),
                "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS" => Some("64".to_string()),
                "RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY" => Some("8".to_string()),
                "RUSTFS_FAULT_TEST_PREFILL_CONCURRENCY" => Some("4".to_string()),
                "RUSTFS_FAULT_TEST_SEED" => Some("4242".to_string()),
                "RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS" => Some("7".to_string()),
                "RUSTFS_FAULT_TEST_USE_CLUSTER_IP" => Some("true".to_string()),
                "RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION" => Some("true".to_string()),
                "RUSTFS_FAULT_TEST_DM_NAME" => Some("rustfs-test".to_string()),
                "RUSTFS_FAULT_TEST_DM_NODE" => Some("worker-a".to_string()),
                "RUSTFS_FAULT_TEST_DM_MOUNT_PATH" => {
                    Some("/data/rustfs-fault/dm-volume".to_string())
                }
                "RUSTFS_FAULT_TEST_DM_FAULT_TABLE" => Some("0 1024 error".to_string()),
                "RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE" => {
                    Some("0 1024 linear /dev/loop0 0".to_string())
                }
                "RUSTFS_FAULT_TEST_WARP_DURATION_SECONDS" => Some("30".to_string()),
                "RUSTFS_FAULT_TEST_DM_HELPER_IMAGE" => Some("busybox:test".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        )
        .expect("fault config");

        assert_eq!(
            config.expected_context.as_deref(),
            Some("production-test-cluster")
        );
        assert_eq!(config.scenario, "dm-flakey");
        assert_eq!(config.duration, std::time::Duration::from_secs(45));
        assert_eq!(config.percent, 35);
        assert!(config.percent_overridden);
        assert_eq!(config.workload.object_count, 64);
        assert_eq!(config.workload.concurrency, 8);
        assert_eq!(config.prefill_concurrency, 4);
        assert_eq!(config.workload_seed, Some(4242));
        assert_eq!(config.request_timeout, std::time::Duration::from_secs(7));
        assert!(config.use_cluster_ip);
        assert!(config.require_client_disruption);
        assert_eq!(config.dm_name.as_deref(), Some("rustfs-test"));
        assert_eq!(config.dm_node.as_deref(), Some("worker-a"));
        assert_eq!(
            config.dm_mount_path.as_deref(),
            Some("/data/rustfs-fault/dm-volume")
        );
        assert_eq!(config.dm_fault_table.as_deref(), Some("0 1024 error"));
        assert_eq!(
            config.dm_recovery_table.as_deref(),
            Some("0 1024 linear /dev/loop0 0")
        );
        assert_eq!(config.warp_duration, std::time::Duration::from_secs(30));
        assert_eq!(config.dm_helper_image, "busybox:test");
    }

    #[test]
    fn workload_object_count_must_cover_all_mixed_operations() {
        assert!(FaultWorkloadProfile::new(11, 1).is_err());
        assert!(FaultWorkloadProfile::new(12, 12).is_ok());
    }

    #[test]
    fn kind_context_is_rejected_for_fault_tests() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("local-storage".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                _ => None,
            },
            "kind-rustfs-e2e".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_workload_seed_is_rejected() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_SEED" => Some("not-a-number".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn expected_context_is_optional_but_checked_when_set() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" => Some("other-cluster".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn explicit_server_image_is_required() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_workload_numbers_are_rejected() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS" => Some("not-a-number".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn invalid_prefill_concurrency_is_rejected() {
        let result = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS" => Some("64".to_string()),
                "RUSTFS_FAULT_TEST_PREFILL_CONCURRENCY" => Some("0".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        );

        assert!(result.is_err());
    }

    #[test]
    fn dynamic_storage_class_is_required() {
        assert!(validate_storage_class(r#"{"provisioner":"ebs.csi.aws.com"}"#, false).is_ok());
        assert!(
            validate_storage_class(r#"{"provisioner":"kubernetes.io/no-provisioner"}"#, false)
                .is_err()
        );
        assert!(
            validate_storage_class(r#"{"provisioner":"kubernetes.io/no-provisioner"}"#, true)
                .is_ok()
        );
    }

    #[test]
    fn disk_full_defaults_to_full_enospc_injection() {
        let config = FaultTestConfig::from_env_with(
            |name| match name {
                "RUSTFS_FAULT_TEST_STORAGE_CLASS" => Some("fast-csi".to_string()),
                "RUSTFS_FAULT_TEST_SERVER_IMAGE" => Some("rustfs/rustfs:test".to_string()),
                "RUSTFS_FAULT_TEST_SCENARIO" => Some("disk-full".to_string()),
                _ => None,
            },
            "production-test-cluster".to_string(),
        )
        .expect("fault config");

        assert_eq!(config.percent, 100);
    }
}
