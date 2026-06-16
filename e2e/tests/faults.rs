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
use kube::Api;
use operator::types::v1alpha1::tenant::Tenant;
use rustfs_operator_e2e::framework::{
    artifacts::ArtifactCollector,
    chaos_mesh::{self, IoChaosSpec},
    checker,
    config::E2eConfig,
    fault_scenarios::FaultScenario,
    history::OperationOutcome,
    history::Recorder,
    kube_client, live,
    port_forward::{PortForwardGuard, PortForwardSpec},
    resources,
    s3_workload::{ObjectSpec, S3WorkloadClient, wait_for_s3_endpoint},
    storage, wait,
};
use serde::Serialize;
use std::time::Duration;
use uuid::Uuid;

const IO_EIO_CASE: &str = "fault_io_eio_preserves_committed_objects";
const RUSTFS_DATA_VOLUME: &str = "/data/rustfs0";
const SMALL_OBJECT_SIZE_BYTES: usize = 4 * 1024;

#[test]
fn faults_are_not_destructive_without_explicit_opt_in() {
    let config = E2eConfig::defaults();

    assert!(!config.destructive_enabled);
    assert!(live::require_destructive_enabled(&config).is_err());
}

#[test]
#[ignore = "reserved for destructive fault scenarios; run through `make e2e-live-faults`"]
fn fault_live_suite_requires_explicit_destructive_opt_in() -> Result<()> {
    let mut config = E2eConfig::from_env();

    live::require_live_enabled(&config)?;
    live::require_destructive_enabled(&config)?;
    let context = live::use_current_context(&mut config)?;
    eprintln!("confirmed destructive fault e2e context: {context}");

    Ok(())
}

#[tokio::test]
#[ignore = "destructive RustFS workload fault scenario; run through `make e2e-live-faults`"]
async fn fault_io_eio_preserves_committed_objects() -> Result<()> {
    let mut config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::require_destructive_enabled(&config)?;
    let context = live::use_current_context(&mut config)?;
    eprintln!("running destructive RustFS fault e2e against current context: {context}");

    let collector = ArtifactCollector::new(&config.artifacts_dir);
    let result = run_io_eio_case(&config, &collector).await;

    if let Err(error) = &result {
        match collector.collect_kubernetes_snapshot(IO_EIO_CASE, &config) {
            Ok(report) => {
                eprintln!("collected e2e artifacts under {}", report.dir.display());
                eprintln!("{}", report.diagnosis);
            }
            Err(artifact_error) => {
                eprintln!("failed to collect e2e artifacts after {error}: {artifact_error}");
            }
        }
    }

    result
}

