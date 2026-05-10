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

use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_CLUSTER_NAME: &str = "rustfs-e2e";
pub const DEFAULT_STORAGE_HOST_DIR_PREFIX: &str = "/tmp/rustfs-e2e-storage";
pub const DEFAULT_RUSTFS_IMAGE: &str = "rustfs/rustfs:latest";
pub const KIND_WORKER_COUNT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eConfig {
    pub cluster_name: String,
    pub context: String,
    pub operator_namespace: String,
    pub test_namespace_prefix: String,
    pub test_namespace: String,
    pub tenant_name: String,
    pub storage_class: String,
    pub pv_count: usize,
    pub operator_image: String,
    pub console_web_image: String,
    pub rustfs_image: String,
    pub kind_config: PathBuf,
    pub artifacts_dir: PathBuf,
    pub live_enabled: bool,
    pub destructive_enabled: bool,
    pub timeout: Duration,
}

impl E2eConfig {
    pub fn defaults() -> Self {
        Self::from_env_with(|_| None)
    }

    pub fn from_env() -> Self {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    pub fn from_env_with<F>(get_env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let cluster_name = DEFAULT_CLUSTER_NAME.to_string();
        let context = format!("kind-{DEFAULT_CLUSTER_NAME}");
        let test_namespace_prefix = env_or(&get_env, "RUSTFS_E2E_NAMESPACE_PREFIX", "rustfs-e2e");
        let test_namespace_default = format!("{test_namespace_prefix}-smoke");
        let test_namespace = env_or(&get_env, "RUSTFS_E2E_NAMESPACE", &test_namespace_default);

        Self {
            cluster_name,
            context,
            operator_namespace: env_or(&get_env, "RUSTFS_E2E_OPERATOR_NAMESPACE", "rustfs-system"),
            test_namespace_prefix,
            test_namespace,
            tenant_name: env_or(&get_env, "RUSTFS_E2E_TENANT", "e2e-tenant"),
            storage_class: env_or(&get_env, "RUSTFS_E2E_STORAGE_CLASS", "local-storage"),
            pv_count: env_usize(&get_env, "RUSTFS_E2E_PV_COUNT", 12),
            operator_image: "rustfs/operator:e2e".to_string(),
            console_web_image: "rustfs/console-web:e2e".to_string(),
            rustfs_image: env_or(&get_env, "RUSTFS_E2E_SERVER_IMAGE", DEFAULT_RUSTFS_IMAGE),
            kind_config: PathBuf::from(env_or(
                &get_env,
                "RUSTFS_E2E_KIND_CONFIG",
                "e2e/manifests/kind-rustfs-e2e.yaml",
            )),
            artifacts_dir: PathBuf::from(env_or(
                &get_env,
                "RUSTFS_E2E_ARTIFACTS",
                "target/e2e/artifacts",
            )),
            live_enabled: env_bool(&get_env, "RUSTFS_E2E_LIVE"),
            destructive_enabled: env_bool(&get_env, "RUSTFS_E2E_DESTRUCTIVE"),
            timeout: Duration::from_secs(env_u64(&get_env, "RUSTFS_E2E_TIMEOUT_SECONDS", 300)),
        }
    }

    pub fn is_dedicated_kind_context(&self, actual_context: &str) -> bool {
        actual_context == self.context
    }
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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_RUSTFS_IMAGE, E2eConfig};

    #[test]
    fn default_config_uses_dedicated_kind_context() {
        let config = E2eConfig::defaults();

        assert_eq!(config.cluster_name, "rustfs-e2e");
        assert_eq!(config.context, "kind-rustfs-e2e");
        assert_eq!(config.test_namespace, "rustfs-e2e-smoke");
        assert_eq!(config.tenant_name, "e2e-tenant");
        assert_eq!(config.storage_class, "local-storage");
        assert_eq!(config.pv_count, 12);
        assert_eq!(config.rustfs_image, DEFAULT_RUSTFS_IMAGE);
        assert_eq!(
            config.kind_config,
            std::path::PathBuf::from("e2e/manifests/kind-rustfs-e2e.yaml")
        );
        assert!(config.is_dedicated_kind_context("kind-rustfs-e2e"));
        assert!(!config.is_dedicated_kind_context("kind-rustfs-cluster"));
    }

    #[test]
    fn env_overrides_do_not_change_dedicated_cluster_or_built_images() {
        let config = E2eConfig::from_env_with(|name| match name {
            "RUSTFS_E2E_CLUSTER" => Some("custom-e2e".to_string()),
            "RUSTFS_E2E_CONTEXT" => Some("kind-custom-e2e".to_string()),
            "RUSTFS_E2E_OPERATOR_IMAGE" => Some("rustfs/operator:other".to_string()),
            "RUSTFS_E2E_CONSOLE_WEB_IMAGE" => Some("rustfs/console-web:other".to_string()),
            "RUSTFS_E2E_SERVER_IMAGE" => Some("rustfs/rustfs:dev".to_string()),
            "RUSTFS_E2E_LIVE" => Some("true".to_string()),
            _ => None,
        });

        assert_eq!(config.cluster_name, "rustfs-e2e");
        assert_eq!(config.context, "kind-rustfs-e2e");
        assert_eq!(config.operator_image, "rustfs/operator:e2e");
        assert_eq!(config.console_web_image, "rustfs/console-web:e2e");
        assert_eq!(config.rustfs_image, "rustfs/rustfs:dev");
        assert!(config.live_enabled);
    }
}
