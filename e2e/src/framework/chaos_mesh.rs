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
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::framework::{config::ClusterTestConfig, kubectl::Kubectl};

const IOCHAOS_CRD: &str = "iochaos.chaos-mesh.org";
const RUN_ID_LABEL: &str = "rustfs-fault-test/run-id";
const SCENARIO_LABEL: &str = "rustfs-fault-test/scenario";
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const MANAGED_BY_VALUE: &str = "rustfs-operator-fault-test";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoChaosSpec {
    pub name: String,
    pub namespace: String,
    pub run_id: String,
    pub scenario: String,
    pub target_namespace: String,
    pub tenant_name: String,
    pub container_name: String,
    pub volume_path: String,
    pub methods: Vec<String>,
    pub errno: u8,
    pub percent: u8,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct ChaosGuard {
    config: ClusterTestConfig,
    kind: &'static str,
    namespace: String,
    name: String,
    deleted: bool,
}

impl IoChaosSpec {
    pub fn eio_on_rustfs_volume(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        volume_path: impl Into<String>,
        percent: u8,
        duration: Duration,
    ) -> Result<Self> {
        ensure!(
            (1..=100).contains(&percent),
            "IOChaos percent must be in 1..=100, got {percent}"
        );
        ensure!(
            duration > Duration::ZERO,
            "IOChaos duration must be positive"
        );

        let run_id = run_id.into();
        let short_run_id = run_id.chars().take(12).collect::<String>();
        let scenario = scenario.into();

        Ok(Self {
            name: format!("rustfs-fault-io-eio-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario,
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            container_name: "rustfs".to_string(),
            volume_path: volume_path.into(),
            methods: vec!["READ".to_string(), "WRITE".to_string()],
            errno: 5,
            percent,
            duration,
        })
    }

    pub fn manifest(&self) -> String {
        let methods = self
            .methods
            .iter()
            .map(|method| format!("    - {method}"))
            .collect::<Vec<_>>()
            .join("\n");
        let seconds = self.duration.as_secs();

        format!(
            r#"apiVersion: chaos-mesh.org/v1alpha1
kind: IOChaos
metadata:
  name: {name}
  namespace: {namespace}
  labels:
    {run_id_label}: "{run_id}"
    {scenario_label}: "{scenario}"
    {managed_by_label}: {managed_by_value}
spec:
  action: fault
  mode: one
  selector:
    namespaces:
      - {target_namespace}
    labelSelectors:
      rustfs.tenant: {tenant_name}
  containerNames:
    - {container_name}
  volumePath: {volume_path}
  path: {volume_path}/**/*
  methods:
{methods}
  errno: {errno}
  percent: {percent}
  duration: "{seconds}s"
"#,
            name = self.name,
            namespace = self.namespace,
            run_id_label = RUN_ID_LABEL,
            run_id = self.run_id,
            scenario_label = SCENARIO_LABEL,
            scenario = self.scenario,
            managed_by_label = MANAGED_BY_LABEL,
            managed_by_value = MANAGED_BY_VALUE,
            target_namespace = self.target_namespace,
            tenant_name = self.tenant_name,
            container_name = self.container_name,
            volume_path = self.volume_path,
            methods = methods,
            errno = self.errno,
            percent = self.percent,
        )
    }
}

pub fn require_iochaos_crd(config: &ClusterTestConfig) -> Result<()> {
    let output = Kubectl::new(config)
        .command(["get", "crd", IOCHAOS_CRD])
        .run()?;
    ensure!(
        output.code == Some(0),
        "Chaos Mesh IOChaos CRD {IOCHAOS_CRD} is required for fault tests; install Chaos Mesh before running faults\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr
    );
    Ok(())
}

pub fn cleanup_run(config: &ClusterTestConfig, namespace: &str, run_id: &str) -> Result<()> {
    let selector = format!("{RUN_ID_LABEL}={run_id}");
    Kubectl::new(config)
        .namespaced(namespace)
        .command(["delete", "iochaos", "-l", &selector, "--ignore-not-found"])
        .run_checked()?;
    Ok(())
}

pub fn cleanup_managed_iochaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    let selector = format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}");
    Kubectl::new(config)
        .namespaced(namespace)
        .command(["delete", "iochaos", "-l", &selector, "--ignore-not-found"])
        .run_checked()?;
    Ok(())
}

pub fn apply_iochaos(config: &ClusterTestConfig, spec: &IoChaosSpec) -> Result<ChaosGuard> {
    cleanup_run(config, &spec.namespace, &spec.run_id)?;
    Kubectl::new(config)
        .namespaced(&spec.namespace)
        .apply_yaml_command(spec.manifest())
        .run_checked()?;

    Ok(ChaosGuard {
        config: config.clone(),
        kind: "iochaos",
        namespace: spec.namespace.clone(),
        name: spec.name.clone(),
        deleted: false,
    })
}

