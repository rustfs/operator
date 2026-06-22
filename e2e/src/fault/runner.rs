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

use crate::{
    fault::{
        backends::{
            chaos_mesh::{
                self, ChaosGuard, IoChaosSpec, NetworkChaosSpec, PodChaosSpec, StressChaosSpec,
            },
            host::{self, DmFlakeyGuard, DmFlakeySpec, DmStatusSnapshot},
        },
        checker,
        config::FaultTestConfig,
        fixture,
        history::{OperationOutcome, OperationRecord, Recorder},
        plan::{FaultInjection, FaultKind, FaultPlan, FaultSelection},
        scenarios::{self, FaultBackend, FaultIsolation, FaultScenario},
        workload::{ObjectSpec, S3WorkloadClient, WorkloadPlan, sha256_hex, wait_for_s3_endpoint},
    },
    framework::{
        artifacts::ArtifactCollector,
        command::CommandSpec,
        config::ClusterTestConfig,
        kube_client,
        kubectl::Kubectl,
        port_forward::{PortForwardGuard, PortForwardSpec},
        resources, wait,
    },
};
use anyhow::{Context, Result, bail, ensure};
use futures::{StreamExt, TryStreamExt, stream};
use kube::Api;
use operator::types::v1alpha1::tenant::Tenant;
use serde::Serialize;
use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::{Duration, Instant};
use tokio::time::sleep as async_sleep;
use uuid::Uuid;

const FAULT_TENANT_POD_COUNT: usize = 4;
const RUSTFS_POD_STABLE_WINDOW: Duration = Duration::from_secs(60);
const PREFILL_VERIFY_ATTEMPTS: usize = 3;
const PREFILL_VERIFY_RETRY_DELAY: Duration = Duration::from_secs(2);

