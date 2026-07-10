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

use crate::types;
use crate::types::v1alpha1::encryption::LocalKmsMasterKeySecretRef;
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{DeleteParams, ListParams, ObjectList, Patch, PatchParams, PostParams};
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::{Resource, ResourceExt, api::Api};
use serde::Serialize;
use serde::de::DeserializeOwned;
use snafu::Snafu;
use snafu::futures::TryFutureExt;
use std::fmt::Debug;
use std::path::{Component, Path, PathBuf};
use tracing::info;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Kubernetes API error: {}", source))]
    Kube { source: kube::Error },

    #[snafu(display("record event error: {}", source))]
    Record { source: kube::Error },

    #[snafu(transparent)]
    Types { source: types::error::Error },

    #[snafu(display("credential secret '{}' not found", name))]
    CredentialSecretNotFound { name: String },

    #[snafu(display("credential secret '{}' missing required key '{}'", secret_name, key))]
    CredentialSecretMissingKey { secret_name: String, key: String },

    #[snafu(display(
        "credential secret '{}' has invalid data encoding for key '{}'",
        secret_name,
        key
    ))]
    CredentialSecretInvalidEncoding { secret_name: String, key: String },

    #[snafu(display(
        "credential secret '{}' key '{}' must be at least 8 characters (got {} characters)",
        secret_name,
        key,
        length
    ))]
    CredentialSecretTooShort {
        secret_name: String,
        key: String,
        length: usize,
    },

    #[snafu(display("KMS secret '{}' not found", name))]
    KmsSecretNotFound { name: String },

    #[snafu(display("KMS secret '{}' missing required key '{}'", secret_name, key))]
    KmsSecretMissingKey { secret_name: String, key: String },

    #[snafu(display("KMS configuration invalid: {}", message))]
    KmsConfigInvalid { message: String },

    #[snafu(transparent)]
    Serde { source: serde_json::Error },
}

fn local_kms_key_directory(
    local: Option<&types::v1alpha1::encryption::LocalKmsConfig>,
    pools: &[types::v1alpha1::pool::Pool],
) -> String {
    local
        .and_then(|l| l.key_directory.clone())
        .unwrap_or_else(|| {
            let base_path = pools
                .first()
                .and_then(|pool| pool.persistence.path.as_deref());
            types::v1alpha1::persistence::default_local_kms_key_directory(base_path)
        })
}

fn normalize_absolute_path(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }

    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) => return None,
        }
    }

    Some(normalized)
}

fn local_kms_key_dir_is_on_data_volume(
    key_dir: &str,
    pools: &[types::v1alpha1::pool::Pool],
) -> bool {
    let Some(key_dir) = normalize_absolute_path(key_dir) else {
        return false;
    };

    pools.iter().any(|pool| {
        (0..pool.persistence.volumes_per_server).any(|shard| {
            let mount_path = types::v1alpha1::persistence::data_volume_mount_path(
                pool.persistence.path.as_deref(),
                shard,
            );
            normalize_absolute_path(&mount_path)
                .is_some_and(|mount_path| key_dir != mount_path && key_dir.starts_with(mount_path))
        })
    })
}

fn local_kms_data_volume_hint(pools: &[types::v1alpha1::pool::Pool]) -> String {
    pools
        .first()
        .map(|pool| {
            types::v1alpha1::persistence::default_local_kms_key_directory(
                pool.persistence.path.as_deref(),
            )
        })
        .unwrap_or_else(|| types::v1alpha1::persistence::default_local_kms_key_directory(None))
}

fn local_kms_legacy_path_migration_message(
    key_dir: &str,
    pools: &[types::v1alpha1::pool::Pool],
) -> String {
    let target_dir = local_kms_data_volume_hint(pools);
    format!(
        "Local KMS keyDirectory '{}' is the legacy non-PVC path. Copy existing key files and .master-key.salt into '{}', then set spec.encryption.local.keyDirectory to that PVC-backed subdirectory before rolling RustFS pods.",
        key_dir, target_dir
    )
}

