//! Grant provisioning state machine (durable secrets + CAS ownership).

use std::collections::HashMap;

use kube::Client;
use rustfs_admin::RustfsAdminClient;
use thiserror::Error;
use tracing::info;

use crate::credentials::{
    CredentialStoreError, StoredCredentials, access_key_hash, credential_secret_name,
    load_or_create_credentials,
};
use crate::ownership::{GrantOwnershipState, OwnershipError, OwnershipStore};
use crate::parameters::{BackendParameters, bucket_policy_document_for, grant_policy_name};

#[derive(Debug, Error)]
pub enum GrantError {
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    #[error(transparent)]
    Credentials(#[from] CredentialStoreError),
    #[error("rustfs admin error: {0}")]
    Admin(String),
    #[error(
        "preferredAccessKey `{account_id}` is already bound to another BucketAccess; \
         omit preferredAccessKey or choose a unique value"
    )]
    AccountConflict { account_id: String },
    #[error(
        "access key `{account_id}` exists in RustFS without a matching Ready ownership \
         proof for grant `{grant_name}`"
    )]
    OrphanUserConflict {
        account_id: String,
        grant_name: String,
    },
    #[error("external policy `{0}` does not exist")]
    MissingExternalPolicy(String),
}

impl GrantError {
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::AccountConflict { .. }
                | Self::OrphanUserConflict { .. }
                | Self::Ownership(OwnershipError::AccountConflict { .. })
        )
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
    secrets.insert("ACCESSKEY".to_string(), access_key.to_string());
    secrets.insert("SECRETKEY".to_string(), secret_key.to_string());
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
                let msg = err.to_string();
                if msg.contains("not found") || msg.contains("NoSuch") {
                    GrantError::MissingExternalPolicy(policy_name.clone())
                } else {
                    GrantError::Admin(msg)
                }
            })?;
    } else {
        debug_assert!(should_write_canned_policy(false));
        let doc = bucket_policy_document_for(policy_buckets);
        client
            .add_canned_policy(&policy_name, &doc)
            .await
            .map_err(|err| GrantError::Admin(err.to_string()))?;
    }

    client
        .set_user_policy(access_key, &[policy_name])
        .await
        .map_err(|err| GrantError::Admin(err.to_string()))?;
    Ok(())
}

fn map_ownership_conflict(err: OwnershipError, account_id: &str) -> GrantError {
    match err {
        OwnershipError::AccountConflict { account_id, .. } => {
            GrantError::AccountConflict { account_id }
        }
        other => {
            let _ = account_id;
            GrantError::Ownership(other)
        }
    }
}

/// Provision or resume a grant with durable credentials and CAS ownership.
///
/// Flow: PendingCreate (CAS) → durable Secret → add_user + policies → Ready.
/// Orphan RustFS users (no matching proof) are not adopted.
pub async fn grant_bucket_access(
    kube: &Client,
    client: &RustfsAdminClient,
    params: &BackendParameters,
    grant_name: &str,
    bucket_id: &str,
) -> Result<GrantResult, GrantError> {
    let namespace = state_namespace(kube);
    let access_key = params
        .preferred_access_key
        .clone()
        .unwrap_or_else(|| grant_name.to_string());
    let policy_buckets = params.buckets_for_policy(bucket_id);
    let ak_hash = access_key_hash(&access_key);
    let cred_name = credential_secret_name(grant_name);
    let store = OwnershipStore::new(kube.clone(), namespace.clone());

    let existing_proof = store.get(grant_name).await?;
    let user_info = client
        .get_user_info(&access_key)
        .await
        .map_err(|err| GrantError::Admin(err.to_string()))?;

    // Refuse silent adopt of orphan / foreign RustFS users.
    if user_info.is_some() {
        match &existing_proof {
            Some(proof)
                if proof.account_id == access_key
                    && matches!(
                        proof.state,
                        GrantOwnershipState::PendingCreate | GrantOwnershipState::Ready
                    ) =>
            {
                // Crash resume or idempotent retry for this grant.
            }
            _ => {
                if let Some(owner) = store.find_by_account_id(&access_key).await? {
                    if owner != grant_name {
                        return Err(GrantError::AccountConflict {
                            account_id: access_key,
                        });
                    }
                } else {
                    return Err(GrantError::OrphanUserConflict {
                        account_id: access_key,
                        grant_name: grant_name.to_string(),
                    });
                }
            }
        }
    }

    let proof = store
        .begin_or_resume(grant_name, &access_key, &ak_hash, &cred_name)
        .await
        .map_err(|err| map_ownership_conflict(err, &access_key))?;

    let StoredCredentials {
        access_key,
        secret_key,
        secret_name: _,
    } = load_or_create_credentials(kube, &namespace, grant_name, &proof.account_id).await?;

    info!(
        grant = %grant_name,
        account = %access_key,
        state = ?proof.state,
        buckets = %policy_buckets.join(","),
        "granting bucket access"
    );

    match client
        .get_user_info(&access_key)
        .await
        .map_err(|err| GrantError::Admin(err.to_string()))?
    {
        Some(_) => {
            attach_policies(client, &access_key, grant_name, params, &policy_buckets).await?;
        }
        None => {
            client
                .add_user(&access_key, &secret_key)
                .await
                .map_err(|err| GrantError::Admin(err.to_string()))?;
            attach_policies(client, &access_key, grant_name, params, &policy_buckets).await?;
        }
    }

    if proof.state != GrantOwnershipState::Ready {
        store.mark_ready(grant_name).await?;
    }

    Ok(GrantResult {
        account_id: access_key.clone(),
        secret_key: secret_key.clone(),
        secrets: credential_map(&access_key, &secret_key, params, &policy_buckets),
    })
}

