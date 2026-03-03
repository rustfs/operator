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

use crate::console::{routes, state::AppState};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::{Router, middleware, response::IntoResponse, routing::get};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, api::ListParams};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

/// Build CORS allowed origins from env or default.
/// Env: CORS_ALLOWED_ORIGINS, comma-separated (e.g. "https://console.example.com,http://localhost:3000").
/// When frontend and backend are served under the same host (e.g. Ingress path / and /api/v1),
/// browser requests are same-origin and CORS is not used; this is mainly for dev or split-host deployments.
fn cors_allowed_origins() -> Vec<HeaderValue> {
    let default: Vec<HeaderValue> = [
        "http://localhost:3000",
        "http://localhost:8080",
        "http://127.0.0.1:3000",
        "http://127.0.0.1:8080",
    ]
    .iter()
    .filter_map(|s| s.parse().ok())
    .collect();
    let s = match std::env::var("CORS_ALLOWED_ORIGINS") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return default,
    };
    let parsed: Vec<HeaderValue> = s
        .split(',')
        .map(|o| o.trim())
        .filter(|o| !o.is_empty())
        .filter_map(|o| o.parse().ok())
        .collect();
    if parsed.is_empty() { default } else { parsed }
}

/// 启动 Console HTTP Server
pub async fn run(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Starting RustFS Operator Console on port {}", port);

    // 生成 JWT 密钥 (实际生产应从环境变量读取)
    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "rustfs-console-secret-change-me-in-production".to_string());

    let state = AppState::new(jwt_secret);

    let cors_origins = cors_allowed_origins();

    // 构建应用。CorsLayer 放在最外层，使 OPTIONS 预检由 CORS 直接响应，避免被 auth 或路由影响。
    let app = Router::new()
        // 健康检查 (无需认证)
        .route("/healthz", get(health_check))
        .route("/readyz", get(ready_check))
        // API v1 路由
        .nest("/api/v1", api_routes())
        // 应用状态
        .with_state(state.clone())
        // 中间件：最后添加的最先执行，故请求顺序为 Trace -> Compression -> Cors -> auth
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
        .layer(TraceLayer::new_for_http());

    // 启动服务器
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

/// API 路由组合
fn api_routes() -> Router<AppState> {
    Router::new()
        .merge(routes::auth_routes())
        .merge(routes::tenant_routes())
        .merge(routes::pool_routes())
        .merge(routes::pod_routes())
        .merge(routes::event_routes())
        .merge(routes::cluster_routes())
}

/// 健康检查
async fn health_check() -> impl IntoResponse {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (StatusCode::OK, format!("OK: {}", since_epoch.as_secs()))
}

/// 就绪检查：验证 K8s API 可连通
async fn ready_check() -> impl IntoResponse {
    match check_k8s_connectivity().await {
        Ok(()) => (StatusCode::OK, "Ready".to_string()),
        Err(e) => {
            tracing::warn!("Readiness check failed: {}", e);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Not ready: {}", e),
            )
        }
    }
}

/// 验证 K8s 连接：加载配置、创建客户端、执行轻量级 API 调用
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
