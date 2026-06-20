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
use futures::{StreamExt, TryStreamExt, stream};
use kube::Api;
use operator::types::v1alpha1::tenant::Tenant;
use rustfs_operator_e2e::framework::{
    artifacts::ArtifactCollector,
    chaos_mesh::{self, ChaosGuard, IoChaosSpec, NetworkChaosSpec, PodChaosSpec},
    checker,
    command::CommandSpec,
    config::ClusterTestConfig,
    fault_config::FaultTestConfig,
    fault_scenarios::{
        self, DISK_FULL_SCENARIO, FaultBackend, FaultIsolation, FaultScenario,
        IO_READ_MISTAKE_SCENARIO,
    },
    history::OperationOutcome,
    history::Recorder,
    host_faults::{self, DmFlakeyGuard, DmFlakeySpec, DmStatusSnapshot},
    kube_client,
    kubectl::Kubectl,
    port_forward::{PortForwardGuard, PortForwardSpec},
    resources,
    s3_workload::{ObjectSpec, S3WorkloadClient, WorkloadPlan, wait_for_s3_endpoint},
    wait,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::{Duration, Instant};
use tokio::time::sleep as async_sleep;
use uuid::Uuid;

const RUSTFS_DATA_VOLUME: &str = "/data/rustfs0";
const FAULT_TENANT_POD_COUNT: usize = 4;
const RUSTFS_POD_STABLE_WINDOW: Duration = Duration::from_secs(60);

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO"]
async fn fault_selected_scenario() -> Result<()> {
    let config = FaultTestConfig::from_env()?;
    let scenario = FaultScenario::from_config(&config)?;
    let spec = fault_scenarios::scenario_spec(&scenario.name)?;

    config.require_destructive_enabled()?;
    config.validate_cluster(spec.backend == FaultBackend::DeviceMapper)?;
    eprintln!(
        "running destructive RustFS fault scenario {} against real Kubernetes context: {}",
        scenario.name, config.cluster.context
    );

    let collector = ArtifactCollector::new(&config.cluster.artifacts_dir);
    let result = run_fault_case(&config, &collector, &scenario).await;

    if let Err(error) = &result {
        match collector.collect_kubernetes_snapshot(scenario.case_name, &config.cluster) {
            Ok(report) => {
                eprintln!(
                    "collected fault-test artifacts under {}",
                    report.dir.display()
                );
                eprintln!("{}", report.diagnosis);
            }
            Err(artifact_error) => {
                eprintln!("failed to collect fault-test artifacts after {error}: {artifact_error}");
            }
        }
    }

    result
}

async fn run_fault_case(
    config: &FaultTestConfig,
    collector: &ArtifactCollector,
    scenario: &FaultScenario,
) -> Result<()> {
    let spec = fault_scenarios::scenario_spec(&scenario.name)?;
    require_fault_backend(config, spec.backend)?;
    cleanup_fault_backend(config, spec.backend)?;

    prepare_fault_fixture(&config.cluster, spec.isolation)?;
    wait_for_ready_tenant(&config.cluster).await?;
    wait_for_stable_rustfs_pods(&config.cluster, RUSTFS_POD_STABLE_WINDOW).await?;

    let run_id = format!("run-{}", Uuid::new_v4());
    let workload_seed = config.workload_seed.unwrap_or_else(generated_seed);
    let workload_plan = WorkloadPlan::seeded(
        workload_seed,
        scenario.object_count,
        config.workload_concurrency,
    );
    let bucket = bucket_name(&run_id);
    let history_path = collector.case_dir(scenario.case_name).join("history.jsonl");
    let history = Recorder::create(history_path, &scenario.name, &run_id)?;
    collector.write_text(
        scenario.case_name,
        "workload-plan.json",
        &serde_json::to_string_pretty(&workload_plan)?,
    )?;
    eprintln!(
        "fault workload seed={} objects={} concurrency={} payload_bytes={}",
        workload_plan.seed,
        workload_plan.object_count,
        workload_plan.concurrency,
        workload_plan.total_payload_bytes
    );

    let cluster = &config.cluster;
    let (endpoint, mut port_forward) = s3_access(config)?;
    ensure_s3_access(&mut port_forward, cluster, &endpoint).await?;

    let (access_key, secret_key) = resources::test_credentials();
    let s3 = S3WorkloadClient::new(
        &endpoint,
        &bucket,
        access_key,
        secret_key,
        config.request_timeout,
    )
    .await?;
    let bucket_outcome = s3.create_bucket(&history).await?;
    ensure!(
        bucket_outcome == OperationOutcome::Ok,
        "fault workload bucket creation did not succeed: {bucket_outcome:?}"
    );

    let prefilled = prefill_objects(
        &s3,
        &history,
        &run_id,
        &workload_plan,
        scenario.prefill_count(),
    )
    .await?;
    let pods_before = rustfs_pod_identities(cluster)?;
    let mut fault = AppliedFault::apply(config, collector, scenario, spec.backend, &run_id)?;

    if let Err(error) = fault.wait_active(cluster.timeout) {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "wait-active-failed")?;
        return Err(error);
    }
    let active_snapshot = fault.snapshot("active")?;

    if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "port-forward-failed")?;
        return Err(error);
    }

    if spec.backend == FaultBackend::MinioWarpWithChaos {
        let warp_bucket = warp_bucket_name(&run_id);
        if let Err(error) = host_faults::run_warp_mixed(
            config.warp_duration,
            collector,
            scenario.case_name,
            &endpoint,
            &warp_bucket,
            access_key,
            secret_key,
        ) {
            collect_fault_artifacts(collector, scenario.case_name, &fault, "warp-failed")?;
            return Err(error);
        }

        if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
            collect_fault_artifacts(
                collector,
                scenario.case_name,
                &fault,
                "post-warp-port-forward-failed",
            )?;
            return Err(error);
        }
    }

    let mut workload = match run_mixed_workload(
        &s3,
        &history,
        &run_id,
        &workload_plan,
        &prefilled,
        scenario.prefill_count(),
        scenario.mixed_workload_count(),
    )
    .await
    {
        Ok(workload) => workload,
        Err(error) => {
            collect_fault_artifacts(collector, scenario.case_name, &fault, "workload-failed")?;
            return Err(error);
        }
    };
    collector.write_text(
        scenario.case_name,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload.summary)?,
    )?;
    if let Err(error) = workload
        .summary
        .require_fault_evidence(config.require_client_disruption)
    {
        collect_fault_artifacts(
            collector,
            scenario.case_name,
            &fault,
            "workload-no-fault-evidence",
        )?;
        return Err(error);
    }
    if let Err(error) = fault.ensure_active("after fault workload") {
        collect_fault_artifacts(
            collector,
            scenario.case_name,
            &fault,
            "workload-outlived-fault",
        )?;
        return Err(error);
    }
    let workload_snapshot = fault.snapshot("after-workload")?;

    if let Err(error) = fault.delete(cluster.timeout) {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "delete-failed")?;
        return Err(error);
    }

    wait_for_ready_tenant(cluster).await?;
    wait_for_stable_rustfs_pods(cluster, RUSTFS_POD_STABLE_WINDOW).await?;
    let pods_after = rustfs_pod_identities(cluster)?;
    ensure_s3_access(&mut port_forward, cluster, &endpoint).await?;
    workload.summary.recommitted_after_recovery = recommit_unconfirmed_objects(
        &s3,
        &history,
        &workload.unconfirmed_puts,
        workload_plan.concurrency,
    )
    .await?;
    collector.write_text(
        scenario.case_name,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload.summary)?,
    )?;
    let report = checker::check_s3_history(&s3, &history, true, workload_plan.concurrency).await?;
    collector.write_text(
        scenario.case_name,
        "checker-report.json",
        &serde_json::to_string_pretty(&report)?,
    )?;
    let evidence = FaultEvidence {
        scenario: scenario.name.clone(),
        backend: format!("{:?}", spec.backend),
        target: spec.target.to_string(),
        injected: true,
        active_during_workload: true,
        recovered: report.tenant_recovered,
        client_disruptions: workload.summary.disrupted(),
        workload_plan,
        pods_before,
        pods_after,
        active_snapshot,
        workload_snapshot,
        dm_recovery_snapshot: fault.recovery_dm_snapshot(),
    };
    collector.write_text(
        scenario.case_name,
        "fault-evidence.json",
        &serde_json::to_string_pretty(&evidence)?,
    )?;
    ensure!(
        report.committed_puts == scenario.object_count,
        "fault scenario {} expected {} committed objects after recovery reconciliation, got {}",
        scenario.name,
        scenario.object_count,
        report.committed_puts
    );
    report.require_success()?;

    Ok(())
}

