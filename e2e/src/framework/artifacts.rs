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
use std::fs;
use std::path::{Path, PathBuf};

use crate::framework::{command::CommandSpec, config::E2eConfig, kubectl::Kubectl};

const ERASURE_READ_QUORUM: &str = "erasure read quorum";
const DNS_LOOKUP_FAILURE: &str = "failed to lookup address information";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    pub dir: PathBuf,
    pub diagnosis: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotCommand {
    file_name: String,
    command: CommandSpec,
}

#[derive(Debug, Clone)]
pub struct ArtifactCollector {
    root: PathBuf,
}

impl ArtifactCollector {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn case_dir(&self, case_name: &str) -> PathBuf {
        self.root.join(sanitize_case_name(case_name))
    }

    pub fn write_text(&self, case_name: &str, file_name: &str, content: &str) -> Result<PathBuf> {
        let dir = self.case_dir(case_name);
        fs::create_dir_all(&dir)?;
        let path = dir.join(file_name);
        fs::write(&path, content)?;
        Ok(path)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn collect_kubernetes_snapshot(
        &self,
        case_name: &str,
        config: &E2eConfig,
    ) -> Result<SnapshotReport> {
        let mut combined_output = String::new();

        for SnapshotCommand { file_name, command } in kubernetes_snapshot_commands(config) {
            let output = command.run()?;
            let content = format!(
                "$ {cmd}\nexit: {code:?}\n\nstdout:\n{stdout}\n\nstderr:\n{stderr}\n",
                cmd = command.display(),
                code = output.code,
                stdout = output.stdout,
                stderr = output.stderr
            );
            combined_output.push_str(&content);
            combined_output.push('\n');
            self.write_text(case_name, &file_name, &content)?;
        }

        let diagnosis = diagnose_snapshot(&combined_output);
        self.write_text(case_name, "diagnosis.txt", &diagnosis)?;

        Ok(SnapshotReport {
            dir: self.case_dir(case_name),
            diagnosis,
        })
    }
}

fn kubernetes_snapshot_commands(config: &E2eConfig) -> Vec<SnapshotCommand> {
    let kubectl = Kubectl::new(config);
    let operator_kubectl = Kubectl::new(config).namespaced(&config.operator_namespace);
    let test_kubectl = Kubectl::new(config).namespaced(&config.test_namespace);
    let tenant_selector = format!("rustfs.tenant={}", config.tenant_name);
    let pv_jsonpath = concat!(
        "{range .items[*]}",
        "{.metadata.name}{\"\\t\"}",
        "{.spec.local.path}{\"\\t\"}",
        "{.spec.nodeAffinity.required.nodeSelectorTerms[0].matchExpressions[0].values[0]}",
        "{\"\\t\"}{.spec.claimRef.namespace}/{.spec.claimRef.name}{\"\\n\"}",
        "{end}"
    );

    vec![
        SnapshotCommand {
            file_name: "get-all.txt".to_string(),
            command: kubectl.command(["get", "all", "-A", "-o", "wide"]),
        },
        SnapshotCommand {
            file_name: "tenants.yaml".to_string(),
            command: kubectl.command(["get", "tenants", "-A", "-o", "yaml"]),
        },
        SnapshotCommand {
            file_name: "tenant-describe.txt".to_string(),
            command: test_kubectl.command(vec![
                "describe".to_string(),
                "tenant".to_string(),
                config.tenant_name.clone(),
            ]),
        },
        SnapshotCommand {
            file_name: "events.txt".to_string(),
            command: kubectl.command(["get", "events", "-A", "--sort-by=.lastTimestamp"]),
        },
        SnapshotCommand {
            file_name: "operator.log".to_string(),
            command: operator_kubectl.command(["logs", "deployment/rustfs-operator", "--tail=500"]),
        },
        SnapshotCommand {
            file_name: "console.log".to_string(),
            command: operator_kubectl.command([
                "logs",
                "deployment/rustfs-operator-console",
                "--tail=500",
            ]),
        },
        SnapshotCommand {
            file_name: "test-namespace-pods.txt".to_string(),
            command: test_kubectl.command(["get", "pods", "-o", "wide"]),
        },
        SnapshotCommand {
            file_name: "test-namespace-pvcs.txt".to_string(),
            command: test_kubectl.command(["get", "pvc", "-o", "wide"]),
        },
        SnapshotCommand {
            file_name: "pv-paths.txt".to_string(),
            command: kubectl.command(vec![
                "get".to_string(),
                "pv".to_string(),
                "-o".to_string(),
                format!("jsonpath={pv_jsonpath}"),
            ]),
        },
        SnapshotCommand {
            file_name: "rustfs-pods-describe.txt".to_string(),
            command: test_kubectl.command(vec![
                "describe".to_string(),
                "pods".to_string(),
                "-l".to_string(),
                tenant_selector.clone(),
            ]),
        },
        SnapshotCommand {
            file_name: "rustfs-pods-current.log".to_string(),
            command: test_kubectl.command(vec![
                "logs".to_string(),
                "-l".to_string(),
                tenant_selector.clone(),
                "-c".to_string(),
                "rustfs".to_string(),
                "--tail=500".to_string(),
                "--prefix".to_string(),
            ]),
        },
        SnapshotCommand {
            file_name: "rustfs-pods-previous.log".to_string(),
            command: test_kubectl.command(vec![
                "logs".to_string(),
                "-l".to_string(),
                tenant_selector,
                "-c".to_string(),
                "rustfs".to_string(),
                "--previous".to_string(),
                "--tail=500".to_string(),
                "--prefix".to_string(),
            ]),
        },
    ]
}

fn diagnose_snapshot(snapshot: &str) -> String {
    let mut lines = vec![
        "RustFS Operator e2e diagnostic summary".to_string(),
        String::new(),
    ];
    let mut matched = false;

    if snapshot.contains(ERASURE_READ_QUORUM) {
        matched = true;
        lines.extend([
            format!("Detected `{ERASURE_READ_QUORUM}` in RustFS pod logs."),
            "Meaning: RustFS ECStore could not read a majority of matching erasure format metadata during startup.".to_string(),
            "Most likely live-e2e causes: stale or partially initialized data in dedicated local PV host paths, peer startup/DNS timing, or a RustFS bootstrap retry window that ended before quorum converged.".to_string(),
            "Inspect: rustfs-pods-current.log, rustfs-pods-previous.log, tenant-describe.txt, rustfs-pods-describe.txt, and pv-paths.txt.".to_string(),
            "Recovery for the dedicated e2e cluster: RUSTFS_E2E_LIVE=1 make e2e-live-delete && RUSTFS_E2E_LIVE=1 make e2e-live-create && RUSTFS_E2E_LIVE=1 make e2e-live-run".to_string(),
            String::new(),
        ]);
    }

    if snapshot.contains(DNS_LOOKUP_FAILURE) {
        matched = true;
        lines.extend([
            format!("Detected `{DNS_LOOKUP_FAILURE}` in RustFS pod logs."),
            "Meaning: a RustFS peer hostname was not resolvable during early pod startup. Check the headless Service, endpoint publication, and whether pods recovered after restart.".to_string(),
            String::new(),
        ]);
    }

    if !matched {
        lines.push(
            "No built-in RustFS bootstrap signature was detected. Inspect the collected Kubernetes snapshot files for the first failing pod event or container log.".to_string(),
        );
    }

    lines.join("\n")
}

fn sanitize_case_name(case_name: &str) -> String {
    case_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ArtifactCollector, diagnose_snapshot, kubernetes_snapshot_commands};
    use crate::framework::config::E2eConfig;

