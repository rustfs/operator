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
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use snafu::Snafu;

/// Console API 错误类型
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

    #[snafu(display("Internal server error: {}", message))]
    InternalServer { message: String },

    #[snafu(display("Kubernetes API error: {}", source))]
    KubeApi { source: kube::Error },

    #[snafu(display("JWT error: {}", source))]
    Jwt { source: jsonwebtoken::errors::Error },

    #[snafu(display("JSON serialization error: {}", source))]
    Json { source: serde_json::Error },
}

/// API 错误响应格式
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, error_type, message, details) = match &self {
            Error::Unauthorized { message } => {
                (StatusCode::UNAUTHORIZED, "Unauthorized", message.clone(), None)
            }
            Error::Forbidden { message } => {
                (StatusCode::FORBIDDEN, "Forbidden", message.clone(), None)
            }
            Error::NotFound { resource } => (
                StatusCode::NOT_FOUND,
                "NotFound",
                format!("Resource not found: {}", resource),
                None,
            ),
            Error::BadRequest { message } => {
                (StatusCode::BAD_REQUEST, "BadRequest", message.clone(), None)
            }
            Error::InternalServer { message } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                message.clone(),
                None,
            ),
            Error::KubeApi { source } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "KubeApiError",
                "Kubernetes API error".to_string(),
                Some(source.to_string()),
            ),
            Error::Jwt { source } => (
                StatusCode::UNAUTHORIZED,
                "JwtError",
                "Invalid or expired token".to_string(),
                Some(source.to_string()),
            ),
            Error::Json { source } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "JsonError",
                "JSON serialization error".to_string(),
                Some(source.to_string()),
            ),
        };

        let body = Json(ErrorResponse {
            error: error_type.to_string(),
            message,
            details,
        });

        (status, body).into_response()
    }
}

/// Result type for Console API
pub type Result<T> = std::result::Result<T, Error>;
