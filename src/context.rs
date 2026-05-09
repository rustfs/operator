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

/// Validates Local KMS: absolute `keyDirectory` and at most one server replica across pools.
fn validate_local_kms_tenant(
    local: Option<&types::v1alpha1::encryption::LocalKmsConfig>,
    pools: &[types::v1alpha1::pool::Pool],
) -> Result<(), Error> {
    let key_dir = local
        .and_then(|l| l.key_directory.as_deref())
        .unwrap_or("/data/kms-keys");
    if !key_dir.starts_with('/') {
        return Err(Error::KmsConfigInvalid {
            message: format!(
                "Local KMS keyDirectory must be an absolute path (got \"{}\")",
                key_dir
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

        let api: Api<Tenant> = Api::namespaced(self.client.clone(), &resource.namespace()?);
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
            _ => {}
        }

        info!("status update failed due to conflict, retrieve the latest resource and retry.");

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
        let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
        api.delete(name, &DeleteParams::default())
            .context(KubeSnafu)
            .await?;
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
    /// 1. Local KMS: absolute key directory and single replica (sum of pool servers).
    /// 2. Vault endpoint is non-empty when backend is Vault.
    /// 3. KMS Secret exists and contains the correct keys for the auth type.
    pub async fn validate_kms_secret(&self, tenant: &Tenant) -> Result<(), Error> {
        use crate::types::v1alpha1::encryption::KmsBackendType;

        let Some(ref enc) = tenant.spec.encryption else {
            return Ok(());
        };
        if !enc.enabled {
            return Ok(());
        }

        // Local KMS: RustFS requires an absolute key directory; multi-replica tenants need Vault
        // (or a shared filesystem) because each Pod would otherwise have its own key files.
        if enc.backend == KmsBackendType::Local {
            validate_local_kms_tenant(enc.local.as_ref(), &tenant.spec.pools)?;
        }

        // Vault: non-empty endpoint and `kmsSecret` with `vault-token` (RustFS `build_vault_kms_config`).
        if enc.backend == KmsBackendType::Vault {
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
        }

        let Some(ref secret_ref) = enc.kms_secret else {
            return Ok(());
        };
        if secret_ref.name.is_empty() {
            return Ok(());
        }

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

        if enc.backend == KmsBackendType::Vault {
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
    use super::validate_local_kms_tenant;
    use super::{SecretValidationKind, map_secret_get_error};
    use crate::types::v1alpha1::encryption::LocalKmsConfig;
    use crate::types::v1alpha1::persistence::PersistenceConfig;
    use crate::types::v1alpha1::pool::Pool;

    fn pool(servers: i32) -> Pool {
        Pool {
            name: "p".to_string(),
            servers,
            persistence: PersistenceConfig {
                volumes_per_server: 4,
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
    fn local_kms_rejects_relative_key_dir() {
        let local = LocalKmsConfig {
            key_directory: Some("data/kms".to_string()),
            ..Default::default()
        };
        let err = validate_local_kms_tenant(Some(&local), &[pool(1)]).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { .. }));
    }

    #[test]
    fn local_kms_rejects_multi_pool_multi_replica() {
        let local = LocalKmsConfig::default();
        let err = validate_local_kms_tenant(Some(&local), &[pool(2), pool(2)]).unwrap_err();
        assert!(matches!(err, Error::KmsConfigInvalid { .. }));
    }
}