fn require_fault_backend(config: &FaultTestConfig, backend: FaultBackend) -> Result<()> {
    let cluster = &config.cluster;
    match backend {
        FaultBackend::ChaosMeshIoChaos => chaos_mesh::require_iochaos_crd(cluster),
        FaultBackend::MinioWarpWithChaos => {
            chaos_mesh::require_iochaos_crd(cluster)?;
            require_tool("warp", ["--help"])
        }
        FaultBackend::ChaosMeshPodChaos => chaos_mesh::require_podchaos_crd(cluster),
        FaultBackend::ChaosMeshNetworkChaos => chaos_mesh::require_networkchaos_crd(cluster),
        FaultBackend::DeviceMapper => require_dm_flakey_preflight(config),
    }
}

fn require_tool<I, S>(program: &'static str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    CommandSpec::new(program)
        .args(args)
        .run_checked()
        .with_context(|| format!("{program} is required for the selected fault scenario"))?;
    Ok(())
}

fn require_dm_flakey_preflight(config: &FaultTestConfig) -> Result<()> {
    config
        .dm_name
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_NAME is required for dm-flakey")?;
    config
        .dm_node
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_NODE is required for dm-flakey")?;
    config
        .dm_mount_path
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_MOUNT_PATH is required for dm-flakey")?;
    config
        .dm_fault_table
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_FAULT_TABLE is required for dm-flakey")?;
    Ok(())
}

