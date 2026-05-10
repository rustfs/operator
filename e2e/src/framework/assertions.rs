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

use anyhow::{Result, bail, ensure};
use operator::types::v1alpha1::tenant::Tenant;

pub fn current_state(tenant: &Tenant) -> Option<&str> {
    tenant
        .status
        .as_ref()
        .map(|status| status.current_state.as_str())
}

pub fn condition_status<'a>(tenant: &'a Tenant, condition_type: &str) -> Option<&'a str> {
    tenant
        .status
        .as_ref()?
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_type)
        .map(|condition| condition.status.as_str())
}

pub fn require_condition(
    tenant: &Tenant,
    condition_type: &str,
    expected_status: &str,
) -> Result<()> {
    match condition_status(tenant, condition_type) {
        Some(actual) if actual == expected_status => Ok(()),
        Some(actual) => {
            bail!("condition {condition_type} expected {expected_status}, got {actual}")
        }
        None => bail!("condition {condition_type} not found"),
    }
}

pub fn require_observed_generation_current(tenant: &Tenant) -> Result<()> {
    let generation = tenant.metadata.generation;
    let observed = tenant
        .status
        .as_ref()
        .and_then(|status| status.observed_generation);

    ensure!(
        generation.is_some(),
        "tenant metadata.generation is missing"
    );
    ensure!(
        observed == generation,
        "tenant observedGeneration {observed:?} does not match generation {generation:?}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{condition_status, current_state, require_condition};
    use operator::types::v1alpha1::status::{Condition, Status};
    use operator::types::v1alpha1::tenant::{Tenant, TenantSpec};

    #[test]
    fn tenant_condition_helpers_find_status_by_type() {
        let mut tenant = Tenant::new("tenant-a", TenantSpec::default());
        tenant.status = Some(Status {
            current_state: "Ready".to_string(),
            conditions: vec![Condition {
                type_: "Ready".to_string(),
                status: "True".to_string(),
                last_transition_time: None,
                observed_generation: Some(1),
                reason: "ReconcileSucceeded".to_string(),
                message: "ready".to_string(),
            }],
            ..Status::default()
        });

        assert_eq!(current_state(&tenant), Some("Ready"));
        assert_eq!(condition_status(&tenant, "Ready"), Some("True"));
        assert!(require_condition(&tenant, "Ready", "True").is_ok());
        assert!(require_condition(&tenant, "Ready", "False").is_err());
    }
}
