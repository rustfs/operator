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

use crate::framework::{command::CommandSpec, kubectl::Kubectl};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardSpec {
    pub namespace: String,
    pub service: String,
    pub local_port: u16,
    pub remote_port: u16,
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
}

#[cfg(test)]
mod tests {
    use super::PortForwardSpec;
    use crate::framework::{config::E2eConfig, kubectl::Kubectl};

    #[test]
    fn console_port_forward_targets_operator_console_service() {
        let kubectl = Kubectl::new(&E2eConfig::from_env());
        let command = PortForwardSpec::console("rustfs-system").command(&kubectl);

        assert_eq!(
            command.display(),
            "kubectl --context kind-rustfs-e2e -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090"
        );
    }
}
