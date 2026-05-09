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

use crate::framework::command::CommandSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCheck {
    pub name: &'static str,
    pub command: CommandSpec,
}

pub fn required_tool_checks() -> Vec<ToolCheck> {
    vec![
        ToolCheck {
            name: "cargo",
            command: CommandSpec::new("cargo").arg("--version"),
        },
        ToolCheck {
            name: "kind",
            command: CommandSpec::new("kind").arg("version"),
        },
        ToolCheck {
            name: "kubectl",
            command: CommandSpec::new("kubectl").args(["version", "--client"]),
        },
        ToolCheck {
            name: "docker",
            command: CommandSpec::new("docker").arg("version"),
        },
    ]
}

pub fn run_doctor_checks() -> Vec<(&'static str, Result<String>)> {
    required_tool_checks()
        .into_iter()
        .map(|check| {
            let result = check.command.run_checked().map(|output| {
                if output.stdout.trim().is_empty() {
                    output.stderr.trim().to_string()
                } else {
                    output.stdout.lines().next().unwrap_or_default().to_string()
                }
            });
            (check.name, result)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::required_tool_checks;

    #[test]
    fn doctor_checks_cover_required_host_tools() {
        let tools = required_tool_checks()
            .into_iter()
            .map(|check| check.name)
            .collect::<Vec<_>>();

        assert!(tools.contains(&"cargo"));
        assert!(tools.contains(&"kind"));
        assert!(tools.contains(&"kubectl"));
        assert!(tools.contains(&"docker"));
    }
}
