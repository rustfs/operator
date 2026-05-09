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
            Suite::Operator,
            "operator_tenant_ready_lifecycle",
            "Apply a Tenant and assert StatefulSet, Pods, Tenant Ready status, pool rollout, and ReconcileSucceeded event.",
            "operator/reconcile",
            "operator",
        ),
        CaseSpec::new(
            Suite::Operator,
            "operator_status_conditions_events",
            "Assert currentState, observedGeneration, Ready/Degraded/Reconciling conditions, and Kubernetes events.",
            "operator/status",
            "operator",
        ),
        CaseSpec::new(
            Suite::Operator,
            "operator_no_status_churn",
            "Record Tenant resourceVersion after Ready and ensure the operator does not self-trigger status churn.",
            "operator/stability",
            "operator",
        ),
        CaseSpec::new(
            Suite::Operator,
            "operator_pod_delete_recovery",
            "Delete a RustFS server Pod and verify StatefulSet and operator status recover to Ready.",
            "operator/recovery",
            "operator",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn operator_case_inventory_covers_lifecycle_status_and_recovery() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"operator_tenant_ready_lifecycle"));
        assert!(names.contains(&"operator_status_conditions_events"));
        assert!(names.contains(&"operator_pod_delete_recovery"));
    }
}
