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

//! gRPC Identity and Provisioner servers for COSI v1alpha1.

#![allow(clippy::result_large_err)]

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};
use tracing::info;

use crate::backend::{BackendError, BackendFactory};
use crate::parameters::{BackendParameters, ParameterError};
use crate::policy::{bucket_policy_document, policy_name_for};
use crate::proto::cosi::v1alpha1::{
    AuthenticationType, CredentialDetails, DriverCreateBucketRequest, DriverCreateBucketResponse,
    DriverDeleteBucketRequest, DriverDeleteBucketResponse, DriverGetInfoRequest,
    DriverGetInfoResponse, DriverGrantBucketAccessRequest, DriverGrantBucketAccessResponse,
    DriverRevokeBucketAccessRequest, DriverRevokeBucketAccessResponse, Protocol, S3,
    S3SignatureVersion, identity_server::Identity, provisioner_server::Provisioner,
};

pub const DRIVER_NAME: &str = "rustfs.objectstorage.k8s.io";

/// Deterministic secret so DriverGrantBucketAccess is idempotent across sidecar retries.
fn credentials_for_account(account_id: &str) -> String {
    let digest = Sha256::digest(format!("rustfs-cosi-v1:{account_id}").as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod credential_tests {
    use super::credentials_for_account;

    #[test]
    fn credentials_are_deterministic_and_long_enough() {
        let a = credentials_for_account("ba-test-uid");
        let b = credentials_for_account("ba-test-uid");
        assert_eq!(a, b);
        assert!(a.len() >= 8);
        assert_ne!(a, credentials_for_account("other-account"));
    }
}

pub struct IdentityService {
    pub name: String,
}

#[tonic::async_trait]
impl Identity for IdentityService {
    async fn driver_get_info(
        &self,
        _request: Request<DriverGetInfoRequest>,
    ) -> Result<Response<DriverGetInfoResponse>, Status> {
        Ok(Response::new(DriverGetInfoResponse {
            name: self.name.clone(),
        }))
    }
}

pub struct ProvisionerService {
    pub backend: BackendFactory,
}

#[tonic::async_trait]
impl Provisioner for ProvisionerService {
    async fn driver_create_bucket(
        &self,
        request: Request<DriverCreateBucketRequest>,
    ) -> Result<Response<DriverCreateBucketResponse>, Status> {
        let req = request.into_inner();
        let bucket_name = req.name.trim();
        if bucket_name.is_empty() {
            return Err(Status::invalid_argument("bucket name is required"));
        }

        let params = parse_params(&req.parameters)?;
        let client = self
            .backend
            .admin_client(&params)
            .await
            .map_err(map_backend)?;

        info!(bucket = %bucket_name, endpoint = %params.endpoint, "creating bucket");
        client
            .create_bucket(bucket_name, params.region.as_deref(), false)
            .await
            .map_err(map_admin)?;

        Ok(Response::new(DriverCreateBucketResponse {
            bucket_id: bucket_name.to_string(),
            bucket_info: Some(Protocol {
                r#type: Some(crate::proto::cosi::v1alpha1::protocol::Type::S3(S3 {
                    region: params.region.unwrap_or_else(|| "us-east-1".to_string()),
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
        let bucket_id = req.bucket_id.trim();
        if bucket_id.is_empty() {
            return Err(Status::invalid_argument("bucket_id is required"));
        }

        let params = parse_params(&req.delete_context)?;
        let client = self
            .backend
            .admin_client(&params)
            .await
            .map_err(map_backend)?;

        info!(bucket = %bucket_id, "deleting bucket");
        client.delete_bucket(bucket_id).await.map_err(map_admin)?;

        Ok(Response::new(DriverDeleteBucketResponse {}))
    }

    async fn driver_grant_bucket_access(
        &self,
        request: Request<DriverGrantBucketAccessRequest>,
    ) -> Result<Response<DriverGrantBucketAccessResponse>, Status> {
        let req = request.into_inner();
        let bucket_id = req.bucket_id.trim();
        let account_name = req.name.trim();
        if bucket_id.is_empty() {
            return Err(Status::invalid_argument("bucket_id is required"));
        }
        if account_name.is_empty() {
            return Err(Status::invalid_argument("account name is required"));
        }
        if req.authentication_type != AuthenticationType::Key as i32
            && req.authentication_type != AuthenticationType::UnknownAuthenticationType as i32
        {
            return Err(Status::invalid_argument(
                "only KEY authentication is supported",
            ));
        }

        let params = parse_params(&req.parameters)?;
        let client = self
            .backend
            .admin_client(&params)
            .await
            .map_err(map_backend)?;

        let account_id = account_name.to_string();
        let secret_key = credentials_for_account(&account_id);
        let policy_name = policy_name_for(&account_id, bucket_id);
        let policy_doc = bucket_policy_document(bucket_id, params.access_policy);

        info!(
            bucket = %bucket_id,
            account = %account_id,
            policy = %policy_name,
            "granting bucket access"
        );

        client
            .add_canned_policy(&policy_name, &policy_doc)
            .await
            .map_err(map_admin)?;

        if !client.user_exists(&account_id).await.map_err(map_admin)? {
            client
                .add_user(&account_id, &secret_key)
                .await
                .map_err(map_admin)?;
        }
        client
            .set_user_policy(&account_id, std::slice::from_ref(&policy_name))
            .await
            .map_err(map_admin)?;

        let mut secrets = HashMap::new();
        secrets.insert("endpoint".to_string(), params.endpoint.clone());
        secrets.insert(
            "region".to_string(),
            params.region.unwrap_or_else(|| "us-east-1".to_string()),
        );
        secrets.insert("accessKeyID".to_string(), account_id.clone());
        secrets.insert("accessSecretKey".to_string(), secret_key);
        secrets.insert("bucketName".to_string(), bucket_id.to_string());

        let mut credentials = HashMap::new();
        credentials.insert("s3".to_string(), CredentialDetails { secrets });

        Ok(Response::new(DriverGrantBucketAccessResponse {
            account_id,
            credentials,
        }))
    }

    async fn driver_revoke_bucket_access(
        &self,
        request: Request<DriverRevokeBucketAccessRequest>,
    ) -> Result<Response<DriverRevokeBucketAccessResponse>, Status> {
        let req = request.into_inner();
        let bucket_id = req.bucket_id.trim();
        let account_id = req.account_id.trim();
        if account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }

        let params = parse_params(&req.revoke_access_context)?;
        let client = self
            .backend
            .admin_client(&params)
            .await
            .map_err(map_backend)?;

        let policy_name = if bucket_id.is_empty() {
            None
        } else {
            Some(policy_name_for(account_id, bucket_id))
        };

        info!(account = %account_id, bucket = %bucket_id, "revoking bucket access");
        client.remove_user(account_id).await.map_err(map_admin)?;
        if let Some(policy_name) = policy_name {
            client
                .remove_canned_policy(&policy_name)
                .await
                .map_err(map_admin)?;
        }

        Ok(Response::new(DriverRevokeBucketAccessResponse {}))
    }
}

fn parse_params(params: &HashMap<String, String>) -> Result<BackendParameters, Status> {
    BackendParameters::from_map(params).map_err(map_params)
}

fn map_params(err: ParameterError) -> Status {
    Status::invalid_argument(err.to_string())
}

fn map_backend(err: BackendError) -> Status {
    Status::failed_precondition(err.to_string())
}

fn map_admin(err: rustfs_admin::RustfsClientError) -> Status {
    match &err {
        rustfs_admin::RustfsClientError::UnexpectedStatus { status, .. }
            if status.as_u16() == 409 =>
        {
            Status::already_exists(err.to_string())
        }
        rustfs_admin::RustfsClientError::InvalidPolicyName
        | rustfs_admin::RustfsClientError::InvalidPolicyDocument
        | rustfs_admin::RustfsClientError::InvalidCredentialValue { .. }
        | rustfs_admin::RustfsClientError::EmptyCredentialValue { .. }
        | rustfs_admin::RustfsClientError::MissingCredentialKey { .. }
        | rustfs_admin::RustfsClientError::RequestBuildFailed => {
            Status::invalid_argument(err.to_string())
        }
        _ => Status::internal(err.to_string()),
    }
}
