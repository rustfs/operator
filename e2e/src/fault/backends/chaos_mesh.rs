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
const PODCHAOS_CRD: &str = "podchaos.chaos-mesh.org";
const NETWORKCHAOS_CRD: &str = "networkchaos.chaos-mesh.org";
const STRESSCHAOS_CRD: &str = "stresschaos.chaos-mesh.org";
const RUN_ID_LABEL: &str = "rustfs-fault-test/run-id";
const SCENARIO_LABEL: &str = "rustfs-fault-test/scenario";
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const MANAGED_BY_VALUE: &str = "rustfs-operator-fault-test";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoChaosAction {
    Fault {
        errno: u8,
    },
    Latency {
        delay: String,
    },
    Mistake {
        filling: String,
        max_occurrences: u8,
        max_length: usize,
    },
}

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
    pub action: IoChaosAction,
    pub percent: u8,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PodChaosAction {
    PodKill,
    PodFailure { duration: Duration },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodChaosSpec {
    pub name: String,
    pub namespace: String,
    pub run_id: String,
    pub scenario: String,
    pub target_namespace: String,
    pub tenant_name: String,
    pub action: PodChaosAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkChaosAction {
    Partition,
    Delay {
        latency: String,
        jitter: String,
        correlation: String,
    },
    Loss {
        loss: String,
        correlation: String,
    },
    Corrupt {
        corrupt: String,
        correlation: String,
    },
    Duplicate {
        duplicate: String,
        correlation: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkChaosSpec {
    pub name: String,
    pub namespace: String,
    pub run_id: String,
    pub scenario: String,
    pub target_namespace: String,
    pub tenant_name: String,
    pub action: NetworkChaosAction,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub enum StressChaosAction {
    Cpu { workers: u32, load: u32 },
    Memory { workers: u32, size: String },
}

#[derive(Debug, Clone)]
pub struct StressChaosSpec {
    pub name: String,
    pub namespace: String,
    pub run_id: String,
    pub scenario: String,
    pub target_namespace: String,
    pub tenant_name: String,
    pub action: StressChaosAction,
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
            action: IoChaosAction::Fault { errno: 5 },
            percent,
            duration,
        })
    }

    pub fn read_mistake_on_rustfs_volume(
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
            name: format!("rustfs-fault-io-mistake-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario,
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            container_name: "rustfs".to_string(),
            volume_path: volume_path.into(),
            methods: vec!["READ".to_string()],
            action: IoChaosAction::Mistake {
                filling: "random".to_string(),
                max_occurrences: 1,
                max_length: 4096,
            },
            percent,
            duration,
        })
    }

    pub fn latency_on_rustfs_volume(
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
            name: format!("rustfs-fault-io-latency-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario,
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            container_name: "rustfs".to_string(),
            volume_path: volume_path.into(),
            methods: vec!["READ".to_string(), "WRITE".to_string()],
            action: IoChaosAction::Latency {
                delay: "250ms".to_string(),
            },
            percent,
            duration,
        })
    }

    pub fn enospc_on_rustfs_volume(
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
            name: format!("rustfs-fault-enospc-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario,
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            container_name: "rustfs".to_string(),
            volume_path: volume_path.into(),
            methods: vec!["WRITE".to_string()],
            action: IoChaosAction::Fault { errno: 28 },
            percent,
            duration,
        })
    }

    pub fn with_name_suffix(mut self, suffix: &str) -> Self {
        self.name.push_str(suffix);
        self
    }

    pub fn manifest(&self) -> String {
        let methods = self
            .methods
            .iter()
            .map(|method| format!("    - {method}"))
            .collect::<Vec<_>>()
            .join("\n");
        let seconds = self.duration.as_secs();
        let action = self.action_manifest();

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
{action}
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
            percent = self.percent,
            action = action,
        )
    }

    fn action_manifest(&self) -> String {
        match &self.action {
            IoChaosAction::Fault { errno } => {
                format!("  action: fault\n  errno: {errno}")
            }
            IoChaosAction::Latency { delay } => {
                format!("  action: latency\n  delay: {delay}")
            }
            IoChaosAction::Mistake {
                filling,
                max_occurrences,
                max_length,
            } => format!(
                r#"  action: mistake
  mistake:
    filling: {filling}
    maxOccurrences: {max_occurrences}
    maxLength: {max_length}"#
            ),
        }
    }
}

