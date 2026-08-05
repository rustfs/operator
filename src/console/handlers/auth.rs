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
    Extension, Json,
    extract::{OriginalUri, State},
    http::{HeaderMap, HeaderName, Uri, header},
    response::IntoResponse,
};
use kube::Client;

use crate::console::{
    error::{Error, Result},
    json::ConsoleJson,
    models::auth::{LoginRequest, LoginResponse, SessionResponse},
    state::{
        AppState, Claims, MAX_SESSION_TOKEN_BYTES, SESSION_TTL_SECONDS, SessionError,
        session_cookie_value,
    },
};
use crate::types::v1alpha1::tenant::Tenant;

type LoginHttpResponse = ([(HeaderName, String); 1], Json<LoginResponse>);

/// Exchange a Kubernetes bearer token for a server-side Console session.
// TOKEN=$(kubectl create token rustfs-operator-console -n rustfs-system --duration=24h)
// curl -X POST http://localhost:9090/api/v1/login \
//   -H "Content-Type: application/json" \
//   -d "{\"token\": \"$TOKEN\"}"
pub async fn login(
    State(state): State<AppState>,
    ConsoleJson(req): ConsoleJson<LoginRequest>,
) -> Result<impl IntoResponse> {
    tracing::info!("Console login attempt");

    if req.token.len() > MAX_SESSION_TOKEN_BYTES {
        return Err(Error::BadRequest {
            message: format!("Kubernetes bearer token exceeds {MAX_SESSION_TOKEN_BYTES} bytes"),
        });
    }

    // Validate the bearer token by building a client
    let client = create_k8s_client(&req.token).await?;

    // Permission smoke test: list Tenant CRs (limit 1)
    let api: kube::Api<Tenant> = kube::Api::all(client);
    api.list(&kube::api::ListParams::default().limit(1))
        .await
        .map_err(|error| {
            tracing::warn!(
                %error,
                "Console login Kubernetes API permission check failed"
            );
            Error::Unauthorized {
                message: "Invalid or insufficient permissions".to_string(),
            }
        })?;

    complete_validated_login(&state, req.token)
}

fn complete_validated_login(state: &AppState, k8s_token: String) -> Result<LoginHttpResponse> {
    let session_id = state
        .create_session(k8s_token)
        .map_err(|source| match source {
            SessionError::Capacity | SessionError::PerTokenCapacity => Error::TooManyRequests {
                message: "Too many active Console sessions".to_string(),
            },
            source => Error::Session { source },
        })?;
    Ok((
        [(header::SET_COOKIE, session_cookie(&session_id))],
        Json(LoginResponse {
            success: true,
            message: "Login successful".to_string(),
        }),
    ))
}

pub async fn logout(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    if !is_trusted_logout_request(&headers, &uri) {
        return Err(Error::Forbidden {
            message: "Cross-site logout request denied".to_string(),
        });
    }

    if let Some(session_id) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(session_cookie_value)
    {
        state
            .revoke_session(session_id)
            .map_err(|source| Error::Session { source })?;
    }

    let cookie = expired_session_cookie();
    let headers = [(header::SET_COOKIE, cookie)];

    Ok((
        headers,
        Json(LoginResponse {
            success: true,
            message: "Logout successful".to_string(),
        }),
    ))
}

fn is_trusted_logout_request(headers: &HeaderMap, uri: &Uri) -> bool {
    let allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default();
    is_trusted_logout_request_with_config(
        headers,
        uri,
        &allowed_origins,
        session_cookie_is_secure(),
    )
}

fn is_trusted_logout_request_with_config(
    headers: &HeaderMap,
    uri: &Uri,
    allowed_origins: &str,
    secure: bool,
) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if let Some(origin) = origin {
        let scheme = if secure { "https" } else { "http" };
        let same_origin = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .into_iter()
            .chain(uri.authority().map(|authority| authority.as_str()))
            .any(|authority| origin.eq_ignore_ascii_case(&format!("{scheme}://{authority}")));
        let allowed_origin = allowed_origins
            .split(',')
            .map(str::trim)
            .any(|allowed| allowed.eq_ignore_ascii_case(origin));
        return same_origin || allowed_origin;
    }

    !headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

#[cfg(test)]
mod tests {
    use super::{
        complete_validated_login, is_trusted_logout_request, is_trusted_logout_request_with_config,
        login, logout,
    };
    use crate::console::{
        middleware::auth::auth_middleware,
        state::{AppState, MAX_ACTIVE_SESSIONS, MAX_SESSION_TOKEN_BYTES, MAX_SESSIONS_PER_TOKEN},
    };
    use axum::{
        Router,
        body::Body,
        http::{HeaderMap, Request, StatusCode, Uri, header},
        middleware,
        response::IntoResponse,
        routing::{get, post},
    };
    use tower::ServiceExt;