fn cleanup_fault_backend(config: &FaultTestConfig, backend: FaultBackend) -> Result<()> {
    match backend {
        FaultBackend::ChaosMeshIoChaos | FaultBackend::MinioWarpWithChaos => {
            chaos_mesh::cleanup_managed_iochaos(&config.cluster, &config.chaos_namespace)
        }
        FaultBackend::ChaosMeshPodChaos => {
            chaos_mesh::cleanup_managed_podchaos(&config.cluster, &config.chaos_namespace)
        }
        FaultBackend::ChaosMeshNetworkChaos => {
            chaos_mesh::cleanup_managed_networkchaos(&config.cluster, &config.chaos_namespace)
        }
        FaultBackend::DeviceMapper => Ok(()),
    }
}

fn prepare_fault_fixture(config: &ClusterTestConfig, isolation: FaultIsolation) -> Result<()> {
    match isolation {
        FaultIsolation::ReusableTenant => resources::apply_fault_tenant_resources(config)?,
        FaultIsolation::FreshTenant | FaultIsolation::DedicatedLinuxBlockDevice => {
            resources::reset_fault_tenant_resources(config)?;
            resources::apply_fault_tenant_resources(config)?;
        }
    }
    Ok(())
}

enum AppliedFault {
    Chaos {
        guard: Box<ChaosGuard>,
        active_required: bool,
    },
    PodKill {
        guard: Box<ChaosGuard>,
        before_pods: Vec<PodIdentity>,
        config: Box<ClusterTestConfig>,
    },
    DmFlakey(Box<DmFlakeyGuard>),
}

impl AppliedFault {
    fn apply(
        config: &FaultTestConfig,
        collector: &ArtifactCollector,
        scenario: &FaultScenario,
        backend: FaultBackend,
        run_id: &str,
    ) -> Result<Self> {
        let cluster = &config.cluster;
        match backend {
            FaultBackend::ChaosMeshIoChaos if scenario.name == DISK_FULL_SCENARIO => {
                let chaos = IoChaosSpec::enospc_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    RUSTFS_DATA_VOLUME,
                    scenario.percent,
                    scenario.duration,
                )?;
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultBackend::ChaosMeshIoChaos if scenario.name == IO_READ_MISTAKE_SCENARIO => {
                let chaos = IoChaosSpec::read_mistake_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    RUSTFS_DATA_VOLUME,
                    scenario.percent,
                    scenario.duration,
                )?;
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultBackend::ChaosMeshIoChaos => {
                let chaos = IoChaosSpec::eio_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    RUSTFS_DATA_VOLUME,
                    scenario.percent,
                    scenario.duration,
                )?;
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultBackend::ChaosMeshPodChaos => {
                let before_pods = rustfs_pod_identities(cluster)?;
                let chaos = PodChaosSpec::kill_one_rustfs_pod(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                );
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                Ok(Self::PodKill {
                    guard: Box::new(chaos_mesh::apply_podchaos(cluster, &chaos)?),
                    before_pods,
                    config: Box::new(cluster.clone()),
                })
            }
            FaultBackend::ChaosMeshNetworkChaos => {
                let chaos = NetworkChaosSpec::partition_one_rustfs_pod(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    scenario.duration,
                )?;
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_networkchaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultBackend::DeviceMapper => {
                let name = config
                    .dm_name
                    .as_deref()
                    .context("RUSTFS_FAULT_TEST_DM_NAME is required for dm-flakey")?;
                let fault_table = config
                    .dm_fault_table
                    .as_deref()
                    .context("RUSTFS_FAULT_TEST_DM_FAULT_TABLE is required for dm-flakey")?;
                let node = config
                    .dm_node
                    .as_deref()
                    .context("RUSTFS_FAULT_TEST_DM_NODE is required for dm-flakey")?;
                let mount_path = config
                    .dm_mount_path
                    .as_deref()
                    .context("RUSTFS_FAULT_TEST_DM_MOUNT_PATH is required for dm-flakey")?;
                Ok(Self::DmFlakey(Box::new(host_faults::apply_dm_flakey(
                    cluster,
                    &DmFlakeySpec {
                        node,
                        mount_path,
                        helper_image: &config.dm_helper_image,
                        name,
                        fault_table,
                        recovery_table: config.dm_recovery_table.as_deref(),
                        run_id,
                    },
                    collector,
                    scenario.case_name,
                )?)))
            }
            FaultBackend::MinioWarpWithChaos => {
                let chaos = IoChaosSpec::eio_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    RUSTFS_DATA_VOLUME,
                    scenario.percent,
                    scenario.duration,
                )?;
                collector.write_text(
                    scenario.case_name,
                    "chaos-manifest.yaml",
                    &chaos.manifest(),
                )?;
                let guard = chaos_mesh::apply_iochaos(cluster, &chaos)?;
                Ok(Self::Chaos {
                    guard: Box::new(guard),
                    active_required: true,
                })
            }
        }
    }

