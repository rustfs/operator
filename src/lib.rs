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
use crate::types::v1alpha1::policy_binding::PolicyBinding;
use crate::types::v1alpha1::tenant::Tenant;
use axum::{
    Router, body::Body, extract::State, http::StatusCode, middleware, response::IntoResponse,
    routing::get,
};
use futures::StreamExt;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::{Controller, watcher};
use kube::{Api, Client, CustomResourceExt, Resource, api::ListParams};
use kube_leader_election::{
    LeaderCallbacks, LeaderElector, LeaderElectorConfig, LeaseLock, SystemClock,
};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Once};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use tracing::{info, warn};

const RUSTFS_TENANT_LABEL: &str = "rustfs.tenant";
const CERT_MANAGER_GROUP: &str = "cert-manager.io";
const CERT_MANAGER_VERSION: &str = "v1";
const CERT_MANAGER_CERTIFICATE_KIND: &str = "Certificate";
const CERT_MANAGER_CERTIFICATE_PLURAL: &str = "certificates";

/// Options for the operator server command.
pub struct ServerOptions {
    /// Whether to enable leader election.
    pub leader_elect: bool,
    /// Name of the Lease resource for leader election.
    pub leader_elect_lease_name: String,
    /// Namespace of the Lease resource.
    pub leader_elect_namespace: String,
    /// Identity of this instance in leader election.
    pub leader_elect_identity: String,
}

pub fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn init_tracing() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
            .with_target(true)
            .try_init();
    });
}

mod context;
pub mod metrics;
pub mod reconcile;
mod status;
mod tenant_monitor;
pub mod types;
pub mod utils;

// Console module (Web UI)
pub mod console;
pub mod sts;

#[cfg(test)]
pub mod tests;

