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

use crate::cluster_dns;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use kube::Client;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use std::sync::Arc;

pub const SESSION_TTL_SECONDS: usize = 12 * 3600;
const SESSION_AAD: &[u8] = b"rustfs-operator-console-session-v1";
const SESSION_KEY_CONTEXT: &[u8] = b"rustfs-operator-console-session-key-v1";
const SESSION_NONCE_LEN: usize = 12;

/// Shared Axum application state.
///
/// Holds global config such as the Console session encryption secret.
#[derive(Clone)]
pub struct AppState {
    /// Symmetric key source for encrypting session cookies.
    pub jwt_secret: Arc<String>,

    /// Optional Kubernetes client used by control-plane APIs that need cluster access.
    ///
    /// Most unit tests run without a live cluster, so this is optional.
    pub kube_client: Option<Client>,

    /// Kubernetes cluster DNS domain used by handlers that call Tenant services.
    pub cluster_domain: Arc<String>,
}

impl AppState {
    /// Build state with the given session secret.
    pub fn new(jwt_secret: String) -> Self {
        Self {
            jwt_secret: Arc::new(jwt_secret),
            kube_client: None,
            cluster_domain: Arc::new(cluster_dns::DEFAULT_CLUSTER_DOMAIN.to_string()),
        }
    }

    /// Attach a Kubernetes client for request handlers that need cluster reads.
    pub fn with_kube_client(mut self, kube_client: Client) -> Self {
        self.kube_client = Some(kube_client);
        self
    }

    pub fn with_cluster_domain(mut self, cluster_domain: &str) -> Self {
        self.cluster_domain = Arc::new(cluster_domain.to_string());
        self
    }

    pub fn create_session(&self, k8s_token: String) -> Result<String, SessionError> {
        let iat = current_timestamp();
        let exp = iat.saturating_add(SESSION_TTL_SECONDS);
        let claims = SessionClaims {
            k8s_token,
            exp,
            iat,
        };
        seal_session_token(&self.jwt_secret, &claims)
    }

    pub fn resolve_session(&self, token: &str) -> Option<Claims> {
        let session_claims = match open_session_token(&self.jwt_secret, token) {
            Ok(claims) => claims,
            Err(error) => {
                tracing::warn!(%error, "Console session token validation failed");
                return None;
            }
        };
        let now = current_timestamp();
        if session_claims.exp < now {
            return None;
        }

        Some(Claims {
            k8s_token: session_claims.k8s_token,
            exp: session_claims.exp,
            iat: session_claims.iat,
        })
    }
}

/// Authenticated request context inserted by middleware.
#[derive(Debug, Clone)]
pub struct Claims {
    pub k8s_token: String,
    pub exp: usize,
    pub iat: usize,
}

/// Encrypted browser cookie session claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionClaims {
    pub k8s_token: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Snafu)]
pub enum SessionError {
    #[snafu(display("failed to generate session nonce"))]
    Random,

    #[snafu(display("failed to serialize session claims: {}", source))]
    Serialize { source: serde_json::Error },

    #[snafu(display("failed to deserialize session claims: {}", source))]
    Deserialize { source: serde_json::Error },

    #[snafu(display("failed to decode session token: {}", source))]
    Decode { source: base64::DecodeError },

    #[snafu(display("session token has invalid format"))]
    InvalidFormat,

    #[snafu(display("failed to initialize session encryption key"))]
    Key,

    #[snafu(display("failed to encrypt session token"))]
    Encrypt,

    #[snafu(display("failed to decrypt session token"))]
    Decrypt,
}

fn current_timestamp() -> usize {
    usize::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

fn seal_session_token(jwt_secret: &str, claims: &SessionClaims) -> Result<String, SessionError> {
    let mut nonce_bytes = [0u8; SESSION_NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| SessionError::Random)?;

    let mut ciphertext =
        serde_json::to_vec(claims).map_err(|source| SessionError::Serialize { source })?;
    session_key(jwt_secret)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(SESSION_AAD),
            &mut ciphertext,
        )
        .map_err(|_| SessionError::Encrypt)?;

    let mut token = Vec::with_capacity(SESSION_NONCE_LEN + ciphertext.len());
    token.extend_from_slice(&nonce_bytes);
    token.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(token))
}

fn open_session_token(jwt_secret: &str, token: &str) -> Result<SessionClaims, SessionError> {
    let mut token_bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|source| SessionError::Decode { source })?;
    if token_bytes.len() <= SESSION_NONCE_LEN {
        return Err(SessionError::InvalidFormat);
    }

    let mut nonce_bytes = [0u8; SESSION_NONCE_LEN];
    nonce_bytes.copy_from_slice(&token_bytes[..SESSION_NONCE_LEN]);
    let mut ciphertext = token_bytes.split_off(SESSION_NONCE_LEN);
    let plaintext = session_key(jwt_secret)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(SESSION_AAD),
            &mut ciphertext,
        )
        .map_err(|_| SessionError::Decrypt)?;

    serde_json::from_slice(plaintext).map_err(|source| SessionError::Deserialize { source })
}

fn session_key(jwt_secret: &str) -> Result<LessSafeKey, SessionError> {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_KEY_CONTEXT);
    hasher.update([0]);
    hasher.update(jwt_secret.as_bytes());
    let digest = hasher.finalize();
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&digest);
    let key = UnboundKey::new(&aead::AES_256_GCM, &key_bytes).map_err(|_| SessionError::Key)?;
    Ok(LessSafeKey::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_token_does_not_embed_kubernetes_token() {
        let state = AppState::new("test-secret".to_string());
        let token = state
            .create_session("sensitive-k8s-token".to_string())
            .expect("session token is encrypted");

        assert!(!token.contains("sensitive-k8s-token"));

        let claims = state
            .resolve_session(&token)
            .expect("encrypted session resolves");
        assert_eq!(claims.k8s_token, "sensitive-k8s-token");
    }

    #[test]
    fn session_cookie_token_resolves_across_replicas_with_same_secret() {
        let first_replica = AppState::new("shared-secret".to_string());
        let second_replica = AppState::new("shared-secret".to_string());
        let token = first_replica
            .create_session("replica-safe-token".to_string())
            .expect("session token is encrypted");

        let claims = second_replica
            .resolve_session(&token)
            .expect("same secret resolves session");
        assert_eq!(claims.k8s_token, "replica-safe-token");
    }

    #[test]
    fn session_cookie_token_rejects_different_secret() {
        let first_replica = AppState::new("first-secret".to_string());
        let second_replica = AppState::new("second-secret".to_string());
        let token = first_replica
            .create_session("replica-safe-token".to_string())
            .expect("session token is encrypted");

        assert!(second_replica.resolve_session(&token).is_none());
    }

    #[test]
    fn session_cookie_token_rejects_tampering() {
        let state = AppState::new("test-secret".to_string());
        let token = state
            .create_session("sensitive-k8s-token".to_string())
            .expect("session token is encrypted");
        let mut token_bytes = URL_SAFE_NO_PAD
            .decode(&token)
            .expect("session token decodes");
        let last_byte = token_bytes.last_mut().expect("session token is non-empty");
        *last_byte ^= 1;
        let tampered_token = URL_SAFE_NO_PAD.encode(token_bytes);

        assert!(state.resolve_session(&tampered_token).is_none());
    }
}
