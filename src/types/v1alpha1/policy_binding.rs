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

use kube::{CustomResource, KubeSchema};
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[kube(
    group = "sts.rustfs.com",
    version = "v1alpha1",
    kind = "PolicyBinding",
    namespaced,
    status = "PolicyBindingStatus",
    shortname = "policybinding",
    plural = "policybindings",
    singular = "policybinding",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.currentState"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    crates(serde_json = "k8s_openapi::serde_json")
)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBindingSpec {
    pub application: PolicyBindingApplication,
    #[x_kube(validation = Rule::new("self.size() > 0").message("policies must contain at least one policy"))]
    pub policies: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBindingApplication {
    pub namespace: String,
    pub serviceaccount: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBindingStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<PolicyBindingUsage>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBindingUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorizations: Option<u64>,
}
