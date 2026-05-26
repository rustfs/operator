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

pub mod encryption;
pub mod k8s;
pub mod logging;
pub mod persistence;
pub mod policy_binding;
pub mod pool;
pub mod pool_lifecycle;
pub mod provisioning;
pub mod status;
pub mod tenant;
pub mod tls;

// Re-export commonly used types
pub use pool::{SchedulingConfig, validate_pool_total_volumes};

#[cfg(test)]
mod policy_binding_tests {
    use super::policy_binding::{
        PolicyBinding, PolicyBindingApplication, PolicyBindingSpec, PolicyBindingStatus,
        PolicyBindingUsage,
    };
    use kube::{CustomResourceExt, Resource};
    use serde_json::json;

    #[test]
    fn policy_binding_serializes_minio_aligned_field_names() {
        let binding = PolicyBinding::new(
            "readonly-binding",
            PolicyBindingSpec {
                application: PolicyBindingApplication {
                    namespace: "apps".to_string(),
                    serviceaccount: "readonly-sa".to_string(),
                },
                policies: vec!["readonly".to_string(), "diagnostics".to_string()],
            },
        );

        let value = serde_json::to_value(&binding).expect("PolicyBinding serializes to JSON");

        assert_eq!(value["apiVersion"], json!("sts.rustfs.com/v1alpha1"));
        assert_eq!(value["kind"], json!("PolicyBinding"));
        assert_eq!(value["spec"]["application"]["namespace"], json!("apps"));
        assert_eq!(
            value["spec"]["application"]["serviceaccount"],
            json!("readonly-sa")
        );
        assert_eq!(
            value["spec"]["policies"],
            json!(["readonly", "diagnostics"])
        );
        assert!(value["spec"]["application"]["serviceAccount"].is_null());
    }

    #[test]
    fn policy_binding_status_serializes_optional_usage_authorizations() {
        let status = PolicyBindingStatus {
            current_state: Some("Ready".to_string()),
            usage: Some(PolicyBindingUsage {
                authorizations: Some(3),
            }),
        };

        let value = serde_json::to_value(status).expect("PolicyBindingStatus serializes to JSON");

        assert_eq!(value["currentState"], json!("Ready"));
        assert_eq!(value["usage"]["authorizations"], json!(3));
    }

    #[test]
    fn policy_binding_crd_has_sts_group_kind_namespaced_scope_and_required_schema() {
        let crd = serde_json::to_value(PolicyBinding::crd()).expect("PolicyBinding CRD serializes");

        assert_eq!(crd["spec"]["group"], json!("sts.rustfs.com"));
        assert_eq!(crd["spec"]["names"]["kind"], json!("PolicyBinding"));
        assert_eq!(crd["spec"]["scope"], json!("Namespaced"));

        let schema = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"];
        assert_eq!(schema["required"], json!(["application", "policies"]));
        assert_eq!(
            schema["properties"]["application"]["required"],
            json!(["namespace", "serviceaccount"])
        );
        assert_eq!(
            schema["properties"]["application"]["properties"]["namespace"]["type"],
            json!("string")
        );
        assert_eq!(
            schema["properties"]["application"]["properties"]["serviceaccount"]["type"],
            json!("string")
        );
        assert_eq!(
            schema["properties"]["policies"]["items"]["type"],
            json!("string")
        );
        assert_eq!(
            schema["properties"]["policies"]["x-kubernetes-validations"][0]["rule"],
            json!("self.size() > 0")
        );
        assert_eq!(
            schema["properties"]["policies"]["x-kubernetes-validations"][0]["message"],
            json!("policies must contain at least one policy")
        );

        let status_schema =
            &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["status"];
        assert_eq!(
            status_schema["properties"]["currentState"]["type"],
            json!("string")
        );
        assert_eq!(
            status_schema["properties"]["usage"]["properties"]["authorizations"]["type"],
            json!("integer")
        );
    }

    #[test]
    fn policy_binding_resource_metadata_is_namespaced() {
        assert_eq!(PolicyBinding::api_version(&()), "sts.rustfs.com/v1alpha1");
        assert_eq!(PolicyBinding::kind(&()), "PolicyBinding");
        assert_eq!(PolicyBinding::plural(&()), "policybindings");
    }
}

#[cfg(test)]
mod tenant_provisioning_crd_tests {
    use super::tenant::Tenant;
    use kube::CustomResourceExt;
    use serde_json::json;

    #[test]
    fn tenant_crd_includes_provisioning_spec_status_and_uniqueness_rules() {
        let crd = serde_json::to_value(Tenant::crd()).expect("Tenant CRD serializes");
        let schema = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"];
        let spec = &schema["properties"]["spec"];
        let status = &schema["properties"]["status"];

        assert_eq!(spec["properties"]["policies"]["type"], json!("array"));
        assert_eq!(spec["properties"]["users"]["type"], json!("array"));
        assert_eq!(spec["properties"]["buckets"]["type"], json!("array"));
        assert_eq!(
            status["properties"]["provisioning"]["properties"]["policies"]["type"],
            json!("array")
        );
        assert_eq!(
            status["properties"]["provisioning"]["properties"]["phase"]["enum"],
            json!(["Pending", "Ready", "Failed", null])
        );

        assert_eq!(
            spec["properties"]["policies"]["x-kubernetes-validations"][0]["message"],
            json!("policy names must be unique")
        );
        let user_policy_validations = &spec["properties"]["users"]["items"]["properties"]["policies"]
            ["x-kubernetes-validations"];
        assert!(
            user_policy_validations
                .as_array()
                .expect("user policy validations are present")
                .iter()
                .any(|rule| rule["message"] == json!("user policy names must be unique"))
        );
        assert_eq!(
            spec["properties"]["buckets"]["items"]["properties"]["name"]["x-kubernetes-validations"]
                [0]["message"],
            json!("bucket name must be a valid RustFS/S3 bucket name")
        );
    }
}