pub async fn run(options: ServerOptions) -> Result<(), Box<dyn std::error::Error>> {
    install_rustls_crypto_provider();
    init_tracing();

    let client = Client::try_default().await?;
    if operator_metrics_enabled() {
        let metrics_port = operator_metrics_port();
        let metrics_client = client.clone();
        tokio::spawn(async move {
            if let Err(error) =
                run_operator_observability_server(metrics_client, metrics_port).await
            {
                warn!(%error, "operator observability server stopped unexpectedly");
            }
        });
    } else {
        info!("operator metrics server disabled by OPERATOR_METRICS_ENABLED=false");
    }

    if operator_sts_enabled() {
        let sts_port = operator_sts_port();
        let sts_state =
            crate::console::state::AppState::new(String::new()).with_kube_client(client.clone());
        let sts_tls_config = crate::sts::tls::OperatorStsTlsConfig::from_env();
        let tls_server_config = if sts_tls_config.enabled {
            let material =
                crate::sts::tls::load_or_create_sts_tls_material(&client, &sts_tls_config).await?;
            Some(Arc::new(crate::sts::tls::build_tls_server_config(
                &material,
            )?))
        } else {
            warn!("Operator STS TLS disabled by OPERATOR_STS_TLS_ENABLED=false");
            None
        };
        let sts_listener = bind_sts_listener(sts_port, tls_server_config.is_some()).await?;
        tokio::spawn(async move {
            if let Err(error) = run_sts_server(sts_listener, sts_state, tls_server_config).await {
                warn!(%error, "Operator STS server stopped unexpectedly");
            }
        });
    } else {
        tracing::info!("Operator STS server disabled by OPERATOR_STS_ENABLED=false");
    }

    if options.leader_elect {
        info!(
            identity = %options.leader_elect_identity,
            lease = %format!("{}/{}", options.leader_elect_namespace, options.leader_elect_lease_name),
            "starting with leader election enabled"
        );

        let lock = LeaseLock::new(
            client.clone(),
            &options.leader_elect_lease_name,
            &options.leader_elect_namespace,
            &options.leader_elect_identity,
        );

        let config = LeaderElectorConfig {
            identity: options.leader_elect_identity.clone(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            release_on_cancel: true,
        };

        let callbacks = ControllerCallbacks {
            client: client.clone(),
        };

        let cancel = CancellationToken::new();
        let elector = LeaderElector::new(config, lock, SystemClock)?;
        elector.run(callbacks, cancel).await?;
    } else {
        info!("starting with leader election disabled");
        metrics::set_operator_leader(true);
        run_active_leader_tasks(client, CancellationToken::new()).await;
        metrics::set_operator_leader(false);
    }

    Ok(())
}

/// Build and run the controller reconcile loop.
async fn run_controller(client: Client, cancel: CancellationToken) {
    let tenant_client = Api::<Tenant>::all(client.clone());
    let context = Context::new(client.clone());
    let controller = Controller::new(tenant_client, watcher::Config::default())
        .watches(
            Api::<corev1::ConfigMap>::all(client.clone()),
            watcher::Config::default(),
            tenant_refs_for_config_map,
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
        .watches(
            Api::<corev1::Pod>::all(client.clone()),
            watcher::Config::default(),
            tenant_refs_for_pod,
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

    let mut reconcile_stream = controller
        .run(
            instrumented_reconcile_rustfs,
            error_policy,
            Arc::new(context),
        )
        .boxed();

    tokio::select! {
        _ = cancel.cancelled() => {
            warn!("controller cancellation requested, stopping");
        }
        _ = async {
            while let Some(res) = reconcile_stream.next().await {
                match res {
                    Ok((tenant, _)) => {
                        info!(
                            tenant = %tenant.name,
                            namespace = %tenant.namespace.as_deref().unwrap_or("<unknown>"),
                            "reconcile completed successfully"
                        );
                    }
                    Err(error) => warn!(%error, "controller reconcile stream item failed"),
                }
            }
        } => {}
    }
}

async fn instrumented_reconcile_rustfs(
    tenant: Arc<Tenant>,
    ctx: Arc<Context>,
) -> Result<kube::runtime::controller::Action, reconcile::Error> {
    let started = metrics::reconcile_started();
    let result = reconcile_rustfs(tenant, ctx).await;
    metrics::reconcile_finished(result.is_ok(), started.elapsed());
    result
}

async fn run_active_leader_tasks(client: Client, cancel: CancellationToken) {
    let tasks_cancel = CancellationToken::new();
    let controller_client = client.clone();
    let controller_cancel = tasks_cancel.clone();
    let mut controller_handle = tokio::spawn(async move {
        run_controller(controller_client, controller_cancel).await;
    });

    let mut monitor_handle = if tenant_monitor::is_enabled() {
        let monitor_cancel = tasks_cancel.clone();
        Some(tokio::spawn(async move {
            tenant_monitor::run(client, monitor_cancel).await;
        }))
    } else {
        info!("tenant storage monitor disabled by OPERATOR_TENANT_MONITOR_ENABLED=false");
        None
    };

    let mut controller_finished = false;
    tokio::select! {
        result = &mut controller_handle => {
            controller_finished = true;
            if let Err(error) = result {
                warn!(%error, "controller task failed");
            } else {
                info!("controller finished");
            }
        }
        _ = cancel.cancelled() => {
            info!("leader task cancellation requested");
        }
    }

    tasks_cancel.cancel();
    if !controller_finished {
        stop_task("controller", controller_handle).await;
    }
    if let Some(handle) = monitor_handle.take() {
        stop_task("tenant storage monitor", handle).await;
    }
}

async fn stop_task(name: &str, mut handle: JoinHandle<()>) {
    if tokio::time::timeout(Duration::from_secs(5), &mut handle)
        .await
        .is_err()
    {
        warn!(task = name, "task stop timed out, forcing shutdown");
        handle.abort();
        let _ = handle.await;
    }
}

/// Callbacks for running the controller inside leader election.
struct ControllerCallbacks {
    client: Client,
}

#[async_trait::async_trait]
impl LeaderCallbacks for ControllerCallbacks {
    async fn on_started_leading(&self, cancel: CancellationToken) {
        info!("acquired leader lease, starting active leader tasks");
        metrics::set_operator_leader(true);
        run_active_leader_tasks(self.client.clone(), cancel).await;
        metrics::set_operator_leader(false);
    }

    async fn on_stopped_leading(&self) {
        metrics::set_operator_leader(false);
        warn!("stopped leading");
    }

    async fn on_new_leader(&self, identity: String) {
        info!(new_leader = %identity, "observed new leader");
    }
}

#[derive(Clone)]
struct OperatorObservabilityState {
    client: Client,
}

async fn run_operator_observability_server(
    client: Client,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = OperatorObservabilityState { client };
    let app = Router::new()
        .route("/metrics", get(metrics::handler))
        .route("/healthz", get(operator_health_check))
        .route("/readyz", get(operator_ready_check))
        .with_state(state)
        .layer(middleware::from_fn(metrics::record_operator_http));

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "operator observability server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn operator_health_check() -> impl IntoResponse {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (StatusCode::OK, format!("OK: {}", since_epoch.as_secs()))
}

async fn operator_ready_check(
    State(state): State<OperatorObservabilityState>,
) -> impl IntoResponse {
    match check_operator_control_plane(&state.client).await {
        Ok(()) => (StatusCode::OK, "Ready".to_string()),
        Err(error) => {
            warn!(%error, "operator readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Not ready: {error}"),
            )
        }
    }
}

async fn check_operator_control_plane(client: &Client) -> Result<(), String> {
    let tenants: Api<Tenant> = Api::all(client.clone());
    tenants
        .list(&ListParams::default().limit(1))
        .await
        .map_err(|error| format!("Tenant API: {error}"))?;
    Ok(())
}

fn operator_metrics_port() -> u16 {
    let default_port: u16 = 8080;
    match std::env::var("OPERATOR_METRICS_PORT") {
        Ok(raw_port) => match raw_port.parse::<u16>() {
            Ok(port) => port,
            Err(error) => {
                warn!(
                    %error,
                    raw_port,
                    "invalid OPERATOR_METRICS_PORT value, using default"
                );
                default_port
            }
        },
        Err(_) => default_port,
    }
}

fn operator_metrics_enabled() -> bool {
    match std::env::var("OPERATOR_METRICS_ENABLED") {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                warn!(
                    value,
                    "invalid OPERATOR_METRICS_ENABLED value, defaulting to enabled"
                );
                true
            }
        },
        Err(_) => true,
    }
}

