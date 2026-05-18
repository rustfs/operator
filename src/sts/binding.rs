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

use crate::PolicyBinding;

/// Select policy bindings that match a namespace/service account pair.
pub fn find_matching_bindings(
    policy_bindings: &[PolicyBinding],
    namespace: &str,
    service_account: &str,
) -> Vec<PolicyBinding> {
    policy_bindings
        .iter()
        .filter(|policy_binding| {
            policy_binding.spec.application.namespace == namespace
                && policy_binding.spec.application.serviceaccount == service_account
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::types::v1alpha1::policy_binding::{
        PolicyBinding, PolicyBindingApplication, PolicyBindingSpec,
    };

    #[test]
    fn match_policy_bindings_by_serviceaccount_and_namespace() {
        let bindings = vec![
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "build-sa".to_string(),
                    },
                    policies: vec!["policy-a".to_string()],
                },
                status: None,
            },
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-b".to_string(),
                        serviceaccount: "ci-sa".to_string(),
                    },
                    policies: vec!["policy-b".to_string()],
                },
                status: None,
            },
        ];

        let matches = super::find_matching_bindings(&bindings, "tenant-a", "build-sa");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].spec.application.namespace, "tenant-a");
        assert_eq!(matches[0].spec.application.serviceaccount, "build-sa");
    }

    #[test]
    fn no_match_if_namespace_or_serviceaccount_differs() {
        let bindings = vec![
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "build-sa".to_string(),
                    },
                    policies: vec!["policy-a".to_string()],
                },
                status: None,
            },
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "ops-sa".to_string(),
                    },
                    policies: vec!["policy-b".to_string()],
                },
                status: None,
            },
        ];

        assert!(super::find_matching_bindings(&bindings, "tenant-b", "build-sa").is_empty());
        assert!(super::find_matching_bindings(&bindings, "tenant-a", "admin-sa").is_empty());
    }

    #[test]
    fn match_returns_all_bindings_for_same_namespace_and_service_account() {
        let bindings = vec![
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "sa-a".to_string(),
                    },
                    policies: vec!["policy-1".to_string()],
                },
                status: None,
            },
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "sa-a".to_string(),
                    },
                    policies: vec!["policy-2".to_string()],
                },
                status: None,
            },
            PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "sa-b".to_string(),
                    },
                    policies: vec!["policy-3".to_string()],
                },
                status: None,
            },
        ];

        let matches = super::find_matching_bindings(&bindings, "tenant-a", "sa-a");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].spec.policies[0], "policy-1");
        assert_eq!(matches[1].spec.policies[0], "policy-2");
    }
}
