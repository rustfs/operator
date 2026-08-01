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
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};

use crate::{
    console::{handlers, models::common::ConsoleErrorResponse, state::AppState},
    http_admission::{AdmissionConfig, AdmissionControl, AdmissionEndpoint, AdmissionRejection},
    metrics::UnauthenticatedRequestOutcome,
};

/// Login / session routes (partially unauthenticated)
pub fn auth_routes() -> Router<AppState> {
    auth_routes_with_config(AdmissionConfig::for_endpoint(
        AdmissionEndpoint::ConsoleLogin,
    ))
}

pub(crate) fn auth_routes_with_config(config: AdmissionConfig) -> Router<AppState> {
    auth_routes_with_admission(AdmissionControl::new(config))
}

fn auth_routes_with_admission(admission: AdmissionControl) -> Router<AppState> {
    let login = Router::new().route(
        "/login",
        post(handlers::auth::login).route_layer(middleware::from_fn_with_state(
            admission,
            enforce_login_admission,
        )),
    );
    Router::new()
        .merge(login)
        .route("/logout", post(handlers::auth::logout))
        .route("/session", get(handlers::auth::session_check))
}

async fn enforce_login_admission(
    State(admission): State<AdmissionControl>,
    request: Request,
    next: Next,
) -> Response {
    let result = admission
        .execute(async {
            let request = admission.read_bounded_request(request).await?;
            Ok(next.run(request).await)
        })
        .await;

    let (response, outcome) = match result {
        Ok(response) => {
            let outcome = UnauthenticatedRequestOutcome::from_status(response.status());
            (response, outcome)
        }
        Err(rejection) => (
            login_admission_rejection_response(rejection),
            UnauthenticatedRequestOutcome::Rejected(rejection.reason()),
        ),
    };
    crate::metrics::record_unauthenticated_request(AdmissionEndpoint::ConsoleLogin, outcome);
    response
}

fn login_admission_rejection_response(rejection: AdmissionRejection) -> Response {
    let (status, code, reason, message, retry_after) = match rejection {
        AdmissionRejection::RateLimited {
            retry_after_seconds,
        } => (
            StatusCode::TOO_MANY_REQUESTS,
            "TooManyRequests",
            "LoginRateLimited",
            "Login request rate exceeded; retry later.",
            Some(retry_after_seconds),
        ),
        AdmissionRejection::ConcurrencyLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "TooManyRequests",
            "LoginConcurrencyLimited",
            "Too many login requests are already in progress.",
            Some(1),
        ),
        AdmissionRejection::BodyTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "RequestBodyError",
            "InvalidRequestBody",
            "The login request body exceeds the maximum allowed size.",
            None,
        ),
        AdmissionRejection::BodyReadFailed => (
            StatusCode::BAD_REQUEST,
            "RequestBodyError",
            "InvalidRequestBody",
            "The login request body could not be read.",
            None,
        ),
        AdmissionRejection::TimedOut => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailable",
            "LoginTimedOut",
            "The login request timed out; retry later.",
            Some(1),
        ),
    };
    let mut response = (
        status,
        Json(ConsoleErrorResponse {
            code: code.to_string(),
            reason: reason.to_string(),
            message: message.to_string(),
            next_actions: Vec::new(),
            details: None,
        }),
    )
        .into_response();
    if let Some(retry_after_seconds) = retry_after
        && let Ok(retry_after) = retry_after_seconds.to_string().parse::<HeaderValue>()
    {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, retry_after);
    }
    response
}

/// Tenant CRUD, YAML, encryption, security context
pub fn tenant_routes() -> Router<AppState> {
    Router::new()
        .route("/tenants", get(handlers::tenants::list_all_tenants))
        .route(
            "/tenants/state-counts",
            get(handlers::tenants::get_all_tenant_state_counts),
        )
        .route("/tenants", post(handlers::tenants::create_tenant))
        .route(
            "/tenants/yaml",
            post(handlers::tenants::create_tenant_from_yaml),
        )
        .route(
            "/namespaces/:namespace/tenants",
            get(handlers::tenants::list_tenants_by_namespace),
        )
        .route(
            "/namespaces/:namespace/tenants/state-counts",
            get(handlers::tenants::get_tenant_state_counts_by_namespace),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            get(handlers::tenants::get_tenant_details),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            put(handlers::tenants::update_tenant),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            delete(handlers::tenants::delete_tenant),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/yaml",
            get(handlers::tenants::get_tenant_yaml),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/yaml",
            put(handlers::tenants::put_tenant_yaml),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/encryption",
            get(handlers::encryption::get_encryption),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/encryption",
            put(handlers::encryption::update_encryption),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/security-context",
            get(handlers::security_context::get_security_context),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/security-context",
            put(handlers::security_context::update_security_context),
        )
}

