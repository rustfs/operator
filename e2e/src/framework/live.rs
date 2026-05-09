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

use anyhow::{Result, ensure};

use crate::framework::{command::CommandSpec, config::E2eConfig};

pub fn require_live_enabled(config: &E2eConfig) -> Result<()> {
    ensure!(
        config.live_enabled,
        "live e2e is disabled; set RUSTFS_E2E_LIVE=1 to run cluster-backed tests"
    );
    Ok(())
}

pub fn require_destructive_enabled(config: &E2eConfig) -> Result<()> {
    ensure!(
        config.destructive_enabled,
        "destructive e2e faults are disabled; set RUSTFS_E2E_DESTRUCTIVE=1 explicitly"
    );
    Ok(())
}

pub fn current_context() -> Result<String> {
    let output = CommandSpec::new("kubectl")
        .args(["config", "current-context"])
        .run_checked()?;
    Ok(output.stdout.trim().to_string())
}

pub fn ensure_dedicated_context(config: &E2eConfig) -> Result<String> {
    let actual = current_context()?;
    ensure!(
        config.is_dedicated_kind_context(&actual),
        "refusing to run e2e against context {actual:?}; expected dedicated kind context {:?}",
        config.context
    );
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::{require_destructive_enabled, require_live_enabled};
    use crate::framework::config::E2eConfig;

    #[test]
    fn live_and_destructive_guards_are_disabled_by_default() {
        let config = E2eConfig::from_env();

        assert!(require_live_enabled(&config).is_err());
        assert!(require_destructive_enabled(&config).is_err());
    }
}
