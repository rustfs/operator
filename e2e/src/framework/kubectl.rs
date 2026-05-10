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

use crate::framework::{command::CommandSpec, config::E2eConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kubectl {
    context: String,
    namespace: Option<String>,
}

impl Kubectl {
    pub fn new(config: &E2eConfig) -> Self {
        Self {
            context: config.context.clone(),
            namespace: None,
        }
    }

    pub fn namespaced(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    pub fn command<I, S>(&self, args: I) -> CommandSpec
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut kubectl_args = vec!["--context".to_string(), self.context.clone()];
        if let Some(namespace) = &self.namespace {
            kubectl_args.push("-n".to_string());
            kubectl_args.push(namespace.clone());
        }
        kubectl_args.extend(args.into_iter().map(Into::into));
        CommandSpec::new("kubectl").args(kubectl_args)
    }

    pub fn apply_yaml_command(&self, yaml: impl Into<String>) -> CommandSpec {
        self.command(["apply", "-f", "-"]).stdin(yaml)
    }
}

#[cfg(test)]
mod tests {
    use super::Kubectl;
    use crate::framework::config::E2eConfig;

    #[test]
    fn kubectl_commands_pin_the_expected_context() {
        let kubectl = Kubectl::new(&E2eConfig::defaults()).namespaced("rustfs-system");
        let command = kubectl.command(["get", "pods"]);

        assert_eq!(
            command.display(),
            "kubectl --context kind-rustfs-e2e -n rustfs-system get pods"
        );
    }

    #[test]
    fn kubectl_apply_yaml_uses_stdin_without_exposing_payload() {
        let kubectl = Kubectl::new(&E2eConfig::defaults());
        let command = kubectl.apply_yaml_command("kind: Namespace");

        assert_eq!(
            command.display(),
            "kubectl --context kind-rustfs-e2e apply -f -"
        );
    }
}