/// Pool list / add / delete under a tenant
pub fn pool_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/namespaces/:namespace/tenants/:name/pools",
            get(handlers::pools::list_pools),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools",
            post(handlers::pools::add_pool),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools/:pool",
            delete(handlers::pools::delete_pool),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools/:pool/decommission",
            post(handlers::pools::start_pool_decommission),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools/:pool/decommission/cancel",
            post(handlers::pools::cancel_pool_decommission),
        )
}

/// Pod list, detail, delete, restart, logs
pub fn pod_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/namespaces/:namespace/tenants/:name/pods",
            get(handlers::pods::list_pods),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod",
            get(handlers::pods::get_pod_details),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod",
            delete(handlers::pods::delete_pod),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod/restart",
            post(handlers::pods::restart_pod),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod/logs",
            get(handlers::pods::get_pod_logs),
        )
}

/// Kubernetes events for a tenant (SSE)
pub fn event_routes() -> Router<AppState> {
    Router::new().route(
        "/namespaces/:namespace/tenants/:tenant/events/stream",
        get(handlers::events::stream_tenant_events),
    )
}

/// Nodes, cluster capacity, namespaces
pub fn cluster_routes() -> Router<AppState> {
    Router::new()
        .route("/cluster/nodes", get(handlers::cluster::list_nodes))
        .route(
            "/cluster/resources",
            get(handlers::cluster::get_cluster_resources),
        )
        .route("/namespaces", get(handlers::cluster::list_namespaces))
        .route("/namespaces", post(handlers::cluster::create_namespace))
}