fn operator_sts_port() -> u16 {
    let default_port: u16 = 4223;
    match std::env::var("OPERATOR_STS_PORT") {
        Ok(raw_port) => match raw_port.parse::<u16>() {
            Ok(port) => port,
            Err(error) => {
                warn!(
                    %error,
                    raw_port,
                    "invalid OPERATOR_STS_PORT value, using default"
                );
                default_port
            }
        },
        Err(_) => default_port,
    }
}

fn operator_sts_enabled() -> bool {
    match std::env::var("OPERATOR_STS_ENABLED") {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                warn!(
                    value,
                    "invalid OPERATOR_STS_ENABLED value, defaulting to enabled"
                );
                true
            }
        },
        Err(_) => true,
    }
}

async fn bind_sts_listener(
    port: u16,
    tls_enabled: bool,
) -> Result<tokio::net::TcpListener, Box<dyn std::error::Error>> {
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let scheme = if tls_enabled { "https" } else { "http" };
    tracing::info!(%scheme, %addr, "Operator STS server listening");
    Ok(listener)
}

async fn run_sts_server(
    listener: tokio::net::TcpListener,
    state: crate::console::state::AppState,
    tls_config: Option<Arc<rustls::ServerConfig>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .merge(crate::sts::server::routes())
        .with_state(state);

    if let Some(tls_config) = tls_config {
        serve_tls_sts_server(listener, app, tls_config).await?;
    } else {
        axum::serve(listener, app).await?;
    }
    Ok(())
}

