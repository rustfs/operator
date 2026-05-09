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
            Suite::Console,
            "console_auth_session_logout",
            "Login with a short-lived Kubernetes token, verify session, and verify logout clears access.",
            "console/auth",
            "console",
        ),
        CaseSpec::new(
            Suite::Console,
            "console_tenant_projection_matches_kubernetes",
            "Compare Console tenant list/detail/state-counts against the live Tenant CR status.",
            "console/tenant-projection",
            "console",
        ),
        CaseSpec::new(
            Suite::Console,
            "console_pods_logs_restart",
            "Verify pod list/detail/logs and restart/delete actions reconcile back to Ready.",
            "console/pods",
            "console",
        ),
        CaseSpec::new(
            Suite::Console,
            "console_events_stream_snapshot",
            "Open the tenant events SSE stream and validate snapshot plus event delivery after a deterministic action.",
            "console/events-sse",
            "console",
        ),
        CaseSpec::new(
            Suite::Console,
            "console_topology_overview_matches_cluster",
            "Verify topology overview summarizes namespaces, Tenants, pools, Pods, and node capacity from Kubernetes.",
            "console/topology",
            "console",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn console_case_inventory_covers_auth_projection_events_and_topology() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"console_auth_session_logout"));
        assert!(names.contains(&"console_tenant_projection_matches_kubernetes"));
        assert!(names.contains(&"console_events_stream_snapshot"));
        assert!(names.contains(&"console_topology_overview_matches_cluster"));
    }
}