/// Topology overview for the dashboard
pub fn topology_routes() -> Router<AppState> {
    Router::new().route(
        "/topology/overview",
        get(handlers::topology::get_topology_overview),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        auth_routes, auth_routes_with_admission, cluster_routes,
        login_admission_rejection_response, pod_routes, pool_routes, tenant_routes,
    };
    use crate::console::error::JSON_REJECTION_MESSAGE_MAX_BYTES;
    use crate::console::state::{AppState, Claims};
    use crate::http_admission::{AdmissionConfig, AdmissionControl, AdmissionRejection};
    use crate::metrics::{UnauthenticatedRequestOutcome, capture_request_metrics};
    use axum::{
        Extension, Router,
        body::{Body, to_bytes},
        http::{HeaderValue, Method, Request, StatusCode, header},
    };
    use serde_json::Value;
    use tower::ServiceExt;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn test_app() -> Router {
        tenant_routes()
            .merge(pool_routes())
            .merge(pod_routes())
            .merge(cluster_routes())
            .merge(auth_routes())
            .layer(Extension(Claims {
                k8s_token: "test-token".to_string(),
                exp: usize::MAX,
                iat: 0,
            }))
            .with_state(AppState::new("test-secret".to_string()))
    }

    async fn send_json_request(
        method: Method,
        path: &str,
        content_type: Option<&str>,
        body: &str,
    ) -> TestResult<(StatusCode, Value)> {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let response = test_app()
            .oneshot(request.body(Body::from(body.to_string()))?)
            .await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, serde_json::from_slice(&body)?))
    }

    fn assert_error_envelope(body: &Value, code: &str, reason: &str) {
        assert_eq!(body.get("code").and_then(Value::as_str), Some(code));
        assert_eq!(body.get("reason").and_then(Value::as_str), Some(reason));
        assert!(body.get("message").and_then(Value::as_str).is_some());
    }

    fn restrictive_admission(body_limit_bytes: usize) -> AdmissionControl {
        AdmissionControl::new(AdmissionConfig {
            requests_per_second: 0.1,
            burst: 1,
            max_in_flight: 1,
            body_limit_bytes,
            timeout: std::time::Duration::from_secs(1),
        })
    }

    #[tokio::test]
    async fn login_rate_limit_uses_console_error_contract_and_does_not_cover_logout() -> TestResult
    {
        let admission = restrictive_admission(64 * 1024);
        admission
            .execute(async { Ok::<(), AdmissionRejection>(()) })
            .await
            .expect("test should consume the initial token");
        let app = auth_routes_with_admission(admission)
            .with_state(AppState::new("test-secret".to_string()));

        let (response, captured) = capture_request_metrics(
            app.clone().oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"test"}"#))?,
            ),
        )
        .await;
        let response = response?;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            captured.unauthenticated_requests,
            vec![(
                crate::http_admission::AdmissionEndpoint::ConsoleLogin,
                UnauthenticatedRequestOutcome::Rejected(
                    crate::http_admission::AdmissionReason::RateLimit
                ),
            )]
        );
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("10"))
        );
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let body: Value = serde_json::from_slice(&body)?;
        assert_error_envelope(&body, "TooManyRequests", "LoginRateLimited");

        let (response, captured) = capture_request_metrics(
            app.oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/logout")
                    .body(Body::empty())?,
            ),
        )
        .await;
        let response = response?;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(captured.unauthenticated_requests.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn login_oversized_body_is_rejected_before_json_extraction() -> TestResult {
        let app = auth_routes_with_admission(restrictive_admission(4))
            .with_state(AppState::new("test-secret".to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"token":"test"}"#))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let body: Value = serde_json::from_slice(&body)?;
        assert_error_envelope(&body, "RequestBodyError", "InvalidRequestBody");
        Ok(())
    }

    #[tokio::test]
    async fn login_timeout_uses_retryable_console_error_contract() -> TestResult {
        let response = login_admission_rejection_response(AdmissionRejection::TimedOut);

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from_static("1"))
        );
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let body: Value = serde_json::from_slice(&body)?;
        assert_error_envelope(&body, "ServiceUnavailable", "LoginTimedOut");
        Ok(())
    }

    #[tokio::test]
    async fn every_json_write_route_maps_syntax_errors_to_console_envelope() -> TestResult {
        let cases = [
            (Method::POST, "/login"),
            (Method::POST, "/tenants"),
            (Method::POST, "/tenants/yaml"),
            (Method::PUT, "/namespaces/storage/tenants/tenant-a"),
            (Method::PUT, "/namespaces/storage/tenants/tenant-a/yaml"),
            (
                Method::PUT,
                "/namespaces/storage/tenants/tenant-a/encryption",
            ),
            (
                Method::PUT,
                "/namespaces/storage/tenants/tenant-a/security-context",
            ),
            (Method::POST, "/namespaces/storage/tenants/tenant-a/pools"),
            (
                Method::POST,
                "/namespaces/storage/tenants/tenant-a/pools/primary/decommission",
            ),
            (
                Method::POST,
                "/namespaces/storage/tenants/tenant-a/pools/primary/decommission/cancel",
            ),
            (
                Method::POST,
                "/namespaces/storage/tenants/tenant-a/pods/tenant-a-0/restart",
            ),
            (Method::POST, "/namespaces"),
        ];

        for (method, path) in cases {
            let (status, body) =
                send_json_request(method, path, Some("application/json"), "{").await?;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
            assert_error_envelope(&body, "BadRequest", "InvalidJsonSyntax");
        }
        Ok(())
    }

    #[tokio::test]
    async fn json_write_route_maps_unsupported_content_type_to_console_envelope() -> TestResult {
        for content_type in [None, Some("text/plain")] {
            let (status, body) =
                send_json_request(Method::POST, "/login", content_type, r#"{"token":"test"}"#)
                    .await?;

            assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
            assert_error_envelope(&body, "UnsupportedMediaType", "UnsupportedJsonContentType");
        }
        Ok(())
    }

    #[tokio::test]
    async fn json_write_route_maps_invalid_data_to_console_envelope() -> TestResult {
        let (status, body) = send_json_request(
            Method::POST,
            "/login",
            Some("application/json"),
            r#"{"token":42}"#,
        )
        .await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_error_envelope(&body, "UnprocessableEntity", "InvalidJsonData");
        Ok(())
    }

    #[tokio::test]
    async fn json_rejection_message_is_bounded_for_large_unknown_fields() -> TestResult {
        let unknown_field = "x".repeat(JSON_REJECTION_MESSAGE_MAX_BYTES * 4);
        let request_body = format!(r#"{{"{unknown_field}":true,"token":"test"}}"#);
        let (status, body) = send_json_request(
            Method::POST,
            "/login",
            Some("application/json"),
            &request_body,
        )
        .await?;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_error_envelope(&body, "UnprocessableEntity", "InvalidJsonData");
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .expect("error message should be present");
        assert!(message.len() <= JSON_REJECTION_MESSAGE_MAX_BYTES);
        assert!(message.ends_with("... [truncated]"));
        Ok(())
    }

    #[tokio::test]
    async fn oversized_json_body_maps_to_console_envelope() -> TestResult {
        const DEFAULT_JSON_BODY_LIMIT: usize = 2 * 1024 * 1024;
        let body = serde_json::json!({
            "token": "x".repeat(DEFAULT_JSON_BODY_LIMIT),
        })
        .to_string();
        assert!(body.len() > DEFAULT_JSON_BODY_LIMIT);

        let (status, body) =
            send_json_request(Method::POST, "/login", Some("application/json"), &body).await?;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_error_envelope(&body, "RequestBodyError", "InvalidRequestBody");
        Ok(())
    }

    #[tokio::test]
    async fn encryption_route_maps_invalid_data_to_console_envelope() -> TestResult {
        for body in [
            r#"{"enabled":"true"}"#,
            r#"{"enabled":true,"backend":"valut"}"#,
            r#"{"enabled":true,"enabeld":true}"#,
        ] {
            let (status, body) = send_json_request(
                Method::PUT,
                "/namespaces/storage/tenants/tenant-a/encryption",
                Some("application/json"),
                body,
            )
            .await?;

            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_error_envelope(&body, "UnprocessableEntity", "InvalidJsonData");
        }
        Ok(())
    }
}