impl PodChaosSpec {
    pub fn kill_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
    ) -> Self {
        let run_id = run_id.into();
        let short_run_id = run_id.chars().take(12).collect::<String>();
        Self {
            name: format!("rustfs-fault-pod-kill-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario: scenario.into(),
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            action: PodChaosAction::PodKill,
        }
    }

    pub fn fail_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        ensure!(
            duration > Duration::ZERO,
            "PodChaos duration must be positive"
        );

        let run_id = run_id.into();
        let short_run_id = run_id.chars().take(12).collect::<String>();
        Ok(Self {
            name: format!("rustfs-fault-pod-failure-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario: scenario.into(),
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            action: PodChaosAction::PodFailure { duration },
        })
    }

    pub fn with_name_suffix(mut self, suffix: &str) -> Self {
        self.name.push_str(suffix);
        self
    }

    pub fn manifest(&self) -> String {
        let action = self.action_manifest();
        format!(
            r#"apiVersion: chaos-mesh.org/v1alpha1
kind: PodChaos
metadata:
  name: {name}
  namespace: {namespace}
  labels:
    {run_id_label}: "{run_id}"
    {scenario_label}: "{scenario}"
    {managed_by_label}: {managed_by_value}
spec:
{action}
  mode: one
  selector:
    namespaces:
      - {target_namespace}
    labelSelectors:
      rustfs.tenant: {tenant_name}
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
            action = action,
        )
    }

    fn action_manifest(&self) -> String {
        match self.action {
            PodChaosAction::PodKill => "  action: pod-kill".to_string(),
            PodChaosAction::PodFailure { duration } => {
                format!(
                    "  action: pod-failure\n  duration: \"{}s\"",
                    duration.as_secs()
                )
            }
        }
    }
}

impl NetworkChaosSpec {
    pub fn partition_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "net-partition",
            NetworkChaosAction::Partition,
        )
    }

    pub fn delay_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "net-delay",
            NetworkChaosAction::Delay {
                latency: "200ms".to_string(),
                jitter: "50ms".to_string(),
                correlation: "25".to_string(),
            },
        )
    }

    pub fn loss_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "net-loss",
            NetworkChaosAction::Loss {
                loss: "25".to_string(),
                correlation: "25".to_string(),
            },
        )
    }

    pub fn corrupt_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "net-corrupt",
            NetworkChaosAction::Corrupt {
                corrupt: "5".to_string(),
                correlation: "25".to_string(),
            },
        )
    }

    pub fn duplicate_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "net-duplicate",
            NetworkChaosAction::Duplicate {
                duplicate: "10".to_string(),
                correlation: "25".to_string(),
            },
        )
    }

    fn one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
        name_action: &str,
        action: NetworkChaosAction,
    ) -> Result<Self> {
        ensure!(
            duration > Duration::ZERO,
            "NetworkChaos duration must be positive"
        );

        let run_id = run_id.into();
        let short_run_id = run_id.chars().take(12).collect::<String>();
        Ok(Self {
            name: format!("rustfs-fault-{name_action}-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario: scenario.into(),
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            action,
            duration,
        })
    }

    pub fn with_name_suffix(mut self, suffix: &str) -> Self {
        self.name.push_str(suffix);
        self
    }

    pub fn manifest(&self) -> String {
        let seconds = self.duration.as_secs();
        let action = self.action_manifest();
        format!(
            r#"apiVersion: chaos-mesh.org/v1alpha1
kind: NetworkChaos
metadata:
  name: {name}
  namespace: {namespace}
  labels:
    {run_id_label}: "{run_id}"
    {scenario_label}: "{scenario}"
    {managed_by_label}: {managed_by_value}
spec:
{action}
  mode: one
  selector:
    namespaces:
      - {target_namespace}
    labelSelectors:
      rustfs.tenant: {tenant_name}
  direction: both
  target:
    mode: all
    selector:
      namespaces:
        - {target_namespace}
      labelSelectors:
        rustfs.tenant: {tenant_name}
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
            action = action,
        )
    }

    fn action_manifest(&self) -> String {
        match &self.action {
            NetworkChaosAction::Partition => "  action: partition".to_string(),
            NetworkChaosAction::Delay {
                latency,
                jitter,
                correlation,
            } => format!(
                r#"  action: delay
  delay:
    latency: "{latency}"
    jitter: "{jitter}"
    correlation: "{correlation}""#
            ),
            NetworkChaosAction::Loss { loss, correlation } => format!(
                r#"  action: loss
  loss:
    loss: "{loss}"
    correlation: "{correlation}""#
            ),
            NetworkChaosAction::Corrupt {
                corrupt,
                correlation,
            } => format!(
                r#"  action: corrupt
  corrupt:
    corrupt: "{corrupt}"
    correlation: "{correlation}""#
            ),
            NetworkChaosAction::Duplicate {
                duplicate,
                correlation,
            } => format!(
                r#"  action: duplicate
  duplicate:
    duplicate: "{duplicate}"
    correlation: "{correlation}""#
            ),
        }
    }
}

