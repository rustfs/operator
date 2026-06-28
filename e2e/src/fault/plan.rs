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

use anyhow::{Result, bail, ensure};
use std::time::Duration;

use crate::fault::{
    config::{DEFAULT_RUSTFS_VOLUME_PATH, FaultTestConfig, validate_rustfs_volume_path},
    scenarios::{
        DISK_FULL_SCENARIO, DM_FLAKEY_SCENARIO, FaultBackend, FaultScenario, FaultScenarioSpec,
        IO_EIO_SCENARIO, IO_LATENCY_SCENARIO, IO_READ_MISTAKE_SCENARIO, NETWORK_CORRUPT_SCENARIO,
        NETWORK_DELAY_SCENARIO, NETWORK_DUPLICATE_SCENARIO, NETWORK_LOSS_SCENARIO,
        NETWORK_PARTITION_ONE_SCENARIO, POD_FAILURE_SCENARIO, POD_KILL_ONE_SCENARIO,
        STRESS_CPU_SCENARIO, STRESS_MEMORY_SCENARIO, WARP_UNDER_CHAOS_SCENARIO,
    },
};

pub const DEFAULT_RUSTFS_DATA_VOLUME: &str = DEFAULT_RUSTFS_VOLUME_PATH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultWorkloadMode {
    S3Mixed,
    S3MixedWithWarp,
}

