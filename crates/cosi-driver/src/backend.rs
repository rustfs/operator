//! Admin credential lookup + RustFS admin client construction.

use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::{Api, Client};
use operator::sts::rustfs_client::RustfsAdminClient;
use thiserror::Error;
use tracing::info;

use crate::parameters::BackendParameters;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("failed to read Secret {namespace}/{name}: {source}")]
    SecretLookup {
        namespace: String,
        name: String,
        #[source]
        source: kube::Error,
    },
    #[error("secret key missing: {0}")]
    MissingSecretKey(&'static str),
    #[error("secret key empty: {0}")]
    EmptySecretKey(&'static str),
    #[error("secret key is not valid utf8: {0}")]
    InvalidSecretKey(&'static str),
    #[error("failed to read ConfigMap {namespace}/{name}: {source}")]
    ConfigMapLookup {
        namespace: String,
        name: String,
        #[source]
        source: kube::Error,
    },
    #[error("configmap key missing: {0}")]
    MissingCaData(&'static str),
    #[error(transparent)]
    ClientBuild(#[from] operator::sts::rustfs_client::RustfsClientError),
}

#[allow(clippy::result_large_err)]
fn secret_value<'a>(secret: &'a Secret, keys: &[&'static str]) -> Result<&'a str, BackendError> {
    let data = secret
        .data
        .as_ref()
        .ok_or(BackendError::MissingSecretKey(keys[0]))?;
    for key in keys {
        if let Some(bytes) = data.get(*key) {
            let value =
                std::str::from_utf8(&bytes.0).map_err(|_| BackendError::InvalidSecretKey(key))?;
            if value.is_empty() {
                return Err(BackendError::EmptySecretKey(key));
            }
            return Ok(value);
        }
    }
    Err(BackendError::MissingSecretKey(keys[0]))
}

pub async fn admin_client_from_params(
    kube: &Client,
    params: &BackendParameters,
) -> Result<RustfsAdminClient, BackendError> {
    let secrets: Api<Secret> =
        Api::namespaced(kube.clone(), &params.object_store_user_secret_namespace);
    let secret = secrets
        .get(&params.object_store_user_secret_name)
        .await
        .map_err(|source| BackendError::SecretLookup {
            namespace: params.object_store_user_secret_namespace.clone(),
            name: params.object_store_user_secret_name.clone(),
            source,
        })?;

    let access_key = secret_value(
        &secret,
        &[
            "accesskey",
            "accessKey",
            "ACCESSKEY",
            "AWS_ACCESS_KEY_ID",
            "access_key",
            "access-key",
            "access_key_id",
            "access-key-id",
            "RUSTFS_ACCESS_KEY",
        ],
    )?;
    let secret_key = secret_value(
        &secret,
        &[
            "secretkey",
            "secretKey",
            "SECRETKEY",
            "AWS_SECRET_ACCESS_KEY",
            "secret_key",
            "secret-key",
            "secret_access_key",
            "secret-access-key",
            "RUSTFS_SECRET_KEY",
        ],
    )?;

    info!(
        endpoint = %params.endpoint,
        secret = %params.object_store_user_secret_name,
        "building RustFS admin client"
    );

    if let (Some(cm_name), Some(cm_ns)) = (
        params.tls_ca_configmap_name.as_ref(),
        params
            .tls_ca_configmap_namespace
            .as_ref()
            .or(Some(&params.object_store_user_secret_namespace)),
    ) {
        let cms: Api<ConfigMap> = Api::namespaced(kube.clone(), cm_ns);
        let cm = cms
            .get(cm_name)
            .await
            .map_err(|source| BackendError::ConfigMapLookup {
                namespace: cm_ns.clone(),
                name: cm_name.clone(),
                source,
            })?;
        let ca = cm
            .data
            .as_ref()
            .and_then(|d| {
                d.get("ca.crt")
                    .or_else(|| d.get("tls.crt"))
                    .or_else(|| d.get("ca-bundle.crt"))
            })
            .ok_or(BackendError::MissingCaData("ca.crt"))?;
        return Ok(RustfsAdminClient::new_with_base_url_and_ca_pem(
            params.endpoint.clone(),
            access_key,
            secret_key,
            ca.as_bytes(),
        )?);
    }

    Ok(RustfsAdminClient::new_with_base_url(
        params.endpoint.clone(),
        access_key,
        secret_key,
    ))
}