impl StressChaosSpec {
    pub fn cpu_on_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "stress-cpu",
            StressChaosAction::Cpu {
                workers: 1,
                load: 80,
            },
        )
    }

    pub fn memory_on_one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
    ) -> Result<Self> {
        Self::one_rustfs_pod(
            config,
            chaos_namespace,
            run_id,
            scenario,
            duration,
            "stress-memory",
            StressChaosAction::Memory {
                workers: 1,
                size: "512MiB".to_string(),
            },
        )
    }

    fn one_rustfs_pod(
        config: &ClusterTestConfig,
        chaos_namespace: impl Into<String>,
        run_id: impl Into<String>,
        scenario: impl Into<String>,
        duration: Duration,
        name_action: &str,
        action: StressChaosAction,
    ) -> Result<Self> {
        ensure!(
            duration > Duration::ZERO,
            "StressChaos duration must be positive"
        );

        let run_id = run_id.into();
        let short_run_id = run_id.chars().take(12).collect::<String>();
        Ok(Self {
            name: format!("rustfs-fault-{name_action}-{short_run_id}"),
            namespace: chaos_namespace.into(),
            run_id,
            scenario: scenario.into(),
            target_namespace: config.test_namespace.clone(),
            tenant_name: config.tenant_name.clone(),
            action,
            duration,
        })
    }

    pub fn with_name_suffix(mut self, suffix: &str) -> Self {
        self.name.push_str(suffix);
        self
    }

    pub fn manifest(&self) -> String {
        let seconds = self.duration.as_secs();
        let stressors = self.stressors_manifest();
        format!(
            r#"apiVersion: chaos-mesh.org/v1alpha1
kind: StressChaos
metadata:
  name: {name}
  namespace: {namespace}
  labels:
    {run_id_label}: "{run_id}"
    {scenario_label}: "{scenario}"
    {managed_by_label}: {managed_by_value}
spec:
  mode: one
  selector:
    namespaces:
      - {target_namespace}
    labelSelectors:
      rustfs.tenant: {tenant_name}
  stressors:
{stressors}
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
            stressors = stressors,
        )
    }

    fn stressors_manifest(&self) -> String {
        match &self.action {
            StressChaosAction::Cpu { workers, load } => format!(
                r#"    cpu:
      workers: {workers}
      load: {load}"#
            ),
            StressChaosAction::Memory { workers, size } => format!(
                r#"    memory:
      workers: {workers}
      size: "{size}""#
            ),
        }
    }
}

pub fn require_iochaos_crd(config: &ClusterTestConfig) -> Result<()> {
    require_crd(config, IOCHAOS_CRD, "Chaos Mesh IOChaos")
}

pub fn require_podchaos_crd(config: &ClusterTestConfig) -> Result<()> {
    require_crd(config, PODCHAOS_CRD, "Chaos Mesh PodChaos")
}

pub fn require_networkchaos_crd(config: &ClusterTestConfig) -> Result<()> {
    require_crd(config, NETWORKCHAOS_CRD, "Chaos Mesh NetworkChaos")
}

pub fn require_stresschaos_crd(config: &ClusterTestConfig) -> Result<()> {
    require_crd(config, STRESSCHAOS_CRD, "Chaos Mesh StressChaos")
}

fn require_crd(config: &ClusterTestConfig, crd: &str, description: &str) -> Result<()> {
    let output = Kubectl::new(config).command(["get", "crd", crd]).run()?;
    ensure!(
        output.code == Some(0),
        "{description} CRD {crd} is required for fault tests; install Chaos Mesh before running faults\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr
    );
    Ok(())
}

