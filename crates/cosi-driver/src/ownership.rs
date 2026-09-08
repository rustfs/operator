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

//! Crash-safe, atomic grant ownership (PendingCreate → Ready → Revoked).
//!
//! Every `{backend_id, account_id}` claim is its own Kubernetes ConfigMap,
//! created with `Api::create()` — a single atomic operation on the object's
//! name, not a read-then-write race on a shared blob. Existence of the
//! object under its deterministic name *is* the uniqueness proof, so the
//! invariant is checked on every retry (including CAS 409s), not just once
//! up front. A companion per-grant "grant record" ConfigMap lets
//! `grant_bucket_access` resume the same COSI request idempotently without
//! needing `account_id` as an input.
//!
//! Revoked claims are tombstoned, never deleted: once `{backend_id,
//! account_id}` has been revoked it can never be claimed again. This makes a
//! delayed/retried revoke for an old grant provably unable to affect a newer
//! grant that reused the same preferred access key, because the newer grant
//! could never have acquired the key in the first place.

use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::Client;
use kube::api::{Api, PostParams};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::{ResultExt, Snafu};
use tracing::info;

const PROOF_KEY: &str = "proof";
const CAS_RETRIES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaimState {
    PendingCreate,
    Ready,
    Revoked,
}

/// Atomic `{backend_id, account_id}` uniqueness claim. Its existence under
/// [`account_claim_name`] *is* the ownership proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountClaim {
    pub grant_name: String,
    pub backend_id: String,
    pub account_id: String,
    pub access_key_hash: String,
    pub cred_secret_name: String,
    pub state: ClaimState,
}

/// `grant_name -> account_id` pointer, enabling idempotent resume of a COSI
/// `DriverGrantBucketAccess` retry without trusting the caller-supplied
/// `preferredAccessKey` parameter on that retry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrantRecord {
    pub grant_name: String,
    pub backend_id: String,
    pub account_id: String,
    pub cred_secret_name: String,
    pub state: ClaimState,
}

#[derive(Debug, Snafu)]
pub enum OwnershipError {
    #[snafu(display("kubernetes error: {source}"))]
    Kube { source: kube::Error },
    #[snafu(display("invalid ownership proof for grant `{grant}`: {detail}"))]
    InvalidProof { grant: String, detail: String },
    #[snafu(display(
        "ownership conflict: access key `{account_id}` is claimed by grant `{owner}`, \
         not `{requester}`"
    ))]
    AccountConflict {
        account_id: String,
        owner: String,
        requester: String,
    },
    #[snafu(display(
        "access key `{account_id}` was previously revoked on backend `{backend_id}` and can \
         never be reused; choose a different preferredAccessKey"
    ))]
    AccountRetired {
        account_id: String,
        backend_id: String,
    },
    #[snafu(display(
        "grant `{grant_name}` is already bound to access key `{existing}`; access keys are \
         immutable once claimed (requested `{requested}`)"
    ))]
    AccessKeyImmutable {
        grant_name: String,
        existing: String,
        requested: String,
    },
    #[snafu(display("CAS conflict writing ownership object `{name}` (retry)"))]
    CasConflict { name: String },
}

fn account_claim_name(backend_id: &str, account_id: &str) -> String {
    let digest = hex::encode(Sha256::digest(account_id.as_bytes()));
    format!("cosi-acct-{backend_id}-{}", &digest[..24])
}

fn grant_record_name(grant_name: &str) -> String {
    format!(
        "cosi-grant-{}",
        crate::parameters::sanitize_policy_fragment(grant_name)
    )
}

#[derive(Clone)]
pub struct OwnershipStore {
    api: Api<ConfigMap>,
}

impl OwnershipStore {
    pub fn new(client: Client, namespace: String) -> Self {
        Self {
            api: Api::namespaced(client, &namespace),
        }
    }

    async fn get_object<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>, OwnershipError> {
        let cm = match self.api.get(name).await {
            Ok(cm) => cm,
            Err(kube::Error::Api(err)) if err.code == 404 => return Ok(None),
            Err(source) => return Err(source).context(KubeSnafu),
        };
        let raw = cm
            .data
            .as_ref()
            .and_then(|data| data.get(PROOF_KEY))
            .ok_or_else(|| OwnershipError::InvalidProof {
                grant: name.to_string(),
                detail: "missing proof data".into(),
            })?;
        let value: T = serde_json::from_str(raw).map_err(|err| OwnershipError::InvalidProof {
            grant: name.to_string(),
            detail: err.to_string(),
        })?;
        Ok(Some(value))
    }

