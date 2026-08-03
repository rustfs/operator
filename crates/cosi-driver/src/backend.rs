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

//! Kubernetes Secret / ConfigMap loading and RustFS admin client construction.

use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::{Api, Client};
use rustfs_admin::{RustfsAdminClient, RustfsClientError, RustfsCredentials};
use snafu::{ResultExt, Snafu};
use std::collections::BTreeMap;

use crate::parameters::BackendParameters;

fn box_kube(err: kube::Error) -> Box<kube::Error> {
    Box::new(err)
}

#[derive(Debug, Snafu)]
pub enum BackendError {
    #[snafu(display("failed to create kubernetes client: {source}"))]
    KubeClient { source: Box<kube::Error> },
    #[snafu(display("failed to read Secret {namespace}/{name}: {source}"))]
    SecretLookup {
        namespace: String,
        name: String,
        source: Box<kube::Error>,
    },
    #[snafu(display("failed to read ConfigMap {namespace}/{name}: {source}"))]
    ConfigMapLookup {
        namespace: String,
        name: String,
        source: Box<kube::Error>,
    },
    #[snafu(display("Secret {namespace}/{name} missing key `{key}`"))]
    MissingSecretKey {
        namespace: String,
        name: String,
        key: &'static str,
    },
    #[snafu(display("Secret {namespace}/{name} key `{key}` is not valid UTF-8"))]
    InvalidSecretKey {
        namespace: String,
        name: String,
        key: &'static str,
    },
    #[snafu(display("Secret {namespace}/{name} key `{key}` is empty"))]
    EmptySecretKey {
        namespace: String,
        name: String,
        key: &'static str,
    },
    #[snafu(display("ConfigMap {namespace}/{name} missing CA data key"))]
    MissingCaData { namespace: String, name: String },
    #[snafu(display("failed to build RustFS admin client: {source}"))]
    ClientBuild { source: RustfsClientError },
}

#[derive(Clone)]
pub struct BackendFactory {
    kube: Client,
}

impl BackendFactory {
    pub async fn try_default() -> Result<Self, BackendError> {
        let kube = Client::try_default()
            .await
            .map_err(box_kube)
            .context(KubeClientSnafu)?;
        Ok(Self { kube })
    }

    #[cfg(test)]
    pub fn from_client(kube: Client) -> Self {
        Self { kube }
    }

    pub async fn admin_client(
        &self,
        params: &BackendParameters,
    ) -> Result<RustfsAdminClient, BackendError> {
        let credentials = self
            .load_credentials(&params.secret_namespace, &params.secret_name)
            .await?;

        match (
            params.tls_ca_configmap_name.as_deref(),
            params.tls_ca_configmap_namespace.as_deref(),
        ) {
            (Some(name), Some(namespace)) => {
                let ca_pem = self.load_ca_pem(namespace, name).await?;
                RustfsAdminClient::new_with_base_url_and_ca_pem(
                    params.endpoint.clone(),
                    credentials.access_key,
                    credentials.secret_key,
                    &ca_pem,
                )
                .context(ClientBuildSnafu)
            }
            _ => Ok(RustfsAdminClient::new_with_base_url(
                params.endpoint.clone(),
                credentials.access_key,
                credentials.secret_key,
            )),
        }
    }

    async fn load_credentials(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<RustfsCredentials, BackendError> {
        let api: Api<Secret> = Api::namespaced(self.kube.clone(), namespace);
        let secret = api
            .get(name)
            .await
            .map_err(box_kube)
            .context(SecretLookupSnafu {
                namespace: namespace.to_string(),
                name: name.to_string(),
            })?;
        let data = secret.data.as_ref();
        Ok(RustfsCredentials {
            access_key: secret_value(data, namespace, name, "accesskey")?,
            secret_key: secret_value(data, namespace, name, "secretkey")?,
        })
    }

    async fn load_ca_pem(&self, namespace: &str, name: &str) -> Result<Vec<u8>, BackendError> {
        let api: Api<ConfigMap> = Api::namespaced(self.kube.clone(), namespace);
        let cm = api
            .get(name)
            .await
            .map_err(box_kube)
            .context(ConfigMapLookupSnafu {
                namespace: namespace.to_string(),
                name: name.to_string(),
            })?;

        if let Some(data) = cm.data.as_ref() {
            for key in ["ca.crt", "tls.crt", "ca-bundle.crt"] {
                if let Some(value) = data.get(key).filter(|v| !v.trim().is_empty()) {
                    return Ok(value.as_bytes().to_vec());
                }
            }
        }
        if let Some(bin) = cm.binary_data.as_ref() {
            for key in ["ca.crt", "tls.crt", "ca-bundle.crt"] {
                if let Some(value) = bin.get(key).filter(|v| !v.0.is_empty()) {
                    return Ok(value.0.clone());
                }
            }
        }

        Err(BackendError::MissingCaData {
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }
}

fn secret_value(
    data: Option<&BTreeMap<String, ByteString>>,
    namespace: &str,
    name: &str,
    key: &'static str,
) -> Result<String, BackendError> {
    let raw =
        data.and_then(|data| data.get(key))
            .ok_or_else(|| BackendError::MissingSecretKey {
                namespace: namespace.to_string(),
                name: name.to_string(),
                key,
            })?;
    let value = String::from_utf8(raw.0.clone()).map_err(|_| BackendError::InvalidSecretKey {
        namespace: namespace.to_string(),
        name: name.to_string(),
        key,
    })?;
    if value.is_empty() {
        return Err(BackendError::EmptySecretKey {
            namespace: namespace.to_string(),
            name: name.to_string(),
            key,
        });
    }
    Ok(value)
}