pub async fn revoke_bucket_access(
    kube: &Client,
    client: &RustfsAdminClient,
    account_id: &str,
) -> Result<(), GrantError> {
    let namespace = state_namespace(kube);
    let store = OwnershipStore::new(kube.clone(), namespace);

    if let Some(grant) = store.find_by_account_id(account_id).await? {
        store.remove(&grant).await?;
    }

    client
        .remove_user(account_id)
        .await
        .map_err(|err| GrantError::Admin(err.to_string()))?;
    Ok(())
}

/// Pure helpers for unit tests (no kube / RustFS).
#[cfg(test)]
pub mod logic {
    use super::*;

    /// Whether an existing RustFS user may be resumed for this grant.
    pub fn may_resume_existing_user(
        grant_name: &str,
        access_key: &str,
        proof: Option<&crate::ownership::GrantOwnershipProof>,
        account_owner: Option<&str>,
    ) -> Result<(), GrantError> {
        match proof {
            Some(p)
                if p.account_id == access_key
                    && matches!(
                        p.state,
                        GrantOwnershipState::PendingCreate | GrantOwnershipState::Ready
                    ) =>
            {
                Ok(())
            }
            _ => {
                if let Some(owner) = account_owner {
                    if owner != grant_name {
                        return Err(GrantError::AccountConflict {
                            account_id: access_key.to_string(),
                        });
                    }
                    Ok(())
                } else {
                    Err(GrantError::OrphanUserConflict {
                        account_id: access_key.to_string(),
                        grant_name: grant_name.to_string(),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::logic::may_resume_existing_user;
    use super::{resolve_policy_name, should_write_canned_policy};
    use crate::ownership::{GrantOwnershipProof, GrantOwnershipState};
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
    fn pending_create_resumes_after_partial_failure() {
        let proof = GrantOwnershipProof {
            grant_name: "ba-1".into(),
            account_id: "mlflow".into(),
            access_key_hash: "h".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state: GrantOwnershipState::PendingCreate,
        };
        assert!(may_resume_existing_user("ba-1", "mlflow", Some(&proof), Some("ba-1")).is_ok());
    }

    #[test]
    fn concurrent_preferred_key_conflicts() {
        let err = may_resume_existing_user("ba-2", "mlflow", None, Some("ba-1")).unwrap_err();
        assert!(matches!(err, super::GrantError::AccountConflict { .. }));
        assert!(err.is_conflict());
    }

    #[test]
    fn orphan_user_is_not_adopted() {
        let err = may_resume_existing_user("ba-1", "mlflow", None, None).unwrap_err();
        assert!(matches!(err, super::GrantError::OrphanUserConflict { .. }));
    }

    #[test]
    fn ready_proof_allows_idempotent_retry() {
        let proof = GrantOwnershipProof {
            grant_name: "ba-1".into(),
            account_id: "mlflow".into(),
            access_key_hash: "h".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state: GrantOwnershipState::Ready,
        };
        assert!(may_resume_existing_user("ba-1", "mlflow", Some(&proof), Some("ba-1")).is_ok());
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
}
