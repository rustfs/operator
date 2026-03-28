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

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Login request body
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    /// Kubernetes ServiceAccount Token
    pub token: String,
}

/// Login response
#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    pub success: bool,
    pub message: String,
}

/// Session check response
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionResponse {
    pub valid: bool,
    pub expires_at: Option<String>,
}
