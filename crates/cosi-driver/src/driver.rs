//! COSI Identity + Provisioner gRPC services.

use std::collections::HashMap;

use kube::Client;
use operator::sts::rustfs_client::{CreateBucketResult, RustfsAdminClient};
use tonic::{Request, Response, Status};
use tracing::{error, info};

use crate::backend::{BackendError, admin_client_from_params};
use crate::parameters::{
    BackendParameters, DRIVER_NAME, bucket_policy_document_for, credentials_for_account,
    grant_owner_policy_document, grant_owner_policy_name, policy_name_for,
};
use crate::proto::cosi::v1alpha1::{
    AuthenticationType, CredentialDetails, DriverCreateBucketRequest, DriverCreateBucketResponse,
    DriverDeleteBucketRequest, DriverDeleteBucketResponse, DriverGetInfoRequest,
    DriverGetInfoResponse, DriverGrantBucketAccessRequest, DriverGrantBucketAccessResponse,
    DriverRevokeBucketAccessRequest, DriverRevokeBucketAccessResponse, Protocol, S3,
    S3SignatureVersion, identity_server::Identity, provisioner_server::Provisioner,
};

#[derive(Clone)]
pub struct Driver {
    kube: Client,
}

impl Driver {
    pub fn new(kube: Client) -> Self {
        Self { kube }
    }
}

fn map_backend(err: BackendError) -> Status {
    error!(error = %err, "backend error");
    Status::internal(err.to_string())
}

fn map_params(err: crate::parameters::ParameterError) -> Status {
    Status::invalid_argument(err.to_string())
}

