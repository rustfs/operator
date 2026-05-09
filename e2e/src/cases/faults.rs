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

use super::{CaseSpec, Suite};

pub fn cases() -> Vec<CaseSpec> {
    vec![
        CaseSpec::new(
            Suite::Faults,
            "fault_bad_image_degrades_tenant",
            "Use imagePullPolicy=Never with a missing image and assert Tenant Degraded and Console projection.",
            "fault/workload-image",
            "faults",
        ),
        CaseSpec::new(
            Suite::Faults,
            "fault_missing_credentials_blocks_tenant",
            "Reference a missing credentials Secret and assert CredentialsReady=False without reporting Ready.",
            "fault/credentials",
            "faults",
        ),
        CaseSpec::new(
            Suite::Faults,
            "fault_operator_console_restart_recovers",
            "Restart operator and console deployments and verify reconcile and API availability recover.",
            "fault/control-plane-restart",
            "faults",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn fault_case_inventory_covers_workload_credentials_and_control_plane_faults() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"fault_bad_image_degrades_tenant"));
        assert!(names.contains(&"fault_missing_credentials_blocks_tenant"));
        assert!(names.contains(&"fault_operator_console_restart_recovers"));
    }
}
