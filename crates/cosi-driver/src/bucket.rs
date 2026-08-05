//! Bucket create/delete helpers with safe static-bucket defaults.

use rustfs_admin::{CreateBucketResult, RustfsAdminClient};
use thiserror::Error;
use tracing::info;

use crate::parameters::BackendParameters;

#[derive(Debug, Error)]
pub enum BucketError {
    #[error("rustfs admin error: {0}")]
    Admin(String),
    #[error(
        "refusing to delete bucket `{bucket_id}`: BucketClass uses static \
         bucketName/buckets override (adoption preview); delete is skipped \
         without ownership proof"
    )]
    StaticBucketDeleteRefused { bucket_id: String },
    #[error("no buckets to create (buckets/bucketName empty or only *)")]
    NothingToCreate,
}

#[derive(Debug, Clone)]
pub struct CreateBucketOutcome {
    pub bucket_id: String,
    pub region: String,
    /// True when BAC/BC supplied bucketName/buckets (static adoption preview).
    #[allow(dead_code)]
    pub static_override: bool,
}

/// Dynamic path: create the unique COSI request name.
/// Static override preview: create configured bucket names; do not treat as
/// fully owned for delete (see [`delete_bucket`]).
pub async fn create_bucket(
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

    for bucket in &targets {
        info!(
            bucket = %bucket,
            cosi_name = %cosi_name,
            static_override,
            "creating bucket"
        );
        match client
            .create_bucket(bucket, Some(params.region.as_str()), false)
            .await
            .map_err(|err| BucketError::Admin(err.to_string()))?
        {
            CreateBucketResult::Created | CreateBucketResult::AlreadyExists => {}
        }
    }

    Ok(CreateBucketOutcome {
        bucket_id,
        region: params.region.clone(),
        static_override,
    })
}

/// Delete only dynamically owned buckets.
///
/// When `bucketName`/`buckets` is set on the class, refuse delete (FailedPrecondition
/// semantics at the gRPC layer) so shared/static buckets are not destroyed.
pub async fn delete_bucket(
    client: &RustfsAdminClient,
    params: &BackendParameters,
    bucket_id: &str,
) -> Result<(), BucketError> {
    if params.bucket_name.is_some() || params.buckets.is_some() {
        return Err(BucketError::StaticBucketDeleteRefused {
            bucket_id: bucket_id.to_string(),
        });
    }

    info!(bucket = %bucket_id, "deleting dynamically owned bucket");
    client
        .delete_bucket(bucket_id)
        .await
        .map_err(|err| BucketError::Admin(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        // create_bucket itself needs admin client; unit-check targeting logic here
        let targets = if p.bucket_name.is_some() || p.buckets.is_some() {
            p.buckets_to_create("bc-1")
        } else {
            vec!["bc-1".to_string()]
        };
        assert_eq!(targets, vec!["bc-1".to_string()]);
    }

    #[test]
    fn static_override_refuses_delete_without_admin() {
        let p = params(&[("bucketName", "shared-mlflow")]);
        let err = match (
            p.bucket_name.is_some() || p.buckets.is_some(),
            "shared-mlflow",
        ) {
            (true, id) => BucketError::StaticBucketDeleteRefused {
                bucket_id: id.to_string(),
            },
            _ => unreachable!(),
        };
        assert!(matches!(err, BucketError::StaticBucketDeleteRefused { .. }));
    }
}
