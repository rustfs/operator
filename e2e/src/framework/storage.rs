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
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::framework::{
    command::CommandSpec,
    config::{E2eConfig, KIND_WORKER_COUNT},
    kubectl::Kubectl,
};

pub const RUSTFS_RUN_AS_UID: u32 = 10001;
const STORAGE_RESET_TIMEOUT: Duration = Duration::from_secs(120);
const STORAGE_RESET_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStorageLayout {
    pub storage_class: String,
    pub pv_name_prefix: String,
    pub volume_path_prefix: String,
    pub pv_count: usize,
}

impl LocalStorageLayout {
    pub fn from_config(config: &E2eConfig) -> Self {
        Self {
            storage_class: config.storage_class.clone(),
            pv_name_prefix: format!("{}-pv", config.cluster_name),
            volume_path_prefix: "/mnt/data".to_string(),
            pv_count: config.pv_count,
        }
    }

    pub fn new(
        storage_class: impl Into<String>,
        pv_name_prefix: impl Into<String>,
        volume_path_prefix: impl Into<String>,
        pv_count: usize,
    ) -> Self {
        Self {
            storage_class: storage_class.into(),
            pv_name_prefix: pv_name_prefix.into(),
            volume_path_prefix: volume_path_prefix.into(),
            pv_count,
        }
    }

    fn pv_name(&self, index: usize) -> String {
        format!("{}-{index}", self.pv_name_prefix)
    }

    fn pv_names(&self) -> Vec<String> {
        (1..=self.pv_count)
            .map(|index| self.pv_name(index))
            .collect()
    }

    fn volume_path(&self, index: usize) -> String {
        format!(
            "{}/vol{index}",
            self.volume_path_prefix.trim_end_matches('/')
        )
    }
}

pub fn worker_node_names(config: &E2eConfig) -> Vec<String> {
    (1..=KIND_WORKER_COUNT)
        .map(|index| match index {
            1 => format!("{}-worker", config.cluster_name),
            _ => format!("{}-worker{index}", config.cluster_name),
        })
        .collect()
}

pub fn volume_path(index: usize) -> String {
    format!("/mnt/data/vol{index}")
}

pub fn volume_directory_commands(config: &E2eConfig) -> Vec<CommandSpec> {
    volume_directory_commands_for_layout(config, &LocalStorageLayout::from_config(config))
}

pub fn storage_mount_validation_commands(config: &E2eConfig) -> Vec<CommandSpec> {
    worker_node_names(config)
        .into_iter()
        .map(|node| {
            CommandSpec::new("docker").args([
                "exec".to_string(),
                node,
                "stat".to_string(),
                "-c".to_string(),
                "%h".to_string(),
                "/mnt/data".to_string(),
            ])
        })
        .collect()
}

pub fn volume_directory_commands_for_layout(
    config: &E2eConfig,
    layout: &LocalStorageLayout,
) -> Vec<CommandSpec> {
    let mut commands = Vec::new();
    for node in worker_node_names(config) {
        for index in 1..=layout.pv_count {
            let path = layout.volume_path(index);
            commands.push(CommandSpec::new("docker").args([
                "exec".to_string(),
                node.clone(),
                "mkdir".to_string(),
                "-p".to_string(),
                path.clone(),
            ]));
            commands.push(CommandSpec::new("docker").args([
                "exec".to_string(),
                node.clone(),
                "chown".to_string(),
                "-R".to_string(),
                format!("{uid}:{uid}", uid = RUSTFS_RUN_AS_UID),
                path,
            ]));
        }
    }
    commands
}

pub fn local_storage_manifest(config: &E2eConfig) -> String {
    local_storage_manifest_for_layout(config, &LocalStorageLayout::from_config(config))
}

