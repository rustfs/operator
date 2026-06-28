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
use serde::{Deserialize, Serialize};

use crate::fault::{
    config::FaultTestConfig,
    plan::{FaultInjection, FaultPlan, FaultSelection, FaultTarget, FaultWorkloadMode},
    scenarios::{FaultScenario, FaultScenarioSpec},
    workload::WorkloadPlan,
};

pub const FAULT_RUN_API_VERSION: &str = "rustfs.com/fault-test/v1alpha1";
pub const FAULT_RUN_KIND: &str = "FaultRun";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunSpec {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: FaultRunMetadata,
    pub cluster: FaultRunClusterSpec,
    pub scenario: FaultRunScenarioSpec,
    pub workload: FaultRunWorkloadSpec,
    pub recovery: FaultRunRecoverySpec,
    pub faults: Vec<FaultRunFaultSpec>,
    pub artifacts: FaultRunArtifactSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunMetadata {
    pub name: String,
    pub run_id: String,
    pub bucket: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunClusterSpec {
    pub context: String,
    pub namespace: String,
    pub tenant: String,
    pub storage_class: String,
    pub rustfs_image: String,
    pub chaos_namespace: String,
    pub use_cluster_ip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunScenarioSpec {
    pub name: String,
    pub case_name: String,
    pub priority: String,
    pub isolation: String,
    pub impact_policy: String,
    pub boundary: String,
    pub validation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunWorkloadSpec {
    pub mode: String,
    pub object_count: usize,
    pub concurrency: usize,
    pub prefill_concurrency: usize,
    pub request_timeout_seconds: u64,
    pub seed: u64,
    pub plan: WorkloadPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunRecoverySpec {
    pub timeout_seconds: u64,
    pub expected_rustfs_pod_count: usize,
    pub stable_pod_window_seconds: u64,
    pub recommit_unconfirmed_writes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunFaultSpec {
    pub name: String,
    pub kind: String,
    pub backend: String,
    pub target: FaultRunTargetSpec,
    pub selection: FaultRunSelectionSpec,
    pub duration_seconds: u64,
    pub observability: String,
    pub conflict_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunTargetSpec {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunSelectionSpec {
    pub kind: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultRunArtifactSpec {
    pub required: Vec<String>,
    pub event_stream: String,
}

impl FaultRunSpec {
    pub fn resolved(
        config: &FaultTestConfig,
        scenario: &FaultScenario,
        scenario_spec: &FaultScenarioSpec,
        plan: &FaultPlan,
        workload_plan: &WorkloadPlan,
        run_id: &str,
        bucket: &str,
    ) -> Self {
        Self {
            api_version: FAULT_RUN_API_VERSION.to_string(),
            kind: FAULT_RUN_KIND.to_string(),
            metadata: FaultRunMetadata {
                name: scenario.case_name.to_string(),
                run_id: run_id.to_string(),
                bucket: bucket.to_string(),
            },
            cluster: FaultRunClusterSpec {
                context: config.cluster.context.clone(),
                namespace: config.cluster.test_namespace.clone(),
                tenant: config.cluster.tenant_name.clone(),
                storage_class: config.cluster.storage_class.clone(),
                rustfs_image: config.cluster.rustfs_image.clone(),
                chaos_namespace: config.chaos_namespace.clone(),
                use_cluster_ip: config.use_cluster_ip,
            },
            scenario: FaultRunScenarioSpec {
                name: scenario.name.clone(),
                case_name: scenario.case_name.to_string(),
                priority: scenario_spec.priority.as_str().to_string(),
                isolation: scenario_spec.isolation.as_str().to_string(),
                impact_policy: scenario_spec.impact_policy.as_str().to_string(),
                boundary: scenario_spec.boundary.to_string(),
                validation: scenario_spec.validation.to_string(),
            },
            workload: FaultRunWorkloadSpec {
                mode: workload_mode_name(plan.workload_mode).to_string(),
                object_count: workload_plan.object_count,
                concurrency: workload_plan.concurrency,
                prefill_concurrency: config.prefill_concurrency,
                request_timeout_seconds: config.request_timeout.as_secs(),
                seed: workload_plan.seed,
                plan: workload_plan.clone(),
            },
            recovery: FaultRunRecoverySpec {
                timeout_seconds: config.cluster.timeout.as_secs(),
                expected_rustfs_pod_count: config.expected_rustfs_pod_count,
                stable_pod_window_seconds: config.rustfs_pod_stable_window.as_secs(),
                recommit_unconfirmed_writes: true,
            },
            faults: plan
                .faults()
                .iter()
                .enumerate()
                .map(|(index, fault)| {
                    FaultRunFaultSpec::from_fault(index, scenario, scenario_spec, fault)
                })
                .collect(),
            artifacts: FaultRunArtifactSpec::default(),
        }
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(serde_yaml_ng::to_string(self)?)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

impl FaultRunFaultSpec {
    fn from_fault(
        index: usize,
        scenario: &FaultScenario,
        scenario_spec: &FaultScenarioSpec,
        fault: &FaultInjection,
    ) -> Self {
        Self {
            name: format!("{}-{:02}-{}", scenario.name, index, fault.kind().as_str()),
            kind: fault.kind().as_str().to_string(),
            backend: fault.backend().as_str().to_string(),
            target: FaultRunTargetSpec::from_target(fault.target()),
            selection: FaultRunSelectionSpec::from_selection(fault.selection()),
            duration_seconds: fault.duration().as_secs(),
            observability: scenario_spec.observability.to_string(),
            conflict_domain: scenario_spec.conflict_domain.to_string(),
        }
    }
}

impl FaultRunTargetSpec {
    fn from_target(target: &FaultTarget) -> Self {
        match target {
            FaultTarget::RustfsVolume { path } => Self {
                kind: "rustfs-volume".to_string(),
                path: Some(path.clone()),
            },
            FaultTarget::RustfsServerPod => Self {
                kind: "rustfs-server-pod".to_string(),
                path: None,
            },
            FaultTarget::RustfsServerPeerNetwork => Self {
                kind: "rustfs-server-peer-network".to_string(),
                path: None,
            },
            FaultTarget::RustfsServerResource => Self {
                kind: "rustfs-server-resource".to_string(),
                path: None,
            },
            FaultTarget::DedicatedBlockDevice => Self {
                kind: "dedicated-block-device".to_string(),
                path: None,
            },
        }
    }
}

impl FaultRunSelectionSpec {
    fn from_selection(selection: FaultSelection) -> Self {
        match selection {
            FaultSelection::Percent(percent) => Self {
                kind: "percent".to_string(),
                value: percent as u32,
            },
            FaultSelection::FixedTargets(count) => Self {
                kind: "fixed-targets".to_string(),
                value: count,
            },
        }
    }
}

impl Default for FaultRunArtifactSpec {
    fn default() -> Self {
        Self {
            required: [
                "run-spec.yaml",
                "run-spec.json",
                "run-events.jsonl",
                "run-metadata.json",
                "workload-plan.json",
                "history.jsonl",
                "workload-summary.json",
                "recommit-report.json",
                "checker-pre-recommit-report.json",
                "checker-report.json",
                "fault-evidence.json",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            event_stream: "run-events.jsonl".to_string(),
        }
    }
}

fn workload_mode_name(mode: FaultWorkloadMode) -> &'static str {
    match mode {
        FaultWorkloadMode::S3Mixed => "s3-mixed",
        FaultWorkloadMode::S3MixedWithWarp => "s3-mixed-with-warp",
    }
}

#[cfg(test)]
mod tests {
    use super::{FAULT_RUN_API_VERSION, FaultRunSpec};
    use crate::fault::{
        config::FaultTestConfig,
        plan::{FaultPlan, FaultPlanOptions},
        scenarios::{FaultScenario, scenario_spec},
        workload::WorkloadPlan,
    };

    #[test]
    fn resolved_spec_exports_yaml_ready_contract() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let scenario = FaultScenario::from_config(&config).expect("scenario");
        let scenario_spec = scenario_spec(&scenario.name).expect("scenario spec");
        let plan = FaultPlan::from_scenario_with_options(
            &scenario,
            scenario_spec,
            FaultPlanOptions::from_config(&config),
        )
        .expect("plan");
        let workload_plan =
            WorkloadPlan::seeded(42, scenario.object_count, config.workload.concurrency);

        let spec = FaultRunSpec::resolved(
            &config,
            &scenario,
            scenario_spec,
            &plan,
            &workload_plan,
            "run-1",
            "bucket-1",
        );

        assert_eq!(spec.api_version, FAULT_RUN_API_VERSION);
        assert_eq!(spec.faults.len(), 1);
        assert_eq!(spec.faults[0].target.path.as_deref(), Some("/data/rustfs0"));
        assert_eq!(spec.scenario.priority, "p0");
        assert_eq!(spec.scenario.isolation, "fresh-tenant");
        assert_eq!(spec.faults[0].backend, "chaos-mesh-io-chaos");
        assert_eq!(spec.recovery.expected_rustfs_pod_count, 4);
        assert!(
            spec.artifacts
                .required
                .contains(&"run-events.jsonl".to_string())
        );
        assert!(
            spec.artifacts
                .required
                .contains(&"run-spec.json".to_string())
        );
        assert!(spec.to_yaml().expect("yaml").contains("apiVersion:"));
        assert!(spec.to_json().expect("json").contains("\"faults\""));
        let decoded =
            serde_json::from_str::<FaultRunSpec>(&spec.to_json().expect("json")).expect("json");
        assert_eq!(decoded.api_version, spec.api_version);
        assert_eq!(decoded.scenario.priority, spec.scenario.priority);
        assert_eq!(decoded.workload.object_count, spec.workload.object_count);
        assert_eq!(
            decoded.workload.plan.size_distribution,
            spec.workload.plan.size_distribution
        );
        let decoded =
            serde_yaml_ng::from_str::<FaultRunSpec>(&spec.to_yaml().expect("yaml")).expect("yaml");
        assert_eq!(decoded.api_version, spec.api_version);
        assert_eq!(decoded.scenario.priority, spec.scenario.priority);
        assert_eq!(decoded.workload.object_count, spec.workload.object_count);
        assert_eq!(
            decoded.workload.plan.size_distribution,
            spec.workload.plan.size_distribution
        );
    }
}
