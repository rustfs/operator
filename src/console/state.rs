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

use std::sync::Arc;

/// Shared Axum application state.
///
/// Holds global config such as the JWT signing secret.
#[derive(Clone)]
pub struct AppState {
    /// Symmetric key for signing session JWTs
    pub jwt_secret: Arc<String>,
}

impl AppState {
    /// Build state with the given JWT secret
    pub fn new(jwt_secret: String) -> Self {
        Self {
            jwt_secret: Arc::new(jwt_secret),
        }
    }
}

/// JWT Claims
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Claims {
    /// Kubernetes ServiceAccount Token
    pub k8s_token: String,
    /// Expiry time (Unix seconds)
    pub exp: usize,
    /// Issued-at time (Unix seconds)
    pub iat: usize,
}

impl Claims {
    /// New claims with a 12-hour lifetime
    pub fn new(k8s_token: String) -> Self {
        let now = chrono::Utc::now().timestamp() as usize;
        Self {
            k8s_token,
            iat: now,
            exp: now + 12 * 3600, // 12 hours
        }
    }
}