    #[test]
    fn artifact_paths_are_case_scoped_and_sanitized() {
        let collector = ArtifactCollector::new("target/e2e/artifacts");

        assert_eq!(
            collector.case_dir("console auth/session"),
            std::path::PathBuf::from("target/e2e/artifacts/console_auth_session")
        );
    }

    #[test]
    fn snapshot_collects_tenant_logs_and_pv_paths() {
        let config = E2eConfig::defaults();
        let commands = kubernetes_snapshot_commands(&config);
        let displays: Vec<_> = commands
            .iter()
            .map(|command| command.command.display())
            .collect();

        assert!(
            displays
                .iter()
                .any(|display| display.contains("describe tenant e2e-tenant"))
        );
        assert!(
            displays
                .iter()
                .any(|display| display.contains("get pv -o jsonpath="))
        );
        assert!(displays.iter().any(|display| {
            display.contains("logs -l rustfs.tenant=e2e-tenant -c rustfs --tail=500 --prefix")
        }));
        assert!(displays.iter().any(|display| {
            display.contains(
                "logs -l rustfs.tenant=e2e-tenant -c rustfs --previous --tail=500 --prefix",
            )
        }));
    }

    #[test]
    fn diagnosis_explains_erasure_read_quorum() {
        let diagnosis = diagnose_snapshot(
            "[FATAL] store init failed to load formats after 10 retries: erasure read quorum",
        );

        assert!(diagnosis.contains("Detected `erasure read quorum`"));
        assert!(diagnosis.contains("ECStore could not read a majority"));
        assert!(diagnosis.contains("e2e-live-delete"));
    }

    #[test]
    fn diagnosis_explains_dns_lookup_failure() {
        let diagnosis =
            diagnose_snapshot("failed to lookup address information when contacting peers");

        assert!(diagnosis.contains("Detected `failed to lookup address information`"));
        assert!(diagnosis.contains("peer hostname was not resolvable"));
    }

    #[test]
    fn diagnosis_defaults_to_unknown_signature_when_no_known_pattern_matches() {
        let diagnosis = diagnose_snapshot("some unrelated startup issue");

        assert!(diagnosis.contains(
            "No built-in RustFS bootstrap signature was detected. Inspect the collected Kubernetes snapshot files for the first failing pod event or container log."
        ));
    }

    #[test]
    fn diagnosis_supports_multiple_signatures() {
        let diagnosis = diagnose_snapshot(
            "store init failed: erasure read quorum\nfailed to lookup address information",
        );

        assert!(diagnosis.contains("Detected `erasure read quorum`"));
        assert!(diagnosis.contains("Detected `failed to lookup address information`"));
    }
}
