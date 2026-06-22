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
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::fault::{
    history::{OperationKind, OperationOutcome, OperationRecord, Recorder},
    workload::{GetObjectResult, ObjectSpec, S3WorkloadClient, sha256_hex},
};

const MAX_WARNING_SAMPLES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckerReport {
    pub scenario: String,
    pub run_id: String,
    pub committed_puts: usize,
    pub expected_live_objects: usize,
    pub verified_live_objects: usize,
    pub missing_committed_objects: Vec<String>,
    pub unavailable_committed_objects: Vec<String>,
    pub unknown_committed_read_failures: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub successful_corrupted_reads: Vec<String>,
    pub unexpected_visible_deleted_objects: Vec<String>,
    pub unknown_writes_materialized: Vec<String>,
    pub list_history_warning_count: usize,
    pub final_list_warning_count: usize,
    pub list_history_warnings: Vec<String>,
    pub list_warnings: Vec<String>,
    pub final_listed_objects: Option<usize>,
    pub tenant_recovered: bool,
    pub passed: bool,
}

impl CheckerReport {
    pub fn require_success(&self) -> Result<()> {
        ensure!(
            self.passed,
            "fault checker failed for scenario {} run {}: {}",
            self.scenario,
            self.run_id,
            serde_json::to_string_pretty(self)?
        );
        Ok(())
    }
}

