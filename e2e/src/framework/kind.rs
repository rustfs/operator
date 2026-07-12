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
use serde_yaml_ng::Value;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::framework::{
    command::CommandSpec,
    config::{DEFAULT_STORAGE_HOST_DIR_PREFIX, E2eConfig, KIND_WORKER_COUNT},
};

const RUSTFS_FORMAT_MARKER_PATHS: [&str; 2] = [".rustfs.sys/format.json", ".minio.sys/format.json"];
const DOCKER_ROOT_UID: u32 = 0;
const CLEANUP_HELPER_FALLBACK_IMAGES: [&str; 2] = ["rustfs/rustfs:latest", "busybox:latest"];

#[derive(Debug, Clone)]
pub struct KindCluster {
    config: E2eConfig,
}

impl KindCluster {
    pub fn new(config: E2eConfig) -> Self {
        Self { config }
    }

    pub fn create_command(&self) -> Result<CommandSpec> {
        let source = fs::read_to_string(&self.config.kind_config)
            .with_context(|| format!("read Kind config {}", self.config.kind_config.display()))?;
        let rendered = render_kind_config(&source, &self.config.cluster_domain)?;

        Ok(CommandSpec::new("kind")
            .args([
                "create".to_string(),
                "cluster".to_string(),
                "--name".to_string(),
                self.config.cluster_name.clone(),
                "--config=-".to_string(),
            ])
            .stdin(rendered))
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
            ensure_host_storage_dir_is_empty(&dir)?;
        }
        Ok(())
    }

    pub fn cleanup_host_storage_dirs(&self) -> Result<()> {
        for dir in self.host_storage_dirs() {
            ensure_dedicated_host_storage_dir(&dir)?;
            if fs::symlink_metadata(&dir).is_ok() {
                self.remove_host_storage_dir(&dir)?;
                ensure_host_storage_dir_is_absent(&dir)?;
            }
        }
        Ok(())
    }

    pub fn stale_local_rustfs_format_paths(&self) -> Result<Vec<PathBuf>> {
        let mut stale_paths = Vec::new();

        for dir in self.host_storage_dirs() {
            if !dir.exists() {
                continue;
            }

            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }

                for marker_path in RUSTFS_FORMAT_MARKER_PATHS {
                    let format_path = entry.path().join(marker_path);
                    if format_path.exists() {
                        stale_paths.push(format_path);
                    }
                }
            }
        }

        Ok(stale_paths)
    }

    fn remove_host_storage_dir(&self, dir: &Path) -> Result<()> {
        let metadata = ensure_existing_dedicated_host_storage_dir_is_safe(dir)?;
        ensure_host_storage_dir_owner_allows_cleanup(dir, metadata.uid())?;
        println!("removing dedicated e2e storage dir {}", dir.display());

        match fs::remove_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(error) => {
                println!(
                    "direct removal failed for {}: {error}; retrying through Docker helper as root",
                    dir.display()
                );
                self.docker_clean_host_storage(dir)?;
                fs::remove_dir_all(dir).with_context(|| {
                    format!("remove dedicated e2e host storage dir {}", dir.display())
                })
            }
        }
    }

    fn docker_clean_host_storage(&self, dir: &Path) -> Result<()> {
        let uid = current_uid()?;
        let gid = current_gid()?;
        let mut last_error = None;

        for image in self.cleanup_helper_images() {
            if !docker_image_exists(&image) {
                println!("Docker cleanup helper image {image} is not present locally; skipping");
                continue;
            }
            let command = self.docker_clean_host_storage_command(dir, &image, uid, gid)?;
            match command.run_checked() {
                Ok(_) => return Ok(()),
                Err(error) => {
                    println!("Docker cleanup helper image {image} failed: {error}");
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "no local Docker cleanup helper image is available; build rustfs/operator:e2e or ensure rustfs/rustfs:latest is present"
            )
        }))
    }

    fn cleanup_helper_images(&self) -> Vec<String> {
        let mut images = Vec::new();
        for image in std::iter::once(self.config.operator_image.as_str())
            .chain(std::iter::once(self.config.rustfs_image.as_str()))
            .chain(CLEANUP_HELPER_FALLBACK_IMAGES)
        {
            if !images.iter().any(|existing| existing == image) {
                images.push(image.to_string());
            }
        }
        images
    }

    fn docker_clean_host_storage_command(
        &self,
        dir: &Path,
        image: &str,
        uid: u32,
        gid: u32,
    ) -> Result<CommandSpec> {
        ensure_dedicated_host_storage_dir(dir)?;

        Ok(CommandSpec::new("docker").args([
            "run".to_string(),
            "--rm".to_string(),
            "--pull".to_string(),
            "never".to_string(),
            "--user".to_string(),
            "0:0".to_string(),
            "--entrypoint".to_string(),
            "/bin/sh".to_string(),
            "-v".to_string(),
            format!("{}:/e2e-storage", dir.display()),
            image.to_string(),
            "-c".to_string(),
            format!(
                "find /e2e-storage -mindepth 1 -maxdepth 1 -exec rm -rf -- {{}} + && chown {uid}:{gid} /e2e-storage"
            ),
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

    pub fn load_image(&self, image: &str) -> Result<()> {
        for node in self.node_names()? {
            println!("loading {image} into {node} through containerd import");
            self.ctr_import_image_command(image, &node)
                .run_checked()
                .with_context(|| format!("import {image} into Kind node {node}"))?;
        }
        Ok(())
    }

    fn node_names(&self) -> Result<Vec<String>> {
        let output = CommandSpec::new("kind")
            .args([
                "get".to_string(),
                "nodes".to_string(),
                "--name".to_string(),
                self.config.cluster_name.clone(),
            ])
            .run_checked()?;
        let nodes: Vec<String> = output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect();

        if nodes.is_empty() {
            bail!("kind cluster {} has no nodes", self.config.cluster_name);
        }

        Ok(nodes)
    }

    fn ctr_import_image_command(&self, image: &str, node: &str) -> CommandSpec {
        CommandSpec::new("sh").args([
            "-c".to_string(),
            "docker save \"$1\" | docker exec --privileged -i \"$2\" ctr --namespace=k8s.io images import --digests --snapshotter=overlayfs -".to_string(),
            "sh".to_string(),
            image.to_string(),
            node.to_string(),
        ])
    }
}

fn render_kind_config(source: &str, cluster_domain: &str) -> Result<String> {
    let mut config: Value = serde_yaml_ng::from_str(source).context("parse Kind config")?;
    let nodes = config
        .get_mut("nodes")
        .and_then(Value::as_sequence_mut)
        .context("Kind config must contain a nodes list")?;
    let control_plane = nodes
        .iter_mut()
        .find(|node| node.get("role").and_then(Value::as_str) == Some("control-plane"))
        .context("Kind config must contain a control-plane node")?;
    let control_plane = control_plane
        .as_mapping_mut()
        .context("Kind control-plane node must be an object")?;
    let patches = control_plane
        .entry(Value::String("kubeadmConfigPatches".to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .context("Kind control-plane kubeadmConfigPatches must be a list")?;

    patches.push(Value::String(format!(
        "kind: ClusterConfiguration\nnetworking:\n  dnsDomain: \"{cluster_domain}\"\n"
    )));

    serde_yaml_ng::to_string(&config).context("serialize Kind config")
}

fn ensure_host_storage_dir_is_empty(dir: &Path) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("read e2e host storage dir {}", dir.display()))?;

    if let Some(entry) = entries.next() {
        let entry =
            entry.with_context(|| format!("inspect e2e host storage dir {}", dir.display()))?;
        bail!(
            "dedicated e2e storage dir {} is not empty after cleanup; first leftover entry: {}",
            dir.display(),
            entry.path().display()
        );
    }

    Ok(())
}

fn ensure_host_storage_dir_is_absent(dir: &Path) -> Result<()> {
    if fs::symlink_metadata(dir).is_ok() {
        bail!(
            "dedicated e2e storage dir {} still exists after cleanup",
            dir.display()
        );
    }

    Ok(())
}

fn ensure_existing_dedicated_host_storage_dir_is_safe(path: &Path) -> Result<fs::Metadata> {
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

    Ok(metadata)
}

fn ensure_host_storage_dir_owner_allows_cleanup(path: &Path, owner_uid: u32) -> Result<()> {
    let current_uid = current_uid()?;
    if owner_uid != current_uid && owner_uid != DOCKER_ROOT_UID {
        bail!(
            "refusing Docker-root cleanup for e2e host storage dir {} owned by uid {}; current uid is {}; only current-user or root-owned dedicated e2e storage dirs can be cleaned",
            path.display(),
            owner_uid,
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

fn current_gid() -> Result<u32> {
    let output = CommandSpec::new("id").arg("-g").run_checked()?;
    output
        .stdout
        .trim()
        .parse::<u32>()
        .context("parse current gid from `id -g`")
}

fn docker_image_exists(image: &str) -> bool {
    match CommandSpec::new("docker")
        .args(["image", "inspect", image])
        .run()
    {
        Ok(output) => output.code == Some(0),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KindCluster, ensure_dedicated_host_storage_dir, ensure_host_storage_dir_is_absent,
        ensure_host_storage_dir_is_empty, ensure_host_storage_dir_owner_allows_cleanup,
        render_kind_config,
    };
    use crate::framework::config::E2eConfig;

    #[test]
    fn ctr_import_image_command_streams_docker_archive_to_node_containerd() {
        let kind = KindCluster::new(E2eConfig::defaults());
        let command = kind.ctr_import_image_command("rustfs/rustfs:latest", "rustfs-e2e-worker");

        assert_eq!(
            command.display(),
            "sh -c docker save \"$1\" | docker exec --privileged -i \"$2\" ctr --namespace=k8s.io images import --digests --snapshotter=overlayfs - sh rustfs/rustfs:latest rustfs-e2e-worker"
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
    fn rendered_kind_config_uses_custom_cluster_domain() {
        let rendered = render_kind_config(
            include_str!("../../manifests/kind-rustfs-e2e.yaml"),
            "k8s.mse.cloud",
        )
        .expect("Kind config should render");
        let config: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&rendered).expect("rendered Kind config should parse");
        let patches = config["nodes"]
            .as_sequence()
            .and_then(|nodes| {
                nodes.iter().find(|node| {
                    node.get("role").and_then(serde_yaml_ng::Value::as_str) == Some("control-plane")
                })
            })
            .and_then(|node| node.get("kubeadmConfigPatches"))
            .and_then(serde_yaml_ng::Value::as_sequence)
            .expect("control-plane patches should exist");
        let cluster_config = patches
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .filter_map(|patch| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(patch).ok())
            .find(|patch| patch["kind"].as_str() == Some("ClusterConfiguration"))
            .expect("ClusterConfiguration patch should exist");

        assert_eq!(
            cluster_config["networking"]["dnsDomain"].as_str(),
            Some("k8s.mse.cloud")
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
            .docker_clean_host_storage_command(
                std::path::Path::new("/tmp/rustfs-e2e-storage-1"),
                "rustfs/rustfs:latest",
                1000,
                1000,
            )
            .expect("dedicated storage dir should be accepted");

        assert_eq!(
            command.display(),
            "docker run --rm --pull never --user 0:0 --entrypoint /bin/sh -v /tmp/rustfs-e2e-storage-1:/e2e-storage rustfs/rustfs:latest -c find /e2e-storage -mindepth 1 -maxdepth 1 -exec rm -rf -- {} + && chown 1000:1000 /e2e-storage"
        );
        assert!(
            kind.docker_clean_host_storage_command(
                std::path::Path::new("/tmp/other"),
                "rustfs/rustfs:latest",
                1000,
                1000,
            )
            .is_err()
        );
    }

    #[test]
    fn host_storage_owner_guard_accepts_current_user_and_docker_root() {
        let current_uid = super::current_uid().expect("current uid is available");

        ensure_host_storage_dir_owner_allows_cleanup(
            std::path::Path::new("/tmp/rustfs-e2e-storage-1"),
            current_uid,
        )
        .expect("current user-owned storage should be accepted");
        ensure_host_storage_dir_owner_allows_cleanup(
            std::path::Path::new("/tmp/rustfs-e2e-storage-1"),
            0,
        )
        .expect("Docker root-owned storage should be accepted");

        let other_uid = if current_uid == 1 { 2 } else { 1 };
        let error = ensure_host_storage_dir_owner_allows_cleanup(
            std::path::Path::new("/tmp/rustfs-e2e-storage-1"),
            other_uid,
        )
        .expect_err("other user-owned storage should be rejected");
        assert!(error.to_string().contains("owned by uid"));
    }

    #[test]
    fn cleanup_helper_images_prefer_configured_images_and_deduplicate_defaults() {
        let kind = KindCluster::new(E2eConfig::defaults());

        assert_eq!(
            kind.cleanup_helper_images(),
            vec![
                "rustfs/operator:e2e".to_string(),
                "rustfs/rustfs:latest".to_string(),
                "busybox:latest".to_string(),
            ]
        );
    }

    #[test]
    fn host_storage_empty_guard_rejects_leftovers() {
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_host_storage_dir_is_empty(temp.path()).expect("empty dir is accepted");

        std::fs::write(temp.path().join("leftover"), "data").expect("write leftover");

        let error = ensure_host_storage_dir_is_empty(temp.path()).expect_err("leftover rejected");
        assert!(error.to_string().contains("is not empty after cleanup"));
    }

    #[test]
    fn host_storage_absent_guard_rejects_existing_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let error =
            ensure_host_storage_dir_is_absent(temp.path()).expect_err("existing path rejected");

        assert!(error.to_string().contains("still exists after cleanup"));
    }
}
