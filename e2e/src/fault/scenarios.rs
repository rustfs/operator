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

use anyhow::{Result, ensure};
use serde::Serialize;
use std::time::Duration;

use crate::fault::config::FaultTestConfig;

pub const IO_EIO_SCENARIO: &str = "io-eio";
pub const POD_KILL_ONE_SCENARIO: &str = "pod-kill-one";
pub const NETWORK_PARTITION_ONE_SCENARIO: &str = "network-partition-one";
pub const NETWORK_DELAY_SCENARIO: &str = "network-delay";
pub const NETWORK_LOSS_SCENARIO: &str = "network-loss";
pub const NETWORK_CORRUPT_SCENARIO: &str = "network-corrupt";
pub const NETWORK_DUPLICATE_SCENARIO: &str = "network-duplicate";
pub const IO_READ_MISTAKE_SCENARIO: &str = "io-read-mistake";
pub const IO_LATENCY_SCENARIO: &str = "io-latency";
pub const DISK_FULL_SCENARIO: &str = "disk-full";
pub const POD_FAILURE_SCENARIO: &str = "pod-failure";
pub const STRESS_CPU_SCENARIO: &str = "stress-cpu";
pub const STRESS_MEMORY_SCENARIO: &str = "stress-memory";
pub const DM_FLAKEY_SCENARIO: &str = "dm-flakey";
pub const WARP_UNDER_CHAOS_SCENARIO: &str = "warp-under-chaos";

const IOCHAOS_CRD: &str = "iochaos.chaos-mesh.org";
const PODCHAOS_CRD: &str = "podchaos.chaos-mesh.org";
const NETWORKCHAOS_CRD: &str = "networkchaos.chaos-mesh.org";
const STRESSCHAOS_CRD: &str = "stresschaos.chaos-mesh.org";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultScenarioStatus {
    Executable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultPriority {
    P0,
    P1,
    P2,
    P3,
}

impl FaultPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultBackend {
    ChaosMeshIoChaos,
    ChaosMeshPodChaos,
    ChaosMeshNetworkChaos,
    ChaosMeshStressChaos,
    DeviceMapper,
    MinioWarpWithChaos,
}

impl FaultBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChaosMeshIoChaos => "chaos-mesh-io-chaos",
            Self::ChaosMeshPodChaos => "chaos-mesh-pod-chaos",
            Self::ChaosMeshNetworkChaos => "chaos-mesh-network-chaos",
            Self::ChaosMeshStressChaos => "chaos-mesh-stress-chaos",
            Self::DeviceMapper => "device-mapper",
            Self::MinioWarpWithChaos => "minio-warp-with-chaos",
        }
    }

    pub fn accepts_percent(self) -> bool {
        matches!(self, Self::ChaosMeshIoChaos | Self::MinioWarpWithChaos)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultIsolation {
    FreshTenant,
    ReusableTenant,
    DedicatedLinuxBlockDevice,
}

impl FaultIsolation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreshTenant => "fresh-tenant",
            Self::ReusableTenant => "reusable-tenant",
            Self::DedicatedLinuxBlockDevice => "dedicated-linux-block-device",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultImpactPolicy {
    ClientDisruptionRequired,
    ClientDisruptionOptional,
}

impl FaultImpactPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientDisruptionRequired => "client-disruption-required",
            Self::ClientDisruptionOptional => "client-disruption-optional",
        }
    }

    pub fn requires_client_disruption(self) -> bool {
        matches!(self, Self::ClientDisruptionRequired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FaultScenarioSpec {
    pub scenario: &'static str,
    pub case_name: &'static str,
    pub description: &'static str,
    pub priority: FaultPriority,
    pub backend: FaultBackend,
    pub status: FaultScenarioStatus,
    pub isolation: FaultIsolation,
    pub crds: &'static [&'static str],
    pub required_tools: &'static [&'static str],
    pub percent_supported: bool,
    pub impact_policy: FaultImpactPolicy,
    pub boundary: &'static str,
    pub ci_phase: &'static str,
    pub target: &'static str,
    pub validation: &'static str,
    pub observability: &'static str,
    pub conflict_domain: &'static str,
}

impl FaultScenarioSpec {
    pub fn requires_static_storage(self) -> bool {
        self.isolation == FaultIsolation::DedicatedLinuxBlockDevice
    }

    pub fn requires_chaos_mesh(self) -> bool {
        !self.crds.is_empty()
    }
}

