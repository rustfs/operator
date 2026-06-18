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
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::framework::{
    history::{OperationKind, OperationOutcome, OperationRecord, Recorder},
    s3_workload::{ObjectSpec, S3WorkloadClient, sha256_hex},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckerReport {
    pub scenario: String,
    pub run_id: String,
    pub committed_puts: usize,
    pub missing_committed_objects: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub successful_corrupted_reads: Vec<String>,
    pub unknown_writes_materialized: Vec<String>,
    pub list_warnings: Vec<String>,
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
    recorder: &mut Recorder,
    tenant_recovered: bool,
) -> Result<CheckerReport> {
    let initial_records = recorder.records().to_vec();
    let committed = committed_puts(&initial_records);
    let unknown_writes = unknown_puts(&initial_records);
    let mut report = CheckerReport {
        scenario: recorder.scenario().to_string(),
        run_id: recorder.run_id().to_string(),
        committed_puts: committed.len(),
        missing_committed_objects: Vec::new(),
        hash_mismatches: Vec::new(),
        successful_corrupted_reads: successful_corrupted_reads(&initial_records, &committed),
        unknown_writes_materialized: Vec::new(),
        list_warnings: Vec::new(),
        tenant_recovered,
        passed: false,
    };

    for (key, expected_hash) in &committed {
        match s3.get_object(key, recorder).await? {
            Some(body) => {
                let actual_hash = sha256_hex(&body);
                if actual_hash != *expected_hash {
                    report.hash_mismatches.push(format!(
                        "{key}: expected {expected_hash}, got {actual_hash}"
                    ));
                }
            }
            None => report.missing_committed_objects.push(key.clone()),
        }
    }

    for (key, attempted_hash) in &unknown_writes {
        if let Some(body) = s3.get_object(key, recorder).await? {
            let actual_hash = sha256_hex(&body);
            report.unknown_writes_materialized.push(format!(
                "{key}: attempted {attempted_hash}, got {actual_hash}"
            ));
        }
    }

    let prefix = ObjectSpec::key_prefix(recorder.run_id());
    match s3.list_prefix(&prefix, recorder).await? {
        Some(keys) => {
            let listed = keys.into_iter().collect::<BTreeSet<_>>();
            for key in committed.keys() {
                if !listed.contains(key) {
                    report.list_warnings.push(format!(
                        "LIST prefix {prefix} did not include committed key {key}"
                    ));
                }
            }
        }
        None => report
            .list_warnings
            .push(format!("LIST prefix {prefix} did not complete")),
    }

    report.passed = report.tenant_recovered
        && report.missing_committed_objects.is_empty()
        && report.hash_mismatches.is_empty()
        && report.successful_corrupted_reads.is_empty();

    Ok(report)
}

fn committed_puts(records: &[OperationRecord]) -> BTreeMap<String, String> {
    records
        .iter()
        .filter(|record| {
            record.kind == OperationKind::Put && record.outcome == OperationOutcome::Ok
        })
        .filter_map(|record| Some((record.key.clone()?, record.value_sha256.clone()?)))
        .collect()
}

fn unknown_puts(records: &[OperationRecord]) -> BTreeMap<String, String> {
    records
        .iter()
        .filter(|record| {
            record.kind == OperationKind::Put
                && matches!(
                    record.outcome,
                    OperationOutcome::Timeout | OperationOutcome::Unknown
                )
        })
        .filter_map(|record| Some((record.key.clone()?, record.value_sha256.clone()?)))
        .collect()
}

fn successful_corrupted_reads(
    records: &[OperationRecord],
    committed: &BTreeMap<String, String>,
) -> Vec<String> {
    records
        .iter()
        .filter(|record| {
            record.kind == OperationKind::Get && record.outcome == OperationOutcome::Ok
        })
        .filter_map(|record| {
            let key = record.key.as_ref()?;
            let expected_hash = committed.get(key)?;
            let actual_hash = record.value_sha256.as_ref()?;
            (expected_hash != actual_hash)
                .then(|| format!("{key}: expected {expected_hash}, got {actual_hash}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CheckerReport, successful_corrupted_reads};
    use crate::framework::history::{OperationKind, OperationOutcome, OperationRecord};
    use std::collections::BTreeMap;

    fn record(
        kind: OperationKind,
        key: &str,
        hash: &str,
        outcome: OperationOutcome,
    ) -> OperationRecord {
        OperationRecord {
            id: "op-1".to_string(),
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
        }
    }

    #[test]
    fn corrupted_successful_get_is_hard_failure_input() {
        let records = vec![record(OperationKind::Get, "k", "bad", OperationOutcome::Ok)];
        let committed = BTreeMap::from([("k".to_string(), "good".to_string())]);

        let corrupted = successful_corrupted_reads(&records, &committed);

        assert_eq!(corrupted, vec!["k: expected good, got bad"]);
    }

    #[test]
    fn report_requires_clean_correctness_verdict() {
        let report = CheckerReport {
            scenario: "io-eio".to_string(),
            run_id: "run-1".to_string(),
            committed_puts: 1,
            missing_committed_objects: Vec::new(),
            hash_mismatches: Vec::new(),
            successful_corrupted_reads: Vec::new(),
            unknown_writes_materialized: Vec::new(),
            list_warnings: Vec::new(),
            tenant_recovered: true,
            passed: true,
        };

        assert!(report.require_success().is_ok());
    }
}
