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

use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use snafu::Snafu;

use crate::console::models::common::{ConsoleErrorDetails, ConsoleErrorResponse};

/// Console HTTP API error type
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Unauthorized: {}", message))]
    Unauthorized { message: String },

    #[snafu(display("Forbidden: {}", message))]
    Forbidden { message: String },

    #[snafu(display("Not found: {}", resource))]
    NotFound { resource: String },

    #[snafu(display("Bad request: {}", message))]
    BadRequest { message: String },

    #[snafu(display("Invalid JSON syntax: {}", message))]
    JsonSyntax { message: String },

    #[snafu(display("Invalid JSON data: {}", message))]
    JsonData { message: String },

    #[snafu(display("Unsupported JSON media type: {}", message))]
    UnsupportedMediaType { message: String },

    #[snafu(display("Request body rejected with status {}: {}", status, message))]
    RequestBody { status: StatusCode, message: String },

    #[snafu(display("Conflict: {}", message))]
    Conflict { message: String },

    #[snafu(display("Action required: {}", message))]
    ActionRequired {
        status: StatusCode,
        code: String,
        reason: String,
        message: String,
        next_actions: Vec<String>,
        details: Option<Box<ConsoleErrorDetails>>,
    },

    #[snafu(display("Internal server error: {}", message))]
    InternalServer { message: String },

    #[snafu(display("Kubernetes API error: {}", source))]
    KubeApi { source: kube::Error },

    #[snafu(display("Session error: {}", source))]
    Session {
        source: crate::console::state::SessionError,
    },

    #[snafu(display("JSON serialization error: {}", source))]
    Json { source: serde_json::Error },
}

/// Map Kubernetes API status errors to the corresponding Console error contract.
pub fn map_kube_error(e: kube::Error, not_found_resource: impl Into<String>) -> Error {
    match &e {
        kube::Error::Api(ae) if ae.code == 401 => Error::Unauthorized {
            message: if ae.message.trim().is_empty() {
                "Kubernetes API authentication required".to_string()
            } else {
                ae.message.clone()
            },
        },
        kube::Error::Api(ae) if matches!(ae.code, 400 | 422) => Error::BadRequest {
            message: if ae.message.trim().is_empty() {
                "Kubernetes API rejected the request".to_string()
            } else {
                ae.message.clone()
            },
        },
        kube::Error::Api(ae) if ae.code == 403 => Error::Forbidden {
            message: if ae.message.is_empty() {
                "Kubernetes API access denied".to_string()
            } else {
                ae.message.clone()
            },
        },
        kube::Error::Api(ae) if ae.code == 404 => Error::NotFound {
            resource: not_found_resource.into(),
        },
        kube::Error::Api(ae) if ae.code == 409 => Error::Conflict {
            message: if ae.message.trim().is_empty() {
                "Kubernetes API reported a resource conflict".to_string()
            } else {
                ae.message.clone()
            },
        },
        _ => Error::KubeApi { source: e },
    }
}