impl ChaosGuard {
    pub fn wait_active(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            let status_snapshot = match self.json() {
                Ok(status) => {
                    if iochaos_is_active(&status)? {
                        return Ok(());
                    }
                    status
                }
                Err(error) => format!("failed to read IOChaos status: {error}"),
            };

            if Instant::now() >= deadline {
                let describe = self
                    .describe()
                    .unwrap_or_else(|error| format!("failed to describe IOChaos: {error}"));
                bail!(
                    "timed out waiting for {kind}/{name} to become active after {timeout:?}\nlast status:\n{status_snapshot}\n\ndescribe:\n{describe}",
                    kind = self.kind,
                    name = self.name,
                );
            }

            sleep(Duration::from_secs(1));
        }
    }

    pub fn ensure_active(&self, stage: &str) -> Result<()> {
        let status = self.json()?;
        ensure!(
            iochaos_is_active(&status)?,
            "{kind}/{name} is not active at {stage}; status:\n{status}",
            kind = self.kind,
            name = self.name
        );
        Ok(())
    }

    pub fn describe(&self) -> Result<String> {
        let output = Kubectl::new(&self.config)
            .namespaced(&self.namespace)
            .command(["describe", self.kind, &self.name])
            .run_checked()?;
        Ok(output.stdout)
    }

    pub fn yaml(&self) -> Result<String> {
        let output = Kubectl::new(&self.config)
            .namespaced(&self.namespace)
            .command(["get", self.kind, &self.name, "-o", "yaml"])
            .run_checked()?;
        Ok(output.stdout)
    }

    pub fn delete(&mut self) -> Result<()> {
        self.delete_inner()?;
        self.deleted = true;
        Ok(())
    }

    fn json(&self) -> Result<String> {
        let output = Kubectl::new(&self.config)
            .namespaced(&self.namespace)
            .command(["get", self.kind, &self.name, "-o", "json"])
            .run_checked()?;
        Ok(output.stdout)
    }

    fn delete_inner(&self) -> Result<()> {
        Kubectl::new(&self.config)
            .namespaced(&self.namespace)
            .command(["delete", self.kind, &self.name, "--ignore-not-found"])
            .run_checked()?;
        Ok(())
    }
}

fn iochaos_is_active(raw: &str) -> Result<bool> {
    let value = serde_json::from_str::<Value>(raw).context("parse IOChaos status json")?;
    let selected = condition_status(&value, "Selected").is_some_and(|status| status == "True");
    let injected = condition_status(&value, "AllInjected")
        .or_else(|| condition_status(&value, "Injected"))
        .is_some_and(|status| status == "True");
    let recovered = condition_status(&value, "AllRecovered").is_some_and(|status| status == "True");

    Ok(selected && injected && !recovered)
}

fn condition_status(value: &Value, condition_type: &str) -> Option<String> {
    value
        .pointer("/status/conditions")?
        .as_array()?
        .iter()
        .find(|condition| condition.get("type").and_then(Value::as_str) == Some(condition_type))?
        .get("status")?
        .as_str()
        .map(str::to_string)
}

impl Drop for ChaosGuard {
    fn drop(&mut self) {
        if !self.deleted {
            let _ = self.delete_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IoChaosSpec, iochaos_is_active};
    use crate::framework::fault_config::FaultTestConfig;
    use std::time::Duration;

    #[test]
    fn iochaos_manifest_targets_rustfs_workload_only() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let spec = IoChaosSpec::eio_on_rustfs_volume(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "io-eio",
            "/data/rustfs0",
            20,
            Duration::from_secs(60),
        )
        .expect("valid io chaos");
        let manifest = spec.manifest();

        assert!(manifest.contains("kind: IOChaos"));
        assert!(manifest.contains("namespace: chaos-mesh"));
        assert!(manifest.contains("rustfs.tenant: fault-test-tenant"));
        assert!(manifest.contains("rustfs-fault-test/run-id"));
        assert!(manifest.contains("rustfs-operator-fault-test"));
        assert!(manifest.contains("containerNames:\n    - rustfs"));
        assert!(manifest.contains("volumePath: /data/rustfs0"));
        assert!(manifest.contains("errno: 5"));
        assert!(manifest.contains("percent: 20"));
    }

    #[test]
    fn iochaos_active_requires_selected_and_injected_not_recovered() {
        let status = r#"{
          "status": {
            "conditions": [
              {"type": "Selected", "status": "True"},
              {"type": "AllInjected", "status": "True"},
              {"type": "AllRecovered", "status": "False"}
            ]
          }
        }"#;

        assert!(iochaos_is_active(status).expect("valid status"));
    }

    #[test]
    fn iochaos_active_rejects_unselected_experiment() {
        let status = r#"{
          "status": {
            "conditions": [
              {"type": "Selected", "status": "False"},
              {"type": "AllInjected", "status": "True"}
            ]
          }
        }"#;

        assert!(!iochaos_is_active(status).expect("valid status"));
    }
}
