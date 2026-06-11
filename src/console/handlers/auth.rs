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

use axum::{Extension, Json, extract::State, http::header, response::IntoResponse};
use kube::Client;
use snafu::ResultExt;

use crate::console::{
    error::{self, Error, Result},
    models::auth::{LoginRequest, LoginResponse, SessionResponse},
    state::{AppState, Claims, SESSION_TTL_SECONDS},
};
use crate::types::v1alpha1::tenant::Tenant;

/// Exchange a Kubernetes bearer token for an encrypted session cookie.
// TOKEN=$(kubectl create token rustfs-operator-console -n rustfs-system --duration=24h)
// curl -X POST http://localhost:9090/api/v1/login \
//   -H "Content-Type: application/json" \
//   -d "{\"token\": \"$TOKEN\"}"
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse> {
    tracing::info!("Login attempt");

    // Validate the bearer token by building a client
    let client = create_k8s_client(&req.token).await?;

    // Permission smoke test: list Tenant CRs (limit 1)
    let api: kube::Api<Tenant> = kube::Api::all(client);
    api.list(&kube::api::ListParams::default().limit(1))
        .await
        .map_err(|e| {
            tracing::warn!("K8s API test failed: {}", e);
            Error::Unauthorized {
                message: "Invalid or insufficient permissions".to_string(),
            }
        })?;

    let token = state
        .create_session(req.token)
        .context(error::SessionSnafu)?;

    // HttpOnly session cookie
    let cookie = session_cookie(&token);

    let headers = [(header::SET_COOKIE, cookie)];

    Ok((
        headers,
        Json(LoginResponse {
            success: true,
            message: "Login successful".to_string(),
        }),
    ))
}

/// Clear the session cookie.
pub async fn logout() -> impl IntoResponse {
    let cookie = expired_session_cookie();
    let headers = [(header::SET_COOKIE, cookie)];

    (
        headers,
        Json(LoginResponse {
            success: true,
            message: "Logout successful".to_string(),
        }),
    )
}

/// Return session validity and expiry from encrypted cookie claims.
pub async fn session_check(Extension(claims): Extension<Claims>) -> Json<SessionResponse> {
    let expires_at = i64::try_from(claims.exp)
        .ok()
        .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0))
        .map(|dt| dt.to_rfc3339());

    Json(SessionResponse {
        valid: true,
        expires_at,
    })
}

/// Build a `kube::Client` using the login bearer token.
async fn create_k8s_client(token: &str) -> Result<Client> {
    // Default kubeconfig (in-cluster or KUBECONFIG)
    let mut config = kube::Config::infer()
        .await
        .map_err(|e| Error::InternalServer {
            message: format!("Failed to load kubeconfig: {}", e),
        })?;

    // Replace auth with the user's token
    config.auth_info.token = Some(token.to_string().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}

fn session_cookie(token: &str) -> String {
    let same_site = console_cookie_same_site();
    let secure = if console_cookie_secure() || same_site == "None" {
        "; Secure"
    } else {
        ""
    };
    format!(
        "session={token}; Path=/; HttpOnly; SameSite={same_site}; Max-Age={SESSION_TTL_SECONDS}{secure}"
    )
}

fn expired_session_cookie() -> String {
    let same_site = console_cookie_same_site();
    let secure = if console_cookie_secure() || same_site == "None" {
        "; Secure"
    } else {
        ""
    };
    format!("session=; Path=/; HttpOnly; SameSite={same_site}; Max-Age=0{secure}")
}

fn console_cookie_secure() -> bool {
    match std::env::var("CONSOLE_COOKIE_SECURE") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

fn console_cookie_same_site() -> &'static str {
    match std::env::var("CONSOLE_COOKIE_SAME_SITE") {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "lax" => "Lax",
            "none" => "None",
            _ => "Strict",
        },
        Err(_) => "Strict",
    }
}
