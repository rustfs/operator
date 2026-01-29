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
    extract::State,
    http::header,
    response::IntoResponse,
    Extension, Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};
use kube::Client;
use snafu::ResultExt;

use crate::console::{
    error::{self, Error, Result},
    models::auth::{LoginRequest, LoginResponse, SessionResponse},
    state::{AppState, Claims},
};
use crate::types::v1alpha1::tenant::Tenant;

/// 登录处理
///
/// 验证 Kubernetes Token 并生成 Console Session Token
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse> {
    tracing::info!("Login attempt");

    // 验证 K8s Token (尝试创建客户端并测试权限)
    let client = create_k8s_client(&req.token).await?;

    // 测试权限 - 尝试列出 Tenant (limit 1)
    let api: kube::Api<Tenant> = kube::Api::all(client);
    api.list(&kube::api::ListParams::default().limit(1))
        .await
        .map_err(|e| {
            tracing::warn!("K8s API test failed: {}", e);
            Error::Unauthorized {
                message: "Invalid or insufficient permissions".to_string(),
            }
        })?;

    // 生成 JWT
    let claims = Claims::new(req.token);
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .context(error::JwtSnafu)?;

    // 设置 HttpOnly Cookie
    let cookie = format!(
        "session={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        token,
        12 * 3600 // 12 hours
    );

    let headers = [(header::SET_COOKIE, cookie)];

    Ok((
        headers,
        Json(LoginResponse {
            success: true,
            message: "Login successful".to_string(),
        }),
    ))
}

/// 登出处理
pub async fn logout() -> impl IntoResponse {
    // 清除 Cookie
    let cookie = "session=; Path=/; HttpOnly; Max-Age=0";
    let headers = [(header::SET_COOKIE, cookie)];

    (
        headers,
        Json(LoginResponse {
            success: true,
            message: "Logout successful".to_string(),
        }),
    )
}

/// 检查会话
pub async fn session_check(Extension(claims): Extension<Claims>) -> Json<SessionResponse> {
    let expires_at = chrono::DateTime::from_timestamp(claims.exp as i64, 0)
        .map(|dt| dt.to_rfc3339());

    Json(SessionResponse {
        valid: true,
        expires_at,
    })
}

/// 创建 Kubernetes 客户端 (使用 Token)
async fn create_k8s_client(token: &str) -> Result<Client> {
    // 使用默认配置加载
    let mut config = kube::Config::infer().await.map_err(|e| Error::InternalServer {
        message: format!("Failed to load kubeconfig: {}", e),
    })?;

    // 覆盖 token
    config.auth_info.token = Some(token.to_string().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}
