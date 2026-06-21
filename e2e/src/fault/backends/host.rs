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
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

use crate::framework::{
    artifacts::ArtifactCollector, command::CommandOutput, command::CommandSpec,
    config::ClusterTestConfig, kubectl::Kubectl,
};

const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const MANAGED_BY_VALUE: &str = "rustfs-operator-fault-test";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DmVolumeMapping {
    pub node: String,
    pub pod: String,
    pub pvc: String,
    pub pv: String,
    pub mount_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DmStatusSnapshot {
    pub stage: String,
    pub helper_pod: String,
    pub mapping: DmVolumeMapping,
    pub table: String,
    pub status: String,
}

#[derive(Debug)]
pub struct DmFlakeyGuard {
    config: ClusterTestConfig,
    helper_pod: String,
    dm_name: String,
    fault_table: String,
    recovery_table: String,
    mapping: DmVolumeMapping,
    recovery_snapshot: Option<DmStatusSnapshot>,
    restored: bool,
}

#[derive(Debug)]
pub struct DmFlakeySpec<'a> {
    pub node: &'a str,
    pub mount_path: &'a str,
    pub helper_image: &'a str,
    pub name: &'a str,
    pub fault_table: &'a str,
    pub recovery_table: Option<&'a str>,
    pub run_id: &'a str,
}

pub fn apply_dm_flakey(
    config: &ClusterTestConfig,
    spec: &DmFlakeySpec<'_>,
    collector: &ArtifactCollector,
    case_name: &str,
) -> Result<DmFlakeyGuard> {
    validate_dm_spec(spec)?;
    let mapping = verify_dm_volume_mapping(config, spec.node, spec.mount_path)?;
    let helper_pod = helper_pod_name(spec.run_id);
    let manifest = dm_helper_manifest(config, &helper_pod, spec.node, spec.helper_image);
    collector.write_text(case_name, "dm-helper-manifest.yaml", &manifest)?;

    let kubectl = Kubectl::new(config).namespaced(&config.test_namespace);
    kubectl
        .command([
            "delete",
            "pod",
            &helper_pod,
            "--ignore-not-found",
            "--wait=true",
        ])
        .run_checked()?;
    kubectl.create_yaml_command(manifest).run_checked()?;

    let mut guard = DmFlakeyGuard {
        config: config.clone(),
        helper_pod,
        dm_name: spec.name.to_string(),
        fault_table: spec.fault_table.to_string(),
        recovery_table: String::new(),
        mapping,
        recovery_snapshot: None,
        restored: false,
    };
    guard.wait_helper_ready()?;
    guard.verify_mount_source()?;

    let original_table = guard.dmsetup(["table", spec.name])?.stdout;
    guard.recovery_table = spec
        .recovery_table
        .map(str::to_string)
        .unwrap_or_else(|| original_table.trim().to_string());
    ensure!(
        !guard.recovery_table.trim().is_empty(),
        "dmsetup returned an empty recovery table for {:?}",
        spec.name
    );

    guard.load_table(spec.fault_table, false)?;
    let active = guard.snapshot("active")?;
    ensure!(
        normalize_dm_table(&active.table) == normalize_dm_table(spec.fault_table),
        "device-mapper target did not switch to the requested fault table; requested {:?}, active {:?}",
        spec.fault_table,
        active.table
    );
    collector.write_text(
        case_name,
        "dm-flakey-active.json",
        &serde_json::to_string_pretty(&active)?,
    )?;

    Ok(guard)
}

