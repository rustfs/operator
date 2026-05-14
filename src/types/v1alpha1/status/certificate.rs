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

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsCertificateStatus>,
}

impl Status {
    pub fn is_empty(&self) -> bool {
        self.tls.is_none()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TlsCertificateStatus {
    pub mode: String,
    pub ready: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_certificate: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_strategy: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_ref: Option<CertificateObjectRef>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_secret_ref: Option<SecretStatusRef>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_secret_ref: Option<SecretStatusRef>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ca_secret_ref: Option<SecretStatusRef>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<i64>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_names: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_addresses: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub san_matched: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_source: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_validated_time: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rollout_trigger_time: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_message: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertificateObjectRef {
    pub api_version: String,
    pub kind: String,
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, ToSchema, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatusRef {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
}
