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

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use uuid::Uuid;

use crate::framework::{command::CommandSpec, config::E2eConfig, kubectl::Kubectl};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardSpec {
    pub namespace: String,
    pub service: String,
    pub local_port: u16,
    pub remote_port: u16,
}

#[derive(Debug)]
pub struct PortForwardGuard {
    child: Child,
    log_path: PathBuf,
    command_display: String,
}

impl PortForwardSpec {
    pub fn console(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            service: "svc/rustfs-operator-console".to_string(),
            local_port: 19090,
            remote_port: 9090,
        }
    }

    pub fn command(&self, kubectl: &Kubectl) -> CommandSpec {
        kubectl.clone().namespaced(&self.namespace).command([
            "port-forward".to_string(),
            self.service.clone(),
            format!("{}:{}", self.local_port, self.remote_port),
        ])
    }

    pub fn start(
        &self,
        kubectl: &Kubectl,
        log_path: impl Into<PathBuf>,
    ) -> Result<PortForwardGuard> {
        let log_path = log_path.into();
        let command = self.command(kubectl);
        let child = command.spawn_background_with_log(&log_path)?;
        Ok(PortForwardGuard {
            child,
            log_path,
            command_display: command.display(),
        })
    }

    pub fn start_with_temp_log(&self, kubectl: &Kubectl) -> Result<PortForwardGuard> {
        let log_path =
            std::env::temp_dir().join(format!("e2e-port-forward-{}.log", Uuid::new_v4()));
        self.start(kubectl, log_path)
    }

    pub fn local_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub fn start_console(config: &E2eConfig) -> Result<PortForwardGuard> {
        let kubectl = Kubectl::new(config);
        Self::console(&config.operator_namespace).start_with_temp_log(&kubectl)
    }
}

impl PortForwardGuard {
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn command_display(&self) -> &str {
        &self.command_display
    }

    pub fn ensure_running(&mut self) -> Result<()> {
        if let Some(status) = self
            .child
            .try_wait()
            .with_context(|| format!("check port-forward process: {}", self.command_display))?
        {
            bail!(
                "port-forward exited early with {status}; command: {}; log {}:\n{}",
                self.command_display,
                self.log_path.display(),
                self.log_contents()
            );
        }
        Ok(())
    }

    pub fn log_contents(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_else(|_| "<unavailable>".to_string())
    }
}

impl Drop for PortForwardGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PortForwardSpec;
    use crate::framework::{config::E2eConfig, kubectl::Kubectl};

    #[test]
    fn console_port_forward_targets_operator_console_service() {
        let kubectl = Kubectl::new(&E2eConfig::defaults());
        let command = PortForwardSpec::console("rustfs-system").command(&kubectl);

        assert_eq!(
            command.display(),
            "kubectl --context kind-rustfs-e2e -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090"
        );
    }
}
