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

#![allow(clippy::single_match)]

use crate::context::Context;
use crate::reconcile::{error_policy, reconcile_rustfs};
use crate::types::v1alpha1::tenant::Tenant;
use futures::StreamExt;
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{Controller, watcher};
use kube::{Api, Client, CustomResourceExt, Resource};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{info, warn};

const RUSTFS_TENANT_LABEL: &str = "rustfs.tenant";
const CERT_MANAGER_GROUP: &str = "cert-manager.io";
const CERT_MANAGER_VERSION: &str = "v1";
const CERT_MANAGER_CERTIFICATE_KIND: &str = "Certificate";
const CERT_MANAGER_CERTIFICATE_PLURAL: &str = "certificates";

mod context;
pub mod reconcile;
mod status;
pub mod types;
pub mod utils;

// Console module (Web UI)
pub mod console;

#[cfg(test)]
pub mod tests;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_level(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .init();

    let client = Client::try_default().await?;
    let tenant_client = Api::<Tenant>::all(client.clone());

    let context = Context::new(client.clone());
    let controller = Controller::new(tenant_client, watcher::Config::default())
        .owns(
            Api::<corev1::ConfigMap>::all(client.clone()),
            watcher::Config::default(),
        )
        .watches(
            Api::<corev1::Secret>::all(client.clone()),
            watcher::Config::default(),
            tenant_refs_for_secret,
        )
        .owns(
            Api::<corev1::ServiceAccount>::all(client.clone()),
            watcher::Config::default(),
        )
        .owns(
            Api::<corev1::Pod>::all(client.clone()),
            watcher::Config::default(),
        )
        .owns(
            Api::<appsv1::StatefulSet>::all(client.clone()),
            watcher::Config::default(),
        );

    let certificate_gvk = cert_manager_certificate_gvk();
    let controller = match kube::discovery::pinned_kind(&client, &certificate_gvk).await {
        Ok((_resource, _capabilities)) => {
            let resource = cert_manager_certificate_api_resource();
            controller.watches_with(
                Api::<DynamicObject>::all_with(client.clone(), &resource),
                resource,
                watcher::Config::default(),
                tenant_refs_for_cert_manager_certificate,
            )
        }
        Err(error) => {
            warn!(
                %error,
                "cert-manager Certificate API not discovered; skipping Certificate watch"
            );
            controller
        }
    };

    controller
        .run(reconcile_rustfs, error_policy, Arc::new(context))
        .for_each(|res| async move {
            match res {
                Ok((tenant, _)) => info!("reconciled successful, object{:?}", tenant.name),
                Err(e) => warn!("reconcile failed: {}", e),
            }
        })
        .await;

    Ok(())
}

fn cert_manager_certificate_gvk() -> GroupVersionKind {
    GroupVersionKind::gvk(
        CERT_MANAGER_GROUP,
        CERT_MANAGER_VERSION,
        CERT_MANAGER_CERTIFICATE_KIND,
    )
}

fn cert_manager_certificate_api_resource() -> ApiResource {
    ApiResource::from_gvk_with_plural(
        &cert_manager_certificate_gvk(),
        CERT_MANAGER_CERTIFICATE_PLURAL,
    )
}

fn tenant_refs_for_secret(secret: corev1::Secret) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_from_metadata(
        secret.metadata.namespace.as_deref(),
        secret.metadata.owner_references.as_deref(),
        secret.metadata.labels.as_ref(),
    )
}

fn tenant_refs_for_cert_manager_certificate(certificate: DynamicObject) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_from_metadata(
        certificate.metadata.namespace.as_deref(),
        certificate.metadata.owner_references.as_deref(),
        certificate.metadata.labels.as_ref(),
    )
}

fn tenant_refs_from_metadata(
    namespace: Option<&str>,
    owner_references: Option<&[metav1::OwnerReference]>,
    labels: Option<&BTreeMap<String, String>>,
) -> Vec<ObjectRef<Tenant>> {
    let mut refs = Vec::new();

    if let Some(owner_references) = owner_references {
        for owner in owner_references {
            if let Some(tenant_ref) = tenant_ref_from_owner_reference(namespace, owner) {
                push_unique_tenant_ref(&mut refs, tenant_ref);
            }
        }
    }

    if let Some(labels) = labels
        && let Some(tenant_ref) = tenant_ref_from_labels(namespace, labels)
    {
        push_unique_tenant_ref(&mut refs, tenant_ref);
    }

    refs
}

