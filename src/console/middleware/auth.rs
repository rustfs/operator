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
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};

use crate::console::state::{AppState, Claims};

/// JWT 认证中间件
///
/// 从 Cookie 中提取 JWT Token,验证后将 Claims 注入到请求扩展中
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 跳过公开路径
    let path = request.uri().path();
    if path == "/healthz" || path == "/readyz" || path.starts_with("/api/v1/login") {
        return Ok(next.run(request).await);
    }

    // 从 Cookie 中提取 Token
    let cookies = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = parse_session_cookie(cookies).ok_or(StatusCode::UNAUTHORIZED)?;

    // 验证 JWT
    let claims = decode::<Claims>(
        &token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| {
        tracing::warn!("JWT validation failed: {}", e);
        StatusCode::UNAUTHORIZED
    })?
    .claims;

    // 检查过期时间
    let now = chrono::Utc::now().timestamp() as usize;
    if claims.exp < now {
        tracing::warn!("Token expired");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 将 Claims 注入请求扩展
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// 从 Cookie 字符串中解析 session token
fn parse_session_cookie(cookies: &str) -> Option<String> {
    cookies
        .split(';')
        .find_map(|cookie| {
            let parts: Vec<&str> = cookie.trim().splitn(2, '=').collect();
            if parts.len() == 2 && parts[0] == "session" {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_cookie() {
        let cookies = "session=test_token; other=value";
        assert_eq!(parse_session_cookie(cookies), Some("test_token".to_string()));

        let cookies = "other=value";
        assert_eq!(parse_session_cookie(cookies), None);
    }
}