fn reserved_kms_env_var_name(tenant: &Tenant) -> Option<&str> {
    tenant
        .spec
        .env
        .iter()
        .find(|env| env.name.starts_with("RUSTFS_KMS_"))
        .map(|env| env.name.as_str())
}

fn validate_no_reserved_kms_env(tenant: &Tenant) -> Result<(), Error> {
    if let Some(env_name) = reserved_kms_env_var_name(tenant) {
        return Err(Error::KmsConfigInvalid {
            message: format!(
                "spec.env must not set reserved KMS environment variable '{}'. Configure KMS through spec.encryption so the operator can validate persistence, migration, and Secret references.",
                env_name
            ),
        });
    }
    Ok(())
}

/// Validates Local KMS: absolute persistent `keyDirectory` and at most one server replica across pools.
fn validate_local_kms_tenant(
    local: Option<&types::v1alpha1::encryption::LocalKmsConfig>,
    pools: &[types::v1alpha1::pool::Pool],
) -> Result<(), Error> {
    let key_dir = local_kms_key_directory(local, pools);
    if !key_dir.starts_with('/') {
        return Err(Error::KmsConfigInvalid {
            message: format!(
                "Local KMS keyDirectory must be an absolute path (got \"{}\")",
                key_dir
            ),
        });
    }
    if key_dir == types::v1alpha1::persistence::LEGACY_LOCAL_KMS_KEY_DIR {
        return Err(Error::KmsConfigInvalid {
            message: local_kms_legacy_path_migration_message(&key_dir, pools),
        });
    }
    if !local_kms_key_dir_is_on_data_volume(&key_dir, pools) {
        return Err(Error::KmsConfigInvalid {
            message: format!(
                "Local KMS keyDirectory must be in a subdirectory under a RustFS data PVC mount (got \"{}\"). Use the default or a path such as \"{}\".",
                key_dir,
                local_kms_data_volume_hint(pools)
            ),
        });
    }
    let total_servers: i32 = pools.iter().map(|p| p.servers).sum();
    if total_servers > 1 {
        return Err(Error::KmsConfigInvalid {
            message: "Local KMS is only supported when the tenant has a single RustFS server replica (sum of pool servers must be 1). For multiple servers use Vault KMS, or use a single-server pool.".to_string(),
        });
    }
    Ok(())
}

fn validate_local_kms_master_key_ref(
    local: Option<&types::v1alpha1::encryption::LocalKmsConfig>,
) -> Result<Option<&LocalKmsMasterKeySecretRef>, Error> {
    let allow_insecure_dev_defaults = local.is_some_and(|l| l.allow_insecure_dev_defaults);
    let selector = local.and_then(|l| l.master_key_secret_ref.as_ref());

    let Some(selector) = selector else {
        if allow_insecure_dev_defaults {
            return Ok(None);
        }
        return Err(Error::KmsConfigInvalid {
            message: "Local KMS requires spec.encryption.local.masterKeySecretRef unless spec.encryption.local.allowInsecureDevDefaults is true for development-only plaintext key storage".to_string(),
        });
    };

    if selector.name.is_empty() {
        return Err(Error::KmsConfigInvalid {
            message: "Local KMS masterKeySecretRef.name must not be empty".to_string(),
        });
    }
    if selector.key.is_empty() {
        return Err(Error::KmsConfigInvalid {
            message: "Local KMS masterKeySecretRef.key must not be empty".to_string(),
        });
    }
    Ok(Some(selector))
}

fn validate_secret_utf8_non_blank(
    secret: &Secret,
    secret_name: &str,
    key: &str,
) -> Result<(), Error> {
    let Some(value) = secret.data.as_ref().and_then(|data| data.get(key)) else {
        return KmsSecretMissingKeySnafu {
            secret_name: secret_name.to_string(),
            key: key.to_string(),
        }
        .fail();
    };

    let value = std::str::from_utf8(&value.0).map_err(|_| Error::KmsConfigInvalid {
        message: format!(
            "KMS Secret '{}' key '{}' must contain valid UTF-8",
            secret_name, key
        ),
    })?;
    if value.trim().is_empty() {
        return Err(Error::KmsConfigInvalid {
            message: format!(
                "KMS Secret '{}' key '{}' must not be blank",
                secret_name, key
            ),
        });
    }

    Ok(())
}

