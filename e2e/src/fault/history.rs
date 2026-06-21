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
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    CreateBucket,
    Put,
    Get,
    Head,
    List,
    Delete,
    CreateMultipartUpload,
    UploadPart,
    CompleteMultipartUpload,
    AbortMultipartUpload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    Ok,
    NotFound,
    Failed,
    Timeout,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: String,
    pub scenario: String,
    pub kind: OperationKind,
    pub bucket: String,
    pub key: Option<String>,
    pub value_sha256: Option<String>,
    pub size_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_keys: Option<Vec<String>>,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    pub outcome: OperationOutcome,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Recorder {
    inner: Arc<Mutex<RecorderState>>,
}

#[derive(Debug)]
struct RecorderState {
    path: PathBuf,
    scenario: String,
    run_id: String,
    next_id: usize,
    records: Vec<OperationRecord>,
    writer: BufWriter<File>,
}

impl Recorder {
    pub fn create(
        path: impl Into<PathBuf>,
        scenario: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let writer = BufWriter::new(File::create(&path)?);
        Ok(Self {
            inner: Arc::new(Mutex::new(RecorderState {
                path,
                scenario: scenario.into(),
                run_id: run_id.into(),
                next_id: 1,
                records: Vec::new(),
                writer,
            })),
        })
    }

    pub fn begin(
        &self,
        kind: OperationKind,
        bucket: impl Into<String>,
        key: Option<String>,
        value_sha256: Option<String>,
        size_bytes: Option<usize>,
    ) -> OperationRecord {
        let mut state = self.state();
        let id = format!("op-{:06}", state.next_id);
        state.next_id += 1;
        let started_at_ms = now_ms();

        OperationRecord {
            id,
            scenario: state.scenario.clone(),
            kind,
            bucket: bucket.into(),
            key,
            value_sha256,
            size_bytes,
            listed_keys: None,
            started_at_ms,
            ended_at_ms: started_at_ms,
            outcome: OperationOutcome::Unknown,
            http_status: None,
            error: None,
        }
    }

    pub fn finish(
        &self,
        mut record: OperationRecord,
        outcome: OperationOutcome,
        http_status: Option<u16>,
        error: Option<String>,
    ) -> Result<OperationRecord> {
        record.ended_at_ms = now_ms();
        record.outcome = outcome;
        record.http_status = http_status;
        record.error = error.map(|message| truncate_error(&message));

        let mut state = self.state();
        serde_json::to_writer(&mut state.writer, &record)?;
        state.writer.write_all(b"\n")?;
        state.writer.flush()?;
        state.records.push(record.clone());
        Ok(record)
    }

    pub fn records(&self) -> Vec<OperationRecord> {
        self.state().records.clone()
    }

    pub fn scenario(&self) -> String {
        self.state().scenario.clone()
    }

    pub fn run_id(&self) -> String {
        self.state().run_id.clone()
    }

    pub fn path(&self) -> PathBuf {
        self.state().path.clone()
    }

    fn state(&self) -> MutexGuard<'_, RecorderState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn truncate_error(message: &str) -> String {
    const MAX_ERROR_LEN: usize = 300;
    if message.len() <= MAX_ERROR_LEN {
        message.to_string()
    } else {
        format!("{}...", &message[..MAX_ERROR_LEN])
    }
}

#[cfg(test)]
mod tests {
    use super::{OperationKind, OperationOutcome, Recorder};
    use std::collections::BTreeSet;

    #[test]
    fn recorder_writes_jsonl_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.jsonl");
        let recorder = Recorder::create(&path, "io-eio", "run-1").expect("recorder");
        let record = recorder.begin(
            OperationKind::Put,
            "bucket",
            Some("key".to_string()),
            Some("abc".to_string()),
            Some(3),
        );

        recorder
            .finish(record, OperationOutcome::Ok, Some(200), None)
            .expect("finish");

        let content = std::fs::read_to_string(&path).expect("history");
        assert!(content.contains("\"scenario\":\"io-eio\""));
        assert!(content.contains("\"kind\":\"put\""));
        assert_eq!(recorder.records().len(), 1);
        assert_eq!(recorder.path(), path);
    }

    #[test]
    fn recorder_assigns_unique_ids_across_concurrent_writers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorder = Recorder::create(dir.path().join("history.jsonl"), "io-eio", "run-1")
            .expect("recorder");
        let writers = (0..8)
            .map(|writer| {
                let recorder = recorder.clone();
                std::thread::spawn(move || {
                    for operation in 0..25 {
                        let record = recorder.begin(
                            OperationKind::Put,
                            "bucket",
                            Some(format!("{writer}-{operation}")),
                            Some("hash".to_string()),
                            Some(4),
                        );
                        recorder
                            .finish(record, OperationOutcome::Ok, Some(200), None)
                            .expect("finish");
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().expect("writer thread");
        }

        let records = recorder.records();
        let ids = records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(records.len(), 200);
        assert_eq!(ids.len(), 200);
    }
}
