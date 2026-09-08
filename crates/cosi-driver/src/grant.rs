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

//! Grant provisioning state machine (durable secrets + atomic CAS ownership).

use std::collections::HashMap;

use kube::Client;
use rustfs_admin::RustfsAdminClient;
use snafu::Snafu;
use tracing::info;

use crate::credentials::{
    CredentialStoreError, access_key_hash, credential_secret_name, load_or_create_credentials,
};
use crate::ownership::{OwnershipError, OwnershipStore};
use crate::parameters::{self, BackendParameters, bucket_policy_document_for, grant_policy_name};

#[derive(Debug, Snafu)]
pub enum GrantError {
    #[snafu(transparent)]
    Ownership { source: OwnershipError },
    #[snafu(transparent)]
    Credentials { source: CredentialStoreError },
    #[snafu(display("rustfs admin error: {msg}"))]
    Admin { msg: String },
    #[snafu(display(
        "access key `{account_id}` exists in RustFS without a matching ownership proof for \
         grant `{grant_name}`; refusing to adopt a user this driver did not create"
    ))]
    OrphanUserConflict {
        account_id: String,
        grant_name: String,
    },
    #[snafu(display("external policy `{policy}` does not exist"))]
    MissingExternalPolicy { policy: String },
}

impl GrantError {
    /// Conflicts that a COSI retry should see as `ALREADY_EXISTS`.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::OrphanUserConflict { .. }
                | Self::Ownership {
                    source: OwnershipError::AccountConflict { .. }
                }
                | Self::Ownership {
                    source: OwnershipError::AccountRetired { .. }
                }
        )
    }

    /// A grant retried with a different `preferredAccessKey` than its first
    /// successful claim — a client bug, not a transient race.
    pub fn is_access_key_immutable(&self) -> bool {
        matches!(
            self,
            Self::Ownership {
                source: OwnershipError::AccessKeyImmutable { .. }
            }
        )
    }
}

