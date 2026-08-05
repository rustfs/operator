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
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub const SESSION_TTL_SECONDS: usize = 12 * 3600;
pub(crate) const MAX_SESSION_TOKEN_BYTES: usize = 16 * 1024;
const SESSION_AAD: &[u8] = b"rustfs-operator-console-session-v1";
const SESSION_KEY_CONTEXT: &[u8] = b"rustfs-operator-console-session-key-v1";
const SESSION_NONCE_LEN: usize = 12;
const SESSION_ID_BYTES: usize = 16;
const SESSION_ID_LEN: usize = SESSION_ID_BYTES * 2;
pub(crate) const MAX_ACTIVE_SESSIONS: usize = 4096;
pub(crate) const MAX_SESSIONS_PER_TOKEN: usize = 8;

/// Shared Axum application state.
///
/// Holds global config such as the Console session encryption secret.
#[derive(Clone)]
pub struct AppState {
    /// Symmetric key source for encrypting server-side session data.
    pub jwt_secret: Arc<String>,

    /// Optional Kubernetes client used by control-plane APIs that need cluster access.
    ///
    /// Most unit tests run without a live cluster, so this is optional.
    pub kube_client: Option<Client>,

    /// Kubernetes cluster DNS domain used by handlers that call Tenant services.
    pub cluster_domain: Arc<String>,

    sessions: Arc<Mutex<HashMap<String, Arc<StoredSession>>>>,
}

#[derive(Clone)]
struct StoredSession {
    sealed_claims: String,
    token_fingerprint: [u8; 32],
    expires_at: usize,
}

impl AppState {
    /// Build state with the given session secret.
    pub fn new(jwt_secret: String) -> Self {
        Self {
            jwt_secret: Arc::new(jwt_secret),
            kube_client: None,
            cluster_domain: Arc::new(cluster_dns::DEFAULT_CLUSTER_DOMAIN.to_string()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
        if k8s_token.len() > MAX_SESSION_TOKEN_BYTES {
            return Err(SessionError::TokenTooLarge);
        }
        let iat = current_timestamp();
        let exp = iat.saturating_add(SESSION_TTL_SECONDS);
        let session_id = generate_session_id()?;
        let token_fingerprint = Sha256::digest(k8s_token.as_bytes()).into();
        let claims = SessionClaims {
            session_id: session_id.clone(),
            k8s_token,
            exp,
            iat,
        };
        let sealed_claims = seal_session_token(&self.jwt_secret, &claims)?;
        let mut sessions = self.sessions.lock().map_err(|_| SessionError::StoreLock)?;
        sessions.retain(|_, session| session.expires_at > iat);
        if sessions.len() >= MAX_ACTIVE_SESSIONS {
            return Err(SessionError::Capacity);
        }
        if sessions
            .values()
            .filter(|session| session.token_fingerprint == token_fingerprint)
            .count()
            >= MAX_SESSIONS_PER_TOKEN
        {
            return Err(SessionError::PerTokenCapacity);
        }
        match sessions.entry(session_id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(StoredSession {
                    sealed_claims,
                    token_fingerprint,
                    expires_at: exp,
                }));
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(SessionError::IdCollision);
            }
        }
        Ok(session_id)
    }

    pub fn resolve_session(&self, session_id: &str) -> Result<Option<Claims>, SessionError> {
        if !is_valid_session_id(session_id) {
            return Ok(None);
        }
        let Some(stored_session) = self
            .sessions
            .lock()
            .map_err(|_| SessionError::StoreLock)?
            .get(session_id)
            .cloned()
        else {
            return Ok(None);
        };
        let session_claims =
            match open_session_token(&self.jwt_secret, &stored_session.sealed_claims) {
                Ok(claims) => claims,
                Err(error) => {
                    tracing::warn!(%error, "Console session token validation failed");
                    return Ok(None);
                }
            };
        if session_claims.session_id != session_id {
            return Ok(None);
        }
        let now = current_timestamp();
        if session_claims.exp <= now {
            self.revoke_session(session_id)?;
            return Ok(None);
        }

        Ok(Some(Claims {
            k8s_token: session_claims.k8s_token,
            exp: session_claims.exp,
            iat: session_claims.iat,
        }))
    }

    pub fn revoke_session(&self, session_id: &str) -> Result<(), SessionError> {
        if !is_valid_session_id(session_id) {
            return Ok(());
        }
        self.sessions
            .lock()
            .map_err(|_| SessionError::StoreLock)?
            .remove(session_id);
        Ok(())
    }
}