pub fn local_storage_manifest_for_layout(
    _config: &E2eConfig,
    layout: &LocalStorageLayout,
) -> String {
    let mut manifest = format!(
        r#"---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: {storage_class}
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
"#,
        storage_class = layout.storage_class
    );

    for index in 1..=layout.pv_count {
        let worker_group = ((index - 1) % KIND_WORKER_COUNT) + 1;
        manifest.push_str(&format!(
            r#"---
apiVersion: v1
kind: PersistentVolume
metadata:
  name: {pv_name}
spec:
  capacity:
    storage: 10Gi
  volumeMode: Filesystem
  accessModes:
    - ReadWriteOnce
  persistentVolumeReclaimPolicy: Retain
  storageClassName: {storage_class}
  local:
    path: {path}
  nodeAffinity:
    required:
      nodeSelectorTerms:
        - matchExpressions:
            - key: worker-group
              operator: In
              values:
                - storage-{worker_group}
"#,
            pv_name = layout.pv_name(index),
            storage_class = layout.storage_class,
            path = layout.volume_path(index),
            worker_group = worker_group
        ));
    }

    manifest
}

pub fn prepare_local_storage(config: &E2eConfig) -> Result<()> {
    prepare_local_storage_with_layout(config, &LocalStorageLayout::from_config(config))
}

pub fn prepare_local_storage_with_layout(
    config: &E2eConfig,
    layout: &LocalStorageLayout,
) -> Result<()> {
    validate_storage_mounts(config)?;

    for command in volume_directory_commands_for_layout(config, layout) {
        command.run_checked()?;
    }

    Kubectl::new(config)
        .apply_yaml_command(local_storage_manifest_for_layout(config, layout))
        .run_checked()?;
    Ok(())
}

pub fn reset_default_local_storage(config: &E2eConfig) -> Result<()> {
    reset_local_storage_for_layout(config, &LocalStorageLayout::from_config(config))
}

pub fn reset_local_storage_for_layout(
    config: &E2eConfig,
    layout: &LocalStorageLayout,
) -> Result<()> {
    validate_storage_mounts(config)?;
    validate_reset_layout(layout)?;
    delete_local_pvs(config, layout)?;
    wait_for_local_pvs_deleted(config, layout, STORAGE_RESET_TIMEOUT)?;

    for command in clean_volume_directory_commands_for_layout(config, layout)? {
        command.run_checked()?;
    }

    prepare_local_storage_with_layout(config, layout)
}

fn delete_local_pvs(config: &E2eConfig, layout: &LocalStorageLayout) -> Result<()> {
    let pv_names = layout.pv_names();
    if pv_names.is_empty() {
        return Ok(());
    }

    println!("deleting dedicated e2e PVs: {}", pv_names.join(", "));

    let mut args = vec!["delete".to_string(), "pv".to_string()];
    args.extend(pv_names);
    args.push("--ignore-not-found".to_string());

    Kubectl::new(config).command(args).run_checked()?;
    Ok(())
}

fn wait_for_local_pvs_deleted(
    config: &E2eConfig,
    layout: &LocalStorageLayout,
    timeout: Duration,
) -> Result<()> {
    let pv_names = layout.pv_names();
    wait_until(
        &format!("PVs {} to be deleted", pv_names.join(", ")),
        timeout,
        || {
            let output = Kubectl::new(config)
                .command(["get", "pv", "-o", "name"])
                .run_checked()?;
            let existing = output
                .stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.strip_prefix("persistentvolume/").unwrap_or(line))
                .collect::<std::collections::BTreeSet<_>>();

            Ok(!pv_names.iter().any(|name| existing.contains(name.as_str())))
        },
    )
}

fn clean_volume_directory_commands_for_layout(
    config: &E2eConfig,
    layout: &LocalStorageLayout,
) -> Result<Vec<CommandSpec>> {
    validate_reset_layout(layout)?;

    let mut commands = Vec::new();
    for node in worker_node_names(config) {
        for index in 1..=layout.pv_count {
            commands.push(clean_volume_directory_command(
                &node,
                &layout.volume_path(index),
            ));
        }
    }

    Ok(commands)
}

fn clean_volume_directory_command(node: &str, path: &str) -> CommandSpec {
    CommandSpec::new("docker").args([
        "exec".to_string(),
        node.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        "mkdir -p \"$1\" && find \"$1\" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} + && chown -R \"$2:$2\" \"$1\"".to_string(),
        "sh".to_string(),
        path.to_string(),
        RUSTFS_RUN_AS_UID.to_string(),
    ])
}

