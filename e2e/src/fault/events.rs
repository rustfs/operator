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
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunEventStatus {
    Started,
    Succeeded,
    Failed,
    Observed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    pub at_ms: u64,
    pub scenario: String,
    pub run_id: String,
    pub stage: String,
    pub status: RunEventStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct RunEventRecorder {
    inner: Arc<Mutex<RunEventRecorderState>>,
}

#[derive(Debug)]
pub struct RunEventCompletionGuard {
    recorder: RunEventRecorder,
    stage: String,
    failure_message: String,
    completed: bool,
}

#[derive(Debug)]
struct RunEventRecorderState {
    scenario: String,
    run_id: String,
    writer: BufWriter<File>,
}

impl RunEventRecorder {
    pub fn create(
        path: impl Into<PathBuf>,
        scenario: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let writer = BufWriter::new(File::create(path)?);
        Ok(Self {
            inner: Arc::new(Mutex::new(RunEventRecorderState {
                scenario: scenario.into(),
                run_id: run_id.into(),
                writer,
            })),
        })
    }

    pub fn record(
        &self,
        stage: impl Into<String>,
        status: RunEventStatus,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Result<()> {
        let mut state = self.state();
        let event = RunEvent {
            at_ms: now_ms(),
            scenario: state.scenario.clone(),
            run_id: state.run_id.clone(),
            stage: stage.into(),
            status,
            message: message.into(),
            details,
        };
        serde_json::to_writer(&mut state.writer, &event)?;
        state.writer.write_all(b"\n")?;
        state.writer.flush()?;
        Ok(())
    }

    pub fn completion_guard(
        &self,
        stage: impl Into<String>,
        failure_message: impl Into<String>,
    ) -> RunEventCompletionGuard {
        RunEventCompletionGuard {
            recorder: self.clone(),
            stage: stage.into(),
            failure_message: failure_message.into(),
            completed: false,
        }
    }

    fn state(&self) -> MutexGuard<'_, RunEventRecorderState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl RunEventCompletionGuard {
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for RunEventCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.recorder
                .record(
                    self.stage.clone(),
                    RunEventStatus::Failed,
                    self.failure_message.clone(),
                    None,
                )
                .ok();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{RunEventRecorder, RunEventStatus};

    #[test]
    fn recorder_writes_jsonl_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorder =
            RunEventRecorder::create(dir.path().join("run-events.jsonl"), "io-eio", "run-1")
                .expect("recorder");

        recorder
            .record("prefill", RunEventStatus::Started, "prefill started", None)
            .expect("record");

        let content = std::fs::read_to_string(dir.path().join("run-events.jsonl")).expect("events");
        assert!(content.contains("\"stage\":\"prefill\""));
        assert!(content.contains("\"status\":\"started\""));
        assert!(content.contains("\"run_id\":\"run-1\""));
    }

    #[test]
    fn completion_guard_records_failed_terminal_event_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recorder =
            RunEventRecorder::create(dir.path().join("run-events.jsonl"), "io-eio", "run-1")
                .expect("recorder");

        {
            let _guard = recorder.completion_guard("run", "run failed before completion");
        }

        let content = std::fs::read_to_string(dir.path().join("run-events.jsonl")).expect("events");
        assert!(content.contains("\"stage\":\"run\""));
        assert!(content.contains("\"status\":\"failed\""));
        assert!(content.contains("run failed before completion"));
    }
}