    fn build_cm<T: Serialize>(name: &str, value: &T) -> Result<ConfigMap, OwnershipError> {
        let raw = serde_json::to_string(value).map_err(|err| OwnershipError::InvalidProof {
            grant: name.to_string(),
            detail: err.to_string(),
        })?;
        Ok(ConfigMap {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                labels: Some(
                    [(
                        "app.kubernetes.io/name".to_string(),
                        "rustfs-cosi-driver".to_string(),
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..Default::default()
            },
            data: Some([(PROOF_KEY.to_string(), raw)].into_iter().collect()),
            ..Default::default()
        })
    }

    /// Atomically create `name` holding `value`. Returns `Ok(true)` if this
    /// call won the create race, `Ok(false)` if the object already existed
    /// (caller must `get_object` to see who won).
    async fn try_create<T: Serialize>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<bool, OwnershipError> {
        let cm = Self::build_cm(name, value)?;
        match self.api.create(&PostParams::default(), &cm).await {
            Ok(_) => Ok(true),
            Err(kube::Error::Api(err)) if err.code == 409 => Ok(false),
            Err(source) => Err(source).context(KubeSnafu),
        }
    }

    /// Replace `name`'s contents with `value`, retrying on resourceVersion
    /// conflicts. `name` must already exist.
    async fn replace_object<T: Serialize>(
        &self,
        name: &str,
        value: &T,
    ) -> Result<(), OwnershipError> {
        let raw = serde_json::to_string(value).map_err(|err| OwnershipError::InvalidProof {
            grant: name.to_string(),
            detail: err.to_string(),
        })?;

        for _ in 0..CAS_RETRIES {
            let mut cm = self.api.get(name).await.context(KubeSnafu)?;
            let mut data = cm.data.take().unwrap_or_default();
            data.insert(PROOF_KEY.to_string(), raw.clone());
            cm.data = Some(data);
            match self.api.replace(name, &PostParams::default(), &cm).await {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(err)) if err.code == 409 => continue,
                Err(source) => return Err(source).context(KubeSnafu),
            }
        }
        Err(OwnershipError::CasConflict {
            name: name.to_string(),
        })
    }

    pub async fn get_grant_record(
        &self,
        grant_name: &str,
    ) -> Result<Option<GrantRecord>, OwnershipError> {
        self.get_object(&grant_record_name(grant_name)).await
    }

    async fn get_account_claim(
        &self,
        backend_id: &str,
        account_id: &str,
    ) -> Result<Option<AccountClaim>, OwnershipError> {
        self.get_object(&account_claim_name(backend_id, account_id))
            .await
    }

    /// Atomically claim `{backend_id, account_id}` for `grant_name`, or
    /// resume an in-flight/completed claim for the same grant. Conflicts
    /// (a different grant owns the key, or the key was revoked) are
    /// evaluated on every call, including CAS retries, since the claim
    /// object's existence under a deterministic name is itself the
    /// uniqueness check.
    ///
    /// `freshly_claimed` is true only when this call is the very first ever
    /// to claim `{backend_id, account_id}` for `grant_name` — i.e. neither a
    /// resumed grant record nor an adopted claim from a prior crashed
    /// attempt of this same grant. Callers use this to detect a RustFS user
    /// that already exists under the access key without this driver ever
    /// having claimed it (an orphan/foreign user).
    pub async fn claim_account(
        &self,
        backend_id: &str,
        grant_name: &str,
        account_id: &str,
        access_key_hash: &str,
        cred_secret_name: &str,
    ) -> Result<(AccountClaim, bool), OwnershipError> {
        if let Some(record) = self.get_grant_record(grant_name).await? {
            logic::check_access_key_immutable(grant_name, account_id, &record)?;
            let claim = self
                .get_account_claim(backend_id, account_id)
                .await?
                .ok_or_else(|| OwnershipError::InvalidProof {
                    grant: grant_name.to_string(),
                    detail: "grant record exists but account claim is missing".into(),
                })?;
            logic::check_resumable(account_id, backend_id, &claim)?;
            return Ok((claim, false));
        }

        let claim_name = account_claim_name(backend_id, account_id);
        let fresh = AccountClaim {
            grant_name: grant_name.to_string(),
            backend_id: backend_id.to_string(),
            account_id: account_id.to_string(),
            access_key_hash: access_key_hash.to_string(),
            cred_secret_name: cred_secret_name.to_string(),
            state: ClaimState::PendingCreate,
        };

        let (claim, freshly_claimed) = if self.try_create(&claim_name, &fresh).await? {
            info!(grant = %grant_name, account = %account_id, "claimed account ownership");
            (fresh, true)
        } else {
            let existing = self
                .get_account_claim(backend_id, account_id)
                .await?
                .ok_or_else(|| OwnershipError::InvalidProof {
                    grant: grant_name.to_string(),
                    detail: "account claim create conflicted but object is now missing".into(),
                })?;
            // Crash resume: we created this claim on a previous attempt but
            // died before the grant record was written below.
            logic::resolve_claim_conflict(grant_name, account_id, backend_id, &existing)?;
            (existing, false)
        };

        let record = GrantRecord {
            grant_name: grant_name.to_string(),
            backend_id: backend_id.to_string(),
            account_id: account_id.to_string(),
            cred_secret_name: cred_secret_name.to_string(),
            state: claim.state,
        };
        if !self
            .try_create(&grant_record_name(grant_name), &record)
            .await?
        {
            let existing = self.get_grant_record(grant_name).await?.ok_or_else(|| {
                OwnershipError::InvalidProof {
                    grant: grant_name.to_string(),
                    detail: "grant record create conflicted but object is now missing".into(),
                }
            })?;
            if existing.account_id != account_id {
                return Err(OwnershipError::InvalidProof {
                    grant: grant_name.to_string(),
                    detail: "grant record race resolved to a different account_id".into(),
                });
            }
        }

        Ok((claim, freshly_claimed))
    }

    pub async fn mark_ready(
        &self,
        backend_id: &str,
        grant_name: &str,
        account_id: &str,
    ) -> Result<(), OwnershipError> {
        let claim_name = account_claim_name(backend_id, account_id);
        if let Some(mut claim) = self.get_account_claim(backend_id, account_id).await? {
            if claim.state != ClaimState::Ready {
                claim.state = ClaimState::Ready;
                self.replace_object(&claim_name, &claim).await?;
            }
        } else {
            return Err(OwnershipError::InvalidProof {
                grant: grant_name.to_string(),
                detail: "missing account claim when promoting to Ready".into(),
            });
        }

        let record_name = grant_record_name(grant_name);
        if let Some(mut record) = self.get_grant_record(grant_name).await?
            && record.state != ClaimState::Ready
        {
            record.state = ClaimState::Ready;
            self.replace_object(&record_name, &record).await?;
        }
        info!(grant = %grant_name, account = %account_id, "promoted grant ownership to Ready");
        Ok(())
    }

    /// Tombstone the claim for `{backend_id, account_id}` if present. A
    /// claim already in `Revoked` state is left untouched (idempotent no-op
    /// for a delayed/duplicate revoke). Returns the grant_name that owned
    /// the claim, if any.
    pub async fn revoke(
        &self,
        backend_id: &str,
        account_id: &str,
    ) -> Result<Option<String>, OwnershipError> {
        let claim_name = account_claim_name(backend_id, account_id);
        let Some(mut claim) = self.get_account_claim(backend_id, account_id).await? else {
            return Ok(None);
        };
        if claim.state == ClaimState::Revoked {
            return Ok(Some(claim.grant_name));
        }

        let grant_name = claim.grant_name.clone();
        claim.state = ClaimState::Revoked;
        self.replace_object(&claim_name, &claim).await?;
        info!(grant = %grant_name, account = %account_id, "revoked (tombstoned) account claim");
        Ok(Some(grant_name))
    }
}

/// Pure decision logic for unit tests (no kube client).
pub mod logic {
    use super::{AccountClaim, ClaimState, GrantRecord, OwnershipError};

