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
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            stdin: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }

    pub fn display(&self) -> String {
        let args = self.args.join(" ");
        if args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, args)
        }
    }

    pub fn run(&self) -> Result<CommandOutput> {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        if self.stdin.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start command: {}", self.display()))?;

        if let Some(stdin) = &self.stdin {
            child
                .stdin
                .as_mut()
                .context("failed to open command stdin")?
                .write_all(stdin.as_bytes())
                .with_context(|| {
                    format!("failed to write stdin for command: {}", self.display())
                })?;
        }

        let output = child
            .wait_with_output()
            .with_context(|| format!("failed to wait for command: {}", self.display()))?;

        Ok(CommandOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    pub fn spawn_background_with_log(&self, log_path: impl AsRef<Path>) -> Result<Child> {
        if self.stdin.is_some() {
            bail!(
                "background command stdin is not supported: {}",
                self.display()
            );
        }

        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path.as_ref())
            .with_context(|| format!("open command log {}", log_path.as_ref().display()))?;
        let stderr = log
            .try_clone()
            .with_context(|| format!("clone command log {}", log_path.as_ref().display()))?;

        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }

        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("failed to start background command: {}", self.display()))
    }

    pub fn run_checked(&self) -> Result<CommandOutput> {
        let output = self.run()?;
        if output.code == Some(0) {
            Ok(output)
        } else {
            bail!(
                "command failed: {}\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
                self.display(),
                output.code,
                output.stdout,
                output.stderr
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CommandSpec;

    #[test]
    fn command_display_keeps_program_and_args_visible() {
        let command = CommandSpec::new("kubectl").args(["get", "pods", "-A"]);

        assert_eq!(command.display(), "kubectl get pods -A");
    }

    #[test]
    fn command_display_does_not_include_stdin_payload() {
        let command = CommandSpec::new("kubectl")
            .args(["apply", "-f", "-"])
            .stdin("secret: value");

        assert_eq!(command.display(), "kubectl apply -f -");
    }
}