fn validate_reset_layout(layout: &LocalStorageLayout) -> Result<()> {
    let prefix = layout.volume_path_prefix.trim_end_matches('/');
    ensure!(
        prefix == "/mnt/data" || prefix.starts_with("/mnt/data/"),
        "refusing to reset e2e storage outside /mnt/data: {}",
        layout.volume_path_prefix
    );
    ensure!(
        !prefix.split('/').any(|part| part == "." || part == ".."),
        "refusing to reset unsafe e2e storage path: {}",
        layout.volume_path_prefix
    );

    for index in 1..=layout.pv_count {
        let path = layout.volume_path(index);
        ensure!(
            path.starts_with("/mnt/data/"),
            "refusing to reset unsafe e2e volume path: {path}"
        );
    }

    Ok(())
}

fn validate_storage_mounts(config: &E2eConfig) -> Result<()> {
    for (node, command) in worker_node_names(config)
        .into_iter()
        .zip(storage_mount_validation_commands(config))
    {
        let output = command.run_checked()?;
        let link_count = parse_link_count(&output.stdout)?;
        if link_count == 0 {
            anyhow::bail!(
                "Kind worker {node} has a stale /mnt/data bind mount; the host storage directory was recreated while the cluster was running. Recreate the dedicated e2e cluster with `make e2e-live-create`."
            );
        }
    }

    Ok(())
}

fn parse_link_count(output: &str) -> Result<u64> {
    Ok(output.trim().parse()?)
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
            anyhow::bail!("timed out waiting for {description} after {timeout:?}");
        }

        sleep(STORAGE_RESET_POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LocalStorageLayout, clean_volume_directory_commands_for_layout, local_storage_manifest,
        parse_link_count, storage_mount_validation_commands, validate_reset_layout,
        volume_directory_commands, worker_node_names,
    };
    use crate::framework::config::E2eConfig;

    #[test]
    fn local_storage_manifest_uses_configured_class_and_pv_count() {
        let config = E2eConfig::defaults();
        let manifest = local_storage_manifest(&config);

        assert!(manifest.contains("name: local-storage"));
        assert_eq!(
            manifest.matches("kind: PersistentVolume").count(),
            config.pv_count
        );
        assert!(manifest.contains("storage-1"));
        assert!(manifest.contains("storage-2"));
        assert!(manifest.contains("storage-3"));
    }

    #[test]
    fn storage_volume_commands_target_dedicated_workers() {
        let config = E2eConfig::defaults();

        assert_eq!(
            worker_node_names(&config),
            vec![
                "rustfs-e2e-worker".to_string(),
                "rustfs-e2e-worker2".to_string(),
                "rustfs-e2e-worker3".to_string(),
            ]
        );
        assert!(
            volume_directory_commands(&config)
                .first()
                .expect("at least one command")
                .display()
                .contains("docker exec rustfs-e2e-worker mkdir -p /mnt/data/vol1")
        );
    }

    #[test]
    fn storage_mount_validation_checks_worker_mount_link_count() {
        let config = E2eConfig::defaults();

        assert!(
            storage_mount_validation_commands(&config)
                .first()
                .expect("at least one command")
                .display()
                .contains("docker exec rustfs-e2e-worker stat -c %h /mnt/data")
        );
    }

    #[test]
    fn parse_link_count_trims_stat_output() {
        assert_eq!(parse_link_count("2\n").expect("valid link count"), 2);
    }

    #[test]
    fn reset_layout_rejects_paths_outside_kind_storage_mount() {
        let layout = LocalStorageLayout::new("local-storage", "pv", "/var/lib/data", 1);

        assert!(validate_reset_layout(&layout).is_err());
    }

    #[test]
    fn clean_volume_commands_clear_only_layout_volume_directories() {
        let config = E2eConfig::defaults();
        let layout = LocalStorageLayout::new("local-storage", "pv", "/mnt/data/reset-case", 1);

        let commands = clean_volume_directory_commands_for_layout(&config, &layout)
            .expect("commands should render");

        assert_eq!(commands.len(), 3);
        assert!(
            commands[0]
                .display()
                .contains("docker exec rustfs-e2e-worker sh -c")
        );
        assert!(commands[0].display().contains("/mnt/data/reset-case/vol1"));
    }
}
