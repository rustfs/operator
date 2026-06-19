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

use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;

use crate::framework::{
    artifacts::ArtifactCollector, command::CommandSpec, config::ClusterTestConfig, kubectl::Kubectl,
};

const RUSTFS_DATA_VOLUME: &str = "/data/rustfs0";

#[derive(Debug, Clone)]
pub struct DiskFillGuard {
    config: ClusterTestConfig,
    pod: String,
    filler_path: String,
    deleted: bool,
}

#[derive(Debug, Clone)]
pub struct DmFlakeyGuard {
    name: String,
    recovery_table: String,
    restored: bool,
}

pub fn fill_rustfs_data_volume(
    config: &ClusterTestConfig,
    fill_mib: u64,
    collector: &ArtifactCollector,
    case_name: &str,
    run_id: &str,
) -> Result<DiskFillGuard> {
    let pod = first_rustfs_pod(config)?;
    let filler_path = format!("{RUSTFS_DATA_VOLUME}/.rustfs-e2e-disk-full-{run_id}");
    let fill_mib = fill_mib.to_string();
    let guard = DiskFillGuard {
        config: config.clone(),
        pod: pod.clone(),
        filler_path,
        deleted: false,
    };
    let script = r#"set -eu
filler="$1"
fill_mib="$2"
dir="$(dirname "$filler")"
rm -f "$filler"
echo "before:"
df -k "$dir"
set +e
dd if=/dev/zero of="$filler" bs=1M count="$fill_mib" oflag=sync
dd_code=$?
set -e
sync
echo "after:"
df -k "$dir"
echo "dd_exit=$dd_code"
used_percent="$(df -k "$dir" | awk 'NR==2 {gsub("%", "", $5); print $5}')"
case "$used_percent" in
  ''|*[!0-9]*)
    echo "unable to parse disk usage percent from df output" >&2
    exit 3
    ;;
esac
if [ "$used_percent" -lt 95 ]; then
  echo "disk fill did not create ENOSPC-grade pressure: used=${used_percent}% dd_exit=$dd_code" >&2
  exit 3
fi
"#;
    let output = rustfs_pod_shell(
        config,
        &pod,
        script,
        [guard.filler_path.as_str(), fill_mib.as_str()],
    )
    .run()?;
    collector.write_text(
        case_name,
        "disk-fill.txt",
        &format!(
            "pod: {pod}\nfiller: {}\ncommand output:\nstdout:\n{}\nstderr:\n{}",
            guard.filler_path, output.stdout, output.stderr
        ),
    )?;
    ensure!(
        output.code == Some(0),
        "disk fill fault did not create observable space pressure; exit {:?}",
        output.code
    );

    Ok(guard)
}

pub fn apply_dm_flakey(
    name: &str,
    fault_table: &str,
    recovery_table: Option<&str>,
    collector: &ArtifactCollector,
    case_name: &str,
) -> Result<DmFlakeyGuard> {
    let original = CommandSpec::new("dmsetup")
        .args(["table".to_string(), name.to_string()])
        .run_checked()?
        .stdout;
    let recovery_table = recovery_table
        .map(str::to_string)
        .unwrap_or_else(|| original.trim().to_string());

    collector.write_text(
        case_name,
        "dm-flakey.txt",
        &format!(
            "target: {name}\noriginal table:\n{original}\nfault table:\n{fault_table}\nrecovery table:\n{recovery_table}\n"
        ),
    )?;
    let guard = DmFlakeyGuard {
        name: name.to_string(),
        recovery_table,
        restored: false,
    };
    dmsetup_load_table(name, fault_table)?;

    Ok(guard)
}

