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
use std::fs;
use std::path::{Path, PathBuf};

use crate::framework::{command::CommandSpec, config::E2eConfig};

#[derive(Debug, Clone)]
pub struct KindCluster {
    config: E2eConfig,
}

impl KindCluster {
    pub fn new(config: E2eConfig) -> Self {
        Self { config }
    }

    pub fn create_command(&self) -> CommandSpec {
        CommandSpec::new("kind").args([
            "create".to_string(),
            "cluster".to_string(),
            "--name".to_string(),
            self.config.cluster_name.clone(),
            "--config".to_string(),
            self.config.kind_config.display().to_string(),
        ])
    }

    pub fn host_storage_dirs(&self) -> Vec<PathBuf> {
        (1..=3)
            .map(|index| {
                PathBuf::from(format!(
                    "{}-{index}",
                    self.config.storage_host_dir_prefix.display()
                ))
            })
            .collect()
    }

    pub fn prepare_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("create e2e host storage dir {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn reset_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            ensure_dedicated_host_storage_dir(&dir)?;
            if dir.exists() {
                println!("removing dedicated e2e storage dir {}", dir.display());
                fs::remove_dir_all(&dir).with_context(|| {
                    format!("remove dedicated e2e host storage dir {}", dir.display())
                })?;
            }
            fs::create_dir_all(&dir)
                .with_context(|| format!("create e2e host storage dir {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn cleanup_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            ensure_dedicated_host_storage_dir(&dir)?;
            if dir.exists() {
                println!("removing dedicated e2e storage dir {}", dir.display());
                fs::remove_dir_all(&dir).with_context(|| {
                    format!("remove dedicated e2e host storage dir {}", dir.display())
                })?;
            }
        }
        Ok(())
    }

    pub fn delete_command(&self) -> CommandSpec {
        CommandSpec::new("kind").args([
            "delete".to_string(),
            "cluster".to_string(),
            "--name".to_string(),
            self.config.cluster_name.clone(),
        ])
    }

    pub fn load_image_command(&self, image: &str) -> CommandSpec {
        CommandSpec::new("kind").args([
            "load".to_string(),
            "docker-image".to_string(),
            image.to_string(),
            "--name".to_string(),
            self.config.cluster_name.clone(),
        ])
    }
}

fn ensure_dedicated_host_storage_dir(path: &Path) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if path.is_absolute()
        && path.starts_with("/tmp")
        && file_name.starts_with("rustfs-e2e-storage-")
    {
        return Ok(());
    }

    bail!(
        "refusing to remove non-dedicated e2e host storage dir: {}",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{KindCluster, ensure_dedicated_host_storage_dir};
    use crate::framework::config::E2eConfig;

    #[test]
    fn load_image_command_targets_the_dedicated_cluster() {
        let kind = KindCluster::new(E2eConfig::from_env());
        let command = kind.load_image_command("rustfs/operator:e2e");

        assert_eq!(
            command.display(),
            "kind load docker-image rustfs/operator:e2e --name rustfs-e2e"
        );
    }

    #[test]
    fn host_storage_dirs_use_e2e_prefix() {
        let kind = KindCluster::new(E2eConfig::from_env());

        assert_eq!(
            kind.host_storage_dirs(),
            vec![
                std::path::PathBuf::from("/tmp/rustfs-e2e-storage-1"),
                std::path::PathBuf::from("/tmp/rustfs-e2e-storage-2"),
                std::path::PathBuf::from("/tmp/rustfs-e2e-storage-3"),
            ]
        );
    }

    #[test]
    fn host_storage_cleanup_only_allows_dedicated_tmp_dirs() {
        assert!(
            ensure_dedicated_host_storage_dir(std::path::Path::new("/tmp/rustfs-e2e-storage-1"))
                .is_ok()
        );
        assert!(ensure_dedicated_host_storage_dir(std::path::Path::new("/tmp/other")).is_err());
        assert!(
            ensure_dedicated_host_storage_dir(std::path::Path::new(
                "/var/tmp/rustfs-e2e-storage-1"
            ))
            .is_err()
        );
    }
}