fn admin_err(err: rustfs_admin::RustfsClientError) -> GrantError {
    GrantError::Admin {
        msg: err.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationAction {
    /// No RustFS user exists yet under this access key.
    AddUser,
    /// A RustFS user exists but the credential Secret was just (re)created —
    /// the driver no longer knows the password RustFS has, so it must be
    /// reset atomically (RustFS's `add-user` is an upsert).
    RotatePassword,
    /// A RustFS user exists and the credential Secret's password is the one
    /// already known to be in effect — only (re)attach policies.
    AttachPoliciesOnly,
}

/// Decide whether `add_user` must be (re)issued. Pure so it can be
/// exhaustively table-tested without a live RustFS/kube client.
pub fn resolve_password_rotation(
    user_exists: bool,
    freshly_created_secret: bool,
) -> RotationAction {
    match (user_exists, freshly_created_secret) {
        (false, _) => RotationAction::AddUser,
        (true, true) => RotationAction::RotatePassword,
        (true, false) => RotationAction::AttachPoliciesOnly,
    }
}

#[derive(Debug, Clone)]
pub struct GrantResult {
    pub account_id: String,
    #[allow(dead_code)]
    pub secret_key: String,
    pub secrets: HashMap<String, String>,
}

pub fn state_namespace(kube: &Client) -> String {
    std::env::var("POD_NAMESPACE")
        .or_else(|_| std::env::var("COSI_STATE_NAMESPACE"))
        .unwrap_or_else(|_| kube.default_namespace().to_string())
}

fn credential_map(
    access_key: &str,
    secret_key: &str,
    params: &BackendParameters,
    policy_buckets: &[String],
) -> HashMap<String, String> {
    let mut secrets = HashMap::new();
    secrets.insert("accessKeyID".to_string(), access_key.to_string());
    secrets.insert("accessSecretKey".to_string(), secret_key.to_string());
    secrets.insert("AWS_ACCESS_KEY_ID".to_string(), access_key.to_string());
    secrets.insert("AWS_SECRET_ACCESS_KEY".to_string(), secret_key.to_string());
    secrets.insert("accesskey".to_string(), access_key.to_string());
    secrets.insert("secretkey".to_string(), secret_key.to_string());
    secrets.insert("endpoint".to_string(), params.endpoint.clone());
    secrets.insert("region".to_string(), params.region.clone());
    secrets.insert(
        "BUCKETS".to_string(),
        params
            .buckets
            .clone()
            .unwrap_or_else(|| policy_buckets.join(",")),
    );
    secrets
}

/// Resolve the policy name attached to this grant.
///
/// - Generated policies are unique per grant (`cosi-pol-{grant}`).
/// - External BAC `policy` names are referenced only (never replaced).
pub fn resolve_policy_name(params: &BackendParameters, grant_name: &str) -> (String, bool) {
    match params.policy.as_deref() {
        Some(external) => (external.to_string(), true),
        None => (grant_policy_name(grant_name), false),
    }
}

/// Whether the driver should call `add_canned_policy` for this grant.
///
/// External BAC policy names are validate-only — never overwritten.
pub fn should_write_canned_policy(external_policy: bool) -> bool {
    !external_policy
}

async fn attach_policies(
    client: &RustfsAdminClient,
    access_key: &str,
    grant_name: &str,
    params: &BackendParameters,
    policy_buckets: &[String],
) -> Result<(), GrantError> {
    let (policy_name, external) = resolve_policy_name(params, grant_name);
    if external {
        // Validate-only: never add_canned_policy / replace.
        client
            .get_canned_policy(&policy_name)
            .await
            .map_err(|err| {
                if err.is_not_found() {
                    GrantError::MissingExternalPolicy {
                        policy: policy_name.clone(),
                    }
                } else {
                    admin_err(err)
                }
            })?;
    } else {
        debug_assert!(should_write_canned_policy(false));
        let doc = bucket_policy_document_for(policy_buckets);
        client
            .add_canned_policy(&policy_name, &doc)
            .await
            .map_err(admin_err)?;
    }

    client
        .set_user_policy(access_key, &[policy_name])
        .await
        .map_err(admin_err)?;
    Ok(())
}

/// Provision or resume a grant with durable credentials and atomic CAS
/// ownership.
///
/// Flow: claim `{backend_id, account_id}` (atomic, race-free even under
/// retries) → durable Secret (create-once) → add_user/rotate + policies →
/// Ready. Orphan RustFS users (no matching claim) are not adopted.
pub async fn grant_bucket_access(
    kube: &Client,
    client: &RustfsAdminClient,
    params: &BackendParameters,
    grant_name: &str,
    bucket_id: &str,
) -> Result<GrantResult, GrantError> {
    let namespace = state_namespace(kube);
    let backend_id = parameters::backend_id(params);
    let access_key = params
        .preferred_access_key
        .clone()
        .unwrap_or_else(|| grant_name.to_string());
    let policy_buckets = params.buckets_for_policy(bucket_id);
    let ak_hash = access_key_hash(&access_key);
    let cred_name = credential_secret_name(grant_name);
    let store = OwnershipStore::new(kube.clone(), namespace.clone());

    let (claim, freshly_claimed) = store
        .claim_account(&backend_id, grant_name, &access_key, &ak_hash, &cred_name)
        .await?;

    let user_info = client.get_user_info(&access_key).await.map_err(admin_err)?;
    if freshly_claimed && user_info.is_some() {
        // Nobody (including this driver, on a previous attempt) had ever
        // claimed this account before now, yet RustFS already has a user
        // under this access key — it was created outside this driver's
        // tracking. Refuse to silently take it over.
        return Err(GrantError::OrphanUserConflict {
            account_id: access_key,
            grant_name: grant_name.to_string(),
        });
    }

    let creds = load_or_create_credentials(kube, &namespace, grant_name, &claim.account_id).await?;

    info!(
        grant = %grant_name,
        account = %creds.access_key,
        state = ?claim.state,
        buckets = %policy_buckets.join(","),
        "granting bucket access"
    );

    match resolve_password_rotation(user_info.is_some(), creds.freshly_created) {
        RotationAction::AddUser | RotationAction::RotatePassword => {
            client
                .add_user(&creds.access_key, &creds.secret_key)
                .await
                .map_err(admin_err)?;
        }
        RotationAction::AttachPoliciesOnly => {}
    }
    attach_policies(
        client,
        &creds.access_key,
        grant_name,
        params,
        &policy_buckets,
    )
    .await?;

    store
        .mark_ready(&backend_id, grant_name, &claim.account_id)
        .await?;

    Ok(GrantResult {
        account_id: creds.access_key.clone(),
        secret_key: creds.secret_key.clone(),
        secrets: credential_map(
            &creds.access_key,
            &creds.secret_key,
            params,
            &policy_buckets,
        ),
    })
}

pub async fn revoke_bucket_access(
    kube: &Client,
    client: &RustfsAdminClient,
    params: &BackendParameters,
    account_id: &str,
) -> Result<(), GrantError> {
    let namespace = state_namespace(kube);
    let backend_id = parameters::backend_id(params);
    let store = OwnershipStore::new(kube.clone(), namespace);

    store.revoke(&backend_id, account_id).await?;

    client.remove_user(account_id).await.map_err(admin_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        RotationAction, resolve_password_rotation, resolve_policy_name, should_write_canned_policy,
    };
    use crate::parameters::BackendParameters;
    use std::collections::HashMap;

    fn params_with_policy(policy: Option<&str>) -> BackendParameters {
        let mut map = HashMap::new();
        map.insert("endpoint".into(), "http://rustfs".into());
        map.insert("objectStoreUserSecretName".into(), "s".into());
        map.insert("objectStoreUserSecretNamespace".into(), "ns".into());
        if let Some(p) = policy {
            map.insert("policy".into(), p.into());
        }
        BackendParameters::from_map(&map).unwrap()
    }

    #[test]
    fn generated_policy_is_unique_per_grant() {
        let params = params_with_policy(None);
        let (name, external) = resolve_policy_name(&params, "ba-1");
        assert!(!external);
        assert_eq!(name, "cosi-pol-ba-1");
        let (other, _) = resolve_policy_name(&params, "ba-2");
        assert_ne!(name, other);
    }

    #[test]
    fn external_policy_is_reference_only() {
        let params = params_with_policy(Some("shared-readonly"));
        let (name, external) = resolve_policy_name(&params, "ba-1");
        assert!(external);
        assert_eq!(name, "shared-readonly");
    }

    #[test]
    fn external_policy_is_never_written() {
        let params = params_with_policy(Some("shared-readonly"));
        let (_, external) = resolve_policy_name(&params, "ba-1");
        assert!(!should_write_canned_policy(external));
        assert!(should_write_canned_policy(false));
    }

    #[test]
    fn retry_reuses_stable_credential_secret_name() {
        use crate::credentials::credential_secret_name;
        assert_eq!(
            credential_secret_name("ba-1"),
            credential_secret_name("ba-1")
        );
        assert_ne!(
            credential_secret_name("ba-1"),
            credential_secret_name("ba-2")
        );
    }

    /// Regression test: retry when the internal Secret is missing but the
    /// RustFS user still exists must rotate the password, never silently
    /// return credentials RustFS never learned.
    #[test]
    fn missing_secret_with_existing_user_forces_password_rotation() {
        assert_eq!(
            resolve_password_rotation(true, true),
            RotationAction::RotatePassword
        );
    }

    #[test]
    fn password_rotation_matrix() {
        assert_eq!(
            resolve_password_rotation(false, false),
            RotationAction::AddUser
        );
        assert_eq!(
            resolve_password_rotation(false, true),
            RotationAction::AddUser
        );
        assert_eq!(
            resolve_password_rotation(true, false),
            RotationAction::AttachPoliciesOnly
        );
        assert_eq!(
            resolve_password_rotation(true, true),
            RotationAction::RotatePassword
        );
    }
}
