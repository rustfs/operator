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
    Router,
    routing::{delete, get, post, put},
};

use crate::console::{handlers, state::AppState};

/// Login / session routes (partially unauthenticated)
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(handlers::auth::login))
        .route("/logout", post(handlers::auth::logout))
        .route("/session", get(handlers::auth::session_check))
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
    use super::{auth_routes, cluster_routes, pod_routes, pool_routes, tenant_routes};
    use crate::console::state::{AppState, Claims};
    use axum::{
        Extension, Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header},
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
