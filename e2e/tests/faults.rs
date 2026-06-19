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
        self, DIRECT_PV_CORRUPTION_SCENARIO, DISK_FULL_SCENARIO, DM_FLAKEY_SCENARIO, FaultBackend,
        FaultScenario, IO_EIO_SCENARIO, IO_READ_MISTAKE_SCENARIO, NETWORK_PARTITION_ONE_SCENARIO,
        POD_KILL_ONE_SCENARIO, WARP_UNDER_CHAOS_SCENARIO, WORKER_RESTART_SCENARIO,
    },
    history::OperationOutcome,
    history::Recorder,
    host_faults::{self, DiskFillGuard, DmFlakeyGuard},
    kube_client,
    port_forward::{PortForwardGuard, PortForwardSpec},
    resources,
    s3_workload::{ObjectSpec, S3WorkloadClient, wait_for_s3_endpoint},
    wait,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::{Duration, Instant};
use uuid::Uuid;

const RUSTFS_DATA_VOLUME: &str = "/data/rustfs0";
const SMALL_OBJECT_SIZE_BYTES: usize = 4 * 1024;

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=io-eio"]
async fn fault_io_eio_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(IO_EIO_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=pod-kill-one"]
async fn fault_pod_kill_one_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(POD_KILL_ONE_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=network-partition-one"]
async fn fault_network_partition_one_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(NETWORK_PARTITION_ONE_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=io-read-mistake"]
async fn fault_io_read_mistake_rejects_corrupt_reads() -> Result<()> {
    run_selected_fault_case(IO_READ_MISTAKE_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=disk-full"]
async fn fault_disk_full_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(DISK_FULL_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=direct-pv-corruption"]
async fn fault_direct_pv_corruption_detects_or_repairs_bad_data() -> Result<()> {
    run_selected_fault_case(DIRECT_PV_CORRUPTION_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=worker-restart"]
async fn fault_worker_restart_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(WORKER_RESTART_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=dm-flakey"]
async fn fault_dm_flakey_preserves_committed_objects() -> Result<()> {
    run_selected_fault_case(DM_FLAKEY_SCENARIO).await
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; select with RUSTFS_FAULT_TEST_SCENARIO=warp-under-chaos"]
async fn fault_warp_under_chaos_reports_performance_separately() -> Result<()> {
    run_selected_fault_case(WARP_UNDER_CHAOS_SCENARIO).await
}

async fn run_selected_fault_case(expected_scenario: &str) -> Result<()> {
    let config = FaultTestConfig::from_env()?;
    let scenario = FaultScenario::from_config(&config)?;
    if scenario.name != expected_scenario {
        eprintln!(
            "skipping fault scenario {expected_scenario}; selected scenario is {}",
            scenario.name
        );
        return Ok(());
    }

    config.require_destructive_enabled()?;
    config.validate_cluster()?;
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

    reset_fault_fixture(&config.cluster)?;
    wait_for_ready_tenant(&config.cluster).await?;

    let run_id = format!("run-{}", Uuid::new_v4());
    let bucket = bucket_name(&run_id);
    let history_path = collector.case_dir(scenario.case_name).join("history.jsonl");
    let mut history = Recorder::create(history_path, &scenario.name, &run_id)?;

    let cluster = &config.cluster;
    let port_forward_spec =
        PortForwardSpec::tenant_io(&cluster.test_namespace, &cluster.tenant_name);
    let endpoint = port_forward_spec.local_base_url();
    let mut port_forward = PortForwardSpec::start_tenant_io(cluster)?;
    wait_for_tenant_s3(&mut port_forward, &endpoint, cluster.timeout).await?;

    let (access_key, secret_key) = resources::test_credentials();
    let s3 = S3WorkloadClient::new(
        &endpoint,
        &bucket,
        access_key,
        secret_key,
        config.request_timeout,
    )
    .await?;
    let bucket_outcome = s3.create_bucket(&mut history).await?;
    ensure!(
        bucket_outcome == OperationOutcome::Ok,
        "fault workload bucket creation did not succeed: {bucket_outcome:?}"
    );

    let prefilled = prefill_objects(&s3, &mut history, &run_id, scenario.prefill_count()).await?;
    let mut fault = AppliedFault::apply(
        config,
        collector,
        scenario,
        spec.backend,
        &run_id,
        &endpoint,
        &bucket,
        access_key,
        secret_key,
    )?;

    if let Err(error) = fault.wait_active(cluster.timeout) {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "wait-active-failed")?;
        return Err(error);
    }

    ensure_port_forward(&mut port_forward, cluster, &endpoint).await?;

    let workload_summary = match run_mixed_workload(
        &s3,
        &mut history,
        &run_id,
        &prefilled,
        scenario.prefill_count(),
        scenario.mixed_workload_count(),
    )
    .await
    {
        Ok(summary) => summary,
        Err(error) => {
            collect_fault_artifacts(collector, scenario.case_name, &fault, "workload-failed")?;
            return Err(error);
        }
    };
    collector.write_text(
        scenario.case_name,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload_summary)?,
    )?;
    if let Err(error) =
        workload_summary.require_fault_evidence(config.require_client_disruption)
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

    if let Err(error) = fault.delete() {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "delete-failed")?;
        return Err(error);
    }

    wait_for_ready_tenant(cluster).await?;
    ensure_port_forward(&mut port_forward, cluster, &endpoint).await?;
    let report = checker::check_s3_history(&s3, &mut history, true).await?;
    collector.write_text(
        scenario.case_name,
        "checker-report.json",
        &serde_json::to_string_pretty(&report)?,
    )?;
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
        FaultBackend::LocalPvFill => Ok(()),
        FaultBackend::KindWorkerFileCorruption | FaultBackend::KindWorkerRestart => {
            require_tool("docker", ["version"])
        }
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
    let name = config
        .dm_name
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_NAME is required for dm-flakey")?;
    config
        .dm_fault_table
        .as_deref()
        .context("RUSTFS_FAULT_TEST_DM_FAULT_TABLE is required for dm-flakey")?;

    require_tool("dmsetup", ["version"])?;
    CommandSpec::new("dmsetup")
        .args(["table", name])
        .run_checked()
        .with_context(|| format!("dm-flakey target {name:?} must exist before fixture reset"))?;
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
        FaultBackend::LocalPvFill
        | FaultBackend::KindWorkerFileCorruption
        | FaultBackend::KindWorkerRestart
        | FaultBackend::DeviceMapper => Ok(()),
    }
}

fn reset_fault_fixture(config: &ClusterTestConfig) -> Result<()> {
    resources::reset_fault_tenant_resources(config)?;
    resources::apply_fault_tenant_resources(config)?;
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
    DiskFill(Box<DiskFillGuard>),
    DmFlakey(Box<DmFlakeyGuard>),
    Completed,
}

impl AppliedFault {
    #[allow(clippy::too_many_arguments)]
    fn apply(
        config: &FaultTestConfig,
        collector: &ArtifactCollector,
        scenario: &FaultScenario,
        backend: FaultBackend,
        run_id: &str,
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<Self> {
        let cluster = &config.cluster;
        match backend {
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
            FaultBackend::LocalPvFill => Ok(Self::DiskFill(Box::new(
                host_faults::fill_rustfs_data_volume(
                    cluster,
                    config.disk_fill_mib,
                    collector,
                    scenario.case_name,
                    run_id,
                )?,
            ))),
            FaultBackend::KindWorkerFileCorruption => {
                host_faults::corrupt_one_kind_local_pv_file(
                    cluster,
                    collector,
                    scenario.case_name,
                )?;
                Ok(Self::Completed)
            }
            FaultBackend::KindWorkerRestart => {
                host_faults::restart_one_kind_worker(cluster, collector, scenario.case_name)?;
                Ok(Self::Completed)
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
                Ok(Self::DmFlakey(Box::new(host_faults::apply_dm_flakey(
                    name,
                    fault_table,
                    config.dm_recovery_table.as_deref(),
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
                guard.wait_active(cluster.timeout)?;
                host_faults::run_warp_mixed(
                    config.warp_duration,
                    collector,
                    scenario.case_name,
                    endpoint,
                    bucket,
                    access_key,
                    secret_key,
                )?;
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
            } => wait_for_rustfs_pod_replacement(config, before_pods, timeout),
            Self::Chaos { .. } | Self::DiskFill(_) | Self::DmFlakey(_) | Self::Completed => Ok(()),
        }
    }

    fn ensure_active(&self, stage: &str) -> Result<()> {
        match self {
            Self::Chaos {
                guard,
                active_required,
            } if *active_required => guard.ensure_active(stage),
            Self::PodKill { .. }
            | Self::Chaos { .. }
            | Self::DiskFill(_)
            | Self::DmFlakey(_)
            | Self::Completed => Ok(()),
        }
    }

    fn delete(&mut self) -> Result<()> {
        match self {
            Self::Chaos { guard, .. } => guard.delete(),
            Self::PodKill { guard, .. } => guard.delete(),
            Self::DiskFill(guard) => guard.delete(),
            Self::DmFlakey(guard) => guard.restore(),
            Self::Completed => Ok(()),
        }
    }

    fn chaos_guard(&self) -> Option<&ChaosGuard> {
        match self {
            Self::Chaos { guard, .. } | Self::PodKill { guard, .. } => Some(guard.as_ref()),
            Self::DiskFill(_) | Self::DmFlakey(_) | Self::Completed => None,
        }
    }
}

fn collect_fault_artifacts(
    collector: &ArtifactCollector,
    case_name: &str,
    fault: &AppliedFault,
    suffix: &str,
) -> Result<()> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PodIdentity {
    name: String,
    uid: String,
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

async fn ensure_port_forward(
    port_forward: &mut PortForwardGuard,
    config: &ClusterTestConfig,
    endpoint: &str,
) -> Result<()> {
    if port_forward.ensure_running().is_err() {
        *port_forward = PortForwardSpec::start_tenant_io(config)?;
    }
    wait_for_tenant_s3(port_forward, endpoint, config.timeout).await
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
    history: &mut Recorder,
    run_id: &str,
    count: usize,
) -> Result<Vec<ObjectSpec>> {
    let mut objects = Vec::with_capacity(count);

    for index in 0..count {
        let object = ObjectSpec::deterministic(run_id, index, SMALL_OBJECT_SIZE_BYTES);
        let put_outcome = s3.put_object(&object, history).await?;
        ensure!(
            put_outcome == OperationOutcome::Ok,
            "prefill PUT failed before fault injection for key {}: {put_outcome:?}",
            object.key
        );
        let head_outcome = s3.head_object(&object.key, history).await?;
        ensure!(
            head_outcome == OperationOutcome::Ok,
            "prefill HEAD failed before fault injection for key {}: {head_outcome:?}",
            object.key
        );
        objects.push(object);
    }

    Ok(objects)
}

async fn run_mixed_workload(
    s3: &S3WorkloadClient,
    history: &mut Recorder,
    run_id: &str,
    prefilled: &[ObjectSpec],
    start_index: usize,
    count: usize,
) -> Result<WorkloadSummary> {
    let mut summary = WorkloadSummary::default();

    for offset in 0..count {
        let object =
            ObjectSpec::deterministic(run_id, start_index + offset, SMALL_OBJECT_SIZE_BYTES);
        let put_outcome = s3.put_object(&object, history).await?;
        summary.puts.record(put_outcome);

        if let Some(existing) = prefilled.get(offset % prefilled.len()) {
            let get_result = s3.get_object_result(&existing.key, history).await?;
            summary.gets.record(get_result.outcome);
        }
    }

    summary.require_exercised()?;
    Ok(summary)
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
struct WorkloadSummary {
    puts: OutcomeCounts,
    gets: OutcomeCounts,
}

impl WorkloadSummary {
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

#[cfg(test)]
mod tests {
    use super::{
        OutcomeCounts, PodIdentity, WorkloadSummary, bucket_name, pod_replacement_observed,
    };
    use rustfs_operator_e2e::framework::history::OperationOutcome;

    #[test]
    fn fault_bucket_name_is_s3_compatible_and_run_scoped() {
        assert_eq!(
            bucket_name("run-12345678-abcd-efgh"),
            "rustfs-fault-run12345678abcde"
        );
    }

    #[test]
    fn workload_summary_counts_disrupted_operations() {
        let mut summary = WorkloadSummary::default();
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
            puts: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
            gets: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
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
}