    fn wait_active(&self, timeout: Duration) -> Result<()> {
        match self {
            Self::Chaos {
                guard,
                active_required,
            } if *active_required => guard.wait_active(timeout),
            Self::PodKill {
                before_pods,
                config,
                ..
            } => wait_for_rustfs_pod_deletion(config, before_pods, timeout),
            Self::Chaos { .. } | Self::DmFlakey(_) => Ok(()),
        }
    }

    fn ensure_active(&self, stage: &str) -> Result<()> {
        match self {
            Self::Chaos {
                guard,
                active_required,
            } if *active_required => guard.ensure_active(stage),
            Self::PodKill { .. } | Self::Chaos { .. } => Ok(()),
            Self::DmFlakey(guard) => {
                guard.ensure_active("after fault workload")?;
                Ok(())
            }
        }
    }

    fn delete(&mut self, timeout: Duration) -> Result<()> {
        match self {
            Self::Chaos { guard, .. } => guard.delete(),
            Self::PodKill {
                guard,
                before_pods,
                config,
            } => {
                guard.delete()?;
                wait_for_rustfs_pod_replacement(config, before_pods, timeout)
            }
            Self::DmFlakey(guard) => guard.restore(),
        }
    }

    fn chaos_guard(&self) -> Option<&ChaosGuard> {
        match self {
            Self::Chaos { guard, .. } | Self::PodKill { guard, .. } => Some(guard.as_ref()),
            Self::DmFlakey(_) => None,
        }
    }

    fn snapshot(&self, stage: &str) -> Result<FaultStatusSnapshot> {
        match self {
            Self::Chaos { guard, .. } | Self::PodKill { guard, .. } => Ok(FaultStatusSnapshot {
                stage: stage.to_string(),
                resource_kind: Some(guard.kind().to_string()),
                resource_name: Some(guard.name().to_string()),
                chaos_status: Some(serde_json::from_str(&guard.json()?)?),
                dm_status: None,
            }),
            Self::DmFlakey(guard) => Ok(FaultStatusSnapshot {
                stage: stage.to_string(),
                resource_kind: Some("device-mapper".to_string()),
                resource_name: None,
                chaos_status: None,
                dm_status: Some(guard.snapshot(stage)?),
            }),
        }
    }

