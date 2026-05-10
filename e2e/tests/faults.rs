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
use rustfs_operator_e2e::framework::{config::E2eConfig, live};

#[test]
fn faults_are_not_destructive_without_explicit_opt_in() {
    let config = E2eConfig::defaults();

    assert!(!config.destructive_enabled);
    assert!(live::require_destructive_enabled(&config).is_err());
}

#[test]
#[ignore = "reserved for destructive fault scenarios; no public live target in the reduced workflow"]
fn fault_live_suite_requires_explicit_destructive_opt_in() -> Result<()> {
    let config = E2eConfig::from_env();

    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;
    live::require_destructive_enabled(&config)?;

    Ok(())
}
