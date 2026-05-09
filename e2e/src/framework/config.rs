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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eConfig {
    pub cluster_name: String,
    pub context: String,
    pub operator_namespace: String,
    pub test_namespace_prefix: String,
    pub test_namespace: String,
    pub tenant_name: String,
    pub console_base_url: String,
    pub storage_class: String,
    pub storage_host_dir_prefix: PathBuf,
    pub pv_count: usize,
    pub operator_image: String,
    pub console_web_image: String,
    pub rustfs_image: String,
    pub kind_config: PathBuf,
    pub artifacts_dir: PathBuf,
    pub keep_cluster: bool,
    pub skip_build: bool,
    pub live_enabled: bool,
    pub destructive_enabled: bool,
    pub timeout: Duration,
}

impl E2eConfig {
    pub fn from_env() -> Self {
        let cluster_name = env_or("RUSTFS_E2E_CLUSTER", "rustfs-e2e");
        let context = env_or("RUSTFS_E2E_CONTEXT", &format!("kind-{cluster_name}"));
        let test_namespace_prefix = env_or("RUSTFS_E2E_NAMESPACE_PREFIX", "rustfs-e2e");
        let test_namespace = env_or(
            "RUSTFS_E2E_NAMESPACE",
            &format!("{test_namespace_prefix}-smoke"),
        );

        Self {
            cluster_name,
            context,
            operator_namespace: env_or("RUSTFS_E2E_OPERATOR_NAMESPACE", "rustfs-system"),
            test_namespace_prefix,
            test_namespace,
            tenant_name: env_or("RUSTFS_E2E_TENANT", "e2e-tenant"),
            console_base_url: env_or("RUSTFS_E2E_CONSOLE_URL", "http://127.0.0.1:19090"),
            storage_class: env_or("RUSTFS_E2E_STORAGE_CLASS", "local-storage"),
            storage_host_dir_prefix: PathBuf::from(env_or(
                "RUSTFS_E2E_STORAGE_HOST_DIR_PREFIX",
                "/tmp/rustfs-e2e-storage",
            )),
            pv_count: env_usize("RUSTFS_E2E_PV_COUNT", 12),
            operator_image: env_or("RUSTFS_E2E_OPERATOR_IMAGE", "rustfs/operator:e2e"),
            console_web_image: env_or("RUSTFS_E2E_CONSOLE_WEB_IMAGE", "rustfs/console-web:e2e"),
            rustfs_image: env_or("RUSTFS_E2E_SERVER_IMAGE", "rustfs/rustfs:e2e"),
            kind_config: PathBuf::from(env_or(
                "RUSTFS_E2E_KIND_CONFIG",
                "e2e/manifests/kind-rustfs-e2e.yaml",
            )),
            artifacts_dir: PathBuf::from(env_or("RUSTFS_E2E_ARTIFACTS", "target/e2e/artifacts")),
            keep_cluster: env_bool("RUSTFS_E2E_KEEP_CLUSTER"),
            skip_build: env_bool("RUSTFS_E2E_SKIP_BUILD"),
            live_enabled: env_bool("RUSTFS_E2E_LIVE"),
            destructive_enabled: env_bool("RUSTFS_E2E_DESTRUCTIVE"),
            timeout: Duration::from_secs(env_u64("RUSTFS_E2E_TIMEOUT_SECONDS", 300)),
        }
    }

    pub fn is_dedicated_kind_context(&self, actual_context: &str) -> bool {
        actual_context == self.context && actual_context.starts_with("kind-")
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::E2eConfig;

    #[test]
    fn default_config_uses_dedicated_kind_context() {
        let config = E2eConfig::from_env();

        assert_eq!(config.cluster_name, "rustfs-e2e");
        assert_eq!(config.context, "kind-rustfs-e2e");
        assert_eq!(config.test_namespace, "rustfs-e2e-smoke");
        assert_eq!(config.tenant_name, "e2e-tenant");
        assert_eq!(config.storage_class, "local-storage");
        assert_eq!(config.pv_count, 12);
        assert_eq!(
            config.kind_config,
            std::path::PathBuf::from("e2e/manifests/kind-rustfs-e2e.yaml")
        );
        assert!(config.is_dedicated_kind_context("kind-rustfs-e2e"));
        assert!(!config.is_dedicated_kind_context("kind-rustfs-cluster"));
    }
}