    #[test]
    fn session_cookie_contains_only_a_random_reference_id() {
        let state = AppState::new("test-secret".to_string());
        let (headers, response) =
            complete_validated_login(&state, "sensitive-k8s-token".to_string())
                .expect("session is created");
        let cookie = &headers[0].1;
        let cookie_value = cookie
            .split(';')
            .next()
            .and_then(|value| value.strip_prefix("session="))
            .expect("session cookie has a value");

        assert_eq!(cookie_value.len(), 32);
        assert!(
            cookie_value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(!cookie.contains("sensitive-k8s-token"));
        assert!(response.success);
    }

    #[test]
    fn per_token_session_capacity_maps_to_too_many_requests() {
        let state = AppState::new("test-secret".to_string());
        for _ in 0..MAX_SESSIONS_PER_TOKEN {
            state
                .create_session("shared-token".to_string())
                .expect("session is within the per-token limit");
        }

        let response = complete_validated_login(&state, "shared-token".to_string())
            .expect_err("per-token capacity is enforced")
            .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn global_session_capacity_maps_to_too_many_requests() {
        let state = AppState::new("test-secret".to_string());
        for token_index in 0..(MAX_ACTIVE_SESSIONS / MAX_SESSIONS_PER_TOKEN) {
            for _ in 0..MAX_SESSIONS_PER_TOKEN {
                state
                    .create_session(format!("token-{token_index}"))
                    .expect("session is within the global limit");
            }
        }

        let response = complete_validated_login(&state, "overflow-token".to_string())
            .expect_err("global capacity is enforced")
            .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn cross_site_fetch_metadata_is_not_trusted_without_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "sec-fetch-site",
            "cross-site".parse().expect("valid header"),
        );

        assert!(!is_trusted_logout_request(
            &headers,
            &Uri::from_static("/api/v1/logout")
        ));
    }

    #[test]
    fn http2_authority_is_accepted_for_same_origin_logout() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            "https://console.example.com".parse().expect("valid origin"),
        );
        let uri: Uri = "https://console.example.com/api/v1/logout"
            .parse()
            .expect("valid URI");

        assert!(is_trusted_logout_request(&headers, &uri));
    }

    #[test]
    fn logout_origin_must_match_cookie_scheme_or_allowlist() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            "console.example.com".parse().expect("valid host"),
        );
        headers.insert(
            header::ORIGIN,
            "http://console.example.com".parse().expect("valid origin"),
        );
        let uri = Uri::from_static("/api/v1/logout");

        assert!(!is_trusted_logout_request_with_config(
            &headers, &uri, "", true
        ));
        assert!(is_trusted_logout_request_with_config(
            &headers, &uri, "", false
        ));
        assert!(is_trusted_logout_request_with_config(
            &headers,
            &uri,
            "https://ui.example.com,http://console.example.com",
            true,
        ));
    }

    #[tokio::test]
    async fn logout_accepts_same_origin_host_and_revokes_session()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new("test-secret".to_string());
        let session_id = state.create_session("k8s-token".to_string())?;
        let app = Router::new()
            .route("/api/v1/logout", post(logout))
            .with_state(state.clone());

        let response = app
            .oneshot(
                Request::post("/api/v1/logout")
                    .header(header::COOKIE, format!("session={session_id}"))
                    .header(header::HOST, "console.example.com")
                    .header(header::ORIGIN, "https://console.example.com")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.resolve_session(&session_id)?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn login_rejects_oversized_bearer_token_before_validation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new("test-secret".to_string());
        let app = Router::new()
            .route("/api/v1/login", post(login))
            .with_state(state);
        let body = serde_json::to_vec(&serde_json::json!({
            "token": "x".repeat(MAX_SESSION_TOKEN_BYTES + 1)
        }))?;

        let response = app
            .oneshot(
                Request::post("/api/v1/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        Ok(())
    }

    #[tokio::test]
    async fn logout_revokes_replayed_session_cookie()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new("test-secret".to_string());
        let session_id = state.create_session("k8s-token".to_string())?;
        let cookie = format!("session={session_id}");
        let app = Router::new()
            .route("/api/v1/logout", post(logout))
            .route("/api/v1/protected", get(|| async { "ok" }))
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(state, auth_middleware));

        let logout_response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/logout")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(logout_response.status(), StatusCode::OK);
        assert!(
            logout_response
                .headers()
                .get(header::SET_COOKIE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("Max-Age=0"))
        );

        let replay_response = app
            .oneshot(
                Request::get("/api/v1/protected")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(replay_response.status(), StatusCode::UNAUTHORIZED);
        Ok(())
    }

    #[tokio::test]
    async fn logout_rejects_untrusted_cross_site_origin()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new("test-secret".to_string());
        let session_id = state.create_session("k8s-token".to_string())?;
        let cookie = format!("session={session_id}");
        let app = Router::new()
            .route("/api/v1/logout", post(logout))
            .with_state(state.clone());

        let response = app
            .oneshot(
                Request::post("/api/v1/logout")
                    .header(header::COOKIE, cookie)
                    .header(header::HOST, "console.example.com")
                    .header(header::ORIGIN, "https://attacker.example")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(state.resolve_session(&session_id)?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn logout_accepts_legacy_empty_post_without_browser_origin()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let state = AppState::new("test-secret".to_string());
        let session_id = state.create_session("k8s-token".to_string())?;
        let cookie = format!("session={session_id}");
        let app = Router::new()
            .route("/api/v1/logout", post(logout))
            .with_state(state.clone());

        let response = app
            .oneshot(
                Request::post("/api/v1/logout")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(state.resolve_session(&session_id)?.is_none());
        Ok(())
    }
}

/// Return session validity and expiry from server-side claims.
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
    let secure = if session_cookie_is_secure() {
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
    let secure = if session_cookie_is_secure() {
        "; Secure"
    } else {
        ""
    };
    format!("session=; Path=/; HttpOnly; SameSite={same_site}; Max-Age=0{secure}")
}

fn session_cookie_is_secure() -> bool {
    console_cookie_secure() || console_cookie_same_site() == "None"
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
