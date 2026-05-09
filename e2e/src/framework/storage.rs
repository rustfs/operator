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

use anyhow::Result;

use crate::framework::{command::CommandSpec, config::E2eConfig, kubectl::Kubectl};

pub const RUSTFS_RUN_AS_UID: u32 = 10001;

pub fn worker_node_names(config: &E2eConfig) -> Vec<String> {
    vec![
        format!("{}-worker", config.cluster_name),
        format!("{}-worker2", config.cluster_name),
        format!("{}-worker3", config.cluster_name),
    ]
}

pub fn volume_path(index: usize) -> String {
    format!("/mnt/data/vol{index}")
}

pub fn volume_directory_commands(config: &E2eConfig) -> Vec<CommandSpec> {
    let mut commands = Vec::new();
    for node in worker_node_names(config) {
        for index in 1..=config.pv_count {
            let path = volume_path(index);
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
    let mut manifest = format!(
        r#"---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: {storage_class}
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
"#,
        storage_class = config.storage_class
    );

    for index in 1..=config.pv_count {
        let worker_group = ((index - 1) % 3) + 1;
        manifest.push_str(&format!(
            r#"---
apiVersion: v1
kind: PersistentVolume
metadata:
  name: {cluster}-pv-{index}
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
            cluster = config.cluster_name,
            index = index,
            storage_class = config.storage_class,
            path = volume_path(index),
            worker_group = worker_group
        ));
    }

    manifest
}

pub fn prepare_local_storage(config: &E2eConfig) -> Result<()> {
    for command in volume_directory_commands(config) {
        command.run_checked()?;
    }

    Kubectl::new(config)
        .apply_yaml_command(local_storage_manifest(config))
        .run_checked()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{local_storage_manifest, volume_directory_commands, worker_node_names};
    use crate::framework::config::E2eConfig;

    #[test]
    fn local_storage_manifest_uses_configured_class_and_pv_count() {
        let config = E2eConfig::from_env();
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
        let config = E2eConfig::from_env();

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
}