pub fn run_warp_mixed(
    duration: Duration,
    collector: &ArtifactCollector,
    case_name: &str,
    endpoint: &str,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<()> {
    let host = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or(endpoint);
    let duration = format!("{}s", duration.as_secs());
    let command = CommandSpec::new("warp").args([
        "mixed".to_string(),
        format!("--host={host}"),
        format!("--access-key={access_key}"),
        format!("--secret-key={secret_key}"),
        format!("--bucket={bucket}"),
        format!("--duration={duration}"),
        "--obj.size=4KiB".to_string(),
        "--tls=false".to_string(),
        "--autoterm".to_string(),
    ]);
    let output = command.run()?;
    let display = command.display().replace(
        &format!("--secret-key={secret_key}"),
        "--secret-key=<redacted>",
    );
    collector.write_text(
        case_name,
        "warp-mixed.txt",
        &format!(
            "$ {}\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
            display, output.code, output.stdout, output.stderr
        ),
    )?;
    ensure!(
        output.code == Some(0),
        "warp mixed command failed with exit {:?}",
        output.code
    );
    Ok(())
}

impl DiskFillGuard {
    pub fn delete(&mut self) -> Result<()> {
        self.delete_inner()?;
        self.deleted = true;
        Ok(())
    }

    fn delete_inner(&self) -> Result<()> {
        let pods = rustfs_pod_names(&self.config).unwrap_or_else(|_| vec![self.pod.clone()]);
        let mut attempts = String::new();
        for pod in pods {
            let command = rustfs_pod_shell(
                &self.config,
                &pod,
                "rm -f \"$1\" && sync",
                [self.filler_path.as_str()],
            );
            let output = command.run()?;
            attempts.push_str(&format!(
                "$ {}\nexit: {:?}\nstdout:\n{}\nstderr:\n{}\n\n",
                command.display(),
                output.code,
                output.stdout,
                output.stderr
            ));
            if output.code == Some(0) {
                return Ok(());
            }
        }
        bail!(
            "failed to remove disk fill artifact {} from RustFS pods\n{}",
            self.filler_path,
            attempts
        )
    }
}

impl Drop for DiskFillGuard {
    fn drop(&mut self) {
        if !self.deleted {
            let _ = self.delete_inner();
        }
    }
}

impl DmFlakeyGuard {
    pub fn restore(&mut self) -> Result<()> {
        dmsetup_load_table(&self.name, &self.recovery_table)?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for DmFlakeyGuard {
    fn drop(&mut self) {
        if !self.restored {
            let _ = dmsetup_load_table(&self.name, &self.recovery_table);
        }
    }
}

fn first_rustfs_pod(config: &ClusterTestConfig) -> Result<String> {
    rustfs_pod_names(config)?
        .into_iter()
        .next()
        .context("no RustFS pods returned")
}

fn rustfs_pod_names(config: &ClusterTestConfig) -> Result<Vec<String>> {
    let selector = format!("rustfs.tenant={}", config.tenant_name);
    let output = Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command([
            "get",
            "pod",
            "-l",
            &selector,
            "-o",
            r#"jsonpath={range .items[*]}{.metadata.name}{"\n"}{end}"#,
        ])
        .run_checked()?;
    let pods = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|pod| !pod.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    ensure!(
        !pods.is_empty(),
        "no RustFS pod found for selector {selector} in namespace {}",
        config.test_namespace
    );
    Ok(pods)
}

fn rustfs_pod_shell<'a, I>(
    config: &ClusterTestConfig,
    pod: &str,
    script: &str,
    args: I,
) -> CommandSpec
where
    I: IntoIterator<Item = &'a str>,
{
    let mut command_args = vec![
        "exec".to_string(),
        pod.to_string(),
        "-c".to_string(),
        "rustfs".to_string(),
        "--".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        script.to_string(),
        "sh".to_string(),
    ];
    command_args.extend(args.into_iter().map(str::to_string));
    Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(command_args)
}

fn dmsetup_load_table(name: &str, table: &str) -> Result<()> {
    CommandSpec::new("dmsetup")
        .args(["suspend".to_string(), name.to_string()])
        .run_checked()?;
    let load = CommandSpec::new("dmsetup")
        .args([
            "load".to_string(),
            name.to_string(),
            "--table".to_string(),
            table.to_string(),
        ])
        .run_checked();
    let resume = CommandSpec::new("dmsetup")
        .args(["resume".to_string(), name.to_string()])
        .run_checked();

    load?;
    resume?;
    Ok(())
}