pub fn cleanup_run(config: &ClusterTestConfig, namespace: &str, run_id: &str) -> Result<()> {
    let selector = format!("{RUN_ID_LABEL}={run_id}");
    for kind in ["iochaos", "podchaos", "networkchaos", "stresschaos"] {
        Kubectl::new(config)
            .namespaced(namespace)
            .command(["delete", kind, "-l", &selector, "--ignore-not-found"])
            .run_checked()?;
    }
    Ok(())
}

pub fn cleanup_run_kind(
    config: &ClusterTestConfig,
    namespace: &str,
    run_id: &str,
    kind: &str,
) -> Result<()> {
    let selector = format!("{RUN_ID_LABEL}={run_id}");
    Kubectl::new(config)
        .namespaced(namespace)
        .command(["delete", kind, "-l", &selector, "--ignore-not-found"])
        .run_checked()?;
    Ok(())
}

pub fn cleanup_managed_chaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    for kind in ["iochaos", "podchaos", "networkchaos", "stresschaos"] {
        cleanup_managed_kind(config, namespace, kind)?;
    }
    Ok(())
}

pub fn cleanup_managed_iochaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    cleanup_managed_kind(config, namespace, "iochaos")
}

pub fn cleanup_managed_podchaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    cleanup_managed_kind(config, namespace, "podchaos")
}

pub fn cleanup_managed_networkchaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    cleanup_managed_kind(config, namespace, "networkchaos")
}

pub fn cleanup_managed_stresschaos(config: &ClusterTestConfig, namespace: &str) -> Result<()> {
    cleanup_managed_kind(config, namespace, "stresschaos")
}

fn cleanup_managed_kind(config: &ClusterTestConfig, namespace: &str, kind: &str) -> Result<()> {
    let selector = format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}");
    Kubectl::new(config)
        .namespaced(namespace)
        .command(["delete", kind, "-l", &selector, "--ignore-not-found"])
        .run_checked()?;
    Ok(())
}