    fn recovery_dm_snapshot(&self) -> Option<DmStatusSnapshot> {
        match self {
            Self::DmFlakey(guard) => guard.recovery_snapshot().cloned(),
            Self::Chaos { .. } | Self::PodKill { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct FaultStatusSnapshot {
    stage: String,
    resource_kind: Option<String>,
    resource_name: Option<String>,
    chaos_status: Option<serde_json::Value>,
    dm_status: Option<DmStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
struct FaultEvidence {
    scenario: String,
    backend: String,
    target: String,
    injected: bool,
    active_during_workload: bool,
    recovered: bool,
    client_disruptions: usize,
    workload_plan: WorkloadPlan,
    pods_before: Vec<PodIdentity>,
    pods_after: Vec<PodIdentity>,
    active_snapshot: FaultStatusSnapshot,
    workload_snapshot: FaultStatusSnapshot,
    dm_recovery_snapshot: Option<DmStatusSnapshot>,
}

fn collect_fault_artifacts(
    collector: &ArtifactCollector,
    case_name: &str,
    fault: &AppliedFault,
    suffix: &str,
) -> Result<()> {
    let status = fault
        .snapshot(suffix)
        .and_then(|snapshot| serde_json::to_string_pretty(&snapshot).map_err(Into::into))
        .unwrap_or_else(|error| format!("failed to collect fault status: {error}"));
    collector.write_text(case_name, &format!("fault-status-{suffix}.json"), &status)?;

    if let Some(guard) = fault.chaos_guard() {
        let describe = guard
            .describe()
            .unwrap_or_else(|error| format!("failed to describe chaos before cleanup: {error}"));
        collector.write_text(
            case_name,
            &format!("chaos-describe-{suffix}.txt"),
            &describe,
        )?;

        let yaml = guard
            .yaml()
            .unwrap_or_else(|error| format!("failed to get chaos yaml before cleanup: {error}"));
        collector.write_text(case_name, &format!("chaos-{suffix}.yaml"), &yaml)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PodIdentity {
    name: String,
    uid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PodRuntimeState {
    name: String,
    uid: String,
    phase: String,
    containers_ready: bool,
    restart_count: u64,
    terminating: bool,
}

fn rustfs_pod_identities(config: &ClusterTestConfig) -> Result<Vec<PodIdentity>> {
    let selector = format!("rustfs.tenant={}", config.tenant_name);
    let output = rustfs_operator_e2e::framework::kubectl::Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(["get", "pod", "-l", &selector, "-o", "json"])
        .run_checked()?;
    let value = serde_json::from_str::<serde_json::Value>(&output.stdout)
        .context("parse RustFS pod list json")?;
    let items = value
        .pointer("/items")
        .and_then(serde_json::Value::as_array)
        .context("RustFS pod list did not contain an items array")?;
    let pods = items
        .iter()
        .filter_map(|item| {
            let metadata = item.get("metadata")?;
            Some(PodIdentity {
                name: metadata.get("name")?.as_str()?.to_string(),
                uid: metadata.get("uid")?.as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();
    ensure!(
        !pods.is_empty(),
        "no RustFS pods found for selector {selector} in namespace {}",
        config.test_namespace
    );
    Ok(pods)
}

fn rustfs_pod_runtime_states(config: &ClusterTestConfig) -> Result<Vec<PodRuntimeState>> {
    let selector = format!("rustfs.tenant={}", config.tenant_name);
    let output = Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(["get", "pod", "-l", &selector, "-o", "json"])
        .run_checked()?;
    let value = serde_json::from_str::<serde_json::Value>(&output.stdout)
        .context("parse RustFS pod list json")?;
    let items = value
        .pointer("/items")
        .and_then(serde_json::Value::as_array)
        .context("RustFS pod list did not contain an items array")?;
    let mut pods = items
        .iter()
        .map(|item| {
            let metadata = item
                .get("metadata")
                .context("RustFS pod did not contain metadata")?;
            let name = metadata
                .get("name")
                .and_then(serde_json::Value::as_str)
                .context("RustFS pod metadata did not contain a name")?;
            let uid = metadata
                .get("uid")
                .and_then(serde_json::Value::as_str)
                .context("RustFS pod metadata did not contain a uid")?;
            let phase = item
                .pointer("/status/phase")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown");
            let container_statuses = item
                .pointer("/status/containerStatuses")
                .and_then(serde_json::Value::as_array);
            let containers_ready = container_statuses.is_some_and(|statuses| {
                !statuses.is_empty()
                    && statuses.iter().all(|status| {
                        status
                            .get("ready")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    })
            });
            let restart_count = container_statuses
                .into_iter()
                .flatten()
                .filter_map(|status| status.get("restartCount"))
                .filter_map(serde_json::Value::as_u64)
                .sum();

            Ok(PodRuntimeState {
                name: name.to_string(),
                uid: uid.to_string(),
                phase: phase.to_string(),
                containers_ready,
                restart_count,
                terminating: metadata.get("deletionTimestamp").is_some(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    pods.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(pods)
}

fn stable_pod_fingerprint(pods: &[PodRuntimeState]) -> Option<Vec<(String, u64)>> {
    if pods.len() != FAULT_TENANT_POD_COUNT
        || pods
            .iter()
            .any(|pod| pod.phase != "Running" || !pod.containers_ready || pod.terminating)
    {
        return None;
    }

    Some(
        pods.iter()
            .map(|pod| (pod.uid.clone(), pod.restart_count))
            .collect(),
    )
}

async fn wait_for_stable_rustfs_pods(
    config: &ClusterTestConfig,
    stable_window: Duration,
) -> Result<()> {
    let deadline = Instant::now() + config.timeout;
    let mut stable_since = None;
    let mut stable_fingerprint = None;
    let mut last_snapshot = Vec::new();
    let mut last_error = "not checked yet".to_string();

    eprintln!(
        "waiting for {FAULT_TENANT_POD_COUNT} RustFS pods to remain ready without restarts for {stable_window:?}"
    );
    loop {
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for stable RustFS pods after {:?}\nlast: {last_snapshot:?}\nlast error: {last_error}",
                config.timeout
            );
        }

        match rustfs_pod_runtime_states(config) {
            Ok(current) => {
                if let Some(fingerprint) = stable_pod_fingerprint(&current) {
                    if stable_fingerprint.as_ref() != Some(&fingerprint) {
                        stable_since = Some(Instant::now());
                        stable_fingerprint = Some(fingerprint);
                    }
                    if stable_since.is_some_and(|started| started.elapsed() >= stable_window) {
                        eprintln!("RustFS pods remained stable for {stable_window:?}");
                        return Ok(());
                    }
                } else {
                    stable_since = None;
                    stable_fingerprint = None;
                }
                last_snapshot = current;
                last_error = "none".to_string();
            }
            Err(error) => {
                stable_since = None;
                stable_fingerprint = None;
                last_error = error.to_string();
            }
        }

        async_sleep(Duration::from_secs(1)).await;
    }
}

fn wait_for_rustfs_pod_replacement(
    config: &ClusterTestConfig,
    before: &[PodIdentity],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_snapshot = Vec::new();
    let mut last_error = "not checked yet".to_string();

    loop {
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for PodChaos to replace a RustFS pod after {timeout:?}\nbefore: {before:?}\nlast: {last_snapshot:?}\nlast error: {last_error}",
            );
        }

        match rustfs_pod_identities(config) {
            Ok(current) => {
                if pod_replacement_observed(before, &current) {
                    return Ok(());
                }
                last_snapshot = current;
                last_error = "none".to_string();
            }
            Err(error) => {
                last_error = error.to_string();
            }
        }

        sleep(Duration::from_secs(1));
    }
}

fn wait_for_rustfs_pod_deletion(
    config: &ClusterTestConfig,
    before: &[PodIdentity],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_snapshot = Vec::new();
    let mut last_error = "not checked yet".to_string();

    loop {
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for PodChaos to delete a RustFS pod after {timeout:?}\nbefore: {before:?}\nlast: {last_snapshot:?}\nlast error: {last_error}",
            );
        }

        match rustfs_pod_identities(config) {
            Ok(current) => {
                if pod_deletion_observed(before, &current) {
                    return Ok(());
                }
                last_snapshot = current;
                last_error = "none".to_string();
            }
            Err(error) => {
                last_error = error.to_string();
            }
        }

        sleep(Duration::from_millis(250));
    }
}

fn pod_deletion_observed(before: &[PodIdentity], current: &[PodIdentity]) -> bool {
    let current_uids = current
        .iter()
        .map(|pod| pod.uid.as_str())
        .collect::<BTreeSet<_>>();
    !before.is_empty()
        && before
            .iter()
            .any(|pod| !current_uids.contains(pod.uid.as_str()))
}

fn pod_replacement_observed(before: &[PodIdentity], current: &[PodIdentity]) -> bool {
    if before.is_empty() || current.is_empty() {
        return false;
    }

    let before_uids = before
        .iter()
        .map(|pod| pod.uid.as_str())
        .collect::<BTreeSet<_>>();
    let current_uids = current
        .iter()
        .map(|pod| pod.uid.as_str())
        .collect::<BTreeSet<_>>();
    let old_uid_removed = before_uids.iter().any(|uid| !current_uids.contains(uid));
    let new_uid_added = current_uids.iter().any(|uid| !before_uids.contains(uid));

    old_uid_removed && new_uid_added
}

async fn wait_for_ready_tenant(config: &ClusterTestConfig) -> Result<Tenant> {
    let client = kube_client::default_client().await?;
    let tenants: Api<Tenant> = kube_client::tenant_api(client, &config.test_namespace);
    wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout).await
}

fn s3_access(config: &FaultTestConfig) -> Result<(String, Option<PortForwardGuard>)> {
    let cluster = &config.cluster;
    if config.use_cluster_ip {
        let service = format!("{}-io", cluster.tenant_name);
        let output = Kubectl::new(cluster)
            .namespaced(&cluster.test_namespace)
            .command([
                "get".to_string(),
                "service".to_string(),
                service.clone(),
                "-o".to_string(),
                "jsonpath={.spec.clusterIP}".to_string(),
            ])
            .run_checked()
            .with_context(|| format!("read ClusterIP for fault-test service {service:?}"))?;
        let cluster_ip = output.stdout.trim();
        ensure!(
            !cluster_ip.is_empty() && cluster_ip != "None",
            "fault-test service {service:?} does not have a ClusterIP"
        );
        let host = if cluster_ip.contains(':') {
            format!("[{cluster_ip}]")
        } else {
            cluster_ip.to_string()
        };
        return Ok((format!("http://{host}:9000"), None));
    }

    let spec = PortForwardSpec::tenant_io(&cluster.test_namespace, &cluster.tenant_name);
    let endpoint = spec.local_base_url();
    Ok((endpoint, Some(PortForwardSpec::start_tenant_io(cluster)?)))
}

async fn ensure_s3_access(
    port_forward: &mut Option<PortForwardGuard>,
    config: &ClusterTestConfig,
    endpoint: &str,
) -> Result<()> {
    if let Some(guard) = port_forward {
        if guard.ensure_running().is_err() {
            *guard = PortForwardSpec::start_tenant_io(config)?;
        }
        return wait_for_tenant_s3(guard, endpoint, config.timeout).await;
    }

    wait_for_s3_endpoint(endpoint, config.timeout).await
}

async fn wait_for_tenant_s3(
    port_forward: &mut PortForwardGuard,
    endpoint: &str,
    timeout: Duration,
) -> Result<()> {
    port_forward.ensure_running()?;
    wait_for_s3_endpoint(endpoint, timeout)
        .await
        .with_context(|| {
            format!(
                "S3 port-forward was not ready; command: {}; log {}:\n{}",
                port_forward.command_display(),
                port_forward.log_path().display(),
                port_forward.log_contents()
            )
        })
}

async fn prefill_objects(
    s3: &S3WorkloadClient,
    history: &Recorder,
    run_id: &str,
    plan: &WorkloadPlan,
    count: usize,
) -> Result<Vec<ObjectSpec>> {
    let tasks = (0..count).map(|index| {
        let s3 = s3.clone();
        let history = history.clone();
        let run_id = run_id.to_string();
        let size_bytes = plan.size_at(index);
        let seed = plan.seed;
        async move {
            let object = ObjectSpec::prepare_seeded(&run_id, index, size_bytes, seed);
            let spec = object.spec.clone();
            let put_outcome = s3.put_object(&object, &history).await?;
            ensure!(
                put_outcome == OperationOutcome::Ok,
                "prefill PUT failed before fault injection for key {}: {put_outcome:?}",
                spec.key
            );
            let head_outcome = s3.head_object(&spec.key, &history).await?;
            ensure!(
                head_outcome == OperationOutcome::Ok,
                "prefill HEAD failed before fault injection for key {}: {head_outcome:?}",
                spec.key
            );
            Ok::<_, anyhow::Error>((index, spec))
        }
    });
    let mut objects = stream::iter(tasks)
        .buffer_unordered(plan.concurrency)
        .try_collect::<Vec<_>>()
        .await?;
    objects.sort_by_key(|(index, _)| *index);

    Ok(objects.into_iter().map(|(_, object)| object).collect())
}

async fn run_mixed_workload(
    s3: &S3WorkloadClient,
    history: &Recorder,
    run_id: &str,
    plan: &WorkloadPlan,
    prefilled: &[ObjectSpec],
    start_index: usize,
    count: usize,
) -> Result<MixedWorkloadResult> {
    let tasks = (0..count).map(|offset| {
        let s3 = s3.clone();
        let history = history.clone();
        let run_id = run_id.to_string();
        let index = start_index + offset;
        let size_bytes = plan.size_at(index);
        let seed = plan.seed;
        let existing = prefilled[offset % prefilled.len()].clone();
        async move {
            let object = ObjectSpec::prepare_seeded(&run_id, index, size_bytes, seed);
            let spec = object.spec.clone();
            let put_outcome = s3.put_object(&object, &history).await?;
            let get_outcome = s3.get_object_result(&existing.key, &history).await?.outcome;
            Ok::<_, anyhow::Error>(MixedTaskResult {
                index,
                object: spec,
                put_outcome,
                get_outcome,
            })
        }
    });
    let results = stream::iter(tasks)
        .buffer_unordered(plan.concurrency)
        .collect::<Vec<_>>()
        .await;
    let mut completed = Vec::with_capacity(count);
    for result in results {
        completed.push(result?);
    }
    completed.sort_by_key(|result| result.index);

    let mut summary = WorkloadSummary::new(plan);
    let mut unconfirmed_puts = Vec::new();
    for result in completed {
        summary.puts.record(result.put_outcome);
        summary.gets.record(result.get_outcome);
        if result.put_outcome != OperationOutcome::Ok {
            unconfirmed_puts.push(result.object);
        }
    }

    summary.require_exercised()?;
    Ok(MixedWorkloadResult {
        summary,
        unconfirmed_puts,
    })
}

async fn recommit_unconfirmed_objects(
    s3: &S3WorkloadClient,
    history: &Recorder,
    objects: &[ObjectSpec],
    concurrency: usize,
) -> Result<usize> {
    let tasks = objects.iter().cloned().map(|object| {
        let s3 = s3.clone();
        let history = history.clone();
        async move {
            let prepared = object.prepare();
            let outcome = s3.put_object(&prepared, &history).await?;
            Ok::<_, anyhow::Error>((object.key, outcome))
        }
    });
    let results = stream::iter(tasks)
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
    for result in results {
        let (key, outcome) = result?;
        ensure!(
            outcome == OperationOutcome::Ok,
            "PUT for previously unconfirmed object {} did not commit after recovery: {outcome:?}",
            key
        );
    }
    Ok(objects.len())
}

#[derive(Debug)]
struct MixedTaskResult {
    index: usize,
    object: ObjectSpec,
    put_outcome: OperationOutcome,
    get_outcome: OperationOutcome,
}

#[derive(Debug)]
struct MixedWorkloadResult {
    summary: WorkloadSummary,
    unconfirmed_puts: Vec<ObjectSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct WorkloadSummary {
    seed: u64,
    object_count: usize,
    concurrency: usize,
    total_payload_bytes: u64,
    puts: OutcomeCounts,
    gets: OutcomeCounts,
    recommitted_after_recovery: usize,
}

impl WorkloadSummary {
    fn new(plan: &WorkloadPlan) -> Self {
        Self {
            seed: plan.seed,
            object_count: plan.object_count,
            concurrency: plan.concurrency,
            total_payload_bytes: plan.total_payload_bytes,
            puts: OutcomeCounts::default(),
            gets: OutcomeCounts::default(),
            recommitted_after_recovery: 0,
        }
    }

    fn require_exercised(&self) -> Result<()> {
        ensure!(
            self.puts.total() > 0 && self.gets.total() > 0,
            "fault workload did not exercise both PUT and GET paths: {self:?}"
        );
        Ok(())
    }

    fn require_fault_evidence(&self, require_client_disruption: bool) -> Result<()> {
        if require_client_disruption {
            ensure!(
                self.disrupted() > 0,
                "fault was applied but the S3 workload observed no client-visible disrupted operation; increase RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS or RUSTFS_FAULT_TEST_PERCENT, or set RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION=0 if this is expected"
            );
        } else if self.disrupted() == 0 {
            eprintln!(
                "fault was applied, but the S3 workload observed no client-visible disrupted operation"
            );
        }
        Ok(())
    }

    fn disrupted(&self) -> usize {
        self.puts.disrupted() + self.gets.disrupted()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
struct OutcomeCounts {
    ok: usize,
    failed: usize,
    timeout: usize,
    unknown: usize,
}

impl OutcomeCounts {
    fn record(&mut self, outcome: OperationOutcome) {
        match outcome {
            OperationOutcome::Ok => self.ok += 1,
            OperationOutcome::Failed => self.failed += 1,
            OperationOutcome::Timeout => self.timeout += 1,
            OperationOutcome::Unknown => self.unknown += 1,
        }
    }

    fn total(&self) -> usize {
        self.ok + self.failed + self.timeout + self.unknown
    }

    fn disrupted(&self) -> usize {
        self.failed + self.timeout + self.unknown
    }
}

fn bucket_name(run_id: &str) -> String {
    let suffix = run_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("rustfs-fault-{suffix}")
}

fn generated_seed() -> u64 {
    let run = Uuid::new_v4();
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&run.as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn warp_bucket_name(run_id: &str) -> String {
    format!("{}-warp", bucket_name(run_id))
}

#[cfg(test)]
mod tests {
    use super::{
        OutcomeCounts, PodIdentity, PodRuntimeState, WorkloadSummary, bucket_name,
        pod_deletion_observed, pod_replacement_observed, stable_pod_fingerprint, warp_bucket_name,
    };
    use rustfs_operator_e2e::framework::history::OperationOutcome;
    use rustfs_operator_e2e::framework::s3_workload::WorkloadPlan;

    #[test]
    fn fault_bucket_name_is_s3_compatible_and_run_scoped() {
        assert_eq!(
            bucket_name("run-12345678-abcd-efgh"),
            "rustfs-fault-run12345678abcde"
        );
        assert_eq!(
            warp_bucket_name("run-12345678-abcd-efgh"),
            "rustfs-fault-run12345678abcde-warp"
        );
    }

    #[test]
    fn workload_summary_counts_disrupted_operations() {
        let mut summary = WorkloadSummary::new(&WorkloadPlan::seeded(42, 40000, 100));
        summary.puts.record(OperationOutcome::Ok);
        summary.gets.record(OperationOutcome::Timeout);

        assert_eq!(summary.puts.total(), 1);
        assert_eq!(summary.gets.total(), 1);
        assert_eq!(summary.disrupted(), 1);
        assert!(summary.require_exercised().is_ok());
        assert!(summary.require_fault_evidence(true).is_ok());
    }

    #[test]
    fn workload_summary_can_require_fault_evidence() {
        let summary = WorkloadSummary {
            seed: 42,
            object_count: 40000,
            concurrency: 100,
            total_payload_bytes: 20_337_459_200,
            puts: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
            gets: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
            recommitted_after_recovery: 0,
        };

        assert!(summary.require_fault_evidence(false).is_ok());
        assert!(summary.require_fault_evidence(true).is_err());
    }

    #[test]
    fn pod_replacement_requires_old_uid_removed_and_new_uid_added() {
        let before = vec![
            PodIdentity {
                name: "rustfs-0".to_string(),
                uid: "uid-a".to_string(),
            },
            PodIdentity {
                name: "rustfs-1".to_string(),
                uid: "uid-b".to_string(),
            },
        ];

        assert!(!pod_replacement_observed(&before, &before));
        assert!(!pod_replacement_observed(&before, &before[..1]));
        assert!(!pod_deletion_observed(&before, &before));
        assert!(pod_deletion_observed(&before, &before[..1]));
        assert!(pod_replacement_observed(
            &before,
            &[
                PodIdentity {
                    name: "rustfs-0".to_string(),
                    uid: "uid-c".to_string(),
                },
                before[1].clone(),
            ],
        ));
    }

    #[test]
    fn stable_pod_fingerprint_requires_four_ready_unchanged_pods() {
        let pods = (0..4)
            .map(|index| PodRuntimeState {
                name: format!("rustfs-{index}"),
                uid: format!("uid-{index}"),
                phase: "Running".to_string(),
                containers_ready: true,
                restart_count: index,
                terminating: false,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stable_pod_fingerprint(&pods),
            Some(vec![
                ("uid-0".to_string(), 0),
                ("uid-1".to_string(), 1),
                ("uid-2".to_string(), 2),
                ("uid-3".to_string(), 3),
            ])
        );
        assert!(stable_pod_fingerprint(&pods[..3]).is_none());

        let mut unready = pods;
        unready[0].containers_ready = false;
        assert!(stable_pod_fingerprint(&unready).is_none());
    }
}
