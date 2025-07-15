//  Copyright 2025 RustFS Team
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http:www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_cert_enabled: Option<bool>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_certificates: Vec<CustomCertificates>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct CustomCertificates {}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct CustomCertificateConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    cert_name: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    domains: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    expiry: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    serial_no: Option<String>,
}