pub fn apply_iochaos(config: &ClusterTestConfig, spec: &IoChaosSpec) -> Result<ChaosGuard> {
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

pub fn apply_podchaos(config: &ClusterTestConfig, spec: &PodChaosSpec) -> Result<ChaosGuard> {
    Kubectl::new(config)
        .namespaced(&spec.namespace)
        .apply_yaml_command(spec.manifest())
        .run_checked()?;

    Ok(ChaosGuard {
        config: config.clone(),
        kind: "podchaos",
        namespace: spec.namespace.clone(),
        name: spec.name.clone(),
        deleted: false,
    })
}

pub fn apply_networkchaos(
    config: &ClusterTestConfig,
    spec: &NetworkChaosSpec,
) -> Result<ChaosGuard> {
    Kubectl::new(config)
        .namespaced(&spec.namespace)
        .apply_yaml_command(spec.manifest())
        .run_checked()?;

    Ok(ChaosGuard {
        config: config.clone(),
        kind: "networkchaos",
        namespace: spec.namespace.clone(),
        name: spec.name.clone(),
        deleted: false,
    })
}

pub fn apply_stresschaos(config: &ClusterTestConfig, spec: &StressChaosSpec) -> Result<ChaosGuard> {
    Kubectl::new(config)
        .namespaced(&spec.namespace)
        .apply_yaml_command(spec.manifest())
        .run_checked()?;

    Ok(ChaosGuard {
        config: config.clone(),
        kind: "stresschaos",
        namespace: spec.namespace.clone(),
        name: spec.name.clone(),
        deleted: false,
    })
}

impl ChaosGuard {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn wait_active(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            let status_snapshot = match self.json() {
                Ok(status) => {
                    if chaos_experiment_is_active(&status)? {
                        return Ok(());
                    }
                    status
                }
                Err(error) => {
                    format!("failed to read {kind} status: {error}", kind = self.kind)
                }
            };

            if Instant::now() >= deadline {
                let describe = self.describe().unwrap_or_else(|error| {
                    format!(
                        "failed to describe {kind}/{name}: {error}",
                        kind = self.kind,
                        name = self.name
                    )
                });
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
            chaos_experiment_is_active(&status)?,
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

    pub fn json(&self) -> Result<String> {
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

fn chaos_experiment_is_active(raw: &str) -> Result<bool> {
    let value = serde_json::from_str::<Value>(raw).context("parse Chaos Mesh status json")?;
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
    use super::{
        IoChaosSpec, NetworkChaosSpec, PodChaosSpec, StressChaosSpec, chaos_experiment_is_active,
    };
    use crate::fault::config::FaultTestConfig;
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
    fn enospc_manifest_targets_only_volume_writes() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let spec = IoChaosSpec::enospc_on_rustfs_volume(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "disk-full",
            "/data/rustfs0",
            100,
            Duration::from_secs(60),
        )
        .expect("valid enospc chaos");
        let manifest = spec.manifest();

        assert!(manifest.contains("errno: 28"));
        assert!(manifest.contains("methods:\n    - WRITE"));
        assert!(manifest.contains("percent: 100"));
        assert!(!manifest.contains("    - READ"));
    }

    #[test]
    fn io_latency_manifest_targets_volume_reads_and_writes() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let spec = IoChaosSpec::latency_on_rustfs_volume(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "io-latency",
            "/data/rustfs0",
            20,
            Duration::from_secs(60),
        )
        .expect("valid latency chaos");
        let manifest = spec.manifest();

        assert!(manifest.contains("action: latency"));
        assert!(manifest.contains("delay: 250ms"));
        assert!(manifest.contains("methods:\n    - READ\n    - WRITE"));
    }

    #[test]
    fn pod_failure_manifest_uses_duration() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let spec = PodChaosSpec::fail_one_rustfs_pod(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "pod-failure",
            Duration::from_secs(60),
        )
        .expect("valid pod failure");
        let manifest = spec.manifest();

        assert!(manifest.contains("kind: PodChaos"));
        assert!(manifest.contains("action: pod-failure"));
        assert!(manifest.contains("duration: \"60s\""));
        assert!(manifest.contains("rustfs.tenant: fault-test-tenant"));
    }

    #[test]
    fn network_delay_and_loss_manifests_use_targeted_actions() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let delay = NetworkChaosSpec::delay_one_rustfs_pod(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "network-delay",
            Duration::from_secs(60),
        )
        .expect("valid network delay")
        .manifest();
        let loss = NetworkChaosSpec::loss_one_rustfs_pod(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "network-loss",
            Duration::from_secs(60),
        )
        .expect("valid network loss")
        .manifest();

        assert!(delay.contains("action: delay"));
        assert!(delay.contains("latency: \"200ms\""));
        assert!(loss.contains("action: loss"));
        assert!(loss.contains("loss: \"25\""));
    }

    #[test]
    fn stress_manifests_target_one_rustfs_pod() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let cpu = StressChaosSpec::cpu_on_one_rustfs_pod(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "stress-cpu",
            Duration::from_secs(60),
        )
        .expect("valid cpu stress")
        .manifest();
        let memory = StressChaosSpec::memory_on_one_rustfs_pod(
            &config.cluster,
            "chaos-mesh",
            "run-1234567890",
            "stress-memory",
            Duration::from_secs(60),
        )
        .expect("valid memory stress")
        .manifest();

        assert!(cpu.contains("kind: StressChaos"));
        assert!(cpu.contains("cpu:"));
        assert!(cpu.contains("load: 80"));
        assert!(memory.contains("memory:"));
        assert!(memory.contains("size: \"512MiB\""));
        assert!(memory.contains("rustfs.tenant: fault-test-tenant"));
    }

    #[test]
    fn chaos_name_suffix_keeps_run_label_stable() {
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
        .expect("valid io chaos")
        .with_name_suffix("-01");
        let manifest = spec.manifest();

        assert_eq!(spec.name, "rustfs-fault-io-eio-run-12345678-01");
        assert!(manifest.contains("name: rustfs-fault-io-eio-run-12345678-01"));
        assert!(manifest.contains("rustfs-fault-test/run-id: \"run-1234567890\""));
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

        assert!(chaos_experiment_is_active(status).expect("valid status"));
    }

    #[test]
    fn chaos_experiment_active_rejects_unselected_experiment() {
        let status = r#"{
          "status": {
            "conditions": [
              {"type": "Selected", "status": "False"},
              {"type": "AllInjected", "status": "True"}
            ]
          }
        }"#;

        assert!(!chaos_experiment_is_active(status).expect("valid status"));
    }
}
