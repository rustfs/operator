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
            Suite::Smoke,
            "smoke_dedicated_context_is_active",
            "Assert live execution is explicitly enabled and the active kube context is the dedicated Kind context.",
            "live-safety/context",
            "smoke",
        ),
        CaseSpec::new(
            Suite::Smoke,
            "smoke_control_plane_deployments_are_ready",
            "Verify all e2e control-plane deployments have completed Kubernetes rollout.",
            "control-plane/rollout",
            "smoke",
        ),
        CaseSpec::new(
            Suite::Smoke,
            "smoke_apply_tenant_and_wait_ready",
            "Prepare local storage, apply a Tenant with credentials, and wait for Tenant Ready status.",
            "operator/reconcile",
            "smoke",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn smoke_case_inventory_matches_executable_tests() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "smoke_dedicated_context_is_active",
                "smoke_control_plane_deployments_are_ready",
                "smoke_apply_tenant_and_wait_ready",
            ]
        );
    }
}
