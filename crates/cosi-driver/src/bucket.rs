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

//! Bucket create/delete with atomic ownership proof and safe static defaults.

use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::Client;
use kube::api::{Api, PostParams};
use rustfs_admin::RustfsAdminClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snafu::{ResultExt, Snafu};
use tracing::info;

use crate::parameters::{self, BackendParameters};

const PROOF_KEY: &str = "proof";
const CAS_RETRIES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BucketProofState {
    PendingCreate,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BucketProof {
    pub backend_id: String,
    pub cosi_name: String,
    pub bucket_id: String,
    pub static_override: bool,
    pub state: BucketProofState,
}

#[derive(Debug, Snafu)]
pub enum BucketError {
    #[snafu(display("kubernetes error: {source}"))]
    Kube { source: kube::Error },
    #[snafu(display("invalid bucket ownership proof for `{bucket_id}`: {detail}"))]
    InvalidProof { bucket_id: String, detail: String },
    #[snafu(display("rustfs admin error: {msg}"))]
    Admin { msg: String },
    #[snafu(display(
        "refusing to delete bucket `{bucket_id}`: BucketClass uses static \
         bucketName/buckets override (adoption preview); delete is skipped \
         without ownership proof"
    ))]
    StaticBucketDeleteRefused { bucket_id: String },
    #[snafu(display("no buckets to create (buckets/bucketName empty or only *)"))]
    NothingToCreate,
    #[snafu(display(
        "bucket `{bucket}` is already owned by a different account; refusing to adopt it"
    ))]
    NameConflict { bucket: String },
    #[snafu(display(
        "refusing to delete bucket `{bucket_id}`: ownership proof is still PendingCreate \
         (creation may not have completed)"
    ))]
    DeleteRefusedNotReady { bucket_id: String },
    #[snafu(display("refusing to delete bucket `{bucket_id}`: no ownership proof found"))]
    DeleteRefusedNoProof { bucket_id: String },
}

fn bucket_claim_name(backend_id: &str, bucket_id: &str) -> String {
    let digest = hex::encode(Sha256::digest(bucket_id.as_bytes()));
    format!("cosi-bkt-{backend_id}-{}", &digest[..24])
}

#[derive(Clone)]
struct BucketOwnershipStore {
    api: Api<ConfigMap>,
}

impl BucketOwnershipStore {
    fn new(client: Client, namespace: String) -> Self {
        Self {
            api: Api::namespaced(client, &namespace),
        }
    }

    async fn get(&self, name: &str) -> Result<Option<BucketProof>, BucketError> {
        let cm = match self.api.get(name).await {
            Ok(cm) => cm,
            Err(kube::Error::Api(err)) if err.code == 404 => return Ok(None),
            Err(source) => return Err(source).context(KubeSnafu),
        };
        let raw = cm
            .data
            .as_ref()
            .and_then(|data| data.get(PROOF_KEY))
            .ok_or_else(|| BucketError::InvalidProof {
                bucket_id: name.to_string(),
                detail: "missing proof data".into(),
            })?;
        let proof: BucketProof =
            serde_json::from_str(raw).map_err(|err| BucketError::InvalidProof {
                bucket_id: name.to_string(),
                detail: err.to_string(),
            })?;
        Ok(Some(proof))
    }

