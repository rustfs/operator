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
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::framework::{
    command::CommandSpec,
    config::{DEFAULT_STORAGE_HOST_DIR_PREFIX, E2eConfig, KIND_WORKER_COUNT},
};

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
        (1..=KIND_WORKER_COUNT)
            .map(|index| PathBuf::from(format!("{}-{index}", DEFAULT_STORAGE_HOST_DIR_PREFIX)))
            .collect()
    }

    pub fn reset_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            ensure_dedicated_host_storage_dir(&dir)?;
            if fs::symlink_metadata(&dir).is_ok() {
                self.remove_host_storage_dir(&dir)?;
            }
            fs::create_dir_all(&dir)
                .with_context(|| format!("create e2e host storage dir {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn cleanup_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            ensure_dedicated_host_storage_dir(&dir)?;
            if fs::symlink_metadata(&dir).is_ok() {
                self.remove_host_storage_dir(&dir)?;
            }
        }
        Ok(())
    }

    fn remove_host_storage_dir(&self, dir: &Path) -> Result<()> {
        ensure_existing_dedicated_host_storage_dir_is_safe(dir)?;
        println!("removing dedicated e2e storage dir {}", dir.display());

        match fs::remove_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(error) => {
                println!(
                    "direct removal failed for {}: {error}; retrying through Docker helper as root",
                    dir.display()
                );
                self.docker_clean_host_storage_command(dir)?.run_checked()?;
                fs::remove_dir_all(dir).with_context(|| {
                    format!("remove dedicated e2e host storage dir {}", dir.display())
                })
            }
        }
    }

    fn docker_clean_host_storage_command(&self, dir: &Path) -> Result<CommandSpec> {
        ensure_dedicated_host_storage_dir(dir)?;

        Ok(CommandSpec::new("docker").args([
            "run".to_string(),
            "--rm".to_string(),
            "--pull".to_string(),
            "never".to_string(),
            "--entrypoint".to_string(),
            "/bin/sh".to_string(),
            "-v".to_string(),
            format!("{}:/e2e-storage", dir.display()),
            self.config.operator_image.clone(),
            "-c".to_string(),
            "find /e2e-storage -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +".to_string(),
        ]))
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

fn ensure_existing_dedicated_host_storage_dir_is_safe(path: &Path) -> Result<()> {
    ensure_dedicated_host_storage_dir(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect e2e host storage dir {}", path.display()))?;

    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to remove symlinked e2e host storage dir: {}",
            path.display()
        );
    }

    if !metadata.is_dir() {
        bail!(
            "refusing to remove non-directory e2e host storage path: {}",
            path.display()
        );
    }

    let current_uid = current_uid()?;
    if metadata.uid() != current_uid {
        bail!(
            "refusing Docker-root cleanup for e2e host storage dir {} owned by uid {}; current uid is {}",
            path.display(),
            metadata.uid(),
            current_uid
        );
    }

    Ok(())
}

fn ensure_dedicated_host_storage_dir(path: &Path) -> Result<()> {
    let is_allowed = (1..=KIND_WORKER_COUNT)
        .any(|index| path == Path::new(&format!("{}-{index}", DEFAULT_STORAGE_HOST_DIR_PREFIX)));

    if is_allowed {
        return Ok(());
    }

    bail!(
        "refusing to remove non-dedicated e2e host storage dir: {}",
        path.display()
    )
}

fn current_uid() -> Result<u32> {
    let output = CommandSpec::new("id").arg("-u").run_checked()?;
    output
        .stdout
        .trim()
        .parse::<u32>()
        .context("parse current uid from `id -u`")
}

#[cfg(test)]
mod tests {
    use super::{KindCluster, ensure_dedicated_host_storage_dir};
    use crate::framework::config::E2eConfig;

    #[test]
    fn load_image_command_targets_the_dedicated_cluster() {
        let kind = KindCluster::new(E2eConfig::defaults());
        let command = kind.load_image_command("rustfs/operator:e2e");

        assert_eq!(
            command.display(),
            "kind load docker-image rustfs/operator:e2e --name rustfs-e2e"
        );
    }

    #[test]
    fn host_storage_dirs_use_e2e_prefix() {
        let kind = KindCluster::new(E2eConfig::defaults());

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
            ensure_dedicated_host_storage_dir(std::path::Path::new("/tmp/rustfs-e2e-storage-99"))
                .is_err()
        );
        assert!(
            ensure_dedicated_host_storage_dir(std::path::Path::new(
                "/var/tmp/rustfs-e2e-storage-1"
            ))
            .is_err()
        );
    }

    #[test]
    fn docker_cleanup_command_mounts_only_dedicated_storage_dir() {
        let kind = KindCluster::new(E2eConfig::defaults());
        let command = kind
            .docker_clean_host_storage_command(std::path::Path::new("/tmp/rustfs-e2e-storage-1"))
            .expect("dedicated storage dir should be accepted");

        assert_eq!(
            command.display(),
            "docker run --rm --pull never --entrypoint /bin/sh -v /tmp/rustfs-e2e-storage-1:/e2e-storage rustfs/operator:e2e -c find /e2e-storage -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +"
        );
        assert!(
            kind.docker_clean_host_storage_command(std::path::Path::new("/tmp/other"))
                .is_err()
        );
    }
}