fn status_semantically_equal(
    current: Option<&types::v1alpha1::status::Status>,
    next: &types::v1alpha1::status::Status,
) -> bool {
    let Some(current) = current else {
        return false;
    };

    let mut current = current.clone();
    let mut next = next.clone();
    normalize_status_for_compare(&mut current);
    normalize_status_for_compare(&mut next);
    current == next
}

fn normalize_status_for_compare(status: &mut types::v1alpha1::status::Status) {
    for pool in &mut status.pools {
        pool.last_update_time = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SecretValidationKind {
    Credential,
    Kms,
}

pub(crate) fn is_kube_not_found(error: &Error) -> bool {
    matches!(
        error,
        Error::Kube {
            source: kube::Error::Api(response),
        } if response.code == 404
    )
}

pub(crate) fn map_secret_get_error(
    error: Error,
    name: String,
    kind: SecretValidationKind,
) -> Error {
    if !is_kube_not_found(&error) {
        return error;
    }

    match kind {
        SecretValidationKind::Credential => Error::CredentialSecretNotFound { name },
        SecretValidationKind::Kms => Error::KmsSecretNotFound { name },
    }
}

pub struct Context {
    pub(crate) client: kube::Client,
    pub(crate) recorder: Recorder,
}

impl Context {
    pub fn new(client: kube::Client) -> Self {
        let reporter = Reporter {
            controller: "rustfs-operator".into(),
            instance: std::env::var("HOSTNAME").ok(),
        };

        let recorder = Recorder::new(client.clone(), reporter);
        Self { client, recorder }
    }

    /// send event
    #[inline]
    pub async fn record(
        &self,
        resource: &Tenant,
        event_type: EventType,
        reason: &str,
        message: &str,
    ) -> Result<(), Error> {
        self.recorder
            .publish(
                &Event {
                    type_: event_type,
                    reason: reason.to_owned(),
                    note: Some(message.into()),
                    action: "Reconcile".into(),
                    secondary: None,
                },
                &resource.object_ref(&()),
            )
            .context(RecordSnafu)
            .await
    }

    pub async fn update_status(
        &self,
        resource: &Tenant,
        status: crate::types::v1alpha1::status::Status,
    ) -> Result<Tenant, Error> {
        use kube::api::{Patch, PatchParams};

        let namespace = resource.namespace()?;
        let api: Api<Tenant> = Api::namespaced(self.client.clone(), &namespace);
        let name = resource.name();

        // Create a JSON merge patch for the status
        let status_patch = serde_json::json!({
            "status": status
        });

        // Try to patch the status
        match api
            .patch_status(
                &name,
                &PatchParams::default(),
                &Patch::Merge(status_patch.clone()),
            )
            .context(KubeSnafu)
            .await
        {
            Ok(t) => return Ok(t),
            Err(error) => {
                info!(
                    tenant = %name,
                    namespace = %namespace,
                    %error,
                    "status update failed; retrying status patch"
                );
            }
        }

        // Retry with the same patch
        api.patch_status(&name, &PatchParams::default(), &Patch::Merge(status_patch))
            .context(KubeSnafu)
            .await
    }

    pub async fn patch_status_if_changed(
        &self,
        resource: &Tenant,
        status: crate::types::v1alpha1::status::Status,
    ) -> Result<Option<Tenant>, Error> {
        if status_semantically_equal(resource.status.as_ref(), &status) {
            return Ok(None);
        }

        self.update_status(resource, status).await.map(Some)
    }

    pub async fn delete<T>(&self, name: &str, namespace: &str) -> Result<(), Error>
    where
        T: Resource<Scope = NamespaceResourceScope> + Clone + DeserializeOwned + Debug,
        <T as kube::Resource>::DynamicType: Default,
    {
        self.delete_with_params::<T>(name, namespace, &DeleteParams::default())
            .await
    }

    pub async fn delete_with_params<T>(
        &self,
        name: &str,
        namespace: &str,
        params: &DeleteParams,
    ) -> Result<(), Error>
    where
        T: Resource<Scope = NamespaceResourceScope> + Clone + DeserializeOwned + Debug,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.delete(name, params).context(KubeSnafu).await?;
        Ok(())
    }

    pub async fn get<T>(&self, name: &str, namespace: &str) -> Result<T, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.get(name).context(KubeSnafu).await
    }

    pub async fn create<T>(&self, resource: &T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.create(&PostParams::default(), resource)
            .context(KubeSnafu)
            .await
    }

    pub async fn list<T>(&self, namespace: &str) -> Result<ObjectList<T>, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.list(&ListParams::default()).context(KubeSnafu).await
    }

    pub async fn list_with_params<T>(
        &self,
        namespace: &str,
        params: &ListParams,
    ) -> Result<ObjectList<T>, Error>
    where
        T: Clone + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.list(params).context(KubeSnafu).await
    }

    pub async fn apply<T>(&self, resource: &T, namespace: &str) -> Result<T, Error>
    where
        T: Clone + Serialize + DeserializeOwned + Debug + Resource<Scope = NamespaceResourceScope>,
        <T as kube::Resource>::DynamicType: Default,
    {
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.patch(
            &resource.name_any(),
            &PatchParams::apply("rustfs-operator"),
            &Patch::Apply(resource),
        )
        .context(KubeSnafu)
        .await
    }

    /// Validates that a credential Secret exists and contains required keys.
    ///
    /// This function only validates the Secret structure when `spec.credsSecret` is configured.
    /// It does NOT extract credential values - that's handled by Kubernetes at pod startup
    /// via `secretKeyRef` in the StatefulSet environment variables.
    ///
    /// # Validation Rules
    /// - Secret must exist in the same namespace as the Tenant
    /// - Secret must contain both `accesskey` and `secretkey` keys
    /// - Both keys must be valid UTF-8 strings
    /// - Both keys must be at least 8 characters long
    ///
    /// # Returns
    /// - `Ok(())` if Secret is valid or not configured
    /// - `Err(...)` if Secret is configured but invalid (not found, missing keys, invalid encoding, too short)
    ///
    /// # Note
    /// If no credentials are provided via Secret or environment variables, RustFS will use
    /// its built-in defaults (`rustfsadmin`/`rustfsadmin`).
    /// **This is acceptable for development but should be changed for production.**
    pub async fn validate_credential_secret(&self, tenant: &Tenant) -> Result<(), Error> {
        // Only validate if credsSecret is configured
        if let Some(ref cfg) = tenant.spec.creds_secret
            && !cfg.name.is_empty()
        {
            let secret: Secret = match self.get(&cfg.name, &tenant.namespace()?).await {
                Ok(secret) => secret,
                Err(error) => {
                    return Err(map_secret_get_error(
                        error,
                        cfg.name.clone(),
                        SecretValidationKind::Credential,
                    ));
                }
            };

            // Validate Secret has required keys
            if let Some(data) = secret.data {
                let access_key = "accesskey".to_string();
                let secret_key = "secretkey".to_string();

                // Validate accesskey exists, is valid UTF-8, and meets minimum length
                if let Some(accesskey_bytes) = data.get(&access_key) {
                    let accesskey = String::from_utf8(accesskey_bytes.0.clone()).map_err(|_| {
                        Error::CredentialSecretInvalidEncoding {
                            secret_name: cfg.name.clone(),
                            key: access_key.clone(),
                        }
                    })?;

                    if accesskey.len() < 8 {
                        return CredentialSecretTooShortSnafu {
                            secret_name: cfg.name.clone(),
                            key: access_key.clone(),
                            length: accesskey.len(),
                        }
                        .fail();
                    }
                } else {
                    return CredentialSecretMissingKeySnafu {
                        secret_name: cfg.name.clone(),
                        key: access_key,
                    }
                    .fail();
                }

                // Validate secretkey exists, is valid UTF-8, and meets minimum length
                if let Some(secretkey_bytes) = data.get(&secret_key) {
                    let secretkey = String::from_utf8(secretkey_bytes.0.clone()).map_err(|_| {
                        Error::CredentialSecretInvalidEncoding {
                            secret_name: cfg.name.clone(),
                            key: secret_key.clone(),
                        }
                    })?;

                    if secretkey.len() < 8 {
                        return CredentialSecretTooShortSnafu {
                            secret_name: cfg.name.clone(),
                            key: secret_key.clone(),
                            length: secretkey.len(),
                        }
                        .fail();
                    }
                } else {
                    return CredentialSecretMissingKeySnafu {
                        secret_name: cfg.name.clone(),
                        key: secret_key,
                    }
                    .fail();
                }
            }
        }

        Ok(())
    }

    /// Validates encryption configuration and the KMS Secret.
    ///
    /// Checks:
    /// 1. Local KMS: absolute key directory, single replica, and a local master key Secret
    ///    unless explicit development-only insecure defaults are enabled.
    /// 2. Vault endpoint is non-empty when backend is Vault.
    /// 3. KMS Secret exists and contains the correct keys for the auth type.
    pub async fn validate_kms_secret(&self, tenant: &Tenant) -> Result<(), Error> {
        use crate::types::v1alpha1::encryption::KmsBackendType;

        validate_no_reserved_kms_env(tenant)?;

        let Some(ref enc) = tenant.spec.encryption else {
            return Ok(());
        };
        if !enc.enabled {
            return Ok(());
        }

        if enc.backend == KmsBackendType::Local {
            validate_local_kms_tenant(enc.local.as_ref(), &tenant.spec.pools)?;
            if let Some(selector) = validate_local_kms_master_key_ref(enc.local.as_ref())? {
                let secret: Secret = match self.get(&selector.name, &tenant.namespace()?).await {
                    Ok(secret) => secret,
                    Err(error) => {
                        return Err(map_secret_get_error(
                            error,
                            selector.name.clone(),
                            SecretValidationKind::Kms,
                        ));
                    }
                };
                validate_secret_utf8_non_blank(&secret, &selector.name, &selector.key)?;
            }
            return Ok(());
        } else if enc.backend == KmsBackendType::Vault {
            // Vault: non-empty endpoint and `kmsSecret` with `vault-token` (RustFS `build_vault_kms_config`).
            let endpoint_empty = enc
                .vault
                .as_ref()
                .map(|v| v.endpoint.is_empty())
                .unwrap_or(true);
            if endpoint_empty {
                return Err(Error::KmsConfigInvalid {
                    message: "Vault endpoint must not be empty".to_string(),
                });
            }
            let secret_missing = enc
                .kms_secret
                .as_ref()
                .map(|s| s.name.is_empty())
                .unwrap_or(true);
            if secret_missing {
                return Err(Error::KmsConfigInvalid {
                    message:
                        "Vault backend requires kmsSecret referencing a Secret with key vault-token"
                            .to_string(),
                });
            }

            let Some(secret_ref) = enc.kms_secret.as_ref() else {
                return Err(Error::KmsConfigInvalid {
                    message:
                        "Vault backend requires kmsSecret referencing a Secret with key vault-token"
                            .to_string(),
                });
            };
            let secret: Secret = match self.get(&secret_ref.name, &tenant.namespace()?).await {
                Ok(secret) => secret,
                Err(error) => {
                    return Err(map_secret_get_error(
                        error,
                        secret_ref.name.clone(),
                        SecretValidationKind::Kms,
                    ));
                }
            };

            let has_token = secret
                .data
                .as_ref()
                .is_some_and(|d| d.contains_key("vault-token"));
            if !has_token {
                return KmsSecretMissingKeySnafu {
                    secret_name: secret_ref.name.clone(),
                    key: "vault-token".to_string(),
                }
                .fail();
            }
        }

        Ok(())
    }

    /// Gets the status of a StatefulSet including rollout progress
    ///
    /// # Returns
    /// The StatefulSet status with replica counts and revision information
    pub async fn get_statefulset_status(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<k8s_openapi::api::apps::v1::StatefulSetStatus, Error> {
        let ss: k8s_openapi::api::apps::v1::StatefulSet = self.get(name, namespace).await?;

        ss.status.ok_or_else(|| Error::Types {
            source: types::error::Error::InternalError {
                msg: format!("StatefulSet {} has no status", name),
            },
        })
    }

    /// Checks if a StatefulSet rollout is complete
    ///
    /// A rollout is considered complete when:
    /// - observedGeneration matches metadata.generation (controller has seen latest spec)
    /// - replicas == readyReplicas (all pods are ready)
    /// - currentRevision == updateRevision (all pods are on the new revision)
    /// - updatedReplicas == replicas (all pods have been updated)
    ///
    /// # Returns
    /// - `Ok(true)` if rollout is complete
    /// - `Ok(false)` if rollout is still in progress
    /// - `Err` if there's an error fetching the StatefulSet
    pub async fn is_rollout_complete(&self, name: &str, namespace: &str) -> Result<bool, Error> {
        let ss: k8s_openapi::api::apps::v1::StatefulSet = self.get(name, namespace).await?;

        let metadata = &ss.metadata;
        let spec = ss.spec.as_ref().ok_or_else(|| Error::Types {
            source: types::error::Error::InternalError {
                msg: format!("StatefulSet {} missing spec", name),
            },
        })?;

        let status = ss.status.as_ref().ok_or_else(|| Error::Types {
            source: types::error::Error::InternalError {
                msg: format!("StatefulSet {} missing status", name),
            },
        })?;

        let desired_replicas = spec.replicas.unwrap_or(1);

        // Check if controller has observed the latest generation
        let generation_current = metadata.generation.is_some()
            && status.observed_generation.is_some()
            && metadata.generation == status.observed_generation;

        // Check if all replicas are ready
        let replicas_ready = status.replicas == desired_replicas
            && status.ready_replicas.unwrap_or(0) == desired_replicas
            && status.updated_replicas.unwrap_or(0) == desired_replicas;

        // Check if all pods are on the same revision
        let revisions_match = status.current_revision.is_some()
            && status.update_revision.is_some()
            && status.current_revision == status.update_revision;

        Ok(generation_current && replicas_ready && revisions_match)
    }

    /// Gets the current and update revision of a StatefulSet
    ///
    /// # Returns
    /// A tuple of (current_revision, update_revision)
    /// Returns None for either value if not available
    pub async fn get_statefulset_revisions(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<(Option<String>, Option<String>), Error> {
        let status = self.get_statefulset_status(name, namespace).await?;

        Ok((status.current_revision, status.update_revision))
    }
}

#[cfg(test)]
mod validate_local_kms_tests {
    use super::Error;
    use super::{SecretValidationKind, map_secret_get_error};
    use super::{
        validate_local_kms_master_key_ref, validate_local_kms_tenant, validate_no_reserved_kms_env,
        validate_secret_utf8_non_blank,
    };
    use crate::types::v1alpha1::encryption::{LocalKmsConfig, LocalKmsMasterKeySecretRef};
    use crate::types::v1alpha1::persistence::PersistenceConfig;
    use crate::types::v1alpha1::pool::Pool;
    use k8s_openapi::ByteString;
    use k8s_openapi::api::core::v1 as corev1;
    use std::collections::BTreeMap;

    fn pool(servers: i32) -> Pool {
        pool_with_path(servers, None)
    }

    fn pool_with_path(servers: i32, path: Option<&str>) -> Pool {
        Pool {
            name: "p".to_string(),
            servers,
            persistence: PersistenceConfig {
                volumes_per_server: 4,
                path: path.map(ToOwned::to_owned),
                ..Default::default()
            },
            scheduling: Default::default(),
        }
    }

    fn api_error(code: u16, reason: &str) -> Error {
        Error::Kube {
            source: kube::Error::Api(kube::error::ErrorResponse {
                status: "Failure".to_string(),
                message: reason.to_string(),
                reason: reason.to_string(),
                code,
            }),
        }
    }

    fn master_key_selector() -> LocalKmsMasterKeySecretRef {
        LocalKmsMasterKeySecretRef {
            name: "local-kms-master-key".to_string(),
            key: "local-master-key".to_string(),
        }
    }

    #[test]
    fn credential_secret_get_maps_only_404_to_not_found() {
        let err = map_secret_get_error(
            api_error(404, "NotFound"),
            "creds".to_string(),
            SecretValidationKind::Credential,
        );

        assert!(matches!(err, Error::CredentialSecretNotFound { name } if name == "creds"));
    }

    #[test]
    fn credential_secret_get_preserves_forbidden_as_kube_error() {
        let err = map_secret_get_error(
            api_error(403, "Forbidden"),
            "creds".to_string(),
            SecretValidationKind::Credential,
        );

        assert!(
            matches!(err, Error::Kube { source: kube::Error::Api(response) } if response.code == 403)
        );
    }

    #[test]
    fn kms_secret_get_maps_only_404_to_not_found() {
        let err = map_secret_get_error(
            api_error(404, "NotFound"),
            "kms".to_string(),
            SecretValidationKind::Kms,
        );

        assert!(matches!(err, Error::KmsSecretNotFound { name } if name == "kms"));
    }

    #[test]
    fn local_kms_default_key_dir_ok_single_replica() {
        validate_local_kms_tenant(None, &[pool(1)]).unwrap();
    }

    #[test]
    fn local_kms_accepts_key_dir_on_data_volume() {
        let local = LocalKmsConfig {
            key_directory: Some("/data/rustfs0/kms".to_string()),
            ..Default::default()
        };
        validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap();
    }

    #[test]
    fn local_kms_accepts_key_dir_on_custom_data_volume() {
        let local = LocalKmsConfig {
            key_directory: Some("/mnt/rustfs/rustfs0/kms".to_string()),
            ..Default::default()
        };
        validate_local_kms_tenant(Some(&local), &[pool_with_path(1, Some("/mnt/rustfs"))]).unwrap();
    }

    #[test]
    fn local_kms_rejects_relative_key_dir() {
        let local = LocalKmsConfig {
            key_directory: Some("data/kms".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { .. }));
    }

    #[test]
    fn local_kms_rejects_key_dir_outside_data_volume() {
        let local = LocalKmsConfig {
            key_directory: Some("/opt/rustfs-kms".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(
            matches!(err, Error::KmsConfigInvalid { message } if message.contains("data PVC mount"))
        );
    }

    #[test]
    fn local_kms_rejects_legacy_key_dir_with_migration_guidance() {
        let local = LocalKmsConfig {
            key_directory: Some("/data/kms-keys".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { message }
                if message.contains("legacy non-PVC path")
                    && message.contains("/data/rustfs0/.kms-keys")
                    && message.contains(".master-key.salt")
                    && message.contains("spec.encryption.local.keyDirectory")));
    }

    #[test]
    fn local_kms_rejects_data_volume_root() {
        let local = LocalKmsConfig {
            key_directory: Some("/data/rustfs0".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(
            matches!(err, Error::KmsConfigInvalid { message } if message.contains("subdirectory"))
        );
    }

    #[test]
    fn local_kms_rejects_prefix_match_outside_data_volume() {
        let local = LocalKmsConfig {
            key_directory: Some("/data/rustfs01/kms".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(
            matches!(err, Error::KmsConfigInvalid { message } if message.contains("data PVC mount"))
        );
    }

    #[test]
    fn local_kms_rejects_multi_pool_multi_replica() {
        let local = LocalKmsConfig::default();
        let err = validate_local_kms_tenant(Some(&local), &[pool(2), pool(2)]).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { .. }));
    }

    #[test]
    fn local_kms_requires_master_key_secret_ref_by_default() {
        let local = LocalKmsConfig::default();
        let err = validate_local_kms_master_key_ref(Some(&local)).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { message }
                if message.contains("masterKeySecretRef")));
    }

    #[test]
    fn local_kms_allows_explicit_insecure_dev_defaults_without_master_key() {
        let local = LocalKmsConfig {
            allow_insecure_dev_defaults: true,
            ..Default::default()
        };

        let selector = validate_local_kms_master_key_ref(Some(&local)).unwrap();

        assert!(selector.is_none());
    }

    #[test]
    fn local_kms_rejects_empty_master_key_secret_ref_name() {
        let local = LocalKmsConfig {
            master_key_secret_ref: Some(LocalKmsMasterKeySecretRef {
                name: String::new(),
                ..master_key_selector()
            }),
            ..Default::default()
        };

        let err = validate_local_kms_master_key_ref(Some(&local)).unwrap_err();

        assert!(matches!(err, Error::KmsConfigInvalid { message }
                if message.contains("name must not be empty")));
    }

    #[test]
    fn local_kms_secret_key_must_be_utf8_and_non_blank() {
        let mut data = BTreeMap::new();
        data.insert("blank".to_string(), ByteString(b"   \n".to_vec()));
        data.insert("invalid".to_string(), ByteString(vec![0xff]));
        let secret = corev1::Secret {
            data: Some(data),
            ..Default::default()
        };

        let missing =
            validate_secret_utf8_non_blank(&secret, "local-kms-master-key", "missing").unwrap_err();
        assert!(matches!(missing, Error::KmsSecretMissingKey { key, .. } if key == "missing"));

        let invalid =
            validate_secret_utf8_non_blank(&secret, "local-kms-master-key", "invalid").unwrap_err();
        assert!(matches!(invalid, Error::KmsConfigInvalid { message }
                if message.contains("valid UTF-8")));

        let blank =
            validate_secret_utf8_non_blank(&secret, "local-kms-master-key", "blank").unwrap_err();
        assert!(matches!(blank, Error::KmsConfigInvalid { message }
                if message.contains("must not be blank")));
    }

    #[test]
    fn local_kms_secret_key_accepts_non_empty_data() {
        let mut data = BTreeMap::new();
        data.insert(
            "local-master-key".to_string(),
            ByteString(b"test-master-key".to_vec()),
        );
        let secret = corev1::Secret {
            data: Some(data),
            ..Default::default()
        };

        validate_secret_utf8_non_blank(&secret, "local-kms-master-key", "local-master-key")
            .unwrap();
    }

    #[test]
    fn reserved_kms_env_is_rejected_without_encryption_spec() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.encryption = None;
        tenant.spec.env = vec![corev1::EnvVar {
            name: "RUSTFS_KMS_ENABLE".to_string(),
            value: Some("true".to_string()),
            ..Default::default()
        }];

        let err = validate_no_reserved_kms_env(&tenant).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { message }
                if message.contains("RUSTFS_KMS_ENABLE")
                    && message.contains("spec.encryption")));
    }

    #[test]
    fn reserved_kms_env_rejects_unmodelled_rustfs_kms_vars() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.env = vec![corev1::EnvVar {
            name: "RUSTFS_KMS_LOCAL_MASTER_KEY".to_string(),
            value: Some("secret".to_string()),
            ..Default::default()
        }];

        let err = validate_no_reserved_kms_env(&tenant).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { message }
                if message.contains("RUSTFS_KMS_LOCAL_MASTER_KEY")));
    }
}