pub async fn run_selected_scenario_from_env() -> Result<()> {
    let config = FaultTestConfig::from_env()?;
    let scenario = FaultScenario::from_config(&config)?;
    let spec = scenarios::scenario_spec(&scenario.name)?;
    let plan = FaultPlan::from_scenario(&scenario, spec)?;

    config.require_destructive_enabled()?;
    config.validate_cluster(plan.requires_static_storage())?;
    eprintln!(
        "running destructive RustFS fault scenario {} against real Kubernetes context: {}",
        scenario.name, config.cluster.context
    );

    let collector = ArtifactCollector::new(&config.cluster.artifacts_dir);
    let result = run_fault_case(&config, &collector, &scenario, &plan).await;

    if let Err(error) = &result {
        write_failure_summary_if_absent(
            &collector,
            scenario.case_name,
            FailureSummary::new(&scenario.name, "scenario", "unknown", error.to_string()),
        )
        .ok();
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
    plan: &FaultPlan,
) -> Result<()> {
    let spec = scenarios::scenario_spec(&scenario.name)?;
    if let Err(error) = require_fault_backends(config, plan) {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fault-backend-preflight",
                "environment_or_fault_backend",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    if let Err(error) = cleanup_fault_backends(config, plan) {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fault-backend-pre-cleanup",
                "environment_or_fault_backend",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    if let Err(error) = prepare_fault_fixture(&config.cluster, spec.isolation) {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fixture-prepare",
                "test_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    if let Err(error) = wait_for_ready_tenant(&config.cluster).await {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "tenant-ready-before-fault",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    if let Err(error) = wait_for_stable_rustfs_pods(&config.cluster, RUSTFS_POD_STABLE_WINDOW).await
    {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "pod-stability-before-fault",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    let run_id = format!("run-{}", Uuid::new_v4());
    let workload_seed = config.workload_seed.unwrap_or_else(generated_seed);
    let workload_plan = WorkloadPlan::seeded(
        workload_seed,
        scenario.object_count,
        config.workload.concurrency,
    );
    let bucket = bucket_name(&run_id);
    let history_path = collector.case_dir(scenario.case_name).join("history.jsonl");
    let history = Recorder::create(history_path, &scenario.name, &run_id)?;
    collector.write_text(
        scenario.case_name,
        "run-metadata.json",
        &serde_json::to_string_pretty(&RunMetadata::from_case(
            config, scenario, plan, &run_id, &bucket,
        ))?,
    )?;
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
    let (endpoint, mut port_forward) = match s3_access(config) {
        Ok(access) => access,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "s3-endpoint",
                    "test_or_environment",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "initial-s3-access",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    let (access_key, secret_key) = resources::test_credentials();
    let s3 = match S3WorkloadClient::new(
        &endpoint,
        &bucket,
        access_key,
        secret_key,
        config.request_timeout,
    )
    .await
    {
        Ok(client) => client,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "s3-client",
                    "test_or_environment",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    let bucket_outcome = match s3.create_bucket(&history).await {
        Ok(outcome) => outcome,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "bucket-create",
                    "test_harness",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    if bucket_outcome != OperationOutcome::Ok {
        let message = format!("fault workload bucket creation did not succeed: {bucket_outcome:?}");
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "bucket-create",
                "product_or_environment",
                message.clone(),
            ),
        )?;
        bail!("{message}");
    }

    let prefilled = match prefill_objects(
        &s3,
        &history,
        &run_id,
        &workload_plan,
        scenario.prefill_count(),
    )
    .await
    {
        Ok(prefilled) => prefilled,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "prefill",
                    "product_or_environment",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    let pods_before = match rustfs_pod_identities(cluster) {
        Ok(pods) => pods,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "pod-identity-before-fault",
                    "test_or_environment",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    let mut fault = match AppliedFaults::apply(config, collector, scenario, plan, &run_id) {
        Ok(fault) => fault,
        Err(error) => {
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "fault-apply",
                    "environment_or_fault_backend",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };

    if let Err(error) = fault.wait_active(cluster.timeout) {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "wait-active-failed")?;
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "wait-active",
                "environment_or_fault_backend",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    let active_snapshots = fault.snapshots("active")?;

    if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "port-forward-failed")?;
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "s3-access-under-fault",
                "environment_or_workload",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    if plan.workload_mode.runs_warp() {
        let warp_bucket = warp_bucket_name(&run_id);
        if let Err(error) = host::run_warp_mixed(
            config.warp_duration,
            collector,
            scenario.case_name,
            &endpoint,
            &warp_bucket,
            access_key,
            secret_key,
        ) {
            collect_fault_artifacts(collector, scenario.case_name, &fault, "warp-failed")?;
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "warp-workload",
                    "workload_or_product",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }

        if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
            collect_fault_artifacts(
                collector,
                scenario.case_name,
                &fault,
                "post-warp-port-forward-failed",
            )?;
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "post-warp-s3-access",
                    "environment_or_workload",
                    error.to_string(),
                ),
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
            write_failure_summary(
                collector,
                scenario.case_name,
                FailureSummary::new(
                    &scenario.name,
                    "mixed-workload",
                    "workload_or_product",
                    error.to_string(),
                ),
            )?;
            return Err(error);
        }
    };
    collector.write_text(
        scenario.case_name,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload.summary)?,
    )?;
    let require_client_disruption =
        config.require_client_disruption || spec.impact_policy.requires_client_disruption();
    if let Err(error) = workload
        .summary
        .require_fault_evidence(require_client_disruption)
    {
        collect_fault_artifacts(
            collector,
            scenario.case_name,
            &fault,
            "workload-no-fault-evidence",
        )?;
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fault-evidence",
                "test_or_environment",
                error.to_string(),
            ),
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
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fault-still-active",
                "test_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    let workload_snapshots = fault.snapshots("after-workload")?;

    if let Err(error) = fault.delete(cluster.timeout) {
        collect_fault_artifacts(collector, scenario.case_name, &fault, "delete-failed")?;
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "fault-delete",
                "environment_or_fault_backend",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    if let Err(error) = wait_for_ready_tenant(cluster).await {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "tenant-recovery",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    if let Err(error) = wait_for_stable_rustfs_pods(cluster, RUSTFS_POD_STABLE_WINDOW).await {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "pod-stability-after-recovery",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    let pods_after = rustfs_pod_identities(cluster)?;
    if let Err(error) = ensure_s3_access(&mut port_forward, cluster, &endpoint).await {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "s3-access-after-recovery",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    let recovered_evidence = FaultEvidence {
        scenario: scenario.name.clone(),
        backend: plan.backend_summary(),
        target: plan.target_summary(),
        injected: true,
        active_during_workload: true,
        recovered: true,
        client_disruptions: workload.summary.disrupted(),
        workload_plan: workload_plan.clone(),
        pods_before: pods_before.clone(),
        pods_after: pods_after.clone(),
        active_snapshots: active_snapshots.clone(),
        workload_snapshots: workload_snapshots.clone(),
        dm_recovery_snapshot: fault.recovery_dm_snapshot(),
    };
    collector.write_text(
        scenario.case_name,
        "fault-evidence.json",
        &serde_json::to_string_pretty(&recovered_evidence)?,
    )?;
    let pre_recommit_report =
        checker::check_s3_history(&s3, &history, true, workload_plan.concurrency).await?;
    collector.write_text(
        scenario.case_name,
        "checker-pre-recommit-report.json",
        &serde_json::to_string_pretty(&pre_recommit_report)?,
    )?;
    if let Err(error) = pre_recommit_report.require_success() {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "checker-pre-recommit-verdict",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }
    let recommit_report = recommit_unconfirmed_objects(
        &s3,
        &history,
        &workload.unconfirmed_puts,
        workload_plan.concurrency,
    )
    .await;
    collector.write_text(
        scenario.case_name,
        "recommit-report.json",
        &serde_json::to_string_pretty(&recommit_report)?,
    )?;
    workload.summary.recommitted_after_recovery = recommit_report.committed;
    collector.write_text(
        scenario.case_name,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload.summary)?,
    )?;
    if recommit_report.has_failures() {
        let message = recommit_report.failure_message();
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "recommit-unconfirmed",
                recommit_report.failure_classification(),
                message.clone(),
            ),
        )?;
        bail!("{message}");
    }
    let report = checker::check_s3_history(&s3, &history, true, workload_plan.concurrency).await?;
    collector.write_text(
        scenario.case_name,
        "checker-report.json",
        &serde_json::to_string_pretty(&report)?,
    )?;
    let evidence = FaultEvidence {
        scenario: scenario.name.clone(),
        backend: plan.backend_summary(),
        target: plan.target_summary(),
        injected: true,
        active_during_workload: true,
        recovered: report.tenant_recovered,
        client_disruptions: workload.summary.disrupted(),
        workload_plan,
        pods_before,
        pods_after,
        active_snapshots,
        workload_snapshots,
        dm_recovery_snapshot: fault.recovery_dm_snapshot(),
    };
    collector.write_text(
        scenario.case_name,
        "fault-evidence.json",
        &serde_json::to_string_pretty(&evidence)?,
    )?;
    if let Err(error) = report.require_success() {
        write_failure_summary(
            collector,
            scenario.case_name,
            FailureSummary::new(
                &scenario.name,
                "checker-verdict",
                "product_or_environment",
                error.to_string(),
            ),
        )?;
        return Err(error);
    }

    Ok(())
}

fn require_fault_backends(config: &FaultTestConfig, plan: &FaultPlan) -> Result<()> {
    for backend in plan.required_backends() {
        require_fault_backend(config, backend)?;
    }
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
        FaultBackend::ChaosMeshStressChaos => chaos_mesh::require_stresschaos_crd(cluster),
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

fn cleanup_fault_backends(config: &FaultTestConfig, plan: &FaultPlan) -> Result<()> {
    for backend in plan.required_backends() {
        cleanup_fault_backend(config, backend)?;
    }
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
        FaultBackend::ChaosMeshStressChaos => {
            chaos_mesh::cleanup_managed_stresschaos(&config.cluster, &config.chaos_namespace)
        }
        FaultBackend::DeviceMapper => Ok(()),
    }
}

fn prepare_fault_fixture(config: &ClusterTestConfig, isolation: FaultIsolation) -> Result<()> {
    match isolation {
        FaultIsolation::ReusableTenant => fixture::apply_tenant_resources(config)?,
        FaultIsolation::FreshTenant | FaultIsolation::DedicatedLinuxBlockDevice => {
            fixture::reset_tenant_resources(config)?;
            fixture::apply_tenant_resources(config)?;
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

struct AppliedFaults {
    items: Vec<AppliedFault>,
}

impl AppliedFaults {
    fn apply(
        config: &FaultTestConfig,
        collector: &ArtifactCollector,
        scenario: &FaultScenario,
        plan: &FaultPlan,
        run_id: &str,
    ) -> Result<Self> {
        ensure!(
            !plan.faults().is_empty(),
            "fault plan {} did not contain any faults",
            plan.scenario
        );

        let total = plan.faults().len();
        let mut items = Vec::with_capacity(total);
        for (index, injection) in plan.faults().iter().enumerate() {
            let manifest_name = chaos_manifest_artifact_name(total, index, injection);
            let resource_name_suffix = chaos_resource_name_suffix(total, index);
            items.push(AppliedFault::apply_one(
                config,
                collector,
                scenario,
                injection,
                run_id,
                &manifest_name,
                &resource_name_suffix,
            )?);
        }

        Ok(Self { items })
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn wait_active(&self, timeout: Duration) -> Result<()> {
        for fault in &self.items {
            fault.wait_active(timeout)?;
        }
        Ok(())
    }

    fn ensure_active(&self, stage: &str) -> Result<()> {
        for fault in &self.items {
            fault.ensure_active(stage)?;
        }
        Ok(())
    }

    fn delete(&mut self, timeout: Duration) -> Result<()> {
        for fault in self.items.iter_mut().rev() {
            fault.delete(timeout)?;
        }
        Ok(())
    }

    fn snapshot(&self, stage: &str) -> Result<FaultStatusSnapshot> {
        ensure!(
            self.items.len() == 1,
            "single fault snapshot requested for {} applied faults",
            self.items.len()
        );
        self.items[0].snapshot(stage)
    }

    fn snapshots(&self, stage: &str) -> Result<Vec<FaultStatusSnapshot>> {
        self.items
            .iter()
            .map(|fault| fault.snapshot(stage))
            .collect()
    }

    fn recovery_dm_snapshot(&self) -> Option<DmStatusSnapshot> {
        self.items
            .iter()
            .find_map(AppliedFault::recovery_dm_snapshot)
    }

    fn chaos_guards(&self) -> Vec<&ChaosGuard> {
        self.items
            .iter()
            .filter_map(AppliedFault::chaos_guard)
            .collect()
    }
}

impl AppliedFault {
    fn apply_one(
        config: &FaultTestConfig,
        collector: &ArtifactCollector,
        scenario: &FaultScenario,
        injection: &FaultInjection,
        run_id: &str,
        manifest_name: &str,
        resource_name_suffix: &str,
    ) -> Result<Self> {
        let cluster = &config.cluster;
        match injection.kind() {
            FaultKind::RustfsVolumeEnospc => {
                let chaos = IoChaosSpec::enospc_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.rustfs_volume_path()?,
                    injection.percent()?,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsVolumeReadMistake => {
                let chaos = IoChaosSpec::read_mistake_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.rustfs_volume_path()?,
                    injection.percent()?,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsVolumeLatency => {
                let chaos = IoChaosSpec::latency_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.rustfs_volume_path()?,
                    injection.percent()?,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsVolumeIoError => {
                let chaos = IoChaosSpec::eio_on_rustfs_volume(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.rustfs_volume_path()?,
                    injection.percent()?,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_iochaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsServerPodKill => {
                let before_pods = rustfs_pod_identities(cluster)?;
                let chaos = PodChaosSpec::kill_one_rustfs_pod(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                )
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::PodKill {
                    guard: Box::new(chaos_mesh::apply_podchaos(cluster, &chaos)?),
                    before_pods,
                    config: Box::new(cluster.clone()),
                })
            }
            FaultKind::RustfsServerPodFailure => {
                let chaos = PodChaosSpec::fail_one_rustfs_pod(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_podchaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsServerNetworkPartition => {
                let chaos = NetworkChaosSpec::partition_one_rustfs_pod(
                    cluster,
                    &config.chaos_namespace,
                    run_id,
                    &scenario.name,
                    injection.duration(),
                )?
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_networkchaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsServerNetworkDelay
            | FaultKind::RustfsServerNetworkLoss
            | FaultKind::RustfsServerNetworkCorrupt
            | FaultKind::RustfsServerNetworkDuplicate => {
                let chaos = match injection.kind() {
                    FaultKind::RustfsServerNetworkDelay => NetworkChaosSpec::delay_one_rustfs_pod(
                        cluster,
                        &config.chaos_namespace,
                        run_id,
                        &scenario.name,
                        injection.duration(),
                    )?,
                    FaultKind::RustfsServerNetworkLoss => NetworkChaosSpec::loss_one_rustfs_pod(
                        cluster,
                        &config.chaos_namespace,
                        run_id,
                        &scenario.name,
                        injection.duration(),
                    )?,
                    FaultKind::RustfsServerNetworkCorrupt => {
                        NetworkChaosSpec::corrupt_one_rustfs_pod(
                            cluster,
                            &config.chaos_namespace,
                            run_id,
                            &scenario.name,
                            injection.duration(),
                        )?
                    }
                    FaultKind::RustfsServerNetworkDuplicate => {
                        NetworkChaosSpec::duplicate_one_rustfs_pod(
                            cluster,
                            &config.chaos_namespace,
                            run_id,
                            &scenario.name,
                            injection.duration(),
                        )?
                    }
                    _ => unreachable!(),
                }
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_networkchaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsServerCpuStress | FaultKind::RustfsServerMemoryStress => {
                let chaos = match injection.kind() {
                    FaultKind::RustfsServerCpuStress => StressChaosSpec::cpu_on_one_rustfs_pod(
                        cluster,
                        &config.chaos_namespace,
                        run_id,
                        &scenario.name,
                        injection.duration(),
                    )?,
                    FaultKind::RustfsServerMemoryStress => {
                        StressChaosSpec::memory_on_one_rustfs_pod(
                            cluster,
                            &config.chaos_namespace,
                            run_id,
                            &scenario.name,
                            injection.duration(),
                        )?
                    }
                    _ => unreachable!(),
                }
                .with_name_suffix(resource_name_suffix);
                collector.write_text(scenario.case_name, manifest_name, &chaos.manifest())?;
                Ok(Self::Chaos {
                    guard: Box::new(chaos_mesh::apply_stresschaos(cluster, &chaos)?),
                    active_required: true,
                })
            }
            FaultKind::RustfsBlockDeviceFlakey => {
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
                Ok(Self::DmFlakey(Box::new(host::apply_dm_flakey(
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

fn chaos_manifest_artifact_name(total: usize, index: usize, injection: &FaultInjection) -> String {
    if total == 1 {
        "chaos-manifest.yaml".to_string()
    } else {
        format!(
            "chaos-manifest-{index:02}-{}.yaml",
            injection.kind().as_str()
        )
    }
}

fn chaos_resource_name_suffix(total: usize, index: usize) -> String {
    if total == 1 {
        String::new()
    } else {
        format!("-{index:02}")
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
    active_snapshots: Vec<FaultStatusSnapshot>,
    workload_snapshots: Vec<FaultStatusSnapshot>,
    dm_recovery_snapshot: Option<DmStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
struct RunMetadata {
    scenario: String,
    case_name: String,
    run_id: String,
    bucket: String,
    backend: String,
    target: String,
    context: String,
    namespace: String,
    tenant: String,
    storage_class: String,
    rustfs_image: String,
    artifacts_dir: String,
    duration_seconds: u64,
    percent: Option<u8>,
    fault_selection: Vec<String>,
    workload_objects: usize,
    workload_concurrency: usize,
    request_timeout_seconds: u64,
    use_cluster_ip: bool,
    require_client_disruption: bool,
    chaos_namespace: String,
}

impl RunMetadata {
    fn from_case(
        config: &FaultTestConfig,
        scenario: &FaultScenario,
        plan: &FaultPlan,
        run_id: &str,
        bucket: &str,
    ) -> Self {
        Self {
            scenario: scenario.name.clone(),
            case_name: scenario.case_name.to_string(),
            run_id: run_id.to_string(),
            bucket: bucket.to_string(),
            backend: plan.backend_summary(),
            target: plan.target_summary(),
            context: config.cluster.context.clone(),
            namespace: config.cluster.test_namespace.clone(),
            tenant: config.cluster.tenant_name.clone(),
            storage_class: config.cluster.storage_class.clone(),
            rustfs_image: config.cluster.rustfs_image.clone(),
            artifacts_dir: config.cluster.artifacts_dir.display().to_string(),
            duration_seconds: scenario.duration.as_secs(),
            percent: plan
                .faults()
                .iter()
                .find_map(|fault| match fault.selection() {
                    FaultSelection::Percent(percent) => Some(percent),
                    FaultSelection::FixedTargets(_) => None,
                }),
            fault_selection: plan
                .faults()
                .iter()
                .map(|fault| fault.selection().summary())
                .collect(),
            workload_objects: scenario.object_count,
            workload_concurrency: config.workload.concurrency,
            request_timeout_seconds: config.request_timeout.as_secs(),
            use_cluster_ip: config.use_cluster_ip,
            require_client_disruption: config.require_client_disruption,
            chaos_namespace: config.chaos_namespace.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct FailureSummary {
    scenario: String,
    stage: String,
    classification: String,
    message: String,
}

impl FailureSummary {
    fn new(
        scenario: impl Into<String>,
        stage: impl Into<String>,
        classification: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            scenario: scenario.into(),
            stage: stage.into(),
            classification: classification.into(),
            message: message.into(),
        }
    }
}

fn write_failure_summary(
    collector: &ArtifactCollector,
    case_name: &str,
    summary: FailureSummary,
) -> Result<()> {
    collector.write_text(
        case_name,
        "failure-summary.json",
        &serde_json::to_string_pretty(&summary)?,
    )?;
    Ok(())
}

fn write_failure_summary_if_absent(
    collector: &ArtifactCollector,
    case_name: &str,
    summary: FailureSummary,
) -> Result<()> {
    let path = collector.case_dir(case_name).join("failure-summary.json");
    if path.exists() {
        return Ok(());
    }
    write_failure_summary(collector, case_name, summary)
}

fn collect_fault_artifacts(
    collector: &ArtifactCollector,
    case_name: &str,
    fault: &AppliedFaults,
    suffix: &str,
) -> Result<()> {
    let status = if fault.len() == 1 {
        fault
            .snapshot(suffix)
            .and_then(|snapshot| serde_json::to_string_pretty(&snapshot).map_err(Into::into))
    } else {
        fault
            .snapshots(suffix)
            .and_then(|snapshots| serde_json::to_string_pretty(&snapshots).map_err(Into::into))
    }
    .unwrap_or_else(|error| format!("failed to collect fault status: {error}"));
    collector.write_text(case_name, &format!("fault-status-{suffix}.json"), &status)?;

    let guards = fault.chaos_guards();
    for (index, guard) in guards.iter().enumerate() {
        let describe = guard
            .describe()
            .unwrap_or_else(|error| format!("failed to describe chaos before cleanup: {error}"));
        let describe_name =
            chaos_artifact_name(guards.len(), index, "chaos-describe", suffix, "txt");
        collector.write_text(case_name, &describe_name, &describe)?;

        let yaml = guard
            .yaml()
            .unwrap_or_else(|error| format!("failed to get chaos yaml before cleanup: {error}"));
        let yaml_name = chaos_artifact_name(guards.len(), index, "chaos", suffix, "yaml");
        collector.write_text(case_name, &yaml_name, &yaml)?;
    }

    Ok(())
}

fn chaos_artifact_name(
    total: usize,
    index: usize,
    prefix: &str,
    suffix: &str,
    extension: &str,
) -> String {
    if total == 1 {
        format!("{prefix}-{suffix}.{extension}")
    } else {
        format!("{prefix}-{suffix}-{index:02}.{extension}")
    }
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
            let write_outcome = s3.put_object(&object, &history).await?;
            ensure!(
                write_outcome == OperationOutcome::Ok,
                "prefill PUT failed before fault injection for key {}: {:?}",
                spec.key,
                write_outcome
            );
            verify_prefill_object(&s3, &history, &spec).await?;
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

async fn verify_prefill_object(
    s3: &S3WorkloadClient,
    history: &Recorder,
    spec: &ObjectSpec,
) -> Result<()> {
    let mut last_outcome = None;
    for attempt in 1..=PREFILL_VERIFY_ATTEMPTS {
        let get = s3.get_object_result(&spec.key, history).await?;
        last_outcome = Some(get.outcome);
        match get.outcome {
            OperationOutcome::Ok => {
                let body = get.body.as_deref().with_context(|| {
                    format!(
                        "prefill GET verification returned no body before fault injection for key {}",
                        spec.key
                    )
                })?;
                ensure!(
                    spec.matches_body(body),
                    "prefill GET verification returned mismatched bytes before fault injection for key {}: expected size={} sha256={}, got size={} sha256={}",
                    spec.key,
                    spec.size_bytes,
                    spec.sha256,
                    body.len(),
                    sha256_hex(body)
                );
                return Ok(());
            }
            OperationOutcome::Timeout | OperationOutcome::Unknown
                if attempt < PREFILL_VERIFY_ATTEMPTS =>
            {
                async_sleep(PREFILL_VERIFY_RETRY_DELAY).await;
            }
            _ => break,
        }
    }

    bail!(
        "prefill GET verification failed before fault injection for key {} after {} attempt(s): {:?}",
        spec.key,
        PREFILL_VERIFY_ATTEMPTS,
        last_outcome
    )
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
            let mut result = MixedTaskResult::new(index);
            match offset % 6 {
                0 => {
                    let object = ObjectSpec::prepare_seeded(&run_id, index, size_bytes, seed);
                    let spec = object.spec.clone();
                    let verified = s3.put_and_verify_object(&object, &history).await?;
                    result.puts.push(verified.write_outcome);
                    if let Some(get_outcome) = verified.verify_get_outcome {
                        result.gets.push(get_outcome);
                    }
                    if verified.write_outcome != OperationOutcome::Ok {
                        result.unconfirmed_puts.push(spec);
                    }
                }
                1 => {
                    let object = existing.prepare_overwrite(index as u64 + 1);
                    let spec = object.spec.clone();
                    let verified = s3.put_and_verify_object(&object, &history).await?;
                    result.puts.push(verified.write_outcome);
                    if let Some(get_outcome) = verified.verify_get_outcome {
                        result.gets.push(get_outcome);
                    }
                    if verified.write_outcome != OperationOutcome::Ok {
                        result.unconfirmed_puts.push(spec);
                    }
                }
                2 => {
                    result
                        .gets
                        .push(s3.get_object_result(&existing.key, &history).await?.outcome);
                }
                3 => {
                    let prefix = ObjectSpec::key_prefix(&run_id);
                    let outcome = if s3.list_prefix(&prefix, &history).await?.is_some() {
                        OperationOutcome::Ok
                    } else {
                        OperationOutcome::Unknown
                    };
                    result.lists.push(outcome);
                }
                4 => {
                    let (delete_outcome, verify_get) =
                        s3.delete_and_verify_absent(&existing.key, &history).await?;
                    result.deletes.push(delete_outcome);
                    if let Some(get_outcome) = verify_get {
                        result.gets.push(get_outcome);
                    }
                }
                _ => {
                    let object = ObjectSpec::prepare_seeded(&run_id, index, size_bytes, seed);
                    let spec = object.spec.clone();
                    let complete_outcome = s3.complete_multipart_object(&object, &history).await?;
                    result.multipart_completes.push(complete_outcome);
                    if complete_outcome == OperationOutcome::Ok {
                        result
                            .gets
                            .push(s3.get_object_result(&spec.key, &history).await?.outcome);
                    } else {
                        result.unconfirmed_puts.push(spec);
                    }
                    let abort_object = ObjectSpec::prepare_seeded(
                        &run_id,
                        plan.object_count + index,
                        4 * 1024,
                        seed,
                    );
                    result
                        .multipart_aborts
                        .push(s3.abort_multipart_object(&abort_object, &history).await?);
                }
            }
            Ok::<_, anyhow::Error>(result)
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
        summary.record_all(&result);
        unconfirmed_puts.extend(result.unconfirmed_puts);
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
) -> RecommitReport {
    let tasks = objects.iter().cloned().map(|object| {
        let s3 = s3.clone();
        let history = history.clone();
        async move {
            let prepared = object.prepare();
            match s3.put_object_record(&prepared, &history).await {
                Ok(record) => {
                    let verify_get_outcome = if record.outcome == OperationOutcome::Ok {
                        match s3.get_object_result(&object.key, &history).await {
                            Ok(get) => Some(get.outcome),
                            Err(_) => Some(OperationOutcome::Unknown),
                        }
                    } else {
                        None
                    };
                    RecommitAttempt::from_record(object, record, verify_get_outcome)
                }
                Err(error) => {
                    RecommitAttempt::from_harness_error(object, format!("record PUT: {error}"))
                }
            }
        }
    });
    let mut attempts = stream::iter(tasks)
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
    attempts.sort_by(|left, right| left.key.cmp(&right.key));
    RecommitReport::from_attempts(attempts)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RecommitReport {
    attempted: usize,
    committed: usize,
    failed: usize,
    harness_errors: usize,
    attempts: Vec<RecommitAttempt>,
}

impl RecommitReport {
    fn from_attempts(attempts: Vec<RecommitAttempt>) -> Self {
        let committed = attempts
            .iter()
            .filter(|attempt| attempt.outcome == Some(OperationOutcome::Ok))
            .count();
        let failed = attempts
            .iter()
            .filter(|attempt| attempt.is_s3_failure() || attempt.verify_get_failed())
            .count();
        let harness_errors = attempts
            .iter()
            .filter(|attempt| attempt.is_harness_error())
            .count();
        Self {
            attempted: attempts.len(),
            committed,
            failed,
            harness_errors,
            attempts,
        }
    }

    fn has_failures(&self) -> bool {
        self.failed > 0 || self.harness_errors > 0
    }

    fn failure_classification(&self) -> &'static str {
        if self.harness_errors > 0 {
            "test_harness"
        } else {
            "product_or_environment"
        }
    }

    fn failure_message(&self) -> String {
        let sample = self
            .attempts
            .iter()
            .filter_map(RecommitAttempt::failure_sample)
            .take(5)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} of {} previously unconfirmed PUTs did not commit after recovery; harness_errors={}{}",
            self.failed,
            self.attempted,
            self.harness_errors,
            if sample.is_empty() {
                String::new()
            } else {
                format!("; sample: {sample}")
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RecommitAttempt {
    key: String,
    size_bytes: usize,
    sha256: String,
    outcome: Option<OperationOutcome>,
    verify_get_outcome: Option<OperationOutcome>,
    http_status: Option<u16>,
    error: Option<String>,
    harness_error: Option<String>,
}

impl RecommitAttempt {
    fn from_record(
        object: ObjectSpec,
        record: OperationRecord,
        verify_get_outcome: Option<OperationOutcome>,
    ) -> Self {
        Self {
            key: object.key,
            size_bytes: object.size_bytes,
            sha256: object.sha256,
            outcome: Some(record.outcome),
            verify_get_outcome,
            http_status: record.http_status,
            error: record.error,
            harness_error: None,
        }
    }

    fn from_harness_error(object: ObjectSpec, error: String) -> Self {
        Self {
            key: object.key,
            size_bytes: object.size_bytes,
            sha256: object.sha256,
            outcome: None,
            verify_get_outcome: None,
            http_status: None,
            error: None,
            harness_error: Some(error),
        }
    }

    fn is_s3_failure(&self) -> bool {
        matches!(
            self.outcome,
            Some(
                OperationOutcome::NotFound
                    | OperationOutcome::Failed
                    | OperationOutcome::Timeout
                    | OperationOutcome::Unknown
            )
        )
    }

    fn is_harness_error(&self) -> bool {
        self.harness_error.is_some()
    }

    fn verify_get_failed(&self) -> bool {
        self.outcome == Some(OperationOutcome::Ok)
            && self.verify_get_outcome != Some(OperationOutcome::Ok)
    }

    fn failure_sample(&self) -> Option<String> {
        if let Some(error) = &self.harness_error {
            return Some(format!("{}=harness_error({error})", self.key));
        }
        let outcome = self.outcome?;
        if outcome == OperationOutcome::Ok {
            if self.verify_get_failed() {
                return Some(format!(
                    "{}=verify_get({:?})",
                    self.key, self.verify_get_outcome
                ));
            }
            return None;
        }
        let status = self
            .http_status
            .map(|status| format!(" status={status}"))
            .unwrap_or_default();
        let error = self
            .error
            .as_ref()
            .map(|error| format!(" error={error}"))
            .unwrap_or_default();
        Some(format!("{}={outcome:?}{status}{error}", self.key))
    }
}

#[derive(Debug)]
struct MixedTaskResult {
    index: usize,
    puts: Vec<OperationOutcome>,
    gets: Vec<OperationOutcome>,
    deletes: Vec<OperationOutcome>,
    lists: Vec<OperationOutcome>,
    multipart_completes: Vec<OperationOutcome>,
    multipart_aborts: Vec<OperationOutcome>,
    unconfirmed_puts: Vec<ObjectSpec>,
}

impl MixedTaskResult {
    fn new(index: usize) -> Self {
        Self {
            index,
            puts: Vec::new(),
            gets: Vec::new(),
            deletes: Vec::new(),
            lists: Vec::new(),
            multipart_completes: Vec::new(),
            multipart_aborts: Vec::new(),
            unconfirmed_puts: Vec::new(),
        }
    }
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
    deletes: OutcomeCounts,
    lists: OutcomeCounts,
    multipart_completes: OutcomeCounts,
    multipart_aborts: OutcomeCounts,
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
            deletes: OutcomeCounts::default(),
            lists: OutcomeCounts::default(),
            multipart_completes: OutcomeCounts::default(),
            multipart_aborts: OutcomeCounts::default(),
            recommitted_after_recovery: 0,
        }
    }

    fn record_all(&mut self, result: &MixedTaskResult) {
        for outcome in &result.puts {
            self.puts.record(*outcome);
        }
        for outcome in &result.gets {
            self.gets.record(*outcome);
        }
        for outcome in &result.deletes {
            self.deletes.record(*outcome);
        }
        for outcome in &result.lists {
            self.lists.record(*outcome);
        }
        for outcome in &result.multipart_completes {
            self.multipart_completes.record(*outcome);
        }
        for outcome in &result.multipart_aborts {
            self.multipart_aborts.record(*outcome);
        }
    }

    fn require_exercised(&self) -> Result<()> {
        ensure!(
            self.puts.total() > 0
                && self.gets.total() > 0
                && self.deletes.total() > 0
                && self.lists.total() > 0
                && self.multipart_completes.total() > 0
                && self.multipart_aborts.total() > 0,
            "fault workload did not exercise every required S3 object path: {self:?}"
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
        self.puts.disrupted()
            + self.gets.disrupted()
            + self.deletes.disrupted()
            + self.lists.disrupted()
            + self.multipart_completes.disrupted()
            + self.multipart_aborts.disrupted()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
struct OutcomeCounts {
    ok: usize,
    not_found: usize,
    failed: usize,
    timeout: usize,
    unknown: usize,
}

impl OutcomeCounts {
    fn record(&mut self, outcome: OperationOutcome) {
        match outcome {
            OperationOutcome::Ok => self.ok += 1,
            OperationOutcome::NotFound => self.not_found += 1,
            OperationOutcome::Failed => self.failed += 1,
            OperationOutcome::Timeout => self.timeout += 1,
            OperationOutcome::Unknown => self.unknown += 1,
        }
    }

    fn total(&self) -> usize {
        self.ok + self.not_found + self.failed + self.timeout + self.unknown
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
        OutcomeCounts, PodIdentity, PodRuntimeState, RecommitAttempt, RecommitReport,
        WorkloadSummary, bucket_name, chaos_manifest_artifact_name, chaos_resource_name_suffix,
        pod_deletion_observed, pod_replacement_observed, stable_pod_fingerprint, warp_bucket_name,
    };
    use crate::fault::history::OperationOutcome;
    use crate::fault::plan::{
        DEFAULT_RUSTFS_DATA_VOLUME, FaultInjection, FaultKind, FaultSelection, FaultTarget,
    };
    use crate::fault::scenarios::FaultBackend;
    use crate::fault::workload::WorkloadPlan;

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
    fn composite_fault_artifacts_and_resource_names_are_indexed() {
        let injection = FaultInjection::new(
            FaultKind::RustfsVolumeIoError,
            FaultBackend::ChaosMeshIoChaos,
            FaultTarget::RustfsVolume {
                path: DEFAULT_RUSTFS_DATA_VOLUME,
            },
            FaultSelection::Percent(20),
            std::time::Duration::from_secs(60),
        )
        .expect("valid fault");

        assert_eq!(
            chaos_manifest_artifact_name(1, 0, &injection),
            "chaos-manifest.yaml"
        );
        assert_eq!(chaos_resource_name_suffix(1, 0), "");
        assert_eq!(
            chaos_manifest_artifact_name(2, 1, &injection),
            "chaos-manifest-01-rustfs_volume_io_error.yaml"
        );
        assert_eq!(chaos_resource_name_suffix(2, 1), "-01");
    }

    #[test]
    fn workload_summary_counts_disrupted_operations() {
        let mut summary = WorkloadSummary::new(&WorkloadPlan::seeded(42, 40000, 80));
        summary.puts.record(OperationOutcome::Ok);
        summary.gets.record(OperationOutcome::Timeout);
        summary.gets.record(OperationOutcome::NotFound);
        summary.deletes.record(OperationOutcome::Ok);
        summary.lists.record(OperationOutcome::Ok);
        summary.multipart_completes.record(OperationOutcome::Ok);
        summary.multipart_aborts.record(OperationOutcome::Ok);

        assert_eq!(summary.puts.total(), 1);
        assert_eq!(summary.gets.total(), 2);
        assert_eq!(summary.disrupted(), 1);
        assert!(summary.require_exercised().is_ok());
        assert!(summary.require_fault_evidence(true).is_ok());
    }

    #[test]
    fn workload_summary_requires_every_object_operation_family() {
        let mut summary = WorkloadSummary::new(&WorkloadPlan::seeded(42, 40000, 80));
        summary.puts.record(OperationOutcome::Ok);
        summary.gets.record(OperationOutcome::Ok);
        summary.deletes.record(OperationOutcome::Ok);
        summary.lists.record(OperationOutcome::Ok);
        summary.multipart_completes.record(OperationOutcome::Ok);

        assert!(summary.require_exercised().is_err());

        summary.multipart_aborts.record(OperationOutcome::Ok);
        assert!(summary.require_exercised().is_ok());
    }

    #[test]
    fn workload_summary_can_require_fault_evidence() {
        let summary = WorkloadSummary {
            seed: 42,
            object_count: 40000,
            concurrency: 80,
            total_payload_bytes: 20_337_459_200,
            puts: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
            gets: OutcomeCounts {
                ok: 1,
                ..OutcomeCounts::default()
            },
            deletes: OutcomeCounts::default(),
            lists: OutcomeCounts::default(),
            multipart_completes: OutcomeCounts::default(),
            multipart_aborts: OutcomeCounts::default(),
            recommitted_after_recovery: 0,
        };

        assert!(summary.require_fault_evidence(false).is_ok());
        assert!(summary.require_fault_evidence(true).is_err());
    }

    #[test]
    fn recommit_report_counts_and_summarizes_failed_attempts() {
        let report = RecommitReport::from_attempts(vec![
            RecommitAttempt {
                key: "object-a".to_string(),
                size_bytes: 4096,
                sha256: "sha-a".to_string(),
                outcome: Some(OperationOutcome::Ok),
                verify_get_outcome: Some(OperationOutcome::Ok),
                http_status: Some(200),
                error: None,
                harness_error: None,
            },
            RecommitAttempt {
                key: "object-b".to_string(),
                size_bytes: 4096,
                sha256: "sha-b".to_string(),
                outcome: Some(OperationOutcome::Failed),
                verify_get_outcome: None,
                http_status: Some(503),
                error: Some("service unavailable".to_string()),
                harness_error: None,
            },
        ]);

        assert_eq!(report.attempted, 2);
        assert_eq!(report.committed, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.harness_errors, 0);
        assert!(report.has_failures());
        assert!(
            report
                .failure_message()
                .contains("object-b=Failed status=503")
        );
        assert_eq!(report.failure_classification(), "product_or_environment");
    }

    #[test]
    fn recommit_report_separates_harness_errors_from_s3_failures() {
        let report = RecommitReport::from_attempts(vec![RecommitAttempt {
            key: "object-a".to_string(),
            size_bytes: 4096,
            sha256: "sha-a".to_string(),
            outcome: None,
            verify_get_outcome: None,
            http_status: None,
            error: None,
            harness_error: Some("record PUT: disk full".to_string()),
        }]);

        assert_eq!(report.attempted, 1);
        assert_eq!(report.committed, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.harness_errors, 1);
        assert!(report.has_failures());
        assert_eq!(report.failure_classification(), "test_harness");
        assert!(
            report
                .failure_message()
                .contains("object-a=harness_error(record PUT: disk full)")
        );
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
