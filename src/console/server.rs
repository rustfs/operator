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

use crate::console::{openapi::ApiDoc, routes, state::AppState};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::{Router, middleware, response::IntoResponse, routing::get};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, api::ListParams};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Build CORS allowed origins from env.
/// Env: CORS_ALLOWED_ORIGINS, comma-separated (e.g. "https://console.example.com,http://localhost:3000").
/// When frontend and backend are served under the same host (e.g. Ingress path / and /api/v1),
/// browser requests are same-origin and CORS is not used; this is mainly for dev or split-host deployments.
fn cors_allowed_origins() -> Vec<HeaderValue> {
    let s = match std::env::var("CORS_ALLOWED_ORIGINS") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Vec::new(),
    };
    s.split(',')
        .map(|o| o.trim())
        .filter(|o| !o.is_empty())
        .filter_map(|o| o.parse().ok())
        .collect()
}

/// Start the Console HTTP server (Axum).
pub async fn run(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    crate::install_rustls_crypto_provider();
    crate::init_tracing();

    tracing::info!("Starting RustFS Operator Console on port {}", port);

    let jwt_secret = load_jwt_secret();

    let state = match Client::try_default().await {
        Ok(kube_client) => {
            tracing::info!("Kubernetes client initialized for STS authorization flow");
            AppState::new(jwt_secret).with_kube_client(kube_client)
        }
        Err(error) => {
            tracing::warn!(
                "Kubernetes client unavailable; STS authorization paths fall back to compatibility mode: {}",
                error
            );
            AppState::new(jwt_secret)
        }
    };

    let cors_origins = cors_allowed_origins();

    // CorsLayer is outermost so OPTIONS preflight is answered by CORS before auth/routing.
    let app = Router::new()
        // Liveness (unauthenticated)
        .route("/healthz", get(health_check))
        .route("/readyz", get(ready_check))
        .route("/metrics", get(crate::metrics::handler))
        // OpenAPI / Swagger (unauthenticated)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        // REST API v1
        .nest("/api/v1", api_routes())
        // Shared state
        .with_state(state.clone())
        // Middleware runs in reverse order: Trace -> Compression -> Cors -> auth
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::console::middleware::auth::auth_middleware,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(cors_origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::COOKIE])
                .allow_credentials(true),
        )
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(crate::metrics::record_console_http));

    // Bind and serve
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("Console server listening on http://{}", addr);
    tracing::info!("API endpoints:");
    tracing::info!("  - POST /api/v1/login");
    tracing::info!("  - GET  /api/v1/tenants");
    tracing::info!("  - GET  /healthz");

    axum::serve(listener, app).await?;

    Ok(())
}

/// Merge all `/api/v1` route trees.
fn api_routes() -> Router<AppState> {
    Router::new()
        .merge(routes::auth_routes())
        .merge(routes::tenant_routes())
        .merge(routes::pool_routes())
        .merge(routes::pod_routes())
        .merge(routes::event_routes())
        .merge(routes::cluster_routes())
        .merge(routes::topology_routes())
}

/// Liveness probe: always OK if process runs.
async fn health_check() -> impl IntoResponse {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (StatusCode::OK, format!("OK: {}", since_epoch.as_secs()))
}

/// Readiness: Kubernetes API reachable.
async fn ready_check() -> impl IntoResponse {
    match check_k8s_connectivity().await {
        Ok(()) => (StatusCode::OK, "Ready".to_string()),
        Err(e) => {
            tracing::warn!("Readiness check failed: {}", e);
            (StatusCode::SERVICE_UNAVAILABLE, format!("Not ready: {}", e))
        }
    }
}

/// Load kubeconfig, build client, list namespaces (limit 1).
async fn check_k8s_connectivity() -> Result<(), String> {
    let config = kube::Config::infer()
        .await
        .map_err(|e| format!("kubeconfig: {}", e))?;
    let client = Client::try_from(config).map_err(|e| format!("client: {}", e))?;
    let api: Api<corev1::Namespace> = Api::all(client);
    api.list(&ListParams::default().limit(1))
        .await
        .map_err(|e| format!("API: {}", e))?;
    Ok(())
}

fn load_jwt_secret() -> String {
    if let Some(secret) = std::env::var("JWT_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return secret;
    }

    tracing::warn!(
        "JWT_SECRET is not set; generated an ephemeral Console session key for this process"
    );
    generate_ephemeral_jwt_secret()
}

fn generate_ephemeral_jwt_secret() -> String {
    use sha2::Digest;

    let mut bytes = [0u8; 32];
    if read_urandom(&mut bytes).is_ok() {
        return hex::encode(bytes);
    }

    let mut hasher = sha2::Sha256::new();
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
            .to_be_bytes(),
    );
    hasher.update(std::process::id().to_be_bytes());
    hex::encode(hasher.finalize())
}

fn read_urandom(bytes: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;

    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(bytes)
}