async fn run_io_eio_case(config: &E2eConfig, collector: &ArtifactCollector) -> Result<()> {
    let scenario = FaultScenario::from_config(config)?;
    chaos_mesh::require_iochaos_crd(config)?;
    chaos_mesh::cleanup_managed_iochaos(config, &config.chaos_namespace)?;

    reset_io_eio_fixture(config)?;
    wait_for_ready_tenant(config).await?;

    let run_id = format!("run-{}", Uuid::new_v4());
    let bucket = bucket_name(&run_id);
    let history_path = collector.case_dir(IO_EIO_CASE).join("history.jsonl");
    let mut history = Recorder::create(history_path, &scenario.name, &run_id)?;

    let port_forward_spec = PortForwardSpec::tenant_io(&config.test_namespace, &config.tenant_name);
    let endpoint = port_forward_spec.local_base_url();
    let mut port_forward = PortForwardSpec::start_tenant_io(config)?;
    wait_for_tenant_s3(&mut port_forward, &endpoint, config.timeout).await?;

    let (access_key, secret_key) = resources::e2e_credentials();
    let s3 = S3WorkloadClient::new(
        &endpoint,
        &bucket,
        access_key,
        secret_key,
        config.fault_request_timeout,
    )
    .await?;
    let bucket_outcome = s3.create_bucket(&mut history).await?;
    ensure!(
        bucket_outcome == OperationOutcome::Ok,
        "fault workload bucket creation did not succeed: {bucket_outcome:?}"
    );

    let prefilled = prefill_objects(&s3, &mut history, &run_id, scenario.prefill_count()).await?;
    let chaos = IoChaosSpec::eio_on_rustfs_volume(
        config,
        &run_id,
        &scenario.name,
        RUSTFS_DATA_VOLUME,
        scenario.percent,
        scenario.duration,
    )?;
    collector.write_text(IO_EIO_CASE, "chaos-manifest.yaml", &chaos.manifest())?;
    let mut guard = chaos_mesh::apply_iochaos(config, &chaos)?;
    match guard.describe() {
        Ok(describe) => {
            collector.write_text(IO_EIO_CASE, "chaos-describe.txt", &describe)?;
        }
        Err(error) => {
            collector.write_text(
                IO_EIO_CASE,
                "chaos-describe.txt",
                &format!("failed to describe IOChaos: {error}"),
            )?;
        }
    }
    if let Err(error) = guard.wait_active(config.timeout) {
        collect_active_chaos_artifacts(collector, &guard, "wait-active-failed")?;
        return Err(error);
    }

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
            collect_active_chaos_artifacts(collector, &guard, "workload-failed")?;
            return Err(error);
        }
    };
    collector.write_text(
        IO_EIO_CASE,
        "workload-summary.json",
        &serde_json::to_string_pretty(&workload_summary)?,
    )?;
    if let Err(error) =
        workload_summary.require_fault_evidence(config.fault_require_client_disruption)
    {
        collect_active_chaos_artifacts(collector, &guard, "workload-no-fault-evidence")?;
        return Err(error);
    }
    if let Err(error) = guard.ensure_active("after fault workload") {
        collect_active_chaos_artifacts(collector, &guard, "workload-outlived-chaos")?;
        return Err(error);
    }

    if let Err(error) = guard.delete() {
        collect_active_chaos_artifacts(collector, &guard, "delete-failed")?;
        return Err(error);
    }

    wait_for_ready_tenant(config).await?;
    let report = checker::check_s3_history(&s3, &mut history, true).await?;
    collector.write_text(
        IO_EIO_CASE,
        "checker-report.json",
        &serde_json::to_string_pretty(&report)?,
    )?;
    report.require_success()?;

    Ok(())
}

fn reset_io_eio_fixture(config: &E2eConfig) -> Result<()> {
    resources::reset_smoke_tenant_resources(config)?;
    if uses_kind_local_storage(config) {
        storage::reset_default_local_storage(config)?;
    } else {
        eprintln!(
            "skipping Kind local storage reset for context {}; using cluster storage class {}",
            config.context, config.storage_class
        );
    }
    resources::apply_smoke_tenant_resources(config)?;
    Ok(())
}

fn uses_kind_local_storage(config: &E2eConfig) -> bool {
    config.context.starts_with("kind-")
}

fn collect_active_chaos_artifacts(
    collector: &ArtifactCollector,
    guard: &chaos_mesh::ChaosGuard,
    suffix: &str,
) -> Result<()> {
    let describe = guard
        .describe()
        .unwrap_or_else(|error| format!("failed to describe IOChaos before cleanup: {error}"));
    collector.write_text(
        IO_EIO_CASE,
        &format!("chaos-describe-{suffix}.txt"),
        &describe,
    )?;

    let yaml = guard
        .yaml()
        .unwrap_or_else(|error| format!("failed to get IOChaos yaml before cleanup: {error}"));
    collector.write_text(IO_EIO_CASE, &format!("chaos-{suffix}.yaml"), &yaml)?;

    Ok(())
}

async fn wait_for_ready_tenant(config: &E2eConfig) -> Result<Tenant> {
    let client = kube_client::default_client().await?;
    let tenants: Api<Tenant> = kube_client::tenant_api(client, &config.test_namespace);
    wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout).await
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
                "IOChaos became active but the S3 workload observed no client-visible disrupted operation; increase RUSTFS_E2E_WORKLOAD_OBJECTS or RUSTFS_E2E_FAULT_PERCENT, or set RUSTFS_E2E_FAULT_REQUIRE_CLIENT_DISRUPTION=0 if this is expected"
            );
        } else if self.disrupted() == 0 {
            eprintln!(
                "IOChaos was active, but the S3 workload observed no client-visible disrupted operation"
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
    use super::{OutcomeCounts, WorkloadSummary, bucket_name};
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
}