pub const FAULT_SCENARIO_CATALOG: &[FaultScenarioSpec] = &[
    FaultScenarioSpec {
        scenario: IO_EIO_SCENARIO,
        case_name: "fault_io_eio_preserves_committed_objects",
        description: "Inject Chaos Mesh IOChaos EIO into one RustFS data volume and verify committed S3 objects remain readable with matching hashes after recovery.",
        priority: FaultPriority::P0,
        backend: FaultBackend::ChaosMeshIoChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::FreshTenant,
        crds: &[IOCHAOS_CRD],
        required_tools: &[],
        percent_supported: true,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/fault-injection",
        ci_phase: "faults",
        target: "one RustFS container data volume selected by tenant label and configured RustFS volume path",
        validation: "prefill succeeds before injection, mixed PUT/GET workload runs while IOChaos is active, committed PUTs are GET+sha256 verified after recovery, and successful GETs cannot return corrupt bytes",
        observability: "history.jsonl, workload-summary.json, checker-report.json, chaos-manifest.yaml, chaos-describe*.txt, Kubernetes snapshot artifacts",
        conflict_domain: "fresh Tenant/PVC/PV fixture and run-scoped IOChaos cleanup",
    },
    FaultScenarioSpec {
        scenario: POD_KILL_ONE_SCENARIO,
        case_name: "fault_pod_kill_one_preserves_committed_objects",
        description: "Inject Chaos Mesh PodChaos against one RustFS Pod and verify StatefulSet recovery preserves committed S3 objects.",
        priority: FaultPriority::P0,
        backend: FaultBackend::ChaosMeshPodChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[PODCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/pod-recovery",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label",
        validation: "the killed Pod is recreated, Tenant returns Ready, committed PUTs remain readable with matching hashes, and failed or unknown operations are recorded without becoming correctness failures",
        observability: "history.jsonl, workload-summary.json, checker-report.json, podchaos manifest/describe/yaml, Pod restart counts, current and previous RustFS logs",
        conflict_domain: "run-scoped PodChaos resource and one target Pod; can reuse a ready Tenant after the prior scenario has cleaned up",
    },
    FaultScenarioSpec {
        scenario: NETWORK_PARTITION_ONE_SCENARIO,
        case_name: "fault_network_partition_one_preserves_committed_objects",
        description: "Inject Chaos Mesh NetworkChaos that partitions one RustFS Pod from its peers and verify recovery does not lose or corrupt committed objects.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshNetworkChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[NETWORKCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/network-partition",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with peer traffic disrupted inside the e2e namespace",
        validation: "network disruption is active during workload, successful reads never return wrong hashes, committed PUTs remain readable after heal, and Tenant recovers Ready",
        observability: "history.jsonl, workload-summary.json, checker-report.json, networkchaos manifest/describe/yaml, endpoints, events, and RustFS logs",
        conflict_domain: "run-scoped NetworkChaos resource; must not overlap with PodChaos or IOChaos in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: NETWORK_DELAY_SCENARIO,
        case_name: "fault_network_delay_preserves_object_model",
        description: "Inject NetworkChaos delay into one RustFS Pod peer path and verify the S3 object model remains explainable.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshNetworkChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[NETWORKCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/network-delay",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with delayed peer traffic inside the e2e namespace",
        validation: "successful reads match a committed value, stable live keys are listed, and recovery preserves the object model",
        observability: "history.jsonl, checker reports, networkchaos manifest/describe/yaml, endpoints, events, and RustFS logs",
        conflict_domain: "run-scoped NetworkChaos resource; must not overlap with other network faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: NETWORK_LOSS_SCENARIO,
        case_name: "fault_network_loss_preserves_object_model",
        description: "Inject NetworkChaos packet loss into one RustFS Pod peer path and verify object-model correctness after recovery.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshNetworkChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[NETWORKCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/network-loss",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with lossy peer traffic inside the e2e namespace",
        validation: "successful reads match a committed value, failed operations are explainable, and recovery preserves the object model",
        observability: "history.jsonl, checker reports, networkchaos manifest/describe/yaml, endpoints, events, and RustFS logs",
        conflict_domain: "run-scoped NetworkChaos resource; must not overlap with other network faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: NETWORK_CORRUPT_SCENARIO,
        case_name: "fault_network_corrupt_preserves_object_model",
        description: "Inject NetworkChaos packet corruption into one RustFS Pod peer path and verify successful S3 reads never return corrupt bytes.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshNetworkChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[NETWORKCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/network-corrupt",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with corrupted peer traffic inside the e2e namespace",
        validation: "successful reads match a committed value and recovery preserves the object model",
        observability: "history.jsonl, checker reports, networkchaos manifest/describe/yaml, endpoints, events, and RustFS logs",
        conflict_domain: "run-scoped NetworkChaos resource; must not overlap with other network faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: NETWORK_DUPLICATE_SCENARIO,
        case_name: "fault_network_duplicate_preserves_object_model",
        description: "Inject NetworkChaos packet duplication into one RustFS Pod peer path and verify object-model correctness after recovery.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshNetworkChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[NETWORKCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/network-duplicate",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with duplicated peer traffic inside the e2e namespace",
        validation: "successful reads match a committed value and recovery preserves the object model",
        observability: "history.jsonl, checker reports, networkchaos manifest/describe/yaml, endpoints, events, and RustFS logs",
        conflict_domain: "run-scoped NetworkChaos resource; must not overlap with other network faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: IO_READ_MISTAKE_SCENARIO,
        case_name: "fault_io_read_mistake_rejects_corrupt_reads",
        description: "Inject Chaos Mesh IOChaos mistake on RustFS read paths and verify RustFS never returns corrupt object bytes as successful S3 reads.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshIoChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::FreshTenant,
        crds: &[IOCHAOS_CRD],
        required_tools: &[],
        percent_supported: true,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/data-integrity",
        ci_phase: "faults",
        target: "one RustFS data volume read path selected by tenant label and configured RustFS volume path",
        validation: "successful GET responses must match the committed hash; RustFS may fail or repair reads but must not return wrong bytes with a successful status",
        observability: "history.jsonl, checker-report.json with successful_corrupted_reads, iochaos manifest/describe/yaml, RustFS logs, events",
        conflict_domain: "fresh Tenant/PVC/PV fixture and run-scoped IOChaos mistake resource",
    },
    FaultScenarioSpec {
        scenario: IO_LATENCY_SCENARIO,
        case_name: "fault_io_latency_preserves_object_model",
        description: "Inject Chaos Mesh IOChaos latency on RustFS data paths and verify delayed storage does not corrupt the S3 object model.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshIoChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::FreshTenant,
        crds: &[IOCHAOS_CRD],
        required_tools: &[],
        percent_supported: true,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/storage-latency",
        ci_phase: "faults",
        target: "one RustFS data volume selected by tenant label with READ/WRITE operations delayed",
        validation: "successful reads match a committed value, timed out operations remain explainable, and recovery preserves the object model",
        observability: "history.jsonl, checker reports, iochaos manifest/describe/yaml, RustFS logs, events",
        conflict_domain: "fresh Tenant/PVC/PV fixture and run-scoped IOChaos latency resource",
    },
    FaultScenarioSpec {
        scenario: DISK_FULL_SCENARIO,
        case_name: "fault_disk_full_preserves_committed_objects",
        description: "Inject ENOSPC on writes to one RustFS data volume and verify committed objects survive storage pressure and recovery.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshIoChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::FreshTenant,
        crds: &[IOCHAOS_CRD],
        required_tools: &[],
        percent_supported: true,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/storage-pressure",
        ci_phase: "faults",
        target: "one RustFS data volume selected by tenant label with WRITE operations returning ENOSPC",
        validation: "new writes may fail with ENOSPC, but previously committed PUTs remain readable after IOChaos recovery",
        observability: "history.jsonl, checker-report.json, fault-evidence.json, IOChaos manifest/status, events, RustFS logs",
        conflict_domain: "fresh Tenant/PVC/PV fixture and run-scoped IOChaos cleanup without consuming node disk capacity",
    },
    FaultScenarioSpec {
        scenario: POD_FAILURE_SCENARIO,
        case_name: "fault_pod_failure_preserves_object_model",
        description: "Inject Chaos Mesh PodChaos pod-failure against one RustFS Pod and verify object-model correctness after recovery.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshPodChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[PODCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/pod-failure",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label and failed for the scenario duration",
        validation: "the failed Pod recovers, Tenant returns Ready, and the S3 object model remains explainable",
        observability: "history.jsonl, checker reports, podchaos manifest/describe/yaml, Pod restart counts, current and previous RustFS logs",
        conflict_domain: "run-scoped PodChaos resource and one target Pod; can reuse a ready Tenant after the prior scenario has cleaned up",
    },
    FaultScenarioSpec {
        scenario: STRESS_CPU_SCENARIO,
        case_name: "fault_stress_cpu_preserves_object_model",
        description: "Inject Chaos Mesh CPU StressChaos into one RustFS Pod and verify object-model correctness under resource pressure.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshStressChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[STRESSCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/cpu-pressure",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with CPU stressors",
        validation: "successful reads match a committed value and recovery preserves the object model",
        observability: "history.jsonl, checker reports, stresschaos manifest/describe/yaml, metrics-adjacent Kubernetes snapshots, events, and RustFS logs",
        conflict_domain: "run-scoped StressChaos resource; should not overlap with other stress faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: STRESS_MEMORY_SCENARIO,
        case_name: "fault_stress_memory_preserves_object_model",
        description: "Inject Chaos Mesh memory StressChaos into one RustFS Pod and verify object-model correctness under memory pressure.",
        priority: FaultPriority::P1,
        backend: FaultBackend::ChaosMeshStressChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::ReusableTenant,
        crds: &[STRESSCHAOS_CRD],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/memory-pressure",
        ci_phase: "faults",
        target: "one RustFS Pod selected by tenant label with memory stressors",
        validation: "successful reads match a committed value and recovery preserves the object model",
        observability: "history.jsonl, checker reports, stresschaos manifest/describe/yaml, metrics-adjacent Kubernetes snapshots, events, and RustFS logs",
        conflict_domain: "run-scoped StressChaos resource; should not overlap with other stress faults in the same Tenant",
    },
    FaultScenarioSpec {
        scenario: DM_FLAKEY_SCENARIO,
        case_name: "fault_dm_flakey_preserves_committed_objects",
        description: "Use a device-mapper flakey or error target for a dedicated test volume and verify RustFS handles block-device instability without data corruption.",
        priority: FaultPriority::P3,
        backend: FaultBackend::DeviceMapper,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::DedicatedLinuxBlockDevice,
        crds: &[],
        required_tools: &[],
        percent_supported: false,
        impact_policy: FaultImpactPolicy::ClientDisruptionRequired,
        boundary: "rustfs-workload/block-device-fault",
        ci_phase: "faults",
        target: "one dedicated Linux block-device-backed PV used only by the e2e Tenant",
        validation: "committed objects remain readable after the device fault is removed, and successful reads never return corrupt bytes",
        observability: "history.jsonl, checker-report.json, dmsetup table/status, kernel logs, PV mapping, events, RustFS logs",
        conflict_domain: "dedicated Linux runner or lab host with an explicitly assigned block device; never part of shared test storage",
    },
    FaultScenarioSpec {
        scenario: WARP_UNDER_CHAOS_SCENARIO,
        case_name: "fault_warp_under_chaos_reports_performance_separately",
        description: "Run MinIO Warp during a selected chaos scenario while keeping performance output separate from the correctness verdict.",
        priority: FaultPriority::P3,
        backend: FaultBackend::MinioWarpWithChaos,
        status: FaultScenarioStatus::Executable,
        isolation: FaultIsolation::FreshTenant,
        crds: &[IOCHAOS_CRD],
        required_tools: &["warp"],
        percent_supported: true,
        impact_policy: FaultImpactPolicy::ClientDisruptionOptional,
        boundary: "rustfs-workload/performance-under-chaos",
        ci_phase: "faults",
        target: "RustFS S3 endpoint under an explicitly selected fault backend",
        validation: "Warp throughput or latency changes are reported separately; correctness still comes only from history and checker reports",
        observability: "warp report, history.jsonl, checker-report.json, selected chaos manifest/describe/yaml, RustFS logs",
        conflict_domain: "performance-only run with isolated bucket prefix and no shared correctness threshold",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultScenario {
    pub name: String,
    pub case_name: &'static str,
    pub duration: Duration,
    pub percent: u8,
    pub object_count: usize,
}

impl FaultScenario {
    pub fn from_config(config: &FaultTestConfig) -> Result<Self> {
        let spec = scenario_spec(&config.scenario)?;
        ensure!(
            spec.status == FaultScenarioStatus::Executable,
            "fault scenario {:?} is cataloged as {:?} but is not executable yet; case {}, backend {:?}, validation: {}",
            config.scenario,
            spec.status,
            spec.case_name,
            spec.backend,
            spec.validation
        );
        ensure!(
            (1..=100).contains(&config.percent),
            "RUSTFS_FAULT_TEST_PERCENT must be in 1..=100, got {}",
            config.percent
        );
        ensure!(
            config.duration > Duration::ZERO,
            "RUSTFS_FAULT_TEST_DURATION_SECONDS must be greater than zero"
        );
        config.workload.validate()?;
        ensure!(
            !config.percent_overridden || spec.percent_supported,
            "RUSTFS_FAULT_TEST_PERCENT only applies to percent-based IOChaos scenarios; scenario {:?} targets {:?} with a fixed target count",
            spec.scenario,
            spec.backend
        );

        Ok(Self {
            name: spec.scenario.to_string(),
            case_name: spec.case_name,
            duration: config.duration,
            percent: config.percent,
            object_count: config.workload.object_count,
        })
    }

    pub fn prefill_count(&self) -> usize {
        self.object_count / 2
    }

    pub fn mixed_workload_count(&self) -> usize {
        self.object_count - self.prefill_count()
    }
}

pub fn scenario_catalog() -> &'static [FaultScenarioSpec] {
    FAULT_SCENARIO_CATALOG
}

pub fn scenario_catalog_json() -> Result<String> {
    Ok(serde_json::to_string_pretty(scenario_catalog())?)
}

pub fn scenario_spec(name: &str) -> Result<&'static FaultScenarioSpec> {
    FAULT_SCENARIO_CATALOG
        .iter()
        .find(|scenario| scenario.scenario == name)
        .ok_or_else(|| {
            let supported = FAULT_SCENARIO_CATALOG
                .iter()
                .map(|scenario| scenario.scenario)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("unsupported fault scenario {name:?}; catalog contains: {supported}")
        })
}

#[cfg(test)]
mod tests {
    use super::{
        FaultScenario, FaultScenarioStatus, IO_EIO_SCENARIO, POD_KILL_ONE_SCENARIO,
        scenario_catalog, scenario_catalog_json,
    };
    use crate::fault::config::{FaultTestConfig, FaultWorkloadProfile};
    use std::time::Duration;

    #[test]
    fn default_fault_scenario_is_io_eio_with_split_workload() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let scenario = FaultScenario::from_config(&config).expect("valid scenario");

        assert_eq!(scenario.name, IO_EIO_SCENARIO);
        assert_eq!(
            scenario.case_name,
            "fault_io_eio_preserves_committed_objects"
        );
        assert_eq!(scenario.duration, Duration::from_secs(7200));
        assert_eq!(scenario.percent, 20);
        assert_eq!(scenario.prefill_count(), 20000);
        assert_eq!(scenario.mixed_workload_count(), 20000);
    }

    #[test]
    fn unsupported_fault_scenario_is_rejected() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        config.scenario = "operator-restart".to_string();

        assert!(FaultScenario::from_config(&config).is_err());
    }

    #[test]
    fn workload_concurrency_must_fit_the_object_count() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        config.workload = FaultWorkloadProfile {
            object_count: 4,
            concurrency: 5,
        };

        assert!(FaultScenario::from_config(&config).is_err());
    }

    #[test]
    fn fixed_target_scenarios_reject_percent_override() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        config.scenario = POD_KILL_ONE_SCENARIO.to_string();
        config.percent = 50;
        config.percent_overridden = true;

        assert!(FaultScenario::from_config(&config).is_err());
    }

    #[test]
    fn all_cataloged_fault_scenarios_are_executable() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");

        for spec in scenario_catalog() {
            config.scenario = spec.scenario.to_string();

            assert_eq!(spec.status, FaultScenarioStatus::Executable);
            assert!(
                FaultScenario::from_config(&config).is_ok(),
                "{} should be selectable through the real-cluster fault-test entrypoint",
                spec.scenario
            );
        }

        assert_eq!(scenario_catalog().len(), 15);
    }

    #[test]
    fn fault_scenario_catalog_has_unique_clear_and_observable_cases() {
        let mut names = std::collections::HashSet::new();
        let mut case_names = std::collections::HashSet::new();

        for scenario in scenario_catalog() {
            assert!(names.insert(scenario.scenario));
            assert!(case_names.insert(scenario.case_name));
            assert!(!scenario.description.is_empty());
            assert_eq!(
                scenario.percent_supported,
                scenario.backend.accepts_percent()
            );
            assert!(!scenario.boundary.is_empty());
            assert!(!scenario.ci_phase.is_empty());
            assert!(!scenario.target.is_empty());
            assert!(!scenario.validation.is_empty());
            assert!(!scenario.observability.is_empty());
            assert!(!scenario.conflict_domain.is_empty());
        }
    }

    #[test]
    fn catalog_exports_machine_readable_json() {
        let json = scenario_catalog_json().expect("catalog json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        assert!(value.as_array().expect("array").len() >= 10);
        assert!(json.contains("\"scenario\": \"io-eio\""));
        assert!(json.contains("\"crds\""));
        assert!(json.contains("\"impact_policy\""));
    }
}