/// Authenticated request context inserted by middleware.
#[derive(Debug, Clone)]
pub struct Claims {
    pub k8s_token: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionClaims {
    pub session_id: String,
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

    #[snafu(display("session store lock is poisoned"))]
    StoreLock,

    #[snafu(display("generated duplicate session identifier"))]
    IdCollision,

    #[snafu(display("maximum active Console sessions reached"))]
    Capacity,

    #[snafu(display("maximum active Console sessions for this token reached"))]
    PerTokenCapacity,

    #[snafu(display("Kubernetes bearer token exceeds the session size limit"))]
    TokenTooLarge,
}

fn current_timestamp() -> usize {
    usize::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

fn generate_session_id() -> Result<String, SessionError> {
    let mut session_id = [0u8; SESSION_ID_BYTES];
    SystemRandom::new()
        .fill(&mut session_id)
        .map_err(|_| SessionError::Random)?;
    Ok(hex::encode(session_id))
}

fn is_valid_session_id(session_id: &str) -> bool {
    session_id.len() == SESSION_ID_LEN
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn session_cookie_value(cookies: &str) -> Option<&str> {
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == "session").then_some(value)
    })
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
            .expect("session store is available")
            .expect("encrypted session resolves");
        assert_eq!(claims.k8s_token, "sensitive-k8s-token");
    }

    #[test]
    fn cloned_state_shares_session_store() {
        let state = AppState::new("shared-secret".to_string());
        let cloned_state = state.clone();
        let token = state
            .create_session("replica-safe-token".to_string())
            .expect("session token is encrypted");

        let claims = cloned_state
            .resolve_session(&token)
            .expect("session store is available")
            .expect("same secret resolves session");
        assert_eq!(claims.k8s_token, "replica-safe-token");
    }

    #[test]
    fn process_restart_drops_existing_sessions() {
        let first_process = AppState::new("shared-secret".to_string());
        let session_id = first_process
            .create_session("k8s-token".to_string())
            .expect("session is created");
        let restarted_process = AppState::new("shared-secret".to_string());

        assert!(
            restarted_process
                .resolve_session(&session_id)
                .expect("session store is available")
                .is_none()
        );
    }

    #[test]
    fn session_cookie_token_rejects_different_secret() {
        let first_replica = AppState::new("first-secret".to_string());
        let second_replica = AppState {
            jwt_secret: Arc::new("second-secret".to_string()),
            kube_client: None,
            cluster_domain: first_replica.cluster_domain.clone(),
            sessions: first_replica.sessions.clone(),
        };
        let token = first_replica
            .create_session("replica-safe-token".to_string())
            .expect("session token is encrypted");

        assert!(
            second_replica
                .resolve_session(&token)
                .expect("session store is available")
                .is_none()
        );
    }

    #[test]
    fn revoked_session_cookie_no_longer_resolves() {
        let state = AppState::new("test-secret".to_string());
        let token = state
            .create_session("sensitive-k8s-token".to_string())
            .expect("session token is encrypted");

        state.revoke_session(&token).expect("session is revoked");

        assert!(
            state
                .resolve_session(&token)
                .expect("session store is available")
                .is_none()
        );
    }

    #[test]
    fn revoking_one_session_keeps_other_sessions_valid() {
        let state = AppState::new("test-secret".to_string());
        let revoked = state
            .create_session("first-token".to_string())
            .expect("first session is created");
        let active = state
            .create_session("second-token".to_string())
            .expect("second session is created");

        state
            .revoke_session(&revoked)
            .expect("first session is revoked");

        assert!(
            state
                .resolve_session(&revoked)
                .expect("session store is available")
                .is_none()
        );
        assert_eq!(
            state
                .resolve_session(&active)
                .expect("session store is available")
                .expect("second session remains active")
                .k8s_token,
            "second-token"
        );
    }

    #[test]
    fn session_payload_cannot_be_moved_to_another_cookie_id() {
        let state = AppState::new("test-secret".to_string());
        let first = state
            .create_session("first-token".to_string())
            .expect("first session is created");
        let second = state
            .create_session("second-token".to_string())
            .expect("second session is created");
        let mut sessions = state
            .sessions
            .lock()
            .expect("session store lock is available");
        let copied_payload = sessions
            .get(&second)
            .expect("second session is stored")
            .sealed_claims
            .clone();
        let first_session =
            Arc::make_mut(sessions.get_mut(&first).expect("first session is stored"));
        first_session.sealed_claims = copied_payload;
        drop(sessions);

        assert!(
            state
                .resolve_session(&first)
                .expect("session store is available")
                .is_none()
        );
    }

    #[test]
    fn tampered_session_payload_does_not_resolve() {
        let state = AppState::new("test-secret".to_string());
        let session_id = state
            .create_session("k8s-token".to_string())
            .expect("session is created");
        let mut sessions = state
            .sessions
            .lock()
            .expect("session store lock is available");
        let stored_session =
            Arc::make_mut(sessions.get_mut(&session_id).expect("session is stored"));
        let replacement = if stored_session.sealed_claims.starts_with('A') {
            "B"
        } else {
            "A"
        };
        stored_session.sealed_claims.replace_range(..1, replacement);
        drop(sessions);

        assert!(
            state
                .resolve_session(&session_id)
                .expect("session store is available")
                .is_none()
        );
    }

    #[test]
    fn request_time_expiry_revokes_session() {
        let state = AppState::new("test-secret".to_string());
        let session_id = "0123456789abcdef0123456789abcdef".to_string();
        let claims = SessionClaims {
            session_id: session_id.clone(),
            k8s_token: "expired-token".to_string(),
            exp: current_timestamp(),
            iat: current_timestamp().saturating_sub(1),
        };
        let sealed_claims = seal_session_token(&state.jwt_secret, &claims)
            .expect("expired session claims are encrypted");
        state
            .sessions
            .lock()
            .expect("session store lock is available")
            .insert(
                session_id.clone(),
                Arc::new(StoredSession {
                    sealed_claims,
                    token_fingerprint: [0; 32],
                    expires_at: usize::MAX,
                }),
            );

        assert!(
            state
                .resolve_session(&session_id)
                .expect("session store is available")
                .is_none()
        );
        assert!(
            state
                .sessions
                .lock()
                .expect("session store lock is available")
                .get(&session_id)
                .is_none()
        );
    }

    #[test]
    fn generated_session_id_is_valid() {
        let session_id = generate_session_id().expect("session id is generated");

        assert_eq!(session_id.len(), SESSION_ID_LEN);
        assert!(is_valid_session_id(&session_id));
    }

    #[test]
    fn invalid_session_ids_are_rejected() {
        for session_id in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdeg",
            "0123456789ABCDEF0123456789ABCDEF",
        ] {
            assert!(!is_valid_session_id(session_id));
        }
    }

    #[test]
    fn active_session_count_is_bounded() {
        let state = AppState::new("test-secret".to_string());
        let mut sessions = state
            .sessions
            .lock()
            .expect("session store lock is available");
        for index in 0..(MAX_ACTIVE_SESSIONS - 1) {
            sessions.insert(
                format!("{index:032x}"),
                Arc::new(StoredSession {
                    sealed_claims: String::new(),
                    token_fingerprint: [0; 32],
                    expires_at: usize::MAX,
                }),
            );
        }
        drop(sessions);

        state
            .create_session("last-token".to_string())
            .expect("last available session slot is usable");

        assert!(matches!(
            state.create_session("another-token".to_string()),
            Err(SessionError::Capacity)
        ));
    }

    #[test]
    fn expired_sessions_do_not_consume_global_capacity() {
        let state = AppState::new("test-secret".to_string());
        let mut sessions = state
            .sessions
            .lock()
            .expect("session store lock is available");
        for index in 0..MAX_ACTIVE_SESSIONS {
            sessions.insert(
                format!("{index:032x}"),
                Arc::new(StoredSession {
                    sealed_claims: String::new(),
                    token_fingerprint: [0; 32],
                    expires_at: 0,
                }),
            );
        }
        drop(sessions);

        state
            .create_session("new-token".to_string())
            .expect("expired sessions release capacity during login");
    }

    #[test]
    fn oversized_session_token_is_rejected() {
        let state = AppState::new("test-secret".to_string());

        state
            .create_session("x".repeat(MAX_SESSION_TOKEN_BYTES))
            .expect("token at the size limit is accepted");

        assert!(matches!(
            state.create_session("x".repeat(MAX_SESSION_TOKEN_BYTES + 1)),
            Err(SessionError::TokenTooLarge)
        ));
    }

    #[test]
    fn sessions_per_token_are_bounded() {
        let state = AppState::new("test-secret".to_string());
        for _ in 0..MAX_SESSIONS_PER_TOKEN {
            state
                .create_session("shared-token".to_string())
                .expect("session is created within the per-token limit");
        }

        assert!(matches!(
            state.create_session("shared-token".to_string()),
            Err(SessionError::PerTokenCapacity)
        ));

        state
            .create_session("different-token".to_string())
            .expect("one token cannot consume another token's session allowance");
    }

    #[test]
    fn expired_sessions_do_not_consume_per_token_capacity() {
        let state = AppState::new("test-secret".to_string());
        let token = "shared-token";
        let token_fingerprint = Sha256::digest(token.as_bytes()).into();
        let mut sessions = state
            .sessions
            .lock()
            .expect("session store lock is available");
        for index in 0..MAX_SESSIONS_PER_TOKEN {
            sessions.insert(
                format!("{index:032x}"),
                Arc::new(StoredSession {
                    sealed_claims: String::new(),
                    token_fingerprint,
                    expires_at: 0,
                }),
            );
        }
        drop(sessions);

        state
            .create_session(token.to_string())
            .expect("expired sessions release the token allowance during login");
    }

    #[test]
    fn session_cookie_value_extracts_session_id() {
        assert_eq!(
            session_cookie_value("session=test_token; other=value"),
            Some("test_token")
        );
        assert_eq!(session_cookie_value("other=value"), None);
    }
}