fn tenant_ref_from_owner_reference(
    namespace: Option<&str>,
    owner: &metav1::OwnerReference,
) -> Option<ObjectRef<Tenant>> {
    if namespace.is_none()
        || owner.api_version != Tenant::api_version(&())
        || owner.kind != Tenant::kind(&())
        || owner.name.is_empty()
    {
        return None;
    }

    Some(ObjectRef::new(&owner.name).within(namespace?))
}

fn tenant_ref_from_labels(
    namespace: Option<&str>,
    labels: &BTreeMap<String, String>,
) -> Option<ObjectRef<Tenant>> {
    let name = labels
        .get(RUSTFS_TENANT_LABEL)
        .map(String::as_str)
        .filter(|name| !name.is_empty())?;

    Some(ObjectRef::new(name).within(namespace?))
}

fn push_unique_tenant_ref(refs: &mut Vec<ObjectRef<Tenant>>, tenant_ref: ObjectRef<Tenant>) {
    if !refs.iter().any(|existing| existing == &tenant_ref) {
        refs.push(tenant_ref);
    }
}

pub async fn crd(file: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer: Pin<Box<dyn AsyncWrite + Send>> = if let Some(file) = file {
        Box::pin(
            tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(file)
                .await?,
        )
    } else {
        Box::pin(tokio::io::stdout())
    };

    writer
        .write_all(serde_yaml_ng::to_string(&Tenant::crd())?.as_bytes())
        .await?;

    Ok(())
}

#[cfg(test)]
mod controller_watch_tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
    use std::collections::BTreeMap;

    #[test]
    fn cert_manager_certificate_api_resource_is_stable() {
        let resource = cert_manager_certificate_api_resource();

        assert_eq!(resource.group, "cert-manager.io");
        assert_eq!(resource.version, "v1");
        assert_eq!(resource.api_version, "cert-manager.io/v1");
        assert_eq!(resource.kind, "Certificate");
        assert_eq!(resource.plural, "certificates");
    }

    #[test]
    fn secret_mapper_uses_tenant_owner_reference() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("server-tls".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_secret(secret);

        assert_single_ref(&refs, "tenant-a", "storage");
    }

    #[test]
    fn secret_mapper_uses_rustfs_tenant_label_for_cert_manager_output_secret() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("server-tls".to_string()),
                namespace: Some("storage".to_string()),
                labels: Some(BTreeMap::from([
                    (
                        "app.kubernetes.io/managed-by".to_string(),
                        "rustfs-operator".to_string(),
                    ),
                    ("rustfs.tenant".to_string(), "tenant-b".to_string()),
                ])),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_secret(secret);

        assert_single_ref(&refs, "tenant-b", "storage");
    }

    #[test]
    fn cert_manager_certificate_mapper_uses_owner_reference_or_label() {
        let resource = cert_manager_certificate_api_resource();
        let mut owned = DynamicObject::new("tenant-c-cert", &resource).within("storage");
        owned.metadata.owner_references = Some(vec![tenant_owner_ref("tenant-c")]);

        let refs = tenant_refs_for_cert_manager_certificate(owned);
        assert_single_ref(&refs, "tenant-c", "storage");

        let mut labeled = DynamicObject::new("tenant-d-cert", &resource).within("storage");
        labeled.metadata.labels = Some(BTreeMap::from([(
            "rustfs.tenant".to_string(),
            "tenant-d".to_string(),
        )]));

        let refs = tenant_refs_for_cert_manager_certificate(labeled);
        assert_single_ref(&refs, "tenant-d", "storage");
    }

    fn tenant_owner_ref(name: &str) -> metav1::OwnerReference {
        metav1::OwnerReference {
            api_version: "rustfs.com/v1alpha1".to_string(),
            kind: "Tenant".to_string(),
            name: name.to_string(),
            uid: format!("{name}-uid"),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }
    }

    fn assert_single_ref(refs: &[ObjectRef<Tenant>], name: &str, namespace: &str) {
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, name);
        assert_eq!(refs[0].namespace.as_deref(), Some(namespace));
    }
}