pub fn run_warp_mixed(
    duration: Duration,
    collector: &ArtifactCollector,
    case_name: &str,
    endpoint: &str,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<()> {
    let host = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    let duration = format!("{}s", duration.as_secs());
    let command = CommandSpec::new("warp").args([
        "mixed".to_string(),
        format!("--host={host}"),
        format!("--access-key={access_key}"),
        format!("--secret-key={secret_key}"),
        format!("--bucket={bucket}"),
        format!("--duration={duration}"),
        "--obj.size=4KiB".to_string(),
        "--tls=false".to_string(),
        "--autoterm".to_string(),
    ]);
    let output = command.run()?;
    let display = command.display().replace(
        &format!("--secret-key={secret_key}"),
        "--secret-key=<redacted>",
    );
    collector.write_text(
        case_name,
        "warp-mixed.txt",
        &format!(
            "$ {}\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
            display, output.code, output.stdout, output.stderr
        ),
    )?;
    ensure!(
        output.code == Some(0),
        "warp mixed command failed with exit {:?}",
        output.code
    );
    Ok(())
}

impl DmFlakeyGuard {
    pub fn ensure_active(&self, stage: &str) -> Result<DmStatusSnapshot> {
        let snapshot = self.snapshot(stage)?;
        ensure!(
            normalize_dm_table(&snapshot.table) == normalize_dm_table(&self.fault_table),
            "device-mapper target {:?} is no longer using the requested fault table at {stage}; expected {:?}, active {:?}",
            self.dm_name,
            self.fault_table,
            snapshot.table
        );
        Ok(snapshot)
    }

    pub fn snapshot(&self, stage: &str) -> Result<DmStatusSnapshot> {
        Ok(DmStatusSnapshot {
            stage: stage.to_string(),
            helper_pod: self.helper_pod.clone(),
            mapping: self.mapping.clone(),
            table: self.dmsetup(["table", self.dm_name.as_str()])?.stdout,
            status: self.dmsetup(["status", self.dm_name.as_str()])?.stdout,
        })
    }

    pub fn restore(&mut self) -> Result<()> {
        let recovery_table = self.recovery_table.clone();
        self.load_table(&recovery_table, true)?;
        self.recovery_snapshot = Some(self.snapshot("recovered")?);
        self.delete_helper()?;
        self.restored = true;
        Ok(())
    }

    pub fn recovery_snapshot(&self) -> Option<&DmStatusSnapshot> {
        self.recovery_snapshot.as_ref()
    }

    fn wait_helper_ready(&self) -> Result<()> {
        Kubectl::new(&self.config)
            .namespaced(&self.config.test_namespace)
            .command([
                "wait",
                "--for=condition=Ready",
                "pod",
                &self.helper_pod,
                "--timeout=60s",
            ])
            .run_checked()?;
        Ok(())
    }

    fn verify_mount_source(&self) -> Result<()> {
        let source = self
            .host_command([
                "/usr/bin/findmnt",
                "-n",
                "-o",
                "SOURCE",
                "--target",
                self.mapping.mount_path.as_str(),
            ])?
            .stdout;
        let mapper = self
            .host_command([
                "/usr/bin/readlink",
                "-f",
                &format!("/dev/mapper/{}", self.dm_name),
            ])?
            .stdout;
        let source = source.trim();
        let canonical_source = self
            .host_command(["/usr/bin/readlink", "-f", source])?
            .stdout;
        ensure!(
            canonical_source.trim() == mapper.trim(),
            "fault-test PV mount {:?} on node {:?} is backed by {:?}, not device-mapper target {:?}",
            self.mapping.mount_path,
            self.mapping.node,
            source,
            self.dm_name
        );
        Ok(())
    }

    fn load_table(&self, table: &str, noflush: bool) -> Result<()> {
        self.dmsetup(dm_suspend_args(&self.dm_name, noflush))?;
        let load = self.dmsetup(["load", self.dm_name.as_str(), "--table", table]);
        let resume = self.dmsetup(dm_resume_args(&self.dm_name));
        load?;
        resume?;
        Ok(())
    }

    fn dmsetup<I, S>(&self, args: I) -> Result<CommandOutput>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut command = vec!["/usr/sbin/dmsetup".to_string()];
        command.extend(args.into_iter().map(Into::into));
        self.host_command(command)
    }

    fn host_command<I, S>(&self, args: I) -> Result<CommandOutput>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut command = vec![
            "exec".to_string(),
            self.helper_pod.clone(),
            "--".to_string(),
            "chroot".to_string(),
            "/host".to_string(),
        ];
        command.extend(args.into_iter().map(Into::into));
        Kubectl::new(&self.config)
            .namespaced(&self.config.test_namespace)
            .command(command)
            .run_checked()
    }

    fn delete_helper(&self) -> Result<()> {
        Kubectl::new(&self.config)
            .namespaced(&self.config.test_namespace)
            .command([
                "delete",
                "pod",
                &self.helper_pod,
                "--ignore-not-found",
                "--wait=true",
            ])
            .run_checked()?;
        Ok(())
    }
}

impl Drop for DmFlakeyGuard {
    fn drop(&mut self) {
        if !self.restored {
            let recovery_table = self.recovery_table.clone();
            if !recovery_table.is_empty() {
                let _ = self.load_table(&recovery_table, true);
            }
            let _ = self.delete_helper();
        }
    }
}

