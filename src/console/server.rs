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
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use axum::{Router, middleware, response::IntoResponse, routing::get};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, api::ListParams};
use std::{
    convert::Infallible,
    future::Future,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};
use tower::Service;
use tower_http::{
    compression::CompressionLayer,
    cors::CorsLayer,
    services::{ServeDir, ServeFile, fs::ServeFileSystemResponseBody},
    trace::TraceLayer,
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

const CONSOLE_STATIC_DIR_ENV: &str = "CONSOLE_STATIC_DIR";
const IMAGE_CONSOLE_STATIC_DIR: &str = "/app/console-web";
const LOCAL_CONSOLE_STATIC_DIR: &str = "console-web/out";

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

    tracing::info!(port, "Starting RustFS Operator Console");

    let jwt_secret = load_jwt_secret();

    let state = match Client::try_default().await {
        Ok(kube_client) => {
            tracing::info!("Kubernetes client initialized for STS authorization flow");
            AppState::new(jwt_secret).with_kube_client(kube_client)
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "Kubernetes client unavailable; STS authorization paths fall back to compatibility mode"
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
        .with_state(state.clone());
    let app = with_static_frontend(app)
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

    tracing::info!(%addr, "Console server listening");
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

fn with_static_frontend(app: Router) -> Router {
    let Some(static_dir) = static_frontend_dir() else {
        tracing::warn!(
            env = CONSOLE_STATIC_DIR_ENV,
            "Console frontend static files not found; serving API only"
        );
        return app;
    };

    tracing::info!(static_dir = %static_dir.display(), "Serving Console frontend");
    app.fallback_service(static_frontend_service(static_dir))
}

fn static_frontend_service(static_dir: PathBuf) -> StaticFrontendService {
    let index_path = static_dir.join("index.html");
    let static_service = ServeDir::new(static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index_path.clone()));

    StaticFrontendService {
        static_service,
        index_file: ServeFile::new(index_path),
    }
}

#[derive(Clone)]
struct StaticFrontendService {
    static_service: ServeDir<ServeFile>,
    index_file: ServeFile,
}

impl Service<Request<Body>> for StaticFrontendService {
    type Response = Response<ServeFileSystemResponseBody>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        <ServeDir<ServeFile> as Service<Request<Body>>>::poll_ready(&mut self.static_service, cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        if is_api_path(request.uri().path()) {
            return Box::pin(async { Ok(api_not_found_response()) });
        }

        let mut static_service = self.static_service.clone();
        let mut index_file = self.index_file.clone();
        let method = request.method().clone();
        Box::pin(async move {
            let response = static_service.call(request).await?;
            if response.status() != StatusCode::NOT_FOUND {
                return Ok(response);
            }

            let mut fallback_request = Request::new(Body::empty());
            *fallback_request.method_mut() = method;
            *fallback_request.uri_mut() = Uri::from_static("/");
            index_file.call(fallback_request).await
        })
    }
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn api_not_found_response() -> Response<ServeFileSystemResponseBody> {
    let mut response = Response::new(ServeFileSystemResponseBody::default());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

fn static_frontend_dir() -> Option<PathBuf> {
    let candidates = match std::env::var(CONSOLE_STATIC_DIR_ENV) {
        Ok(value) if !value.trim().is_empty() => vec![PathBuf::from(value.trim())],
        _ => vec![
            PathBuf::from(IMAGE_CONSOLE_STATIC_DIR),
            PathBuf::from(LOCAL_CONSOLE_STATIC_DIR),
        ],
    };

    candidates
        .into_iter()
        .find(|dir| dir.join("index.html").is_file())
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
        Err(error) => {
            tracing::warn!(%error, "Readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Not ready: {}", error),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tower::ServiceExt;

    static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_static_dir() -> std::io::Result<PathBuf> {
        let id = NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rustfs-console-static-{}-{id}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("index.html"), "console")?;
        Ok(dir)
    }

    #[tokio::test]
    async fn static_frontend_fallback_does_not_handle_api_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = temp_static_dir()?;

        let response = static_frontend_service(dir.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/missing")
                    .body(Body::empty())?,
            )
            .await
            .map_err(|error| match error {})?;

        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn static_frontend_fallback_serves_spa_paths() -> Result<(), Box<dyn std::error::Error>> {
        let dir = temp_static_dir()?;

        let response = static_frontend_service(dir.clone())
            .oneshot(Request::builder().uri("/tenants").body(Body::empty())?)
            .await
            .map_err(|error| match error {})?;

        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }
}