fn map_admin(err: operator::sts::rustfs_client::RustfsClientError) -> Status {
    error!(error = %err, "rustfs admin error");
    Status::internal(err.to_string())
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

fn user_owns_grant(
    policy_names: &[String],
    owner_policy: &str,
    account_id: &str,
    grant_name: &str,
) -> bool {
    // Default Ceph-style path: account id is the COSI grant name itself.
    if account_id == grant_name {
        return true;
    }
    policy_names.iter().any(|name| name == owner_policy)
}

async fn ensure_grant_policies(
    client: &RustfsAdminClient,
    access_key: &str,
    bucket_policy_name: &str,
    bucket_policy_doc: &str,
    owner_policy_name: &str,
) -> Result<(), Status> {
    client
        .add_canned_policy(bucket_policy_name, bucket_policy_doc)
        .await
        .map_err(map_admin)?;
    client
        .add_canned_policy(owner_policy_name, &grant_owner_policy_document())
        .await
        .map_err(map_admin)?;
    client
        .set_user_policy(
            access_key,
            &[
                bucket_policy_name.to_string(),
                owner_policy_name.to_string(),
            ],
        )
        .await
        .map_err(map_admin)?;
    Ok(())
}

#[tonic::async_trait]
impl Identity for Driver {
    async fn driver_get_info(
        &self,
        _request: Request<DriverGetInfoRequest>,
    ) -> Result<Response<DriverGetInfoResponse>, Status> {
        Ok(Response::new(DriverGetInfoResponse {
            name: DRIVER_NAME.to_string(),
        }))
    }
}

#[tonic::async_trait]
impl Provisioner for Driver {
    async fn driver_create_bucket(
        &self,
        request: Request<DriverCreateBucketRequest>,
    ) -> Result<Response<DriverCreateBucketResponse>, Status> {
        let req = request.into_inner();
        if req.name.trim().is_empty() {
            return Err(Status::invalid_argument("bucket name is required"));
        }
        let params = BackendParameters::from_map(&req.parameters).map_err(map_params)?;
        let client = admin_client_from_params(&self.kube, &params)
            .await
            .map_err(map_backend)?;

        let buckets = params.buckets_to_create(&req.name);
        if buckets.is_empty() {
            return Err(Status::invalid_argument(
                "no buckets to create (buckets/bucketName empty or only *)",
            ));
        }
        let bucket_id = params.primary_bucket_id(&req.name);

        for bucket in &buckets {
            info!(bucket = %bucket, cosi_name = %req.name, "creating bucket");
            match client
                .create_bucket(bucket, Some(params.region.as_str()), false)
                .await
                .map_err(map_admin)?
            {
                CreateBucketResult::Created | CreateBucketResult::AlreadyExists => {}
            }
        }

        Ok(Response::new(DriverCreateBucketResponse {
            bucket_id,
            bucket_info: Some(Protocol {
                r#type: Some(crate::proto::cosi::v1alpha1::protocol::Type::S3(S3 {
                    region: params.region,
                    signature_version: S3SignatureVersion::S3v4 as i32,
                })),
            }),
        }))
    }

    async fn driver_delete_bucket(
        &self,
        request: Request<DriverDeleteBucketRequest>,
    ) -> Result<Response<DriverDeleteBucketResponse>, Status> {
        let req = request.into_inner();
        if req.bucket_id.trim().is_empty() {
            return Err(Status::invalid_argument("bucket_id is required"));
        }
        let params = BackendParameters::from_map(&req.delete_context).map_err(map_params)?;
        let client = admin_client_from_params(&self.kube, &params)
            .await
            .map_err(map_backend)?;

        let buckets = params.buckets_to_create(&req.bucket_id);
        let targets = if buckets.is_empty() {
            vec![req.bucket_id.clone()]
        } else {
            buckets
        };

        for bucket in &targets {
            info!(bucket = %bucket, "deleting bucket");
            client.delete_bucket(bucket).await.map_err(map_admin)?;
        }
        Ok(Response::new(DriverDeleteBucketResponse {}))
    }

    async fn driver_grant_bucket_access(
        &self,
        request: Request<DriverGrantBucketAccessRequest>,
    ) -> Result<Response<DriverGrantBucketAccessResponse>, Status> {
        let req = request.into_inner();
        if req.bucket_id.trim().is_empty() {
            return Err(Status::invalid_argument("bucket_id is required"));
        }
        if req.name.trim().is_empty() {
            return Err(Status::invalid_argument("account name is required"));
        }
        if req.authentication_type != AuthenticationType::Key as i32
            && req.authentication_type != AuthenticationType::UnknownAuthenticationType as i32
        {
            return Err(Status::invalid_argument(
                "only KEY authentication is supported",
            ));
        }

        let params = BackendParameters::from_map(&req.parameters).map_err(map_params)?;
        let client = admin_client_from_params(&self.kube, &params)
            .await
            .map_err(map_backend)?;

        let grant_name = req.name.clone();
        let access_key = params
            .preferred_access_key
            .clone()
            .unwrap_or_else(|| grant_name.clone());
        let secret_key = credentials_for_account(&access_key);
        let policy_buckets = params.buckets_for_policy(&req.bucket_id);
        let bucket_policy_name = params
            .policy
            .clone()
            .unwrap_or_else(|| policy_name_for(&access_key));
        let bucket_policy_doc = bucket_policy_document_for(&policy_buckets);
        let owner_policy_name = grant_owner_policy_name(&grant_name);

        info!(
            bucket = %req.bucket_id,
            account = %access_key,
            grant = %grant_name,
            policy = %bucket_policy_name,
            owner_policy = %owner_policy_name,
            buckets = %policy_buckets.join(","),
            "granting bucket access"
        );

        match client.get_user_info(&access_key).await.map_err(map_admin)? {
            Some(info) => {
                if !user_owns_grant(
                    &info.policy_names,
                    &owner_policy_name,
                    &access_key,
                    &grant_name,
                ) {
                    return Err(Status::already_exists(format!(
                        "preferredAccessKey `{access_key}` is already bound to another BucketAccess; \
                         omit preferredAccessKey or choose a unique value"
                    )));
                }
                // Same grant retry (or Ceph-style account == grant name): never rotate secret.
                ensure_grant_policies(
                    &client,
                    &access_key,
                    &bucket_policy_name,
                    &bucket_policy_doc,
                    &owner_policy_name,
                )
                .await?;
            }
            None => {
                client
                    .add_user(&access_key, &secret_key)
                    .await
                    .map_err(map_admin)?;
                ensure_grant_policies(
                    &client,
                    &access_key,
                    &bucket_policy_name,
                    &bucket_policy_doc,
                    &owner_policy_name,
                )
                .await?;
            }
        }

        let secrets = credential_map(&access_key, &secret_key, &params, &policy_buckets);
        let mut credentials = HashMap::new();
        credentials.insert("s3".to_string(), CredentialDetails { secrets });

        Ok(Response::new(DriverGrantBucketAccessResponse {
            account_id: access_key,
            credentials,
        }))
    }

    async fn driver_revoke_bucket_access(
        &self,
        request: Request<DriverRevokeBucketAccessRequest>,
    ) -> Result<Response<DriverRevokeBucketAccessResponse>, Status> {
        let req = request.into_inner();
        if req.bucket_id.trim().is_empty() {
            return Err(Status::invalid_argument("bucket_id is required"));
        }
        if req.account_id.trim().is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }

        let params = BackendParameters::from_map(&req.revoke_access_context).map_err(map_params)?;
        let client = admin_client_from_params(&self.kube, &params)
            .await
            .map_err(map_backend)?;

        info!(
            bucket = %req.bucket_id,
            account = %req.account_id,
            "revoking bucket access"
        );
        client
            .remove_user(&req.account_id)
            .await
            .map_err(map_admin)?;
        Ok(Response::new(DriverRevokeBucketAccessResponse {}))
    }
}

#[cfg(test)]
mod grant_tests {
    use super::user_owns_grant;

    #[test]
    fn same_grant_name_as_account_is_idempotent() {
        assert!(user_owns_grant(&[], "cosi-grant-ba-1", "ba-1", "ba-1"));
    }

    #[test]
    fn preferred_key_requires_owner_marker() {
        assert!(!user_owns_grant(
            &["cosi-mlflow".to_string()],
            "cosi-grant-ba-1",
            "mlflow",
            "ba-1"
        ));
        assert!(user_owns_grant(
            &["cosi-mlflow".to_string(), "cosi-grant-ba-1".to_string()],
            "cosi-grant-ba-1",
            "mlflow",
            "ba-1"
        ));
    }
}