fn validate_dm_spec(spec: &DmFlakeySpec<'_>) -> Result<()> {
    ensure!(
        !spec.node.is_empty()
            && spec
                .node
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-')),
        "RUSTFS_FAULT_TEST_DM_NODE must be a valid node name"
    );
    ensure!(
        spec.mount_path.starts_with('/') && spec.mount_path != "/",
        "RUSTFS_FAULT_TEST_DM_MOUNT_PATH must be an absolute non-root path"
    );
    ensure!(
        !spec.mount_path.contains(['\n', '\r']),
        "RUSTFS_FAULT_TEST_DM_MOUNT_PATH must not contain newlines"
    );
    ensure!(
        !spec.name.is_empty()
            && spec
                .name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+')),
        "RUSTFS_FAULT_TEST_DM_NAME contains unsupported characters"
    );
    ensure!(
        !spec.fault_table.trim().is_empty(),
        "RUSTFS_FAULT_TEST_DM_FAULT_TABLE is required"
    );
    ensure!(
        !spec.helper_image.trim().is_empty()
            && !spec.helper_image.contains(['\n', '\r', ' ', '\t']),
        "RUSTFS_FAULT_TEST_DM_HELPER_IMAGE must be a non-empty image reference"
    );
    Ok(())
}

fn dm_resume_args(name: &str) -> [&str; 3] {
    ["resume", "--noudevsync", name]
}

fn dm_suspend_args(name: &str, noflush: bool) -> Vec<&str> {
    if noflush {
        vec!["suspend", "--noflush", name]
    } else {
        vec!["suspend", name]
    }
}

fn verify_dm_volume_mapping(
    config: &ClusterTestConfig,
    node: &str,
    expected_mount_path: &str,
) -> Result<DmVolumeMapping> {
    let selector = format!("rustfs.tenant={}", config.tenant_name);
    let pods = Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(["get", "pod", "-l", &selector, "-o", "json"])
        .run_checked()?;
    let pods = serde_json::from_str::<Value>(&pods.stdout).context("parse RustFS pod list")?;
    let pod = pods
        .pointer("/items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.pointer("/spec/nodeName").and_then(Value::as_str) == Some(node))
        })
        .with_context(|| format!("no RustFS fault-test Pod is scheduled on DM node {node:?}"))?;
    let pod_name = pod
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .context("DM target Pod is missing metadata.name")?;
    let pvc = pod
        .pointer("/spec/volumes")
        .and_then(Value::as_array)
        .and_then(|volumes| {
            volumes.iter().find_map(|volume| {
                volume
                    .pointer("/persistentVolumeClaim/claimName")
                    .and_then(Value::as_str)
            })
        })
        .context("DM target Pod does not mount a PVC")?;

    let pvc_json = Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(["get", "pvc", pvc, "-o", "json"])
        .run_checked()?;
    let pvc_json =
        serde_json::from_str::<Value>(&pvc_json.stdout).context("parse DM target PVC")?;
    let pv = pvc_json
        .pointer("/spec/volumeName")
        .and_then(Value::as_str)
        .context("DM target PVC is not bound")?;

    let pv_json = Kubectl::new(config)
        .command(["get", "pv", pv, "-o", "json"])
        .run_checked()?;
    let pv_json = serde_json::from_str::<Value>(&pv_json.stdout).context("parse DM target PV")?;
    let local_path = pv_json
        .pointer("/spec/local/path")
        .and_then(Value::as_str)
        .context("DM target PV is not a local PV")?;
    ensure!(
        local_path == expected_mount_path,
        "DM target PV {pv:?} uses local path {local_path:?}, expected {expected_mount_path:?}"
    );
    ensure!(
        pv_targets_node(&pv_json, node),
        "DM target PV {pv:?} node affinity does not target {node:?}"
    );

    Ok(DmVolumeMapping {
        node: node.to_string(),
        pod: pod_name.to_string(),
        pvc: pvc.to_string(),
        pv: pv.to_string(),
        mount_path: local_path.to_string(),
    })
}

fn pv_targets_node(pv: &Value, node: &str) -> bool {
    pv.pointer("/spec/nodeAffinity/required/nodeSelectorTerms")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|term| term.get("matchExpressions").and_then(Value::as_array))
        .flatten()
        .any(|expression| {
            expression.get("key").and_then(Value::as_str) == Some("kubernetes.io/hostname")
                && expression.get("operator").and_then(Value::as_str) == Some("In")
                && expression
                    .get("values")
                    .and_then(Value::as_array)
                    .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(node)))
        })
}

