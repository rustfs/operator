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

use crate::framework::fault_scenarios::{
    DISK_FULL_SCENARIO, DM_FLAKEY_SCENARIO, FaultBackend, FaultScenario, FaultScenarioSpec,
    IO_EIO_SCENARIO, IO_READ_MISTAKE_SCENARIO, NETWORK_PARTITION_ONE_SCENARIO,
    POD_KILL_ONE_SCENARIO, WARP_UNDER_CHAOS_SCENARIO,
};

pub const DEFAULT_RUSTFS_DATA_VOLUME: &str = "/data/rustfs0";

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
    RustfsVolumeReadMistake,
    RustfsVolumeEnospc,
    RustfsServerPodKill,
    RustfsServerNetworkPartition,
    RustfsBlockDeviceFlakey,
}

impl FaultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RustfsVolumeIoError => "rustfs_volume_io_error",
            Self::RustfsVolumeReadMistake => "rustfs_volume_read_mistake",
            Self::RustfsVolumeEnospc => "rustfs_volume_enospc",
            Self::RustfsServerPodKill => "rustfs_server_pod_kill",
            Self::RustfsServerNetworkPartition => "rustfs_server_network_partition",
            Self::RustfsBlockDeviceFlakey => "rustfs_block_device_flakey",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultTarget {
    RustfsVolume { path: &'static str },
    RustfsServerPod,
    RustfsServerPeerNetwork,
    DedicatedBlockDevice,
}

impl FaultTarget {
    pub fn summary(self) -> String {
        match self {
            Self::RustfsVolume { path } => format!("one RustFS volume at {path}"),
            Self::RustfsServerPod => "one RustFS server Pod".to_string(),
            Self::RustfsServerPeerNetwork => {
                "one RustFS server Pod partitioned from its peers".to_string()
            }
            Self::DedicatedBlockDevice => "one dedicated block-device-backed PV".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultInjection {
    pub kind: FaultKind,
    pub backend: FaultBackend,
    pub target: FaultTarget,
    pub percent: u8,
    pub duration: Duration,
}

impl FaultInjection {
    pub fn new(
        kind: FaultKind,
        backend: FaultBackend,
        target: FaultTarget,
        percent: u8,
        duration: Duration,
    ) -> Result<Self> {
        ensure!(
            (1..=100).contains(&percent),
            "fault percent must be in 1..=100, got {percent}"
        );
        ensure!(duration > Duration::ZERO, "fault duration must be positive");

        Ok(Self {
            kind,
            backend,
            target,
            percent,
            duration,
        })
    }

    pub fn rustfs_volume_path(&self) -> Result<&'static str> {
        match self.target {
            FaultTarget::RustfsVolume { path } => Ok(path),
            other => bail!(
                "fault kind {} requires a RustFS volume target, got {:?}",
                self.kind.as_str(),
                other
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultPlan {
    pub scenario: String,
    pub case_name: &'static str,
    pub workload_mode: FaultWorkloadMode,
    faults: Vec<FaultInjection>,
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

        Ok(Self {
            scenario: scenario.into(),
            case_name,
            workload_mode,
            faults,
        })
    }

    pub fn from_scenario(scenario: &FaultScenario, spec: &FaultScenarioSpec) -> Result<Self> {
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
            IO_EIO_SCENARIO => volume_fault(FaultKind::RustfsVolumeIoError, spec, scenario)?,
            POD_KILL_ONE_SCENARIO => FaultInjection::new(
                FaultKind::RustfsServerPodKill,
                spec.backend,
                FaultTarget::RustfsServerPod,
                scenario.percent,
                scenario.duration,
            )?,
            NETWORK_PARTITION_ONE_SCENARIO => FaultInjection::new(
                FaultKind::RustfsServerNetworkPartition,
                spec.backend,
                FaultTarget::RustfsServerPeerNetwork,
                scenario.percent,
                scenario.duration,
            )?,
            IO_READ_MISTAKE_SCENARIO => {
                volume_fault(FaultKind::RustfsVolumeReadMistake, spec, scenario)?
            }
            DISK_FULL_SCENARIO => volume_fault(FaultKind::RustfsVolumeEnospc, spec, scenario)?,
            DM_FLAKEY_SCENARIO => FaultInjection::new(
                FaultKind::RustfsBlockDeviceFlakey,
                spec.backend,
                FaultTarget::DedicatedBlockDevice,
                scenario.percent,
                scenario.duration,
            )?,
            WARP_UNDER_CHAOS_SCENARIO => {
                volume_fault(FaultKind::RustfsVolumeIoError, spec, scenario)?
            }
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
            if !backends.contains(&fault.backend) {
                backends.push(fault.backend);
            }
        }
        backends
    }

    pub fn requires_static_storage(&self) -> bool {
        self.faults
            .iter()
            .any(|fault| fault.backend == FaultBackend::DeviceMapper)
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
            .map(|fault| fault.target.summary())
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

fn volume_fault(
    kind: FaultKind,
    spec: &FaultScenarioSpec,
    scenario: &FaultScenario,
) -> Result<FaultInjection> {
    FaultInjection::new(
        kind,
        spec.backend,
        FaultTarget::RustfsVolume {
            path: DEFAULT_RUSTFS_DATA_VOLUME,
        },
        scenario.percent,
        scenario.duration,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_RUSTFS_DATA_VOLUME, FaultInjection, FaultKind, FaultPlan, FaultTarget,
        FaultWorkloadMode,
    };
    use crate::framework::{
        fault_config::FaultTestConfig,
        fault_scenarios::{
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
        assert_eq!(plan.faults()[0].kind, FaultKind::RustfsVolumeIoError);
        assert_eq!(
            plan.faults()[0].target,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME
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
        assert_eq!(plan.faults()[0].kind, FaultKind::RustfsVolumeIoError);
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
    fn plan_contract_allows_multiple_faults_for_future_composition() {
        let first = FaultInjection::new(
            FaultKind::RustfsVolumeIoError,
            FaultBackend::ChaosMeshIoChaos,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME,
            },
            20,
            Duration::from_secs(60),
        )
        .expect("first fault");
        let second = FaultInjection::new(
            FaultKind::RustfsServerNetworkPartition,
            FaultBackend::ChaosMeshNetworkChaos,
            FaultTarget::RustfsServerPeerNetwork,
            100,
            Duration::from_secs(60),
        )
        .expect("second fault");

        let plan = FaultPlan::new(
            "composite",
            "fault_composite",
            FaultWorkloadMode::S3Mixed,
            vec![first, second],
        )
        .expect("composite plan");

        assert_eq!(plan.faults().len(), 2);
        assert_eq!(
            plan.required_backends(),
            vec![
                FaultBackend::ChaosMeshIoChaos,
                FaultBackend::ChaosMeshNetworkChaos
            ]
        );
        assert!(plan.target_summary().contains(" + "));
    }
}