impl FaultWorkloadMode {
    pub fn runs_warp(self) -> bool {
        matches!(self, Self::S3MixedWithWarp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    RustfsVolumeIoError,
    RustfsVolumeLatency,
    RustfsVolumeReadMistake,
    RustfsVolumeEnospc,
    RustfsServerPodKill,
    RustfsServerPodFailure,
    RustfsServerNetworkPartition,
    RustfsServerNetworkDelay,
    RustfsServerNetworkLoss,
    RustfsServerNetworkCorrupt,
    RustfsServerNetworkDuplicate,
    RustfsServerCpuStress,
    RustfsServerMemoryStress,
    RustfsBlockDeviceFlakey,
}

impl FaultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RustfsVolumeIoError => "rustfs_volume_io_error",
            Self::RustfsVolumeLatency => "rustfs_volume_latency",
            Self::RustfsVolumeReadMistake => "rustfs_volume_read_mistake",
            Self::RustfsVolumeEnospc => "rustfs_volume_enospc",
            Self::RustfsServerPodKill => "rustfs_server_pod_kill",
            Self::RustfsServerPodFailure => "rustfs_server_pod_failure",
            Self::RustfsServerNetworkPartition => "rustfs_server_network_partition",
            Self::RustfsServerNetworkDelay => "rustfs_server_network_delay",
            Self::RustfsServerNetworkLoss => "rustfs_server_network_loss",
            Self::RustfsServerNetworkCorrupt => "rustfs_server_network_corrupt",
            Self::RustfsServerNetworkDuplicate => "rustfs_server_network_duplicate",
            Self::RustfsServerCpuStress => "rustfs_server_cpu_stress",
            Self::RustfsServerMemoryStress => "rustfs_server_memory_stress",
            Self::RustfsBlockDeviceFlakey => "rustfs_block_device_flakey",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultTarget {
    RustfsVolume { path: String },
    RustfsServerPod,
    RustfsServerPeerNetwork,
    RustfsServerResource,
    DedicatedBlockDevice,
}

impl FaultTarget {
    pub fn summary(&self) -> String {
        match self {
            Self::RustfsVolume { path } => format!("one RustFS volume at {path}"),
            Self::RustfsServerPod => "one RustFS server Pod".to_string(),
            Self::RustfsServerPeerNetwork => {
                "one RustFS server Pod partitioned from its peers".to_string()
            }
            Self::RustfsServerResource => {
                "one RustFS server Pod under resource pressure".to_string()
            }
            Self::DedicatedBlockDevice => "one dedicated block-device-backed PV".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultSelection {
    Percent(u8),
    FixedTargets(u32),
}

impl FaultSelection {
    pub fn summary(self) -> String {
        match self {
            Self::Percent(percent) => format!("{percent}%"),
            Self::FixedTargets(count) => format!("{count} target(s)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultInjection {
    kind: FaultKind,
    backend: FaultBackend,
    target: FaultTarget,
    selection: FaultSelection,
    duration: Duration,
}

impl FaultInjection {
    pub fn new(
        kind: FaultKind,
        backend: FaultBackend,
        target: FaultTarget,
        selection: FaultSelection,
        duration: Duration,
    ) -> Result<Self> {
        ensure!(
            fault_kind_accepts_backend(kind, backend),
            "fault kind {} cannot run with backend {:?}",
            kind.as_str(),
            backend
        );
        ensure!(
            fault_kind_accepts_selection(kind, selection),
            "fault kind {} cannot run with selection {:?}",
            kind.as_str(),
            selection
        );
        ensure!(
            fault_kind_accepts_target(kind, &target),
            "fault kind {} cannot run with target {:?}",
            kind.as_str(),
            target
        );
        if let FaultTarget::RustfsVolume { path } = &target {
            validate_rustfs_volume_path(path)?;
        }
        ensure!(duration > Duration::ZERO, "fault duration must be positive");

        Ok(Self {
            kind,
            backend,
            target,
            selection,
            duration,
        })
    }

    pub fn kind(&self) -> FaultKind {
        self.kind
    }

    pub fn backend(&self) -> FaultBackend {
        self.backend
    }

    pub fn target(&self) -> &FaultTarget {
        &self.target
    }

    pub fn selection(&self) -> FaultSelection {
        self.selection
    }

    pub fn percent(&self) -> Result<u8> {
        match self.selection {
            FaultSelection::Percent(percent) => Ok(percent),
            other => bail!(
                "fault kind {} requires a percent selection, got {:?}",
                self.kind.as_str(),
                other
            ),
        }
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn rustfs_volume_path(&self) -> Result<&str> {
        match &self.target {
            FaultTarget::RustfsVolume { path } => Ok(path),
            other => bail!(
                "fault kind {} requires a RustFS volume target, got {:?}",
                self.kind.as_str(),
                other
            ),
        }
    }
}

fn fault_kind_accepts_backend(kind: FaultKind, backend: FaultBackend) -> bool {
    matches!(
        (kind, backend),
        (
            FaultKind::RustfsVolumeIoError,
            FaultBackend::ChaosMeshIoChaos | FaultBackend::MinioWarpWithChaos
        ) | (
            FaultKind::RustfsVolumeLatency
                | FaultKind::RustfsVolumeReadMistake
                | FaultKind::RustfsVolumeEnospc,
            FaultBackend::ChaosMeshIoChaos
        ) | (
            FaultKind::RustfsServerPodKill | FaultKind::RustfsServerPodFailure,
            FaultBackend::ChaosMeshPodChaos
        ) | (
            FaultKind::RustfsServerNetworkPartition
                | FaultKind::RustfsServerNetworkDelay
                | FaultKind::RustfsServerNetworkLoss
                | FaultKind::RustfsServerNetworkCorrupt
                | FaultKind::RustfsServerNetworkDuplicate,
            FaultBackend::ChaosMeshNetworkChaos
        ) | (
            FaultKind::RustfsServerCpuStress | FaultKind::RustfsServerMemoryStress,
            FaultBackend::ChaosMeshStressChaos
        ) | (
            FaultKind::RustfsBlockDeviceFlakey,
            FaultBackend::DeviceMapper
        )
    )
}

fn fault_kind_accepts_selection(kind: FaultKind, selection: FaultSelection) -> bool {
    match kind {
        FaultKind::RustfsVolumeIoError
        | FaultKind::RustfsVolumeLatency
        | FaultKind::RustfsVolumeReadMistake
        | FaultKind::RustfsVolumeEnospc => match selection {
            FaultSelection::Percent(percent) => (1..=100).contains(&percent),
            FaultSelection::FixedTargets(_) => false,
        },
        FaultKind::RustfsServerPodKill
        | FaultKind::RustfsServerPodFailure
        | FaultKind::RustfsServerNetworkPartition
        | FaultKind::RustfsServerNetworkDelay
        | FaultKind::RustfsServerNetworkLoss
        | FaultKind::RustfsServerNetworkCorrupt
        | FaultKind::RustfsServerNetworkDuplicate
        | FaultKind::RustfsServerCpuStress
        | FaultKind::RustfsServerMemoryStress
        | FaultKind::RustfsBlockDeviceFlakey => match selection {
            FaultSelection::FixedTargets(count) => count > 0,
            FaultSelection::Percent(_) => false,
        },
    }
}

fn fault_kind_accepts_target(kind: FaultKind, target: &FaultTarget) -> bool {
    match kind {
        FaultKind::RustfsVolumeIoError
        | FaultKind::RustfsVolumeLatency
        | FaultKind::RustfsVolumeReadMistake
        | FaultKind::RustfsVolumeEnospc => matches!(target, FaultTarget::RustfsVolume { .. }),
        FaultKind::RustfsServerPodKill | FaultKind::RustfsServerPodFailure => {
            matches!(target, FaultTarget::RustfsServerPod)
        }
        FaultKind::RustfsServerNetworkPartition
        | FaultKind::RustfsServerNetworkDelay
        | FaultKind::RustfsServerNetworkLoss
        | FaultKind::RustfsServerNetworkCorrupt
        | FaultKind::RustfsServerNetworkDuplicate => {
            matches!(target, FaultTarget::RustfsServerPeerNetwork)
        }
        FaultKind::RustfsServerCpuStress | FaultKind::RustfsServerMemoryStress => {
            matches!(target, FaultTarget::RustfsServerResource)
        }
        FaultKind::RustfsBlockDeviceFlakey => matches!(target, FaultTarget::DedicatedBlockDevice),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultPlan {
    pub scenario: String,
    pub case_name: &'static str,
    pub workload_mode: FaultWorkloadMode,
    faults: Vec<FaultInjection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultPlanOptions {
    pub rustfs_volume_path: String,
}

impl FaultPlanOptions {
    pub fn from_config(config: &FaultTestConfig) -> Self {
        Self {
            rustfs_volume_path: config.rustfs_volume_path.clone(),
        }
    }
}

impl Default for FaultPlanOptions {
    fn default() -> Self {
        Self {
            rustfs_volume_path: DEFAULT_RUSTFS_DATA_VOLUME.to_string(),
        }
    }
}

impl FaultPlan {
    pub fn new(
        scenario: impl Into<String>,
        case_name: &'static str,
        workload_mode: FaultWorkloadMode,
        faults: Vec<FaultInjection>,
    ) -> Result<Self> {
        ensure!(
            !faults.is_empty(),
            "fault plan must contain at least one fault"
        );
        ensure!(
            faults.len() == 1,
            "composite fault plans require an explicit composition policy before they can be executed safely"
        );

        Ok(Self {
            scenario: scenario.into(),
            case_name,
            workload_mode,
            faults,
        })
    }

    pub fn from_scenario(scenario: &FaultScenario, spec: &FaultScenarioSpec) -> Result<Self> {
        Self::from_scenario_with_options(scenario, spec, FaultPlanOptions::default())
    }

    pub fn from_scenario_with_options(
        scenario: &FaultScenario,
        spec: &FaultScenarioSpec,
        options: FaultPlanOptions,
    ) -> Result<Self> {
        ensure!(
            scenario.name == spec.scenario,
            "fault scenario/spec mismatch: scenario={}, spec={}",
            scenario.name,
            spec.scenario
        );

        let workload_mode = if spec.backend == FaultBackend::MinioWarpWithChaos {
            FaultWorkloadMode::S3MixedWithWarp
        } else {
            FaultWorkloadMode::S3Mixed
        };
        let fault = match scenario.name.as_str() {
            IO_EIO_SCENARIO => volume_fault(
                FaultKind::RustfsVolumeIoError,
                spec,
                scenario,
                &options.rustfs_volume_path,
            )?,
            POD_KILL_ONE_SCENARIO => FaultInjection::new(
                FaultKind::RustfsServerPodKill,
                spec.backend,
                FaultTarget::RustfsServerPod,
                FaultSelection::FixedTargets(1),
                scenario.duration,
            )?,
            POD_FAILURE_SCENARIO => FaultInjection::new(
                FaultKind::RustfsServerPodFailure,
                spec.backend,
                FaultTarget::RustfsServerPod,
                FaultSelection::FixedTargets(1),
                scenario.duration,
            )?,
            NETWORK_PARTITION_ONE_SCENARIO => FaultInjection::new(
                FaultKind::RustfsServerNetworkPartition,
                spec.backend,
                FaultTarget::RustfsServerPeerNetwork,
                FaultSelection::FixedTargets(1),
                scenario.duration,
            )?,
            NETWORK_DELAY_SCENARIO => {
                network_fault(FaultKind::RustfsServerNetworkDelay, spec, scenario)?
            }
            NETWORK_LOSS_SCENARIO => {
                network_fault(FaultKind::RustfsServerNetworkLoss, spec, scenario)?
            }
            NETWORK_CORRUPT_SCENARIO => {
                network_fault(FaultKind::RustfsServerNetworkCorrupt, spec, scenario)?
            }
            NETWORK_DUPLICATE_SCENARIO => {
                network_fault(FaultKind::RustfsServerNetworkDuplicate, spec, scenario)?
            }
            IO_READ_MISTAKE_SCENARIO => volume_fault(
                FaultKind::RustfsVolumeReadMistake,
                spec,
                scenario,
                &options.rustfs_volume_path,
            )?,
            IO_LATENCY_SCENARIO => volume_fault(
                FaultKind::RustfsVolumeLatency,
                spec,
                scenario,
                &options.rustfs_volume_path,
            )?,
            DISK_FULL_SCENARIO => volume_fault(
                FaultKind::RustfsVolumeEnospc,
                spec,
                scenario,
                &options.rustfs_volume_path,
            )?,
            STRESS_CPU_SCENARIO => {
                resource_fault(FaultKind::RustfsServerCpuStress, spec, scenario)?
            }
            STRESS_MEMORY_SCENARIO => {
                resource_fault(FaultKind::RustfsServerMemoryStress, spec, scenario)?
            }
            DM_FLAKEY_SCENARIO => FaultInjection::new(
                FaultKind::RustfsBlockDeviceFlakey,
                spec.backend,
                FaultTarget::DedicatedBlockDevice,
                FaultSelection::FixedTargets(1),
                scenario.duration,
            )?,
            WARP_UNDER_CHAOS_SCENARIO => volume_fault(
                FaultKind::RustfsVolumeIoError,
                spec,
                scenario,
                &options.rustfs_volume_path,
            )?,
            other => bail!("scenario {other:?} has no fault plan mapping"),
        };

        Self::new(
            scenario.name.clone(),
            scenario.case_name,
            workload_mode,
            vec![fault],
        )
    }

    pub fn faults(&self) -> &[FaultInjection] {
        &self.faults
    }

    pub fn required_backends(&self) -> Vec<FaultBackend> {
        let mut backends = Vec::new();
        for fault in &self.faults {
            let backend = fault.backend();
            if !backends.contains(&backend) {
                backends.push(backend);
            }
        }
        backends
    }

    pub fn requires_static_storage(&self) -> bool {
        self.faults
            .iter()
            .any(|fault| fault.backend() == FaultBackend::DeviceMapper)
    }

    pub fn backend_summary(&self) -> String {
        self.required_backends()
            .into_iter()
            .map(|backend| format!("{backend:?}"))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    pub fn target_summary(&self) -> String {
        self.faults
            .iter()
            .map(|fault| {
                format!(
                    "{} via {}",
                    fault.target().summary(),
                    fault.selection().summary()
                )
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

fn volume_fault(
    kind: FaultKind,
    spec: &FaultScenarioSpec,
    scenario: &FaultScenario,
    volume_path: &str,
) -> Result<FaultInjection> {
    FaultInjection::new(
        kind,
        spec.backend,
        FaultTarget::RustfsVolume {
            path: volume_path.to_string(),
        },
        FaultSelection::Percent(scenario.percent),
        scenario.duration,
    )
}

fn network_fault(
    kind: FaultKind,
    spec: &FaultScenarioSpec,
    scenario: &FaultScenario,
) -> Result<FaultInjection> {
    FaultInjection::new(
        kind,
        spec.backend,
        FaultTarget::RustfsServerPeerNetwork,
        FaultSelection::FixedTargets(1),
        scenario.duration,
    )
}

fn resource_fault(
    kind: FaultKind,
    spec: &FaultScenarioSpec,
    scenario: &FaultScenario,
) -> Result<FaultInjection> {
    FaultInjection::new(
        kind,
        spec.backend,
        FaultTarget::RustfsServerResource,
        FaultSelection::FixedTargets(1),
        scenario.duration,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_RUSTFS_DATA_VOLUME, FaultInjection, FaultKind, FaultPlan, FaultSelection,
        FaultTarget, FaultWorkloadMode,
    };
    use crate::fault::{
        config::FaultTestConfig,
        scenarios::{
            FaultBackend, FaultScenario, WARP_UNDER_CHAOS_SCENARIO, scenario_catalog, scenario_spec,
        },
    };
    use std::time::Duration;

    #[test]
    fn scenario_plan_maps_io_eio_to_rustfs_volume_fault() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let scenario = FaultScenario::from_config(&config).expect("scenario");
        let spec = scenario_spec(&scenario.name).expect("spec");

        let plan = FaultPlan::from_scenario(&scenario, spec).expect("plan");

        assert_eq!(plan.workload_mode, FaultWorkloadMode::S3Mixed);
        assert_eq!(
            plan.required_backends(),
            vec![FaultBackend::ChaosMeshIoChaos]
        );
        assert_eq!(plan.faults().len(), 1);
        assert_eq!(plan.faults()[0].kind(), FaultKind::RustfsVolumeIoError);
        assert_eq!(
            plan.faults()[0].target(),
            &FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME.to_string()
            }
        );
    }

    #[test]
    fn warp_scenario_keeps_performance_mode_out_of_fault_kind() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        config.scenario = WARP_UNDER_CHAOS_SCENARIO.to_string();
        let scenario = FaultScenario::from_config(&config).expect("scenario");
        let spec = scenario_spec(&scenario.name).expect("spec");

        let plan = FaultPlan::from_scenario(&scenario, spec).expect("plan");

        assert!(plan.workload_mode.runs_warp());
        assert_eq!(plan.faults()[0].kind(), FaultKind::RustfsVolumeIoError);
        assert_eq!(
            plan.required_backends(),
            vec![FaultBackend::MinioWarpWithChaos]
        );
    }

    #[test]
    fn every_cataloged_scenario_has_one_current_fault_plan() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");

        for spec in scenario_catalog() {
            config.scenario = spec.scenario.to_string();
            let scenario = FaultScenario::from_config(&config).expect("scenario");
            let plan = FaultPlan::from_scenario(&scenario, spec).expect("plan");

            assert_eq!(
                plan.faults().len(),
                1,
                "{} should remain an independent single-fault scenario",
                spec.scenario
            );
        }
    }

    #[test]
    fn plan_rejects_multi_faults_without_composition_policy() {
        let first = FaultInjection::new(
            FaultKind::RustfsVolumeIoError,
            FaultBackend::ChaosMeshIoChaos,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME.to_string(),
            },
            FaultSelection::Percent(20),
            Duration::from_secs(60),
        )
        .expect("first fault");
        let second = FaultInjection::new(
            FaultKind::RustfsServerNetworkPartition,
            FaultBackend::ChaosMeshNetworkChaos,
            FaultTarget::RustfsServerPeerNetwork,
            FaultSelection::FixedTargets(1),
            Duration::from_secs(60),
        )
        .expect("second fault");

        let result = FaultPlan::new(
            "composite",
            "fault_composite",
            FaultWorkloadMode::S3Mixed,
            vec![first, second],
        );

        assert!(result.is_err());
    }

    #[test]
    fn fault_injection_rejects_backend_kind_mismatch() {
        let result = FaultInjection::new(
            FaultKind::RustfsVolumeIoError,
            FaultBackend::ChaosMeshNetworkChaos,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME.to_string(),
            },
            FaultSelection::Percent(20),
            Duration::from_secs(60),
        );

        assert!(result.is_err());
    }

    #[test]
    fn fixed_target_faults_reject_percent_selection() {
        let result = FaultInjection::new(
            FaultKind::RustfsServerPodKill,
            FaultBackend::ChaosMeshPodChaos,
            FaultTarget::RustfsServerPod,
            FaultSelection::Percent(20),
            Duration::from_secs(60),
        );

        assert!(result.is_err());
    }

    #[test]
    fn fault_injection_rejects_kind_target_mismatch() {
        let result = FaultInjection::new(
            FaultKind::RustfsServerPodKill,
            FaultBackend::ChaosMeshPodChaos,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME.to_string(),
            },
            FaultSelection::FixedTargets(1),
            Duration::from_secs(60),
        );

        assert!(result.is_err());
    }
}