fn helper_pod_name(run_id: &str) -> String {
    let suffix = run_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("rustfs-fault-dm-helper-{suffix}")
}

fn dm_helper_manifest(config: &ClusterTestConfig, name: &str, node: &str, image: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Pod
metadata:
  name: {name}
  namespace: {namespace}
  labels:
    {managed_by_label}: {managed_by_value}
spec:
  nodeName: {node}
  hostPID: true
  restartPolicy: Never
  containers:
    - name: host-tools
      image: {image}
      imagePullPolicy: IfNotPresent
      command: ["sh", "-c", "trap : TERM INT; sleep 3600 & wait"]
      securityContext:
        privileged: true
      volumeMounts:
        - name: host-root
          mountPath: /host
          mountPropagation: HostToContainer
  volumes:
    - name: host-root
      hostPath:
        path: /
        type: Directory
"#,
        namespace = config.test_namespace,
        managed_by_label = MANAGED_BY_LABEL,
        managed_by_value = MANAGED_BY_VALUE,
    )
}

fn normalize_dm_table(table: &str) -> String {
    table.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        DmFlakeySpec, dm_helper_manifest, dm_resume_args, dm_suspend_args, helper_pod_name,
        normalize_dm_table, pv_targets_node, validate_dm_spec,
    };
    use crate::fault::config::FaultTestConfig;

    #[test]
    fn dm_helper_is_pinned_to_one_node_and_host_root() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let manifest = dm_helper_manifest(
            &config.cluster,
            "rustfs-fault-dm-helper-run123",
            "worker-a",
            "busybox:test",
        );

        assert!(manifest.contains("nodeName: worker-a"));
        assert!(manifest.contains("privileged: true"));
        assert!(manifest.contains("mountPath: /host"));
        assert!(manifest.contains("path: /"));
        assert!(manifest.contains("rustfs-operator-fault-test"));
    }

    #[test]
    fn dm_resume_disables_udev_synchronization() {
        assert_eq!(
            dm_resume_args("rustfs-fault-dm"),
            ["resume", "--noudevsync", "rustfs-fault-dm"]
        );
    }

    #[test]
    fn dm_recovery_suspend_does_not_flush_faulting_io() {
        assert_eq!(
            dm_suspend_args("rustfs-fault-dm", true),
            ["suspend", "--noflush", "rustfs-fault-dm"]
        );
        assert_eq!(
            dm_suspend_args("rustfs-fault-dm", false),
            ["suspend", "rustfs-fault-dm"]
        );
    }

    #[test]
    fn dm_table_comparison_uses_the_full_normalized_table() {
        assert_eq!(
            normalize_dm_table("0 1024  flakey   /dev/loop0 0 1 15\n"),
            "0 1024 flakey /dev/loop0 0 1 15"
        );
        assert_ne!(
            normalize_dm_table("0 1024 flakey /dev/loop0 0 1 15"),
            normalize_dm_table("0 1024 flakey /dev/loop1 0 1 15")
        );
    }

    #[test]
    fn dm_spec_rejects_unbounded_or_unsafe_targets() {
        let valid = DmFlakeySpec {
            node: "worker-a",
            mount_path: "/data/rustfs-fault/dm-volume",
            helper_image: "busybox:test",
            name: "rustfs-fault-dm",
            fault_table: "0 1024 flakey /dev/loop0 0 1 15",
            recovery_table: None,
            run_id: "run-123",
        };
        assert!(validate_dm_spec(&valid).is_ok());

        let root = DmFlakeySpec {
            mount_path: "/",
            ..valid
        };
        assert!(validate_dm_spec(&root).is_err());
    }

    #[test]
    fn dm_pv_affinity_must_match_target_node() {
        let pv = serde_json::json!({
            "spec": {"nodeAffinity": {"required": {"nodeSelectorTerms": [{
                "matchExpressions": [{
                    "key": "kubernetes.io/hostname",
                    "operator": "In",
                    "values": ["worker-a"]
                }]
            }]}}}
        });

        assert!(pv_targets_node(&pv, "worker-a"));
        assert!(!pv_targets_node(&pv, "worker-b"));
        assert_eq!(
            helper_pod_name("run-ABC-123"),
            "rustfs-fault-dm-helper-runabc123"
        );
    }
}