async fn serve_tls_sts_server(
    listener: tokio::net::TcpListener,
    app: Router,
    tls_config: Arc<rustls::ServerConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let acceptor = TlsAcceptor::from(tls_config);

    loop {
        let (tcp_stream, remote_addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let service = app.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(tcp_stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    warn!(
                        %remote_addr,
                        %error,
                        "Operator STS TLS handshake failed"
                    );
                    return;
                }
            };

            let io = TokioIo::new(tls_stream);
            let tower_service =
                service.map_request(|request: http::Request<Incoming>| request.map(Body::new));
            let hyper_service = TowerToHyperService::new(tower_service);

            if let Err(error) = HyperBuilder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, hyper_service)
                .await
            {
                warn!(
                    %remote_addr,
                    %error,
                    "Operator STS HTTPS connection failed"
                );
            }
        });
    }
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

fn tenant_refs_for_config_map(config_map: corev1::ConfigMap) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_from_metadata(
        config_map.metadata.namespace.as_deref(),
        config_map.metadata.owner_references.as_deref(),
        config_map.metadata.labels.as_ref(),
    )
}

fn tenant_refs_for_pod(pod: corev1::Pod) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_from_metadata(
        pod.metadata.namespace.as_deref(),
        pod.metadata.owner_references.as_deref(),
        pod.metadata.labels.as_ref(),
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

pub fn render_crds_yaml() -> Result<String, serde_yaml_ng::Error> {
    let tenant = serde_yaml_ng::to_string(&Tenant::crd())?;
    let policy_binding = serde_yaml_ng::to_string(&PolicyBinding::crd())?;
    Ok(format!("{tenant}---\n{policy_binding}"))
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

    let yaml = render_crds_yaml()?;
    writer.write_all(yaml.as_bytes()).await?;

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
    fn config_map_mapper_uses_owner_reference_or_label() {
        let owned = corev1::ConfigMap {
            metadata: metav1::ObjectMeta {
                name: Some("policy".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-policy")]),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_config_map(owned);
        assert_single_ref(&refs, "tenant-policy", "storage");

        let labeled = corev1::ConfigMap {
            metadata: metav1::ObjectMeta {
                name: Some("policy".to_string()),
                namespace: Some("storage".to_string()),
                labels: Some(BTreeMap::from([(
                    "rustfs.tenant".to_string(),
                    "tenant-policy-label".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_config_map(labeled);
        assert_single_ref(&refs, "tenant-policy-label", "storage");
    }

    #[test]
    fn pod_mapper_uses_rustfs_tenant_label_for_statefulset_pods() {
        let pod = corev1::Pod {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-pool-0-0".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![metav1::OwnerReference {
                    api_version: "apps/v1".to_string(),
                    kind: "StatefulSet".to_string(),
                    name: "tenant-a-pool-0".to_string(),
                    uid: "statefulset-uid".to_string(),
                    controller: Some(true),
                    ..Default::default()
                }]),
                labels: Some(BTreeMap::from([(
                    "rustfs.tenant".to_string(),
                    "tenant-a".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_pod(pod);

        assert_single_ref(&refs, "tenant-a", "storage");
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

    #[test]
    fn crd_output_includes_tenant_and_policy_binding_documents() {
        let yaml = render_crds_yaml().expect("CRDs render to YAML");
        let documents = yaml
            .split("---")
            .map(str::trim)
            .filter(|document| !document.is_empty())
            .collect::<Vec<_>>();

        assert_eq!(documents.len(), 2);
        assert!(documents[0].contains("name: tenants.rustfs.com"));
        assert!(documents[1].contains("name: policybindings.sts.rustfs.com"));
        assert!(documents[1].contains("kind: PolicyBinding"));
        assert!(documents[1].contains("scope: Namespaced"));
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
