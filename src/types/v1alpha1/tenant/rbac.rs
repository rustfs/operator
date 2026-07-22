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

use super::Tenant;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

impl Tenant {
    pub(crate) fn legacy_role_binding_name(&self) -> String {
        format!("{}-role-binding", self.name())
    }

    pub(crate) fn legacy_role_name(&self) -> String {
        format!("{}-role", self.name())
    }

    pub fn new_service_account(&self) -> corev1::ServiceAccount {
        corev1::ServiceAccount {
            metadata: metav1::ObjectMeta {
                name: Some(self.service_account_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            automount_service_account_token: Some(false),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    // Test: ServiceAccount resource creation
    #[test]
    fn test_new_service_account_structure() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let sa = tenant.new_service_account();

        // Verify metadata
        assert_eq!(sa.metadata.name, Some("test-tenant-sa".to_string()));
        assert_eq!(sa.metadata.namespace, Some("default".to_string()));
        assert_eq!(sa.automount_service_account_token, Some(false));

        // Verify owner reference exists
        if let Some(owner_refs) = &sa.metadata.owner_references {
            assert_eq!(owner_refs.len(), 1);
            assert_eq!(owner_refs[0].kind, "Tenant");
            assert_eq!(owner_refs[0].name, "test-tenant");
            assert_eq!(owner_refs[0].controller, Some(true));
        } else {
            panic!("ServiceAccount should have owner references");
        }
    }

    #[test]
    fn legacy_rbac_names_remain_stable_for_cleanup() {
        let tenant = crate::tests::create_test_tenant(None, None);

        assert_eq!(tenant.legacy_role_name(), "test-tenant-role");
        assert_eq!(
            tenant.legacy_role_binding_name(),
            "test-tenant-role-binding"
        );
    }
}
