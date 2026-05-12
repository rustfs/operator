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

use serde::Serialize;
use utoipa::ToSchema;

/// Standard error payload returned by Console APIs.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleErrorResponse {
    pub code: String,
    pub reason: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ConsoleErrorDetails>,
}

/// Safe metadata describing the resource related to an error.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleErrorDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// Standard acknowledgement payload returned by Console action APIs.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleActionResponse {
    pub success: bool,
    pub message: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_response_serializes_next_actions_with_camel_case() -> Result<(), serde_json::Error> {
        let response = ConsoleErrorResponse {
            code: "TenantBlocked".to_string(),
            reason: "CredentialsSecretMissing".to_string(),
            message: "Tenant credentials secret is missing".to_string(),
            next_actions: vec!["createCredentialsSecret".to_string()],
            details: Some(ConsoleErrorDetails {
                namespace: Some("rustfs-system".to_string()),
                tenant: Some("logs".to_string()),
                resource: None,
            }),
        };

        let value = serde_json::to_value(response)?;

        assert_eq!(
            value,
            json!({
                "code": "TenantBlocked",
                "reason": "CredentialsSecretMissing",
                "message": "Tenant credentials secret is missing",
                "nextActions": ["createCredentialsSecret"],
                "details": {
                    "namespace": "rustfs-system",
                    "tenant": "logs"
                }
            })
        );
        Ok(())
    }

    #[test]
    fn responses_omit_empty_next_actions_and_absent_details() -> Result<(), serde_json::Error> {
        let error_response = ConsoleErrorResponse {
            code: "Conflict".to_string(),
            reason: "ResourceVersionChanged".to_string(),
            message: "Resource was modified by another request".to_string(),
            next_actions: Vec::new(),
            details: None,
        };
        let action_response = ConsoleActionResponse {
            success: true,
            message: "Tenant restart requested".to_string(),
            reason: "RestartRequested".to_string(),
            next_actions: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(error_response)?,
            json!({
                "code": "Conflict",
                "reason": "ResourceVersionChanged",
                "message": "Resource was modified by another request"
            })
        );
        assert_eq!(
            serde_json::to_value(action_response)?,
            json!({
                "success": true,
                "message": "Tenant restart requested",
                "reason": "RestartRequested"
            })
        );
        Ok(())
    }
}