impl Error {
    /// Convert Axum's JSON extraction failures into the Console error contract.
    pub(crate) fn from_json_rejection(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::JsonSyntaxError(error) => Self::JsonSyntax {
                message: error.body_text(),
            },
            JsonRejection::JsonDataError(error) => Self::JsonData {
                message: error.body_text(),
            },
            JsonRejection::MissingJsonContentType(error) => Self::UnsupportedMediaType {
                message: error.body_text(),
            },
            rejection => Self::RequestBody {
                status: rejection.status(),
                message: rejection.body_text(),
            },
        }
    }

    fn log_if_server_error(&self) {
        match self {
            Error::InternalServer { message } => {
                tracing::warn!(
                    status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    code = "InternalServerError",
                    message = %message,
                    "Console request failed"
                );
            }
            Error::KubeApi { source } => {
                tracing::warn!(
                    status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    code = "KubeApiError",
                    error = %source,
                    "Console request failed"
                );
            }
            Error::Session { source } => {
                tracing::warn!(
                    status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    code = "SessionError",
                    error = %source,
                    "Console request failed"
                );
            }
            Error::Json { source } => {
                tracing::warn!(
                    status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    code = "JsonError",
                    error = %source,
                    "Console request failed"
                );
            }
            Error::ActionRequired {
                status,
                code,
                reason,
                message,
                ..
            } if status.is_server_error() => {
                tracing::warn!(
                    status = status.as_u16(),
                    code = %code,
                    reason = %reason,
                    message = %message,
                    "Console request failed"
                );
            }
            _ => {}
        }
    }

    fn into_response_parts(self) -> (StatusCode, ConsoleErrorResponse) {
        let (status, code, reason, message, next_actions, details) = match self {
            Error::Unauthorized { message } => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized".to_string(),
                "Unauthorized".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::Forbidden { message } => (
                StatusCode::FORBIDDEN,
                "Forbidden".to_string(),
                "Forbidden".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::NotFound { resource } => (
                StatusCode::NOT_FOUND,
                "NotFound".to_string(),
                "ResourceNotFound".to_string(),
                format!("Resource not found: {}", resource),
                Vec::new(),
                None,
            ),
            Error::BadRequest { message } => (
                StatusCode::BAD_REQUEST,
                "BadRequest".to_string(),
                "InvalidRequest".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::JsonSyntax { message } => (
                StatusCode::BAD_REQUEST,
                "BadRequest".to_string(),
                "InvalidJsonSyntax".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::JsonData { message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "UnprocessableEntity".to_string(),
                "InvalidJsonData".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::UnsupportedMediaType { message } => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UnsupportedMediaType".to_string(),
                "UnsupportedJsonContentType".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::RequestBody { status, message } => (
                status,
                "RequestBodyError".to_string(),
                "InvalidRequestBody".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::Conflict { message } => (
                StatusCode::CONFLICT,
                "Conflict".to_string(),
                "ResourceConflict".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::ActionRequired {
                status,
                code,
                reason,
                message,
                next_actions,
                details,
            } => (
                status,
                code,
                reason,
                message,
                next_actions,
                details.map(|details| *details),
            ),
            Error::InternalServer { message } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError".to_string(),
                "InternalServerError".to_string(),
                message,
                Vec::new(),
                None,
            ),
            Error::KubeApi { source: _ } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "KubeApiError".to_string(),
                "KubernetesApiError".to_string(),
                "Kubernetes API error".to_string(),
                Vec::new(),
                None,
            ),
            Error::Session { source: _ } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "SessionError".to_string(),
                "SessionError".to_string(),
                "Session error".to_string(),
                Vec::new(),
                None,
            ),
            Error::Json { source: _ } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "JsonError".to_string(),
                "SerializationFailed".to_string(),
                "JSON serialization error".to_string(),
                Vec::new(),
                None,
            ),
        };

        (
            status,
            ConsoleErrorResponse {
                code,
                reason,
                message,
                next_actions,
                details,
            },
        )
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        self.log_if_server_error();
        let (status, body) = self.into_response_parts();

        (status, Json(body)).into_response()
    }
}

/// Result type for Console API
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::models::common::ConsoleErrorDetails;
    use serde_json::json;

    fn kube_api_error(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(kube::error::ErrorResponse {
            status: "Failure".to_string(),
            message: message.to_string(),
            reason: String::new(),
            code,
        })
    }

    #[test]
    fn bad_request_maps_to_stable_error_contract() -> std::result::Result<(), serde_json::Error> {
        let (status, response) = Error::BadRequest {
            message: "invalid tenant name".to_string(),
        }
        .into_response_parts();

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response.code, "BadRequest");
        assert_eq!(response.reason, "InvalidRequest");
        assert_eq!(response.message, "invalid tenant name");
        assert!(response.next_actions.is_empty());
        assert!(response.details.is_none());

        let value = serde_json::to_value(response)?;
        assert_eq!(
            value,
            json!({
                "code": "BadRequest",
                "reason": "InvalidRequest",
                "message": "invalid tenant name"
            })
        );
        assert!(value.get("nextActions").is_none());
        Ok(())
    }

    #[test]
    fn action_required_maps_to_stable_error_contract() -> std::result::Result<(), serde_json::Error>
    {
        let details = ConsoleErrorDetails {
            namespace: Some("rustfs-system".to_string()),
            tenant: Some("logs".to_string()),
            resource: Some("credentials-secret".to_string()),
        };

        let (status, response) = Error::ActionRequired {
            status: StatusCode::PRECONDITION_REQUIRED,
            code: "TenantBlocked".to_string(),
            reason: "CredentialsSecretMissing".to_string(),
            message: "Tenant credentials secret is missing".to_string(),
            next_actions: vec!["createCredentialsSecret".to_string()],
            details: Some(Box::new(details)),
        }
        .into_response_parts();

        assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(response.code, "TenantBlocked");
        assert_eq!(response.reason, "CredentialsSecretMissing");
        assert_eq!(response.message, "Tenant credentials secret is missing");
        assert_eq!(response.next_actions, vec!["createCredentialsSecret"]);
        assert!(response.details.is_some());

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
                    "tenant": "logs",
                    "resource": "credentials-secret"
                }
            })
        );
        Ok(())
    }

    #[test]
    fn kubernetes_unprocessable_entity_maps_to_bad_request() {
        let error = map_kube_error(
            kube_api_error(422, "metadata.labels: Invalid value"),
            "Tenant",
        );

        assert!(matches!(
            error,
            Error::BadRequest { message } if message == "metadata.labels: Invalid value"
        ));
    }

    #[test]
    fn kubernetes_unauthorized_maps_to_unauthorized() {
        let server_message = map_kube_error(
            kube_api_error(401, "Unauthorized: bearer token has expired"),
            "Tenant",
        );
        assert!(matches!(
            server_message,
            Error::Unauthorized { message }
                if message == "Unauthorized: bearer token has expired"
        ));

        let fallback = map_kube_error(kube_api_error(401, ""), "Tenant");
        assert!(matches!(
            fallback,
            Error::Unauthorized { message }
                if message == "Kubernetes API authentication required"
        ));
    }

    #[test]
    fn kubernetes_conflict_preserves_server_message_or_uses_fallback() {
        let server_message = map_kube_error(
            kube_api_error(409, "tenants.rustfs.com \"logs\" already exists"),
            "Tenant",
        );
        assert!(matches!(
            server_message,
            Error::Conflict { message }
                if message == "tenants.rustfs.com \"logs\" already exists"
        ));

        let fallback = map_kube_error(kube_api_error(409, ""), "Tenant");
        assert!(matches!(
            fallback,
            Error::Conflict { message }
                if message == "Kubernetes API reported a resource conflict"
        ));
    }
}