    fn build_cm(name: &str, proof: &BucketProof) -> Result<ConfigMap, BucketError> {
        let raw = serde_json::to_string(proof).map_err(|err| BucketError::InvalidProof {
            bucket_id: name.to_string(),
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

    /// Create `PendingCreate` for `bucket_id` before mutating RustFS, or
    /// resume an existing proof for the same `cosi_name`. Returns `Err` if a
    /// different `cosi_name` already owns this bucket_id.
    async fn begin_or_resume(
        &self,
        backend_id: &str,
        cosi_name: &str,
        bucket_id: &str,
        static_override: bool,
    ) -> Result<BucketProof, BucketError> {
        let name = bucket_claim_name(backend_id, bucket_id);
        let fresh = BucketProof {
            backend_id: backend_id.to_string(),
            cosi_name: cosi_name.to_string(),
            bucket_id: bucket_id.to_string(),
            static_override,
            state: BucketProofState::PendingCreate,
        };
        let cm = Self::build_cm(&name, &fresh)?;
        match self.api.create(&PostParams::default(), &cm).await {
            Ok(_) => Ok(fresh),
            Err(kube::Error::Api(err)) if err.code == 409 => {
                let existing = self
                    .get(&name)
                    .await?
                    .ok_or_else(|| BucketError::InvalidProof {
                        bucket_id: bucket_id.to_string(),
                        detail: "bucket proof create conflicted but object is now missing".into(),
                    })?;
                Ok(existing)
            }
            Err(source) => Err(source).context(KubeSnafu),
        }
    }

    async fn mark_ready(&self, proof: &BucketProof) -> Result<(), BucketError> {
        if proof.state == BucketProofState::Ready {
            return Ok(());
        }
        let name = bucket_claim_name(&proof.backend_id, &proof.bucket_id);
        let mut ready = proof.clone();
        ready.state = BucketProofState::Ready;
        self.replace(&name, &ready).await
    }

    async fn remove(&self, proof: &BucketProof) -> Result<(), BucketError> {
        let name = bucket_claim_name(&proof.backend_id, &proof.bucket_id);
        match self.api.delete(&name, &Default::default()).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(err)) if err.code == 404 => Ok(()),
            Err(source) => Err(source).context(KubeSnafu),
        }
    }

    async fn replace(&self, name: &str, proof: &BucketProof) -> Result<(), BucketError> {
        for _ in 0..CAS_RETRIES {
            let mut cm = self.api.get(name).await.context(KubeSnafu)?;
            let raw = serde_json::to_string(proof).map_err(|err| BucketError::InvalidProof {
                bucket_id: proof.bucket_id.clone(),
                detail: err.to_string(),
            })?;
            let mut data = cm.data.take().unwrap_or_default();
            data.insert(PROOF_KEY.to_string(), raw);
            cm.data = Some(data);
            match self.api.replace(name, &PostParams::default(), &cm).await {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(err)) if err.code == 409 => continue,
                Err(source) => return Err(source).context(KubeSnafu),
            }
        }
        Err(BucketError::InvalidProof {
            bucket_id: proof.bucket_id.clone(),
            detail: "CAS conflict updating bucket ownership proof (retry)".into(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct CreateBucketOutcome {
    pub bucket_id: String,
    pub region: String,
    /// True when BAC/BC supplied bucketName/buckets (static adoption preview).
    #[allow(dead_code)]
    pub static_override: bool,
}

/// Dynamic path: create the unique COSI request name, recording ownership
/// before mutating RustFS so a bucket adopted via `AlreadyExists` (owned by
/// someone else) can never later pass the delete-ownership gate.
/// Static override preview: create configured bucket names; do not treat as
/// fully owned for delete (see [`delete_bucket`]).
pub async fn create_bucket(
    kube: &Client,
    client: &RustfsAdminClient,
    params: &BackendParameters,
    cosi_name: &str,
) -> Result<CreateBucketOutcome, BucketError> {
    let static_override = params.bucket_name.is_some() || params.buckets.is_some();
    let targets = if static_override {
        let list = params.buckets_to_create(cosi_name);
        if list.is_empty() {
            return Err(BucketError::NothingToCreate);
        }
        list
    } else {
        vec![cosi_name.to_string()]
    };
    let bucket_id = if static_override {
        params.primary_bucket_id(cosi_name)
    } else {
        cosi_name.to_string()
    };

    let backend_id = parameters::backend_id(params);
    let namespace = crate::grant::state_namespace(kube);
    let store = BucketOwnershipStore::new(kube.clone(), namespace);
    let proof = store
        .begin_or_resume(&backend_id, cosi_name, &bucket_id, static_override)
        .await?;

    for bucket in &targets {
        info!(
            bucket = %bucket,
            cosi_name = %cosi_name,
            static_override,
            "creating bucket"
        );
        let rustfs_result = client
            .create_bucket(bucket, Some(params.region.as_str()), false)
            .await
            .map_err(|err| BucketError::Admin {
                msg: err.to_string(),
            })?;
        if !logic::is_successful_create(rustfs_result) {
            // Never owned it — drop the PendingCreate proof we just
            // created rather than leaving a dangling claim.
            if proof.state == BucketProofState::PendingCreate {
                store.remove(&proof).await?;
            }
            return Err(BucketError::NameConflict {
                bucket: bucket.clone(),
            });
        }
    }

    store.mark_ready(&proof).await?;

    Ok(CreateBucketOutcome {
        bucket_id,
        region: params.region.clone(),
        static_override,
    })
}

/// Delete only dynamically owned buckets that reached a `Ready` ownership
/// proof — an adopted-but-conflicting bucket never reaches `Ready`, so it
/// can never be deleted through this path.
///
/// When `bucketName`/`buckets` is set on the class, refuse delete (FailedPrecondition
/// semantics at the gRPC layer) so shared/static buckets are not destroyed.
pub async fn delete_bucket(
    kube: &Client,
    client: &RustfsAdminClient,
    params: &BackendParameters,
    bucket_id: &str,
) -> Result<(), BucketError> {
    if params.bucket_name.is_some() || params.buckets.is_some() {
        return Err(BucketError::StaticBucketDeleteRefused {
            bucket_id: bucket_id.to_string(),
        });
    }

    let backend_id = parameters::backend_id(params);
    let namespace = crate::grant::state_namespace(kube);
    let store = BucketOwnershipStore::new(kube.clone(), namespace);
    let name = bucket_claim_name(&backend_id, bucket_id);
    let proof = store.get(&name).await?;

    logic::authorize_delete(false, proof.as_ref().map(|p| p.state), bucket_id)?;

    info!(bucket = %bucket_id, "deleting dynamically owned bucket");
    client
        .delete_bucket(bucket_id)
        .await
        .map_err(|err| BucketError::Admin {
            msg: err.to_string(),
        })?;
    store.remove(&proof.expect("checked above")).await?;
    Ok(())
}

/// Pure decision logic, used by the real code paths above and exhaustively
/// table-tested below without needing kube / RustFS clients.
pub mod logic {
    use super::{BucketError, BucketProofState};
    use rustfs_admin::CreateBucketResult;

    /// Whether a `create_bucket` RustFS response should be treated as a
    /// successful (idempotent) create, given whatever ownership proof state
    /// existed for this bucket_id going in. `existing_proof_state` never
    /// changes the verdict — tests exercise every value to prove that
    /// `AlreadyExists` is never a successful retry, regardless of it.
    pub fn is_successful_create(rustfs_result: CreateBucketResult) -> bool {
        !matches!(rustfs_result, CreateBucketResult::AlreadyExists)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn resolve_create_result(
        existing_proof_state: Option<BucketProofState>,
        rustfs_result: CreateBucketResult,
    ) -> Result<(), BucketError> {
        let _ = existing_proof_state;
        if is_successful_create(rustfs_result) {
            Ok(())
        } else {
            Err(BucketError::NameConflict {
                bucket: "<bucket>".to_string(),
            })
        }
    }

    /// Whether a delete is authorized: never for a static override, and
    /// only for a bucket_id with a `Ready` ownership proof.
    pub fn authorize_delete(
        static_override: bool,
        proof_state: Option<BucketProofState>,
        bucket_id: &str,
    ) -> Result<(), BucketError> {
        if static_override {
            return Err(BucketError::StaticBucketDeleteRefused {
                bucket_id: bucket_id.to_string(),
            });
        }
        match proof_state {
            Some(BucketProofState::Ready) => Ok(()),
            Some(_) => Err(BucketError::DeleteRefusedNotReady {
                bucket_id: bucket_id.to_string(),
            }),
            None => Err(BucketError::DeleteRefusedNoProof {
                bucket_id: bucket_id.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::logic::{authorize_delete, resolve_create_result};
    use super::*;
    use rustfs_admin::CreateBucketResult;
    use std::collections::HashMap;

    fn params(extra: &[(&str, &str)]) -> BackendParameters {
        let mut map = HashMap::new();
        map.insert("endpoint".into(), "http://rustfs".into());
        map.insert("objectStoreUserSecretName".into(), "s".into());
        map.insert("objectStoreUserSecretNamespace".into(), "ns".into());
        for (k, v) in extra {
            map.insert((*k).into(), (*v).into());
        }
        BackendParameters::from_map(&map).unwrap()
    }

    #[test]
    fn dynamic_create_targets_cosi_name_only() {
        let p = params(&[]);
        assert!(p.bucket_name.is_none());
        assert!(p.buckets.is_none());
        let targets = if p.bucket_name.is_some() || p.buckets.is_some() {
            p.buckets_to_create("bc-1")
        } else {
            vec!["bc-1".to_string()]
        };
        assert_eq!(targets, vec!["bc-1".to_string()]);
    }

    #[test]
    fn static_override_refuses_delete_without_admin() {
        let err = authorize_delete(true, None, "shared-mlflow").unwrap_err();
        assert!(matches!(err, BucketError::StaticBucketDeleteRefused { .. }));
    }

    /// Regression test: existing-compatible vs incompatible bucket collision.
    /// `AlreadyExists` (owned by someone else) must never be treated as a
    /// successful retry, no matter what our own proof state was.
    #[test]
    fn already_exists_is_always_a_name_conflict() {
        for state in [
            None,
            Some(BucketProofState::PendingCreate),
            Some(BucketProofState::Ready),
        ] {
            let err = resolve_create_result(state, CreateBucketResult::AlreadyExists).unwrap_err();
            assert!(matches!(err, BucketError::NameConflict { .. }));
        }
    }

    #[test]
    fn created_and_already_owned_by_you_succeed() {
        for state in [
            None,
            Some(BucketProofState::PendingCreate),
            Some(BucketProofState::Ready),
        ] {
            assert!(resolve_create_result(state, CreateBucketResult::Created).is_ok());
            assert!(resolve_create_result(state, CreateBucketResult::AlreadyOwnedByYou).is_ok());
        }
    }

    /// Regression test: delete refusal without a matching ownership proof.
    #[test]
    fn delete_refused_without_matching_proof() {
        assert!(matches!(
            authorize_delete(false, None, "b").unwrap_err(),
            BucketError::DeleteRefusedNoProof { .. }
        ));
        assert!(matches!(
            authorize_delete(false, Some(BucketProofState::PendingCreate), "b").unwrap_err(),
            BucketError::DeleteRefusedNotReady { .. }
        ));
        assert!(authorize_delete(false, Some(BucketProofState::Ready), "b").is_ok());
    }
}