    /// A grant's access key is fixed at first successful claim; a retry
    /// carrying a different `preferredAccessKey` is a hard conflict rather
    /// than a silent rebind.
    pub fn check_access_key_immutable(
        grant_name: &str,
        requested_account_id: &str,
        record: &GrantRecord,
    ) -> Result<(), OwnershipError> {
        if record.account_id != requested_account_id {
            return Err(OwnershipError::AccessKeyImmutable {
                grant_name: grant_name.to_string(),
                existing: record.account_id.clone(),
                requested: requested_account_id.to_string(),
            });
        }
        Ok(())
    }

    /// A grant resuming its own claim must not resurrect one that was
    /// already revoked (e.g. a stale retry arriving after `revoke_bucket_access`
    /// tore this grant's access down).
    pub fn check_resumable(
        account_id: &str,
        backend_id: &str,
        claim: &AccountClaim,
    ) -> Result<(), OwnershipError> {
        if claim.state == ClaimState::Revoked {
            return Err(OwnershipError::AccountRetired {
                account_id: account_id.to_string(),
                backend_id: backend_id.to_string(),
            });
        }
        Ok(())
    }

    /// Decide whether the caller may adopt/resume an `AccountClaim` object
    /// found after losing an atomic-create race for the same name. This is
    /// evaluated on *every* claim attempt (including CAS retries), which is
    /// what makes the uniqueness invariant race-free: two concurrent claims
    /// for the same `account_id` can never both pass.
    pub fn resolve_claim_conflict(
        grant_name: &str,
        account_id: &str,
        backend_id: &str,
        existing: &AccountClaim,
    ) -> Result<(), OwnershipError> {
        if existing.grant_name != grant_name {
            return Err(OwnershipError::AccountConflict {
                account_id: account_id.to_string(),
                owner: existing.grant_name.clone(),
                requester: grant_name.to_string(),
            });
        }
        if existing.state == ClaimState::Revoked {
            return Err(OwnershipError::AccountRetired {
                account_id: account_id.to_string(),
                backend_id: backend_id.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(grant_name: &str, state: ClaimState) -> AccountClaim {
        AccountClaim {
            grant_name: grant_name.into(),
            backend_id: "be-abc".into(),
            account_id: "mlflow".into(),
            access_key_hash: "h".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state,
        }
    }

    #[test]
    fn two_grants_racing_for_the_same_account_id_conflict() {
        // ba-1 wins the create race; ba-2's retry must see the conflict,
        // regardless of who asked first.
        let winner = claim("ba-1", ClaimState::PendingCreate);
        let err = logic::resolve_claim_conflict("ba-2", "mlflow", "be-abc", &winner).unwrap_err();
        assert!(matches!(err, OwnershipError::AccountConflict { .. }));
    }

    #[test]
    fn same_grant_may_adopt_its_own_pending_claim() {
        let mine = claim("ba-1", ClaimState::PendingCreate);
        assert!(logic::resolve_claim_conflict("ba-1", "mlflow", "be-abc", &mine).is_ok());
    }

    #[test]
    fn revoked_account_id_can_never_be_reclaimed() {
        // Simulates: A claimed+revoked "mlflow"; B now races to claim the
        // same key and loses to A's tombstone — reported as an ownership
        // conflict (a different grant's name is on the tombstoned claim),
        // which is just as final as `AccountRetired`: B can never win it.
        let tombstone = claim("ba-1", ClaimState::Revoked);
        let err =
            logic::resolve_claim_conflict("ba-2", "mlflow", "be-abc", &tombstone).unwrap_err();
        assert!(matches!(err, OwnershipError::AccountConflict { .. }));
    }

    #[test]
    fn resuming_a_revoked_grant_is_rejected() {
        // A delayed retry of a grant request must not resurrect access that
        // was already revoked by a prior DriverRevokeBucketAccess call.
        let tombstone = claim("ba-1", ClaimState::Revoked);
        let err = logic::check_resumable("mlflow", "be-abc", &tombstone).unwrap_err();
        assert!(matches!(err, OwnershipError::AccountRetired { .. }));
    }

    #[test]
    fn changing_preferred_access_key_on_retry_is_rejected() {
        let record = GrantRecord {
            grant_name: "ba-1".into(),
            backend_id: "be-abc".into(),
            account_id: "mlflow".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state: ClaimState::Ready,
        };
        assert!(logic::check_access_key_immutable("ba-1", "mlflow", &record).is_ok());
        let err = logic::check_access_key_immutable("ba-1", "other", &record).unwrap_err();
        assert!(matches!(err, OwnershipError::AccessKeyImmutable { .. }));
    }

    #[test]
    fn claim_roundtrip() {
        let claim = AccountClaim {
            grant_name: "ba-1".into(),
            backend_id: "be-abc".into(),
            account_id: "mlflow".into(),
            access_key_hash: "abc".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state: ClaimState::PendingCreate,
        };
        let raw = serde_json::to_string(&claim).unwrap();
        let back: AccountClaim = serde_json::from_str(&raw).unwrap();
        assert_eq!(claim, back);
    }

    #[test]
    fn claim_names_are_deterministic_and_backend_scoped() {
        let a = account_claim_name("be-1", "mlflow");
        let b = account_claim_name("be-1", "mlflow");
        let c = account_claim_name("be-2", "mlflow");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn grant_record_names_are_stable() {
        assert_eq!(grant_record_name("ba-1"), grant_record_name("ba-1"));
        assert_ne!(grant_record_name("ba-1"), grant_record_name("ba-2"));
    }
}
