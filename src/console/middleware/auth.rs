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
    http::{Method, StatusCode, header},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{DecodingKey, Validation, decode};

use crate::console::state::{AppState, Claims};

/// JWT session middleware.
///
/// Reads the `session` cookie, validates the JWT, and inserts `Claims` into request extensions.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Allow CORS preflight without 401 (browser would treat as CORS failure)
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }
    // Unauthenticated paths
    let path = request.uri().path();
    if path == "/healthz"
        || path == "/readyz"
        || path.starts_with("/api/v1/login")
        || path.starts_with("/swagger-ui")
        || path.starts_with("/api-docs")
    {
        return Ok(next.run(request).await);
    }

    // Parse session cookie
    let cookies = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = parse_session_cookie(cookies).ok_or(StatusCode::UNAUTHORIZED)?;

    // Verify JWT signature and claims
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

    // Reject expired tokens
    let now = chrono::Utc::now().timestamp() as usize;
    if claims.exp < now {
        tracing::warn!("Token expired");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Stash claims for handlers
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Extract `session=<jwt>` from a raw `Cookie` header value
fn parse_session_cookie(cookies: &str) -> Option<String> {
    cookies.split(';').find_map(|cookie| {
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
        assert_eq!(
            parse_session_cookie(cookies),
            Some("test_token".to_string())
        );

        let cookies = "other=value";
        assert_eq!(parse_session_cookie(cookies), None);
    }
}