pub async fn check_s3_history(
    s3: &S3WorkloadClient,
    recorder: &Recorder,
    tenant_recovered: bool,
    concurrency: usize,
) -> Result<CheckerReport> {
    let initial_records = recorder.records();
    let model = object_model(&initial_records);
    let read_anomalies = successful_read_anomalies(&initial_records);
    let list_history_warnings = list_history_warnings(&initial_records);
    let mut report = CheckerReport {
        scenario: recorder.scenario(),
        run_id: recorder.run_id(),
        committed_puts: model.committed_writes,
        expected_live_objects: model.live.len(),
        verified_live_objects: 0,
        missing_committed_objects: Vec::new(),
        unavailable_committed_objects: Vec::new(),
        unknown_committed_read_failures: Vec::new(),
        hash_mismatches: Vec::new(),
        successful_corrupted_reads: read_anomalies.corrupted_reads,
        unexpected_visible_deleted_objects: read_anomalies.visible_deleted_objects,
        unknown_writes_materialized: Vec::new(),
        list_history_warning_count: list_history_warnings.total_count,
        final_list_warning_count: 0,
        list_history_warnings: list_history_warnings.samples,
        list_warnings: Vec::new(),
        final_listed_objects: None,
        tenant_recovered,
        passed: false,
    };

    let mut committed_results =
        stream::iter(model.live.clone().into_iter().map(|(key, expected)| {
            let s3 = s3.clone();
            let recorder = recorder.clone();
            async move {
                let get = s3.get_object_result(&key, &recorder).await?;
                Ok::<_, anyhow::Error>((key, expected, get))
            }
        }))
        .buffer_unordered(concurrency);
    while let Some(result) = committed_results.next().await {
        let (key, expected, get) = result?;
        evaluate_committed_get(&mut report, key, &expected, get);
    }

    let mut unknown_results =
        stream::iter(model.unknown_writes.into_iter().map(|(key, attempted)| {
            let s3 = s3.clone();
            let recorder = recorder.clone();
            async move {
                let get = s3.get_object_result(&key, &recorder).await?;
                Ok::<_, anyhow::Error>((key, attempted, get))
            }
        }))
        .buffer_unordered(concurrency);
    while let Some(result) = unknown_results.next().await {
        let (key, attempted, get) = result?;
        if let Some(body) = get.body {
            let actual_hash = sha256_hex(&body);
            report.unknown_writes_materialized.push(format!(
                "{key}: attempted {}, got {actual_hash}",
                attempted.sha256
            ));
        }
    }

    let run_id = recorder.run_id();
    let prefix = ObjectSpec::key_prefix(&run_id);
    let mut final_list_warnings = WarningSummary::default();
    match s3.list_prefix(&prefix, recorder).await? {
        Some(keys) => {
            report.final_listed_objects = Some(keys.len());
            let listed = keys.into_iter().collect::<BTreeSet<_>>();
            for key in model.live.keys() {
                if !listed.contains(key) {
                    final_list_warnings.push(format!(
                        "LIST prefix {prefix} did not include expected live key {key}"
                    ));
                }
            }
            for key in model.deleted {
                if listed.contains(&key) {
                    final_list_warnings
                        .push(format!("LIST prefix {prefix} included deleted key {key}"));
                }
            }
        }
        None => final_list_warnings.push(format!("LIST prefix {prefix} did not complete")),
    }
    report.final_list_warning_count = final_list_warnings.total_count;
    report.list_warnings = final_list_warnings.samples;

    report.missing_committed_objects.sort();
    report.unavailable_committed_objects.sort();
    report.unknown_committed_read_failures.sort();
    report.hash_mismatches.sort();
    report.unknown_writes_materialized.sort();
    report.unexpected_visible_deleted_objects.sort();
    report.list_history_warnings.sort();
    report.list_warnings.sort();
    report.passed = report.tenant_recovered
        && report.missing_committed_objects.is_empty()
        && report.unavailable_committed_objects.is_empty()
        && report.unknown_committed_read_failures.is_empty()
        && report.hash_mismatches.is_empty()
        && report.successful_corrupted_reads.is_empty()
        && report.unexpected_visible_deleted_objects.is_empty()
        && report.final_list_warning_count == 0;

    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedObject {
    sha256: String,
    size_bytes: usize,
}

#[derive(Debug, Default)]
struct ObjectModel {
    live: BTreeMap<String, ExpectedObject>,
    deleted: BTreeSet<String>,
    unknown_writes: BTreeMap<String, ExpectedObject>,
    committed_writes: usize,
}

#[derive(Debug, Default)]
struct ReadAnomalies {
    corrupted_reads: Vec<String>,
    visible_deleted_objects: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WarningSummary {
    total_count: usize,
    samples: Vec<String>,
}

impl WarningSummary {
    fn push(&mut self, warning: String) {
        self.total_count += 1;
        if self.samples.len() < MAX_WARNING_SAMPLES {
            self.samples.push(warning);
        }
    }
}

fn evaluate_committed_get(
    report: &mut CheckerReport,
    key: String,
    expected: &ExpectedObject,
    get: GetObjectResult,
) {
    match (get.outcome, get.body) {
        (OperationOutcome::Ok, Some(body)) => {
            let actual_hash = sha256_hex(&body);
            if actual_hash != expected.sha256 || body.len() != expected.size_bytes {
                report.hash_mismatches.push(format!(
                    "{key}: expected {} ({} bytes), got {actual_hash} ({} bytes)",
                    expected.sha256,
                    expected.size_bytes,
                    body.len()
                ));
            } else {
                report.verified_live_objects += 1;
            }
        }
        (OperationOutcome::NotFound, None) => report.missing_committed_objects.push(key),
        (OperationOutcome::Failed | OperationOutcome::Timeout, None) => report
            .unavailable_committed_objects
            .push(read_failure_message(
                &key,
                get.outcome,
                get.http_status,
                get.error.as_deref(),
            )),
        (OperationOutcome::Unknown, None) | (OperationOutcome::Ok, None) => report
            .unknown_committed_read_failures
            .push(read_failure_message(
                &key,
                get.outcome,
                get.http_status,
                get.error.as_deref(),
            )),
        (outcome, Some(body)) => report.unknown_committed_read_failures.push(format!(
            "{}: unexpected body for {:?} ({} bytes)",
            key,
            outcome,
            body.len()
        )),
    }
}

fn read_failure_message(
    key: &str,
    outcome: OperationOutcome,
    http_status: Option<u16>,
    error: Option<&str>,
) -> String {
    let status = http_status
        .map(|status| format!(" status={status}"))
        .unwrap_or_default();
    let error = error
        .map(|error| format!(" error={error:?}"))
        .unwrap_or_default();
    format!("{key}: outcome={outcome:?}{status}{error}")
}

fn object_model(records: &[OperationRecord]) -> ObjectModel {
    let mut model = ObjectModel::default();
    for record in records {
        apply_record_to_model(&mut model, record);
    }
    model
}

fn object_model_before(records: &[OperationRecord], started_at_ms: u64) -> ObjectModel {
    let mut model = ObjectModel::default();
    for record in records {
        if record.ended_at_ms < started_at_ms {
            apply_record_to_model(&mut model, record);
        }
    }
    model
}

fn apply_record_to_model(model: &mut ObjectModel, record: &OperationRecord) {
    match record.kind {
        OperationKind::Put | OperationKind::CompleteMultipartUpload
            if record.outcome == OperationOutcome::Ok =>
        {
            if let Some((key, object)) = record_object(record) {
                model.committed_writes += 1;
                model.deleted.remove(&key);
                model.live.insert(key, object);
            }
        }
        OperationKind::Put | OperationKind::CompleteMultipartUpload
            if matches!(
                record.outcome,
                OperationOutcome::Timeout | OperationOutcome::Unknown
            ) =>
        {
            if let Some((key, object)) = record_object(record) {
                model.unknown_writes.insert(key, object);
            }
        }
        OperationKind::Delete if record.outcome == OperationOutcome::Ok => {
            if let Some(key) = record.key.clone() {
                model.live.remove(&key);
                model.deleted.insert(key);
            }
        }
        _ => {}
    }
}

fn list_history_warnings(records: &[OperationRecord]) -> WarningSummary {
    let mut warnings = WarningSummary::default();
    for record in records.iter().filter(|record| {
        record.kind == OperationKind::List && record.outcome == OperationOutcome::Ok
    }) {
        let Some(prefix) = record.key.as_deref() else {
            continue;
        };
        let Some(listed_keys) = record.listed_keys.as_ref() else {
            warnings.push(format!("LIST {} did not record returned keys", record.id));
            continue;
        };
        let listed = listed_keys
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let stable = object_model_before(records, record.started_at_ms);
        for key in stable.live.keys().filter(|key| key.starts_with(prefix)) {
            if !listed.contains(key.as_str()) {
                warnings.push(format!(
                    "LIST {} prefix {prefix} did not include stable live key {key}",
                    record.id
                ));
            }
        }
        for key in stable.deleted.iter().filter(|key| key.starts_with(prefix)) {
            if listed.contains(key.as_str()) {
                warnings.push(format!(
                    "LIST {} prefix {prefix} included stable deleted key {key}",
                    record.id
                ));
            }
        }
    }
    warnings
}

fn successful_read_anomalies(records: &[OperationRecord]) -> ReadAnomalies {
    let mut live = BTreeMap::<String, ExpectedObject>::new();
    let mut anomalies = ReadAnomalies::default();
    for record in records {
        match record.kind {
            OperationKind::Put | OperationKind::CompleteMultipartUpload
                if record.outcome == OperationOutcome::Ok =>
            {
                if let Some((key, object)) = record_object(record) {
                    live.insert(key, object);
                }
            }
            OperationKind::Delete if record.outcome == OperationOutcome::Ok => {
                if let Some(key) = record.key.as_ref() {
                    live.remove(key);
                }
            }
            OperationKind::Get if record.outcome == OperationOutcome::Ok => {
                let Some(key) = record.key.as_ref() else {
                    continue;
                };
                let actual_hash = record.value_sha256.as_deref().unwrap_or_default();
                match live.get(key) {
                    Some(expected) if expected.sha256 != actual_hash => {
                        anomalies.corrupted_reads.push(format!(
                            "{key}: expected {}, got {actual_hash}",
                            expected.sha256
                        ));
                    }
                    None => anomalies
                        .visible_deleted_objects
                        .push(format!("{key}: successful GET had no committed live value")),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    anomalies
}

fn record_object(record: &OperationRecord) -> Option<(String, ExpectedObject)> {
    Some((
        record.key.clone()?,
        ExpectedObject {
            sha256: record.value_sha256.clone()?,
            size_bytes: record.size_bytes?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CheckerReport, ExpectedObject, WarningSummary, evaluate_committed_get,
        list_history_warnings, object_model, successful_read_anomalies,
    };
    use crate::fault::history::{OperationKind, OperationOutcome, OperationRecord};
    use crate::fault::workload::GetObjectResult;

    fn record(
        id: &str,
        kind: OperationKind,
        key: &str,
        hash: &str,
        outcome: OperationOutcome,
    ) -> OperationRecord {
        OperationRecord {
            id: id.to_string(),
            scenario: "io-eio".to_string(),
            kind,
            bucket: "bucket".to_string(),
            key: Some(key.to_string()),
            value_sha256: Some(hash.to_string()),
            size_bytes: Some(1),
            started_at_ms: 1,
            ended_at_ms: 2,
            outcome,
            http_status: Some(200),
            error: None,
            listed_keys: None,
        }
    }

    fn list_record(
        id: &str,
        prefix: &str,
        started_at_ms: u64,
        ended_at_ms: u64,
        keys: &[&str],
    ) -> OperationRecord {
        OperationRecord {
            id: id.to_string(),
            scenario: "io-eio".to_string(),
            kind: OperationKind::List,
            bucket: "bucket".to_string(),
            key: Some(prefix.to_string()),
            value_sha256: None,
            size_bytes: Some(keys.len()),
            listed_keys: Some(keys.iter().map(|key| key.to_string()).collect()),
            started_at_ms,
            ended_at_ms,
            outcome: OperationOutcome::Ok,
            http_status: Some(200),
            error: None,
        }
    }

    #[test]
    fn corrupted_successful_get_is_hard_failure_input() {
        let records = vec![
            record(
                "op-1",
                OperationKind::Put,
                "k",
                "good",
                OperationOutcome::Ok,
            ),
            record("op-2", OperationKind::Get, "k", "bad", OperationOutcome::Ok),
        ];

        let anomalies = successful_read_anomalies(&records);

        assert_eq!(anomalies.corrupted_reads, vec!["k: expected good, got bad"]);
    }

    #[test]
    fn object_model_tracks_overwrite_delete_and_multipart_complete() {
        let records = vec![
            record("op-1", OperationKind::Put, "k1", "v1", OperationOutcome::Ok),
            record("op-2", OperationKind::Put, "k1", "v2", OperationOutcome::Ok),
            record("op-3", OperationKind::Put, "k2", "v1", OperationOutcome::Ok),
            record(
                "op-4",
                OperationKind::Delete,
                "k2",
                "",
                OperationOutcome::Ok,
            ),
            record(
                "op-5",
                OperationKind::CompleteMultipartUpload,
                "k3",
                "mp",
                OperationOutcome::Ok,
            ),
        ];

        let model = object_model(&records);

        assert_eq!(model.committed_writes, 4);
        assert_eq!(model.live.get("k1").expect("k1").sha256, "v2");
        assert!(!model.live.contains_key("k2"));
        assert_eq!(model.live.get("k3").expect("k3").sha256, "mp");
        assert!(model.deleted.contains("k2"));
    }

    #[test]
    fn list_history_checks_stable_keys_and_ignores_overlapping_changes() {
        let records = vec![
            OperationRecord {
                started_at_ms: 1,
                ended_at_ms: 2,
                ..record(
                    "op-1",
                    OperationKind::Put,
                    "fault-test/run-1/stable",
                    "v1",
                    OperationOutcome::Ok,
                )
            },
            OperationRecord {
                started_at_ms: 4,
                ended_at_ms: 7,
                ..record(
                    "op-2",
                    OperationKind::Put,
                    "fault-test/run-1/overlap",
                    "v2",
                    OperationOutcome::Ok,
                )
            },
            list_record("op-3", "fault-test/run-1/", 5, 6, &[]),
        ];

        let warnings = list_history_warnings(&records);

        assert_eq!(warnings.total_count, 1);
        assert_eq!(
            warnings.samples,
            vec![
                "LIST op-3 prefix fault-test/run-1/ did not include stable live key fault-test/run-1/stable"
            ]
        );
        assert!(
            !warnings
                .samples
                .iter()
                .any(|warning| warning.contains("overlap"))
        );
    }

    #[test]
    fn committed_get_timeout_is_unavailable_not_missing() {
        let mut report = empty_report();
        let expected = ExpectedObject {
            sha256: "sha".to_string(),
            size_bytes: 1,
        };

        evaluate_committed_get(
            &mut report,
            "k".to_string(),
            &expected,
            GetObjectResult {
                outcome: OperationOutcome::Timeout,
                http_status: Some(200),
                error: Some("get body read timed out".to_string()),
                body: None,
            },
        );

        assert!(report.missing_committed_objects.is_empty());
        assert_eq!(
            report.unavailable_committed_objects,
            vec!["k: outcome=Timeout status=200 error=\"get body read timed out\""]
        );
    }

    #[test]
    fn warning_summary_caps_samples_but_counts_all() {
        let mut warnings = WarningSummary::default();
        for idx in 0..(super::MAX_WARNING_SAMPLES + 3) {
            warnings.push(format!("warning-{idx}"));
        }

        assert_eq!(warnings.total_count, super::MAX_WARNING_SAMPLES + 3);
        assert_eq!(warnings.samples.len(), super::MAX_WARNING_SAMPLES);
    }

    #[test]
    fn report_requires_clean_correctness_verdict() {
        let report = CheckerReport {
            scenario: "io-eio".to_string(),
            run_id: "run-1".to_string(),
            committed_puts: 1,
            expected_live_objects: 1,
            verified_live_objects: 1,
            missing_committed_objects: Vec::new(),
            unavailable_committed_objects: Vec::new(),
            unknown_committed_read_failures: Vec::new(),
            hash_mismatches: Vec::new(),
            successful_corrupted_reads: Vec::new(),
            unexpected_visible_deleted_objects: Vec::new(),
            unknown_writes_materialized: Vec::new(),
            list_history_warning_count: 0,
            final_list_warning_count: 0,
            list_history_warnings: Vec::new(),
            list_warnings: Vec::new(),
            final_listed_objects: Some(1),
            tenant_recovered: true,
            passed: true,
        };

        assert!(report.require_success().is_ok());
    }

    fn empty_report() -> CheckerReport {
        CheckerReport {
            scenario: "io-eio".to_string(),
            run_id: "run-1".to_string(),
            committed_puts: 0,
            expected_live_objects: 0,
            verified_live_objects: 0,
            missing_committed_objects: Vec::new(),
            unavailable_committed_objects: Vec::new(),
            unknown_committed_read_failures: Vec::new(),
            hash_mismatches: Vec::new(),
            successful_corrupted_reads: Vec::new(),
            unexpected_visible_deleted_objects: Vec::new(),
            unknown_writes_materialized: Vec::new(),
            list_history_warning_count: 0,
            final_list_warning_count: 0,
            list_history_warnings: Vec::new(),
            list_warnings: Vec::new(),
            final_listed_objects: None,
            tenant_recovered: true,
            passed: false,
        }
    }
}
