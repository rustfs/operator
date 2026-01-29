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
    middleware,
    routing::get,
    Router,
    http::StatusCode,
    response::IntoResponse,
};
use tower_http::{
    compression::CompressionLayer,
    cors::CorsLayer,
    trace::TraceLayer,
};
use axum::http::{HeaderValue, Method, header};

use crate::console::{state::AppState, routes};

/// 启动 Console HTTP Server
pub async fn run(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Starting RustFS Operator Console on port {}", port);

    // 生成 JWT 密钥 (实际生产应从环境变量读取)
    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "rustfs-console-secret-change-me-in-production".to_string());

    let state = AppState::new(jwt_secret);

    // 构建应用
    let app = Router::new()
        // 健康检查 (无需认证)
        .route("/healthz", get(health_check))
        .route("/readyz", get(ready_check))
        // API v1 路由
        .nest("/api/v1", api_routes())
        // 应用状态
        .with_state(state.clone())
        // 应用中间件层 (从内到外)
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin("http://localhost:3000".parse::<HeaderValue>().unwrap())
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::COOKIE])
                .allow_credentials(true),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::console::middleware::auth::auth_middleware,
        ));

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
        .merge(routes::event_routes())
        .merge(routes::cluster_routes())
}

/// 健康检查
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// 就绪检查
async fn ready_check() -> impl IntoResponse {
    // TODO: 检查 K8s 连接等
    (StatusCode::OK, "Ready")
}
