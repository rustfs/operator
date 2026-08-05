//! COSI Identity + Provisioner gRPC adapters (thin; logic lives in grant/bucket).

use std::collections::HashMap;

use kube::Client;
use tonic::{Request, Response, Status};
use tracing::error;

use crate::backend::{BackendError, admin_client_from_params};
use crate::bucket::{self, BucketError};
use crate::grant::{self, GrantError};
use crate::parameters::{BackendParameters, DRIVER_NAME};
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

fn map_bucket(err: BucketError) -> Status {
    match err {
        BucketError::StaticBucketDeleteRefused { .. } => {
            Status::failed_precondition(err.to_string())
        }
        BucketError::NothingToCreate => Status::invalid_argument(err.to_string()),
        BucketError::Admin(msg) => {
            error!(error = %msg, "rustfs admin error");
            Status::internal(msg)
        }
    }
}

fn map_grant(err: GrantError) -> Status {
    if err.is_conflict() {
        return Status::already_exists(err.to_string());
    }
    match err {
        GrantError::MissingExternalPolicy(_) => Status::failed_precondition(err.to_string()),
        GrantError::Credentials(e) => {
            error!(error = %e, "credential store error");
            Status::internal(e.to_string())
        }
        GrantError::Ownership(e) => {
            error!(error = %e, "ownership store error");
            Status::internal(e.to_string())
        }
        GrantError::Admin(msg) => {
            error!(error = %msg, "rustfs admin error");
            Status::internal(msg)
        }
        other => Status::internal(other.to_string()),
    }
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

        let outcome = bucket::create_bucket(&client, &params, &req.name)
            .await
            .map_err(map_bucket)?;

        Ok(Response::new(DriverCreateBucketResponse {
            bucket_id: outcome.bucket_id,
            bucket_info: Some(Protocol {
                r#type: Some(crate::proto::cosi::v1alpha1::protocol::Type::S3(S3 {
                    region: outcome.region,
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

        bucket::delete_bucket(&client, &params, &req.bucket_id)
            .await
            .map_err(map_bucket)?;
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

        let result =
            grant::grant_bucket_access(&self.kube, &client, &params, &req.name, &req.bucket_id)
                .await
                .map_err(map_grant)?;

        let mut credentials = HashMap::new();
        credentials.insert(
            "s3".to_string(),
            CredentialDetails {
                secrets: result.secrets,
            },
        );

        Ok(Response::new(DriverGrantBucketAccessResponse {
            account_id: result.account_id,
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

        grant::revoke_bucket_access(&self.kube, &client, &req.account_id)
            .await
            .map_err(map_grant)?;
        Ok(Response::new(DriverRevokeBucketAccessResponse {}))
    }
}
