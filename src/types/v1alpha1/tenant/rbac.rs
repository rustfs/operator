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
use k8s_openapi::Resource as _;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::rbac::v1 as rbacv1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::{Resource, ResourceExt};

impl Tenant {
    pub fn new_role_binding(&self, sa_name: &str, role: &rbacv1::Role) -> rbacv1::RoleBinding {
        rbacv1::RoleBinding {
            metadata: metav1::ObjectMeta {
                name: Some(self.role_binding_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            subjects: Some(vec![rbacv1::Subject {
                kind: corev1::ServiceAccount::KIND.to_owned(),
                namespace: self.namespace().ok(),
                name: sa_name.to_owned(),
                ..Default::default()
            }]),
            role_ref: rbacv1::RoleRef {
                api_group: rbacv1::Role::GROUP.to_owned(),
                kind: rbacv1::Role::KIND.to_owned(),
                name: role.name_any(),
            },
        }
    }

    pub fn new_role(&self) -> rbacv1::Role {
        rbacv1::Role {
            metadata: metav1::ObjectMeta {
                name: Some(self.role_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            rules: Some(vec![
                rbacv1::PolicyRule {
                    api_groups: Some(vec![String::new()]),
                    resources: Some(vec!["secrets".to_owned()]),
                    verbs: vec!["get".to_owned(), "list".to_owned(), "watch".to_owned()],
                    ..Default::default()
                },
                rbacv1::PolicyRule {
                    api_groups: Some(vec![String::new()]),
                    resources: Some(vec!["services".to_owned()]),
                    verbs: vec!["create".to_owned(), "delete".to_owned(), "get".to_owned()],
                    ..Default::default()
                },
                rbacv1::PolicyRule {
                    api_groups: Some(vec![Self::group(&()).to_string()]),
                    resources: Some(vec![Self::plural(&()).to_string()]),
                    verbs: vec!["get".to_owned(), "list".to_owned(), "watch".to_owned()],
                    ..Default::default()
                },
            ]),
        }
    }

    pub fn new_service_account(&self) -> corev1::ServiceAccount {
        corev1::ServiceAccount {
            metadata: metav1::ObjectMeta {
                name: Some(self.service_account_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    // Test: ServiceAccount resource creation
    #[test]
    fn test_new_service_account_structure() {
        let tenant = super::super::tests::create_test_tenant(None, None);

        let sa = tenant.new_service_account();

        // Verify metadata
        assert_eq!(sa.metadata.name, Some("test-tenant-sa".to_string()));
        assert_eq!(sa.metadata.namespace, Some("default".to_string()));

        // Verify owner reference exists
        assert!(sa.metadata.owner_references.is_some());
        let owner_refs = sa.metadata.owner_references.unwrap();
        assert_eq!(owner_refs.len(), 1);
        assert_eq!(owner_refs[0].kind, "Tenant");
        assert_eq!(owner_refs[0].name, "test-tenant");
        assert_eq!(owner_refs[0].controller, Some(true));
    }

    // Test: Role structure validation
    #[test]
    fn test_new_role_structure() {
        let tenant = super::super::tests::create_test_tenant(None, None);

        let role = tenant.new_role();

        // Verify metadata
        assert_eq!(role.metadata.name, Some("test-tenant-role".to_string()));
        assert_eq!(role.metadata.namespace, Some("default".to_string()));

        // Verify rules
        let rules = role.rules.expect("Role should have rules");
        assert_eq!(rules.len(), 3, "Role should have 3 policy rules");

        // Verify secrets rule
        let secrets_rule = &rules[0];
        assert_eq!(secrets_rule.resources, Some(vec!["secrets".to_string()]));
        assert!(secrets_rule.verbs.contains(&"get".to_string()));
        assert!(secrets_rule.verbs.contains(&"list".to_string()));
        assert!(secrets_rule.verbs.contains(&"watch".to_string()));

        // Verify services rule
        let services_rule = &rules[1];
        assert_eq!(services_rule.resources, Some(vec!["services".to_string()]));
        assert!(services_rule.verbs.contains(&"create".to_string()));
        assert!(services_rule.verbs.contains(&"delete".to_string()));
        assert!(services_rule.verbs.contains(&"get".to_string()));

        // Verify tenants rule
        let tenants_rule = &rules[2];
        assert_eq!(tenants_rule.resources, Some(vec!["tenants".to_string()]));
        assert!(tenants_rule.verbs.contains(&"get".to_string()));
    }

    // Test: RoleBinding with default SA
    #[test]
    fn test_new_role_binding_default_sa() {
        let tenant = super::super::tests::create_test_tenant(None, None);
        let role = tenant.new_role();
        let sa_name = tenant.service_account_name();

        let role_binding = tenant.new_role_binding(&sa_name, &role);

        // Verify metadata
        assert_eq!(
            role_binding.metadata.name,
            Some("test-tenant-role-binding".to_string())
        );

        // Verify subject points to default SA
        let subjects = role_binding
            .subjects
            .expect("RoleBinding should have subjects");
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].kind, "ServiceAccount");
        assert_eq!(subjects[0].name, "test-tenant-sa");
        assert_eq!(subjects[0].namespace, Some("default".to_string()));

        // Verify role ref
        assert_eq!(role_binding.role_ref.kind, "Role");
        assert_eq!(role_binding.role_ref.name, "test-tenant-role");
    }

    // Test: RoleBinding with custom SA
    #[test]
    fn test_new_role_binding_custom_sa() {
        let tenant =
            super::super::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));
        let role = tenant.new_role();
        let sa_name = tenant.service_account_name();

        let role_binding = tenant.new_role_binding(&sa_name, &role);

        // Verify subject points to custom SA
        let subjects = role_binding
            .subjects
            .expect("RoleBinding should have subjects");
        assert_eq!(subjects.len(), 1);
        assert_eq!(
            subjects[0].name, "my-custom-sa",
            "RoleBinding should reference custom service account"
        );
    }
}
