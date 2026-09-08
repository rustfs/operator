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

//! Durable random S3 credentials stored in Kubernetes Secrets, created once.

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::{ByteString, apimachinery::pkg::apis::meta::v1::ObjectMeta};
use kube::api::PostParams;
use kube::{Api, Client, Error as KubeError};
use rand::{Rng, distributions::Alphanumeric};
use sha2::{Digest, Sha256};
use snafu::Snafu;

use crate::parameters::sanitize_policy_fragment;

#[derive(Debug, Snafu)]
pub enum CredentialStoreError {
    #[snafu(display("failed to read credential Secret {namespace}/{name}: {source}"))]
    Lookup {
        namespace: String,
        name: String,
        source: Box<KubeError>,
    },
    #[snafu(display("failed to persist credential Secret {namespace}/{name}: {source}"))]
    Persist {
        namespace: String,
        name: String,
        source: Box<KubeError>,
    },
    #[snafu(display("credential Secret {namespace}/{name} missing key `{key}`"))]
    MissingKey {
        namespace: String,
        name: String,
        key: &'static str,
    },
    #[snafu(display(
        "credential store inconsistent: Secret {namespace}/{name} holds access key `{found}` \
         but this grant is bound to `{expected}`"
    ))]
    AccessKeyMismatch {
        namespace: String,
        name: String,
        expected: String,
        found: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredentials {
    pub access_key: String,
    pub secret_key: String,
    pub secret_name: String,
    /// True only when *this* call created the Secret; false for a normal
    /// reload and for the loser of a concurrent create race. Consumed by
    /// `grant::resolve_password_rotation` to decide whether the RustFS
    /// password must be (re)issued.
    pub freshly_created: bool,
}

pub fn credential_secret_name(grant_name: &str) -> String {
    format!("cosi-cred-{}", sanitize_policy_fragment(grant_name))
}

pub fn access_key_hash(access_key: &str) -> String {
    hex::encode(Sha256::digest(access_key.as_bytes()))
}

pub fn random_secret_key(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn decode_secret_key(secret: &Secret, key: &'static str) -> Result<String, CredentialStoreError> {
    let namespace = secret.metadata.namespace.clone().unwrap_or_default();
    let name = secret.metadata.name.clone().unwrap_or_default();
    let value = secret
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .ok_or_else(|| CredentialStoreError::MissingKey {
            namespace: namespace.clone(),
            name: name.clone(),
            key,
        })?;
    String::from_utf8(value.0.clone()).map_err(|_| CredentialStoreError::MissingKey {
        namespace,
        name,
        key,
    })
}

fn read_stored(secret: &Secret) -> Result<(String, String), CredentialStoreError> {
    let access = decode_secret_key(secret, "accessKeyID")
        .or_else(|_| decode_secret_key(secret, "AWS_ACCESS_KEY_ID"))?;
    let secret_key = decode_secret_key(secret, "accessSecretKey")
        .or_else(|_| decode_secret_key(secret, "AWS_SECRET_ACCESS_KEY"))?;
    Ok((access, secret_key))
}

/// Load existing credentials for a grant, or atomically create a new random
/// Secret. Two concurrent creators for the same grant converge on the
/// winner's material: the loser reloads instead of overwriting.
pub async fn load_or_create_credentials(
    kube: &Client,
    namespace: &str,
    grant_name: &str,
    access_key: &str,
) -> Result<StoredCredentials, CredentialStoreError> {
    let secret_name = credential_secret_name(grant_name);
    let api: Api<Secret> = Api::namespaced(kube.clone(), namespace);

    match api.get(&secret_name).await {
        Ok(existing) => {
            let (stored_access, secret_key) = read_stored(&existing)?;
            if stored_access != access_key {
                return Err(CredentialStoreError::AccessKeyMismatch {
                    namespace: namespace.to_string(),
                    name: secret_name,
                    expected: access_key.to_string(),
                    found: stored_access,
                });
            }
            Ok(StoredCredentials {
                access_key: stored_access,
                secret_key,
                secret_name,
                freshly_created: false,
            })
        }
        Err(KubeError::Api(err)) if err.code == 404 => {
            let secret_key = random_secret_key(40);
            match create_credentials(kube, namespace, &secret_name, access_key, &secret_key).await {
                Ok(()) => Ok(StoredCredentials {
                    access_key: access_key.to_string(),
                    secret_key,
                    secret_name,
                    freshly_created: true,
                }),
                Err(CredentialStoreError::Persist { source, .. }) if matches!(source.as_ref(), KubeError::Api(err) if err.code == 409) =>
                {
                    // Lost the create race: reload and return the winner's
                    // material rather than our own locally-generated one.
                    let winner = api.get(&secret_name).await.map_err(|source| {
                        CredentialStoreError::Lookup {
                            namespace: namespace.to_string(),
                            name: secret_name.clone(),
                            source: Box::new(source),
                        }
                    })?;
                    let (stored_access, secret_key) = read_stored(&winner)?;
                    if stored_access != access_key {
                        return Err(CredentialStoreError::AccessKeyMismatch {
                            namespace: namespace.to_string(),
                            name: secret_name,
                            expected: access_key.to_string(),
                            found: stored_access,
                        });
                    }
                    Ok(StoredCredentials {
                        access_key: stored_access,
                        secret_key,
                        secret_name,
                        freshly_created: false,
                    })
                }
                Err(err) => Err(err),
            }
        }
        Err(source) => Err(CredentialStoreError::Lookup {
            namespace: namespace.to_string(),
            name: secret_name,
            source: Box::new(source),
        }),
    }
}

async fn create_credentials(
    kube: &Client,
    namespace: &str,
    secret_name: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<(), CredentialStoreError> {
    let api: Api<Secret> = Api::namespaced(kube.clone(), namespace);
    let mut data = std::collections::BTreeMap::new();
    data.insert(
        "accessKeyID".to_string(),
        ByteString(access_key.as_bytes().to_vec()),
    );
    data.insert(
        "accessSecretKey".to_string(),
        ByteString(secret_key.as_bytes().to_vec()),
    );
    data.insert(
        "AWS_ACCESS_KEY_ID".to_string(),
        ByteString(access_key.as_bytes().to_vec()),
    );
    data.insert(
        "AWS_SECRET_ACCESS_KEY".to_string(),
        ByteString(secret_key.as_bytes().to_vec()),
    );

    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(secret_name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(
                [
                    (
                        "app.kubernetes.io/name".to_string(),
                        "rustfs-cosi-driver".to_string(),
                    ),
                    (
                        "rustfs.objectstorage.k8s.io/grant".to_string(),
                        sanitize_policy_fragment(
                            secret_name
                                .strip_prefix("cosi-cred-")
                                .unwrap_or(secret_name),
                        ),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            ..ObjectMeta::default()
        },
        type_: Some("Opaque".to_string()),
        data: Some(data),
        ..Secret::default()
    };

    api.create(&PostParams::default(), &secret)
        .await
        .map_err(|source| CredentialStoreError::Persist {
            namespace: namespace.to_string(),
            name: secret_name.to_string(),
            source: Box::new(source),
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{access_key_hash, credential_secret_name, random_secret_key};

    #[test]
    fn secret_names_are_stable() {
        assert_eq!(credential_secret_name("ba-abc.def"), "cosi-cred-ba-abc-def");
    }

    #[test]
    fn random_secrets_are_not_derived_from_access_key() {
        let a = random_secret_key(40);
        let b = random_secret_key(40);
        assert_ne!(a, b);
        assert_ne!(a, access_key_hash("mlflow"));
        assert!(a.len() >= 40);
    }
}
