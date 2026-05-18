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

use kube::Client;
use std::sync::Arc;

/// Shared Axum application state.
///
/// Holds global config such as the JWT signing secret.
#[derive(Clone)]
pub struct AppState {
    /// Symmetric key for signing session JWTs
    pub jwt_secret: Arc<String>,

    /// Optional Kubernetes client used by control-plane APIs that need cluster access.
    ///
    /// Most unit tests run without a live cluster, so this is optional.
    pub kube_client: Option<Client>,
}

impl AppState {
    /// Build state with the given JWT secret
    pub fn new(jwt_secret: String) -> Self {
        Self {
            jwt_secret: Arc::new(jwt_secret),
            kube_client: None,
        }
    }

    /// Attach a Kubernetes client for request handlers that need cluster reads.
    pub fn with_kube_client(mut self, kube_client: Client) -> Self {
        self.kube_client = Some(kube_client);
        self
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
