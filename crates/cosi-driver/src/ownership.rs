//! Crash-safe grant ownership checkpoints (PendingCreate → Ready).
//!
//! Mirrors the Tenant user CAS pattern: persist PendingCreate before mutating
//! RustFS, then promote to Ready after success. Concurrent claims on the same
//! preferred access key conflict instead of silently adopting.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::Client;
use kube::api::{Api, PostParams};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

const OWNERSHIP_CM_NAME: &str = "rustfs-cosi-ownership";
const PROOF_PREFIX: &str = "grant.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GrantOwnershipState {
    PendingCreate,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrantOwnershipProof {
    pub grant_name: String,
    pub account_id: String,
    pub access_key_hash: String,
    pub cred_secret_name: String,
    pub state: GrantOwnershipState,
}

#[derive(Debug, Error)]
pub enum OwnershipError {
    #[error("kubernetes error: {0}")]
    Kube(#[from] kube::Error),
    #[error("invalid ownership proof for grant `{grant}`: {detail}")]
    InvalidProof { grant: String, detail: String },
    #[error(
        "ownership conflict: access key `{account_id}` is claimed by grant `{owner}`, \
         not `{requester}`"
    )]
    AccountConflict {
        account_id: String,
        owner: String,
        requester: String,
    },
    #[error("CAS conflict writing ownership for grant `{0}` (retry)")]
    CasConflict(String),
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

    fn proof_key(grant_name: &str) -> String {
        format!("{PROOF_PREFIX}{grant_name}")
    }

    pub async fn get(
        &self,
        grant_name: &str,
    ) -> Result<Option<GrantOwnershipProof>, OwnershipError> {
        let Some(cm) = self.get_cm().await? else {
            return Ok(None);
        };
        let Some(data) = cm.data.as_ref() else {
            return Ok(None);
        };
        let Some(raw) = data.get(&Self::proof_key(grant_name)) else {
            return Ok(None);
        };
        let proof: GrantOwnershipProof =
            serde_json::from_str(raw).map_err(|err| OwnershipError::InvalidProof {
                grant: grant_name.to_string(),
                detail: err.to_string(),
            })?;
        Ok(Some(proof))
    }

    pub async fn find_by_account_id(
        &self,
        account_id: &str,
    ) -> Result<Option<String>, OwnershipError> {
        let Some(cm) = self.get_cm().await? else {
            return Ok(None);
        };
        let Some(data) = cm.data.as_ref() else {
            return Ok(None);
        };
        for (key, raw) in data {
            if !key.starts_with(PROOF_PREFIX) {
                continue;
            }
            let proof: GrantOwnershipProof = match serde_json::from_str(raw) {
                Ok(p) => p,
                Err(err) => {
                    warn!(key = %key, error = %err, "skipping corrupt ownership proof");
                    continue;
                }
            };
            if proof.account_id == account_id {
                return Ok(Some(proof.grant_name));
            }
        }
        Ok(None)
    }

    /// CAS: create PendingCreate if absent, or resume existing proof for this grant.
    /// Conflicts if another grant already owns the same account_id.
    pub async fn begin_or_resume(
        &self,
        grant_name: &str,
        account_id: &str,
        access_key_hash: &str,
        cred_secret_name: &str,
    ) -> Result<GrantOwnershipProof, OwnershipError> {
        if let Some(owner) = self.find_by_account_id(account_id).await?
            && owner != grant_name
        {
            return Err(OwnershipError::AccountConflict {
                account_id: account_id.to_string(),
                owner,
                requester: grant_name.to_string(),
            });
        }

        if let Some(existing) = self.get(grant_name).await? {
            if existing.account_id != account_id {
                return Err(OwnershipError::AccountConflict {
                    account_id: account_id.to_string(),
                    owner: existing.grant_name,
                    requester: grant_name.to_string(),
                });
            }
            return Ok(existing);
        }

        let proof = GrantOwnershipProof {
            grant_name: grant_name.to_string(),
            account_id: account_id.to_string(),
            access_key_hash: access_key_hash.to_string(),
            cred_secret_name: cred_secret_name.to_string(),
            state: GrantOwnershipState::PendingCreate,
        };
        self.cas_put(&proof).await?;
        info!(
            grant = %grant_name,
            account = %account_id,
            "recorded PendingCreate ownership checkpoint"
        );
        Ok(proof)
    }

    pub async fn mark_ready(&self, grant_name: &str) -> Result<(), OwnershipError> {
        let Some(mut proof) = self.get(grant_name).await? else {
            return Err(OwnershipError::InvalidProof {
                grant: grant_name.to_string(),
                detail: "missing proof when promoting to Ready".into(),
            });
        };
        proof.state = GrantOwnershipState::Ready;
        self.cas_put(&proof).await?;
        info!(grant = %grant_name, "promoted grant ownership to Ready");
        Ok(())
    }

    pub async fn remove(&self, grant_name: &str) -> Result<(), OwnershipError> {
        let key = Self::proof_key(grant_name);
        for _ in 0..8 {
            let Some(mut cm) = self.get_cm().await? else {
                return Ok(());
            };
            let rv = cm.metadata.resource_version.clone();
            let mut data = cm.data.take().unwrap_or_default();
            if data.remove(&key).is_none() {
                return Ok(());
            }
            cm.data = Some(data);
            cm.metadata.resource_version = rv;
            match self
                .api
                .replace(OWNERSHIP_CM_NAME, &PostParams::default(), &cm)
                .await
            {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(err)) if err.code == 409 => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(OwnershipError::CasConflict(grant_name.to_string()))
    }

    async fn get_cm(&self) -> Result<Option<ConfigMap>, OwnershipError> {
        match self.api.get(OWNERSHIP_CM_NAME).await {
            Ok(cm) => Ok(Some(cm)),
            Err(kube::Error::Api(err)) if err.code == 404 => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn ensure_cm(&self) -> Result<ConfigMap, OwnershipError> {
        if let Some(cm) = self.get_cm().await? {
            return Ok(cm);
        }
        let cm = ConfigMap {
            metadata: ObjectMeta {
                name: Some(OWNERSHIP_CM_NAME.to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::new()),
            ..Default::default()
        };
        match self.api.create(&PostParams::default(), &cm).await {
            Ok(created) => Ok(created),
            Err(kube::Error::Api(err)) if err.code == 409 => self
                .get_cm()
                .await?
                .ok_or_else(|| OwnershipError::CasConflict(OWNERSHIP_CM_NAME.to_string())),
            Err(err) => Err(err.into()),
        }
    }

    async fn cas_put(&self, proof: &GrantOwnershipProof) -> Result<(), OwnershipError> {
        let key = Self::proof_key(&proof.grant_name);
        let value = serde_json::to_string(proof).map_err(|err| OwnershipError::InvalidProof {
            grant: proof.grant_name.clone(),
            detail: err.to_string(),
        })?;

        for _ in 0..8 {
            let mut cm = self.ensure_cm().await?;
            let rv = cm.metadata.resource_version.clone();
            let mut data = cm.data.take().unwrap_or_default();
            data.insert(key.clone(), value.clone());
            cm.data = Some(data);
            cm.metadata.resource_version = rv;
            match self
                .api
                .replace(OWNERSHIP_CM_NAME, &PostParams::default(), &cm)
                .await
            {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(err)) if err.code == 409 => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(OwnershipError::CasConflict(proof.grant_name.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_roundtrip() {
        let proof = GrantOwnershipProof {
            grant_name: "ba-1".into(),
            account_id: "mlflow".into(),
            access_key_hash: "abc".into(),
            cred_secret_name: "cosi-cred-ba-1".into(),
            state: GrantOwnershipState::PendingCreate,
        };
        let raw = serde_json::to_string(&proof).unwrap();
        let back: GrantOwnershipProof = serde_json::from_str(&raw).unwrap();
        assert_eq!(proof, back);
    }
}
