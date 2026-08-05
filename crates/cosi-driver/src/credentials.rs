//! Durable random S3 credentials stored in Kubernetes Secrets.

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::{ByteString, apimachinery::pkg::apis::meta::v1::ObjectMeta};
use kube::api::{Patch, PatchParams};
use kube::{Api, Client, Error as KubeError};
use rand::{Rng, distributions::Alphanumeric};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::parameters::sanitize_policy_fragment;

#[derive(Debug, Error)]
pub enum CredentialStoreError {
    #[error("failed to read credential Secret {namespace}/{name}: {source}")]
    Lookup {
        namespace: String,
        name: String,
        #[source]
        source: Box<KubeError>,
    },
    #[error("failed to persist credential Secret {namespace}/{name}: {source}")]
    Persist {
        namespace: String,
        name: String,
        #[source]
        source: Box<KubeError>,
    },
    #[error("credential Secret {namespace}/{name} missing key `{key}`")]
    MissingKey {
        namespace: String,
        name: String,
        key: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredentials {
    pub access_key: String,
    pub secret_key: String,
    pub secret_name: String,
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
    let value = secret.data.as_ref().and_then(|data| data.get(key)).ok_or(
        CredentialStoreError::MissingKey {
            namespace: namespace.clone(),
            name: name.clone(),
            key,
        },
    )?;
    String::from_utf8(value.0.clone()).map_err(|_| CredentialStoreError::MissingKey {
        namespace,
        name,
        key,
    })
}

/// Load existing credentials for a grant, or create a new random secret and persist it.
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
            let stored_access = decode_secret_key(&existing, "accessKeyID")
                .or_else(|_| decode_secret_key(&existing, "AWS_ACCESS_KEY_ID"))?;
            let secret_key = decode_secret_key(&existing, "accessSecretKey")
                .or_else(|_| decode_secret_key(&existing, "AWS_SECRET_ACCESS_KEY"))?;
            if stored_access != access_key {
                // Access key changed for this grant — rotate secret material under same Secret.
                let secret_key = random_secret_key(40);
                persist_credentials(kube, namespace, &secret_name, access_key, &secret_key).await?;
                return Ok(StoredCredentials {
                    access_key: access_key.to_string(),
                    secret_key,
                    secret_name,
                });
            }
            Ok(StoredCredentials {
                access_key: stored_access,
                secret_key,
                secret_name,
            })
        }
        Err(KubeError::Api(err)) if err.code == 404 => {
            let secret_key = random_secret_key(40);
            persist_credentials(kube, namespace, &secret_name, access_key, &secret_key).await?;
            Ok(StoredCredentials {
                access_key: access_key.to_string(),
                secret_key,
                secret_name,
            })
        }
        Err(source) => Err(CredentialStoreError::Lookup {
            namespace: namespace.to_string(),
            name: secret_name,
            source: Box::new(source),
        }),
    }
}

async fn persist_credentials(
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

    api.patch(
        secret_name,
        &PatchParams::apply("rustfs-cosi-driver").force(),
        &Patch::Apply(&secret),
    )
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
