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

use crate::types::v1alpha1::tenant::Tenant;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher::Event;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct NamespacedName {
    namespace: String,
    name: String,
}

impl NamespacedName {
    fn new(namespace: &str, name: &str) -> Option<Self> {
        if namespace.is_empty() || name.is_empty() {
            return None;
        }

        Some(Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TenantReferences {
    config_maps: BTreeSet<NamespacedName>,
    secrets: BTreeSet<NamespacedName>,
}

impl TenantReferences {
    fn from_tenant(tenant: &Tenant) -> Option<(NamespacedName, Self)> {
        let namespace = tenant.metadata.namespace.as_deref()?;
        let tenant_key = NamespacedName::new(namespace, tenant.metadata.name.as_deref()?)?;
        let mut references = Self::default();

        for policy in &tenant.spec.policies {
            references.insert_config_map(namespace, &policy.document.config_map_key_ref.name);
        }
        for env in &tenant.spec.env {
            if let Some(config_map) = env
                .value_from
                .as_ref()
                .and_then(|source| source.config_map_key_ref.as_ref())
            {
                references.insert_config_map(namespace, &config_map.name);
            }
        }
        for secret_name in tenant.spec.referenced_secret_names() {
            references.insert_secret(namespace, &secret_name);
        }

        Some((tenant_key, references))
    }

    fn insert_config_map(&mut self, namespace: &str, name: &str) {
        if let Some(key) = NamespacedName::new(namespace, name) {
            self.config_maps.insert(key);
        }
    }

    fn insert_secret(&mut self, namespace: &str, name: &str) {
        if let Some(key) = NamespacedName::new(namespace, name) {
            self.secrets.insert(key);
        }
    }
}

#[derive(Debug, Default)]
struct ReferenceIndexState {
    by_tenant: BTreeMap<NamespacedName, TenantReferences>,
    config_maps: BTreeMap<NamespacedName, BTreeSet<NamespacedName>>,
    secrets: BTreeMap<NamespacedName, BTreeSet<NamespacedName>>,
}

impl ReferenceIndexState {
    fn upsert(&mut self, tenant: &Tenant) {
        if tenant.metadata.deletion_timestamp.is_some() {
            self.remove_tenant(tenant);
            return;
        }

        let Some((tenant_key, references)) = TenantReferences::from_tenant(tenant) else {
            return;
        };

        if self.by_tenant.get(&tenant_key) == Some(&references) {
            return;
        }

        self.remove(&tenant_key);
        for key in &references.config_maps {
            self.config_maps
                .entry(key.clone())
                .or_default()
                .insert(tenant_key.clone());
        }
        for key in &references.secrets {
            self.secrets
                .entry(key.clone())
                .or_default()
                .insert(tenant_key.clone());
        }
        self.by_tenant.insert(tenant_key, references);
    }

    fn remove_tenant(&mut self, tenant: &Tenant) {
        let Some(namespace) = tenant.metadata.namespace.as_deref() else {
            return;
        };
        let Some(name) = tenant.metadata.name.as_deref() else {
            return;
        };
        let Some(tenant_key) = NamespacedName::new(namespace, name) else {
            return;
        };

        self.remove(&tenant_key);
    }

    fn remove(&mut self, tenant_key: &NamespacedName) {
        let Some(references) = self.by_tenant.remove(tenant_key) else {
            return;
        };

        for key in references.config_maps {
            remove_reverse_reference(&mut self.config_maps, &key, tenant_key);
        }
        for key in references.secrets {
            remove_reverse_reference(&mut self.secrets, &key, tenant_key);
        }
    }

    fn refs_for(
        references: &BTreeMap<NamespacedName, BTreeSet<NamespacedName>>,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Vec<ObjectRef<Tenant>> {
        let (Some(namespace), Some(name)) = (namespace, name) else {
            return Vec::new();
        };
        let Some(key) = NamespacedName::new(namespace, name) else {
            return Vec::new();
        };

        references
            .get(&key)
            .into_iter()
            .flatten()
            .map(|tenant| ObjectRef::new(&tenant.name).within(&tenant.namespace))
            .collect()
    }
}

fn remove_reverse_reference(
    references: &mut BTreeMap<NamespacedName, BTreeSet<NamespacedName>>,
    resource_key: &NamespacedName,
    tenant_key: &NamespacedName,
) {
    let remove_entry = references.get_mut(resource_key).is_some_and(|tenants| {
        tenants.remove(tenant_key);
        tenants.is_empty()
    });
    if remove_entry {
        references.remove(resource_key);
    }
}

#[derive(Debug, Default)]
struct IndexStates {
    active: ReferenceIndexState,
    initializing: Option<ReferenceIndexState>,
}

/// Maintains reverse references from user-owned resources to the Tenants that consume them.
///
/// A relist is built separately and swapped into service only after `InitDone`, so watch
/// reconnects never expose a partially rebuilt index.
#[derive(Debug, Default)]
pub(crate) struct TenantReferenceIndex {
    states: RwLock<IndexStates>,
}

impl TenantReferenceIndex {
    pub(crate) fn apply_event(&self, event: &Event<Tenant>) {
        let mut states = self
            .states
            .write()
            .unwrap_or_else(|error| error.into_inner());
        match event {
            Event::Apply(tenant) => states.active.upsert(tenant),
            Event::Delete(tenant) => states.active.remove_tenant(tenant),
            Event::Init => states.initializing = Some(ReferenceIndexState::default()),
            Event::InitApply(tenant) => {
                if let Some(initializing) = states.initializing.as_mut() {
                    initializing.upsert(tenant);
                } else {
                    // Be defensive if a non-conforming stream emits InitApply without Init.
                    states.active.upsert(tenant);
                }
            }
            Event::InitDone => {
                if let Some(initializing) = states.initializing.take() {
                    states.active = initializing;
                }
            }
        }
    }

    pub(crate) fn refs_for_config_map(
        &self,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Vec<ObjectRef<Tenant>> {
        let states = self
            .states
            .read()
            .unwrap_or_else(|error| error.into_inner());
        ReferenceIndexState::refs_for(&states.active.config_maps, namespace, name)
    }

    pub(crate) fn refs_for_secret(
        &self,
        namespace: Option<&str>,
        name: Option<&str>,
    ) -> Vec<ObjectRef<Tenant>> {
        let states = self
            .states
            .read()
            .unwrap_or_else(|error| error.into_inner());
        ReferenceIndexState::refs_for(&states.active.secrets, namespace, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::v1alpha1::encryption::{
        EncryptionConfig, LocalKmsConfig, LocalKmsMasterKeySecretRef,
    };
    use crate::types::v1alpha1::provisioning::{
        ConfigMapKeyReference, PolicyDocumentSource, ProvisioningPolicy, ProvisioningUser,
        UserCredentialsSecretRef,
    };
    use crate::types::v1alpha1::tenant::RpcSecretRef;
    use crate::types::v1alpha1::tls::{
        CaTrustConfig, CertManagerTlsConfig, SecretKeyReference, TlsCertificateConfig, TlsConfig,
    };
    use futures::{FutureExt, StreamExt, TryStreamExt, channel::mpsc};
    use k8s_openapi::api::core::v1 as corev1;
    use k8s_openapi::api::core::v1::LocalObjectReference;
    use kube::runtime::reflector;
    use std::sync::Arc;

    #[test]
    fn index_covers_all_tenant_secret_and_config_map_references() {
        let index = TenantReferenceIndex::default();
        let mut tenant = tenant_fixture("tenant-a", "storage");
        tenant.spec.policies.push(ProvisioningPolicy {
            name: "readwrite".to_string(),
            document: PolicyDocumentSource {
                config_map_key_ref: ConfigMapKeyReference {
                    name: "policy-document".to_string(),
                    key: "policy.json".to_string(),
                },
            },
            ..Default::default()
        });
        tenant.spec.creds_secret = Some(local_ref("credentials"));
        tenant.spec.image_pull_secret = Some(local_ref("image-pull"));
        tenant.spec.rpc_secret = Some(RpcSecretRef {
            name: "rpc".to_string(),
            key: "secret".to_string(),
        });
        tenant.spec.env.push(corev1::EnvVar {
            name: "OIDC_CLIENT_SECRET".to_string(),
            value_from: Some(corev1::EnvVarSource {
                secret_key_ref: Some(corev1::SecretKeySelector {
                    key: "client-secret".to_string(),
                    name: "env-secret".to_string(),
                    optional: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.env.push(corev1::EnvVar {
            name: "RUSTFS_REGION".to_string(),
            value_from: Some(corev1::EnvVarSource {
                config_map_key_ref: Some(corev1::ConfigMapKeySelector {
                    key: "region".to_string(),
                    name: "runtime-settings".to_string(),
                    optional: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.users.push(ProvisioningUser {
            name: "provisioned-user".to_string(),
            creds_secret: Some(UserCredentialsSecretRef {
                name: "provisioned-user-credentials".to_string(),
            }),
            policies: vec!["readwrite".to_string()],
            ..Default::default()
        });
        tenant.spec.users.push(ProvisioningUser {
            name: "legacy-user".to_string(),
            policies: vec!["readwrite".to_string()],
            ..Default::default()
        });
        tenant.spec.encryption = Some(EncryptionConfig {
            kms_secret: Some(local_ref("vault-token")),
            local: Some(LocalKmsConfig {
                master_key_secret_ref: Some(LocalKmsMasterKeySecretRef {
                    name: "local-master-key".to_string(),
                    key: "master-key".to_string(),
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        tenant.spec.tls = Some(TlsConfig {
            ca_trust: Some(ca_trust("external-ca", "external-client-ca")),
            cert_manager: Some(CertManagerTlsConfig {
                secret_name: Some("default-certificate".to_string()),
                ca_trust: Some(ca_trust("default-ca", "default-client-ca")),
                ..Default::default()
            }),
            certificates: vec![TlsCertificateConfig {
                name: "tenant-certificate".to_string(),
                default: true,
                hosts: Vec::new(),
                cert_manager: CertManagerTlsConfig {
                    secret_name: Some("named-certificate".to_string()),
                    ca_trust: Some(ca_trust("named-ca", "named-client-ca")),
                    ..Default::default()
                },
            }],
            ..Default::default()
        });

        index.apply_event(&Event::Apply(tenant));

        assert_single_ref(
            &index.refs_for_config_map(Some("storage"), Some("policy-document")),
            "tenant-a",
            "storage",
        );
        assert_single_ref(
            &index.refs_for_config_map(Some("storage"), Some("runtime-settings")),
            "tenant-a",
            "storage",
        );
        for secret in [
            "credentials",
            "env-secret",
            "image-pull",
            "rpc",
            "provisioned-user-credentials",
            "legacy-user",
            "vault-token",
            "local-master-key",
            "external-ca",
            "external-client-ca",
            "default-certificate",
            "default-ca",
            "default-client-ca",
            "named-certificate",
            "named-ca",
            "named-client-ca",
        ] {
            assert_single_ref(
                &index.refs_for_secret(Some("storage"), Some(secret)),
                "tenant-a",
                "storage",
            );
        }
    }

    #[test]
    fn custom_user_secret_updates_do_not_drop_shared_tenant_references() {
        let index = TenantReferenceIndex::default();
        let mut tenant_a = tenant_fixture("tenant-a", "storage");
        tenant_a
            .spec
            .users
            .push(provisioning_user("user-a", "shared-user-creds"));
        let mut tenant_b = tenant_fixture("tenant-b", "storage");
        tenant_b
            .spec
            .users
            .push(provisioning_user("user-b", "shared-user-creds"));

        index.apply_event(&Event::Apply(tenant_a.clone()));
        index.apply_event(&Event::Apply(tenant_b));
        assert_eq!(
            index
                .refs_for_secret(Some("storage"), Some("shared-user-creds"))
                .len(),
            2
        );

        tenant_a.spec.users[0].creds_secret = Some(UserCredentialsSecretRef {
            name: "new-user-creds".to_string(),
        });
        index.apply_event(&Event::Apply(tenant_a));

        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("shared-user-creds")),
            "tenant-b",
            "storage",
        );
        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("new-user-creds")),
            "tenant-a",
            "storage",
        );
    }

    #[test]
    fn index_scopes_shared_resource_names_by_namespace_and_tenant() {
        let index = TenantReferenceIndex::default();
        for (name, namespace) in [
            ("tenant-a", "storage"),
            ("tenant-b", "storage"),
            ("tenant-c", "other"),
        ] {
            let mut tenant = tenant_fixture(name, namespace);
            tenant.spec.creds_secret = Some(local_ref("shared"));
            index.apply_event(&Event::Apply(tenant));
        }

        let storage_refs = index.refs_for_secret(Some("storage"), Some("shared"));
        assert_eq!(storage_refs.len(), 2);
        assert_eq!(storage_refs[0].name, "tenant-a");
        assert_eq!(storage_refs[1].name, "tenant-b");
        assert_single_ref(
            &index.refs_for_secret(Some("other"), Some("shared")),
            "tenant-c",
            "other",
        );
    }

    #[test]
    fn tenant_update_and_delete_remove_old_reverse_references() {
        let index = TenantReferenceIndex::default();
        let mut tenant = tenant_fixture("tenant-a", "storage");
        tenant.spec.creds_secret = Some(local_ref("old-secret"));
        tenant.spec.policies.push(ProvisioningPolicy {
            name: "readwrite".to_string(),
            document: PolicyDocumentSource {
                config_map_key_ref: ConfigMapKeyReference {
                    name: "old-policy".to_string(),
                    key: "policy.json".to_string(),
                },
            },
            ..Default::default()
        });
        index.apply_event(&Event::Apply(tenant.clone()));

        tenant.spec.creds_secret = Some(local_ref("new-secret"));
        tenant.spec.policies[0].document.config_map_key_ref.name = "new-policy".to_string();
        index.apply_event(&Event::Apply(tenant.clone()));

        assert!(
            index
                .refs_for_secret(Some("storage"), Some("old-secret"))
                .is_empty()
        );
        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("new-secret")),
            "tenant-a",
            "storage",
        );
        assert!(
            index
                .refs_for_config_map(Some("storage"), Some("old-policy"))
                .is_empty()
        );
        assert_single_ref(
            &index.refs_for_config_map(Some("storage"), Some("new-policy")),
            "tenant-a",
            "storage",
        );

        index.apply_event(&Event::Delete(tenant));
        assert!(
            index
                .refs_for_secret(Some("storage"), Some("new-secret"))
                .is_empty()
        );
        assert!(
            index
                .refs_for_config_map(Some("storage"), Some("new-policy"))
                .is_empty()
        );
    }

    #[test]
    fn tenant_with_deletion_timestamp_is_removed_on_apply() {
        let index = TenantReferenceIndex::default();
        let mut tenant = tenant_fixture("tenant-a", "storage");
        tenant.spec.creds_secret = Some(local_ref("credentials"));
        index.apply_event(&Event::Apply(tenant.clone()));

        tenant.metadata.deletion_timestamp = Some(
            k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(chrono::Utc::now()),
        );
        index.apply_event(&Event::Apply(tenant));

        assert!(
            index
                .refs_for_secret(Some("storage"), Some("credentials"))
                .is_empty()
        );
    }

    #[test]
    fn relist_swaps_atomically_and_removes_missing_tenants() {
        let index = TenantReferenceIndex::default();
        let mut old = tenant_fixture("old", "storage");
        old.spec.creds_secret = Some(local_ref("old-secret"));
        index.apply_event(&Event::Apply(old));

        let mut replacement = tenant_fixture("replacement", "storage");
        replacement.spec.creds_secret = Some(local_ref("new-secret"));
        index.apply_event(&Event::Init);
        index.apply_event(&Event::InitApply(replacement));

        // The complete old index remains visible until the relist is complete.
        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("old-secret")),
            "old",
            "storage",
        );
        assert!(
            index
                .refs_for_secret(Some("storage"), Some("new-secret"))
                .is_empty()
        );

        index.apply_event(&Event::InitDone);
        assert!(
            index
                .refs_for_secret(Some("storage"), Some("old-secret"))
                .is_empty()
        );
        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("new-secret")),
            "replacement",
            "storage",
        );
    }

    #[tokio::test]
    async fn tenant_trigger_publishes_after_index_and_store_swap() {
        let index = Arc::new(TenantReferenceIndex::default());
        let mut tenant = tenant_fixture("tenant-a", "storage");
        tenant.spec.creds_secret = Some(local_ref("credentials"));

        let (reader, writer) = reflector::store();
        let (sender, receiver) = mpsc::unbounded();
        let indexing = index.clone();
        let events = receiver.inspect_ok(move |event| indexing.apply_event(event));
        let trigger = crate::tenant_trigger_stream(reflector::reflector(writer, events));
        tokio::pin!(trigger);

        assert!(sender.unbounded_send(Ok(Event::Init)).is_ok());
        assert!(
            sender
                .unbounded_send(Ok(Event::InitApply(tenant.clone())))
                .is_ok()
        );

        assert!(
            trigger.next().now_or_never().is_none(),
            "partial relist must not trigger reconciliation"
        );

        assert!(
            index
                .refs_for_secret(Some("storage"), Some("credentials"))
                .is_empty(),
            "partial relist must not be visible"
        );
        assert!(
            reader
                .get(&ObjectRef::new("tenant-a").within("storage"))
                .is_none(),
            "partial relist must not be visible in the Controller store"
        );

        assert!(sender.unbounded_send(Ok(Event::InitDone)).is_ok());
        let published = trigger
            .next()
            .await
            .expect("complete relist should publish the Tenant");
        let published = published.expect("complete relist should be valid");

        assert_eq!(published.metadata.name.as_deref(), Some("tenant-a"));
        assert_single_ref(
            &index.refs_for_secret(Some("storage"), Some("credentials")),
            "tenant-a",
            "storage",
        );
        assert!(
            reader
                .get(&ObjectRef::new("tenant-a").within("storage"))
                .is_some(),
            "published Tenant must already be visible in the Controller store"
        );
    }

    fn tenant_fixture(name: &str, namespace: &str) -> Tenant {
        let mut tenant = Tenant::new(name, Default::default());
        tenant.metadata.namespace = Some(namespace.to_string());
        tenant
    }

    fn local_ref(name: &str) -> LocalObjectReference {
        LocalObjectReference {
            name: name.to_string(),
        }
    }

    fn provisioning_user(name: &str, secret_name: &str) -> ProvisioningUser {
        ProvisioningUser {
            name: name.to_string(),
            creds_secret: Some(UserCredentialsSecretRef {
                name: secret_name.to_string(),
            }),
            policies: vec!["readwrite".to_string()],
            ..Default::default()
        }
    }

    fn ca_trust(ca: &str, client_ca: &str) -> CaTrustConfig {
        CaTrustConfig {
            ca_secret_ref: Some(SecretKeyReference {
                name: ca.to_string(),
                key: "ca.crt".to_string(),
            }),
            client_ca_secret_ref: Some(SecretKeyReference {
                name: client_ca.to_string(),
                key: "client-ca.crt".to_string(),
            }),
            ..Default::default()
        }
    }

    fn assert_single_ref(refs: &[ObjectRef<Tenant>], name: &str, namespace: &str) {
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, name);
        assert_eq!(refs[0].namespace.as_deref(), Some(namespace));
    }
}
