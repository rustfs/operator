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

use crate::framework::{config::E2eConfig, kubectl::Kubectl};

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

    pub fn collect_kubernetes_snapshot(&self, case_name: &str, config: &E2eConfig) -> Result<()> {
        let kubectl = Kubectl::new(config);
        let operator_kubectl = Kubectl::new(config).namespaced(&config.operator_namespace);
        let test_kubectl = Kubectl::new(config).namespaced(&config.test_namespace);

        let commands = vec![
            (
                "get-all.txt",
                kubectl.command(["get", "all", "-A", "-o", "wide"]),
            ),
            (
                "tenants.yaml",
                kubectl.command(["get", "tenants", "-A", "-o", "yaml"]),
            ),
            (
                "events.txt",
                kubectl.command(["get", "events", "-A", "--sort-by=.lastTimestamp"]),
            ),
            (
                "operator.log",
                operator_kubectl.command(["logs", "deployment/rustfs-operator", "--tail=500"]),
            ),
            (
                "console.log",
                operator_kubectl.command([
                    "logs",
                    "deployment/rustfs-operator-console",
                    "--tail=500",
                ]),
            ),
            (
                "test-namespace-pods.txt",
                test_kubectl.command(["get", "pods", "-o", "wide"]),
            ),
        ];

        for (file_name, command) in commands {
            let output = command.run()?;
            let content = format!(
                "$ {cmd}\nexit: {code:?}\n\nstdout:\n{stdout}\n\nstderr:\n{stderr}\n",
                cmd = command.display(),
                code = output.code,
                stdout = output.stdout,
                stderr = output.stderr
            );
            self.write_text(case_name, file_name, &content)?;
        }

        Ok(())
    }
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
    use super::ArtifactCollector;

    #[test]
    fn artifact_paths_are_case_scoped_and_sanitized() {
        let collector = ArtifactCollector::new("target/e2e/artifacts");

        assert_eq!(
            collector.case_dir("console auth/session"),
            std::path::PathBuf::from("target/e2e/artifacts/console_auth_session")
        );
    }
}
