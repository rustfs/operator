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
use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use axum::{
    Router, body::Body, extract::State, http::StatusCode, middleware, response::IntoResponse,
    routing::get,
};
use futures::{Stream, StreamExt, TryStreamExt};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use k8s_openapi::api::apps::v1 as appsv1;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::rbac::v1 as rbacv1;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use kube::core::{
    ApiResource, DynamicObject, GroupVersionKind, PartialObjectMeta, PartialObjectMetaExt,
};
use kube::runtime::reflector::{self, ObjectRef};
use kube::runtime::{Controller, WatchStreamExt, watcher};
use kube::{Api, Client, CustomResourceExt, Resource, api::ListParams};
use kube_leader_election::{
    LeaderCallbacks, LeaderElector, LeaderElectorConfig, LeaseLock, SystemClock,
};
use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Once};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use tracing::{info, warn};

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

mod cluster_dns;
mod context;
pub mod metrics;
pub mod reconcile;
mod status;
mod tenant_monitor;
mod tenant_reference_index;
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

    let cluster_domain = cluster_dns::ClusterDomain::from_env()?;
    info!(
        cluster_domain = %cluster_domain.as_str(),
        "operator cluster DNS domain configured"
    );

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
        let sts_state = crate::console::state::AppState::new(String::new())
            .with_kube_client(client.clone())
            .with_cluster_domain(cluster_domain.as_str());
        let sts_tls_config = crate::sts::tls::OperatorStsTlsConfig::from_env_with_cluster_domain(
            cluster_domain.as_str(),
        );
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
            cluster_domain: cluster_domain.clone(),
        };

        let cancel = CancellationToken::new();
        let elector = LeaderElector::new(config, lock, SystemClock)?;
        elector.run(callbacks, cancel).await?;
    } else {
        info!("starting with leader election disabled");
        metrics::set_operator_leader(true);
        run_active_leader_tasks(client, CancellationToken::new(), cluster_domain).await;
        metrics::set_operator_leader(false);
    }

    Ok(())
}

/// Build and run the controller reconcile loop.
async fn run_controller(
    client: Client,
    cancel: CancellationToken,
    cluster_domain: cluster_dns::ClusterDomain,
) {
    let tenant_client = Api::<Tenant>::all(client.clone());
    let reference_index = Arc::new(tenant_reference_index::TenantReferenceIndex::default());
    let indexing_reference_index = reference_index.clone();
    let (tenant_reader, tenant_writer) = reflector::store();
    // Drive the Controller store, reverse-reference index, and root trigger from one ordered
    // Tenant stream. The reflector and index synchronously apply each event before it reaches the
    // trigger. Relisted Tenants are buffered until InitDone has atomically swapped both caches.
    let tenant_trigger = tenant_trigger_stream(reflector::reflector(
        tenant_writer,
        watcher::watcher(tenant_client, watcher::Config::default())
            .default_backoff()
            .inspect_ok(move |event| indexing_reference_index.apply_event(event)),
    ));

    let context = Context::new_with_cluster_domain(client.clone(), cluster_domain);
    let controller = Controller::for_stream(tenant_trigger, tenant_reader);
    // User-owned Secrets and ConfigMaps remain immutable. A dedicated, event-driven reverse index
    // supports shared resources without scanning every Tenant.
    let config_map_reference_index = reference_index.clone();
    let secret_reference_index = reference_index.clone();
    let config_maps = relist_aware_touched_objects(
        watcher::metadata_watcher(
            Api::<corev1::ConfigMap>::all(client.clone()),
            watcher::Config::default(),
        )
        .default_backoff(),
    );
    let secrets = relist_aware_touched_objects(
        watcher::metadata_watcher(
            Api::<corev1::Secret>::all(client.clone()),
            watcher::Config::default(),
        )
        .default_backoff(),
    );
    let controller = controller
        .watches_stream(config_maps, move |config_map| {
            tenant_refs_for_config_map(config_map, &config_map_reference_index)
        })
        .watches_stream(secrets, move |secret| {
            tenant_refs_for_secret(secret, &secret_reference_index)
        })
        .watches(
            Api::<rbacv1::Role>::all(client.clone()),
            watcher::Config::default(),
            tenant_refs_for_legacy_role,
        )
        .watches(
            Api::<rbacv1::RoleBinding>::all(client.clone()),
            watcher::Config::default(),
            tenant_refs_for_legacy_role_binding,
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

/// Convert reflected Tenant events into triggers without publishing a partial relist.
///
/// The Controller resolves each emitted Tenant through the reflector store, so an object deleted
/// before the Controller processes its trigger can be skipped as stale. Delete events themselves
/// are ignored here, while subsequent Apply events and complete relists continue through the
/// stream.
fn tenant_trigger_stream<S>(events: S) -> impl Stream<Item = Result<Tenant, watcher::Error>> + Send
where
    S: Stream<Item = Result<watcher::Event<Tenant>, watcher::Error>> + Send + 'static,
{
    futures::stream::unfold(
        (
            Box::pin(events),
            None::<VecDeque<Tenant>>,
            VecDeque::<Tenant>::new(),
        ),
        |(mut events, mut initializing, mut pending)| async move {
            loop {
                if let Some(tenant) = pending.pop_front() {
                    return Some((Ok(tenant), (events, initializing, pending)));
                }

                let event = events.next().await?;
                match event {
                    Ok(watcher::Event::Apply(tenant)) => {
                        return Some((Ok(tenant), (events, initializing, pending)));
                    }
                    Ok(watcher::Event::Delete(_)) => {}
                    Ok(watcher::Event::Init) => initializing = Some(VecDeque::new()),
                    Ok(watcher::Event::InitApply(tenant)) => {
                        initializing
                            .get_or_insert_with(VecDeque::new)
                            .push_back(tenant);
                    }
                    Ok(watcher::Event::InitDone) => {
                        pending = initializing.take().unwrap_or_default();
                    }
                    Err(error) => {
                        return Some((Err(error), (events, initializing, pending)));
                    }
                }
            }
        },
    )
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RelatedResourceKey {
    namespace: Option<String>,
    name: String,
}

impl RelatedResourceKey {
    fn from_metadata(metadata: &metav1::ObjectMeta) -> Option<Self> {
        Some(Self {
            namespace: metadata.namespace.clone(),
            name: metadata.name.clone()?,
        })
    }
}

fn related_resource_metadata_refs<K: Resource>(resource: &K) -> Vec<ObjectRef<Tenant>> {
    let metadata = resource.meta();
    tenant_refs_from_metadata(
        metadata.namespace.as_deref(),
        metadata.owner_references.as_deref(),
        None,
    )
}

/// Compact metadata retained across related-resource relists.
///
/// The watch object can contain large annotations and managed fields. Replaying an event only
/// needs its namespace/name plus the Tenant routes that came from owner references. Preserve only
/// those routing fields before caching them.
#[derive(Clone, Debug)]
struct RelatedResourceSnapshot {
    key: Option<RelatedResourceKey>,
    tenant_refs: Vec<ObjectRef<Tenant>>,
}

impl RelatedResourceSnapshot {
    fn from_resource<K: Resource>(resource: &K) -> Self {
        let source = resource.meta();
        Self {
            key: RelatedResourceKey::from_metadata(source),
            tenant_refs: related_resource_metadata_refs(resource),
        }
    }

    fn key(&self) -> Option<RelatedResourceKey> {
        self.key.clone()
    }

    fn to_partial<K>(&self) -> PartialObjectMeta<K> {
        let metadata = self
            .key
            .as_ref()
            .map(|key| metav1::ObjectMeta {
                name: Some(key.name.clone()),
                namespace: key.namespace.clone(),
                // These objects are emitted only inside the controller stream. Reconstruct the
                // minimal owner routes instead of retaining an ObjectMeta per cluster resource.
                owner_references: (!self.tenant_refs.is_empty()).then(|| {
                    self.tenant_refs
                        .iter()
                        .map(|tenant| metav1::OwnerReference {
                            api_version: Tenant::api_version(&()).to_string(),
                            kind: Tenant::kind(&()).to_string(),
                            name: tenant.name.clone(),
                            uid: String::new(),
                            controller: None,
                            block_owner_deletion: None,
                        })
                        .collect()
                }),
                ..Default::default()
            })
            .unwrap_or_default();
        metadata.into_response_partial::<K>()
    }

    fn has_same_routes(&self, other: &Self) -> bool {
        self.tenant_refs == other.tenant_refs
    }
}

/// Decode related-resource watch events while recovering deletes lost across an RV relist.
///
/// Kubernetes represents a relist as Init/InitApply/InitDone, without explicit Delete events for
/// objects absent from the new snapshot. Keep only compact routing snapshots from the last
/// complete list and replay objects that disappeared or changed Tenant routing after InitDone, so
/// both the old and new Tenant mappings are reconciled.
fn relist_aware_touched_objects<K, S>(
    events: S,
) -> impl Stream<Item = Result<PartialObjectMeta<K>, watcher::Error>> + Send
where
    K: Resource + Send + 'static,
    S: Stream<Item = Result<watcher::Event<PartialObjectMeta<K>>, watcher::Error>> + Send + 'static,
{
    futures::stream::unfold(
        (
            Box::pin(events),
            BTreeMap::<RelatedResourceKey, Arc<RelatedResourceSnapshot>>::new(),
            None::<BTreeMap<RelatedResourceKey, Arc<RelatedResourceSnapshot>>>,
            VecDeque::<Arc<RelatedResourceSnapshot>>::new(),
        ),
        |(mut events, mut active, mut initializing, mut pending)| async move {
            loop {
                if let Some(snapshot) = pending.pop_front() {
                    return Some((
                        Ok(snapshot.to_partial::<K>()),
                        (events, active, initializing, pending),
                    ));
                }

                let event = events.next().await?;
                match event {
                    Ok(watcher::Event::Apply(resource)) => {
                        let snapshot = Arc::new(RelatedResourceSnapshot::from_resource(&resource));
                        if let Some(key) = snapshot.key()
                            && let Some(previous) = active.insert(key, Arc::clone(&snapshot))
                            && !previous.has_same_routes(&snapshot)
                        {
                            pending.push_back(previous);
                        }
                        return Some((
                            Ok(snapshot.to_partial::<K>()),
                            (events, active, initializing, pending),
                        ));
                    }
                    Ok(watcher::Event::Delete(resource)) => {
                        let snapshot = RelatedResourceSnapshot::from_resource(&resource);
                        if let Some(key) = snapshot.key() {
                            active.remove(&key);
                        }
                        return Some((
                            Ok(snapshot.to_partial::<K>()),
                            (events, active, initializing, pending),
                        ));
                    }
                    Ok(watcher::Event::Init) => initializing = Some(BTreeMap::new()),
                    Ok(watcher::Event::InitApply(resource)) => {
                        let snapshot = Arc::new(RelatedResourceSnapshot::from_resource(&resource));
                        if let Some(key) = snapshot.key() {
                            initializing
                                .get_or_insert_with(BTreeMap::new)
                                .insert(key, snapshot);
                        }
                    }
                    Ok(watcher::Event::InitDone) => {
                        if let Some(next) = initializing.take() {
                            pending.extend(next.values().cloned());
                            pending.extend(
                                active
                                    .iter()
                                    .filter(|(key, previous)| {
                                        next.get(*key).is_none_or(|current| {
                                            !current.has_same_routes(previous)
                                        })
                                    })
                                    .map(|(_, snapshot)| Arc::clone(snapshot)),
                            );
                            active = next;
                        }
                    }
                    Err(error) => {
                        return Some((Err(error), (events, active, initializing, pending)));
                    }
                }
            }
        },
    )
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

async fn run_active_leader_tasks(
    client: Client,
    cancel: CancellationToken,
    cluster_domain: cluster_dns::ClusterDomain,
) {
    let tasks_cancel = CancellationToken::new();
    let controller_client = client.clone();
    let controller_cancel = tasks_cancel.clone();
    let controller_cluster_domain = cluster_domain.clone();
    let mut controller_handle = tokio::spawn(async move {
        run_controller(
            controller_client,
            controller_cancel,
            controller_cluster_domain,
        )
        .await;
    });

    let mut monitor_handle = if tenant_monitor::is_enabled() {
        let monitor_cancel = tasks_cancel.clone();
        let monitor_cluster_domain = cluster_domain.as_str().to_string();
        Some(tokio::spawn(async move {
            tenant_monitor::run(client, monitor_cancel, monitor_cluster_domain).await;
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
    cluster_domain: cluster_dns::ClusterDomain,
}

#[async_trait::async_trait]
impl LeaderCallbacks for ControllerCallbacks {
    async fn on_started_leading(&self, cancel: CancellationToken) {
        info!("acquired leader lease, starting active leader tasks");
        metrics::set_operator_leader(true);
        run_active_leader_tasks(self.client.clone(), cancel, self.cluster_domain.clone()).await;
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

fn tenant_refs_for_secret<K: Resource>(
    secret: K,
    index: &tenant_reference_index::TenantReferenceIndex,
) -> Vec<ObjectRef<Tenant>> {
    let metadata = secret.meta();
    let mut refs = tenant_refs_from_metadata(
        metadata.namespace.as_deref(),
        metadata.owner_references.as_deref(),
        None,
    );
    refs.extend(index.refs_for_secret(metadata.namespace.as_deref(), metadata.name.as_deref()));
    deduplicate_tenant_refs(refs)
}

fn tenant_refs_for_config_map<K: Resource>(
    config_map: K,
    index: &tenant_reference_index::TenantReferenceIndex,
) -> Vec<ObjectRef<Tenant>> {
    let metadata = config_map.meta();
    let mut refs = tenant_refs_from_metadata(
        metadata.namespace.as_deref(),
        metadata.owner_references.as_deref(),
        None,
    );
    refs.extend(index.refs_for_config_map(metadata.namespace.as_deref(), metadata.name.as_deref()));
    deduplicate_tenant_refs(refs)
}

fn tenant_refs_for_legacy_role(role: rbacv1::Role) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_for_legacy_rbac(&role, "role")
}

fn tenant_refs_for_legacy_role_binding(
    role_binding: rbacv1::RoleBinding,
) -> Vec<ObjectRef<Tenant>> {
    tenant_refs_for_legacy_rbac(&role_binding, "role-binding")
}

fn tenant_refs_for_legacy_rbac<K: Resource>(resource: &K, suffix: &str) -> Vec<ObjectRef<Tenant>> {
    let metadata = resource.meta();
    let Some(resource_name) = metadata.name.as_deref() else {
        return Vec::new();
    };

    tenant_refs_from_metadata(
        metadata.namespace.as_deref(),
        metadata.owner_references.as_deref(),
        metadata.labels.as_ref(),
    )
    .into_iter()
    .filter(|tenant| resource_name == format!("{}-{suffix}", tenant.name))
    .collect()
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
                refs.push(tenant_ref);
            }
        }
    }

    if let Some(labels) = labels
        && let Some(tenant_ref) = tenant_ref_from_labels(namespace, labels)
    {
        refs.push(tenant_ref);
    }

    deduplicate_tenant_refs(refs)
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

fn deduplicate_tenant_refs(refs: Vec<ObjectRef<Tenant>>) -> Vec<ObjectRef<Tenant>> {
    refs.into_iter()
        .map(|tenant_ref| {
            (
                (tenant_ref.namespace.clone(), tenant_ref.name.clone()),
                tenant_ref,
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// Drops collections that are empty rather than absent.
///
/// kube-rs derives `names.categories` as `Some(vec![])` when the type carries no
/// `category` attribute. The API server prunes empty arrays, so the stored object
/// has no `categories` key at all. Tools that compare the rendered manifest against
/// the live object (Argo CD, Flux, `kubectl diff`) then see a desired `[]` versus an
/// absent field and report the CRD as permanently out of sync - a sync succeeds and
/// the resource immediately reads as drifted again.
fn normalize_crd(crd: &mut CustomResourceDefinition) {
    if crd
        .spec
        .names
        .categories
        .as_ref()
        .is_some_and(Vec::is_empty)
    {
        crd.spec.names.categories = None;
    }
}

pub fn render_crds_yaml() -> Result<String, serde_yaml_ng::Error> {
    let mut tenant_crd = Tenant::crd();
    normalize_crd(&mut tenant_crd);
    let mut policy_binding_crd = PolicyBinding::crd();
    normalize_crd(&mut policy_binding_crd);

    let tenant = serde_yaml_ng::to_string(&tenant_crd)?;
    let policy_binding = serde_yaml_ng::to_string(&policy_binding_crd)?;
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
    use crate::types::v1alpha1::tenant::RpcSecretRef;
    use futures::{TryStreamExt, stream};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
    use serde::Deserialize;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    fn parse_crds(yaml: &str) -> Vec<CustomResourceDefinition> {
        serde_yaml_ng::Deserializer::from_str(yaml)
            .map(|document| {
                CustomResourceDefinition::deserialize(document)
                    .expect("CRD document should deserialize")
            })
            .collect()
    }

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
    fn rendered_crds_omit_empty_categories() {
        let yaml = render_crds_yaml().expect("CRDs render");

        for crd in parse_crds(&yaml) {
            let name = crd.metadata.name.as_deref().unwrap_or("<unnamed>");
            assert!(
                crd.spec.names.categories.is_none(),
                "rendered CRD {name} must omit an empty spec.names.categories list"
            );
        }
    }

    #[test]
    fn normalize_crd_keeps_populated_categories() {
        let mut crd = Tenant::crd();
        crd.spec.names.categories = Some(vec!["storage".to_owned()]);

        normalize_crd(&mut crd);

        assert_eq!(
            crd.spec.names.categories,
            Some(vec!["storage".to_owned()]),
            "normalization must only drop empty category lists"
        );
    }

    #[tokio::test]
    async fn tenant_trigger_continues_after_an_applied_tenant_is_deleted() {
        let index = Arc::new(tenant_reference_index::TenantReferenceIndex::default());
        let tenant_a = tenant_fixture("tenant-a", "storage");
        let tenant_b = tenant_fixture("tenant-b", "storage");
        let indexing = index.clone();
        let events = stream::iter([
            Ok::<_, watcher::Error>(watcher::Event::Apply(tenant_a.clone())),
            Ok(watcher::Event::Delete(tenant_a)),
            Ok(watcher::Event::Apply(tenant_b)),
        ])
        .inspect_ok(move |event| indexing.apply_event(event));
        let (reader, writer) = reflector::store();
        let trigger = tenant_trigger_stream(reflector::reflector(writer, events));

        let emitted = tokio::time::timeout(Duration::from_secs(1), trigger.try_collect::<Vec<_>>())
            .await
            .expect("Tenant trigger must not stall")
            .expect("fixture events must be valid");

        assert_eq!(
            emitted
                .iter()
                .filter_map(|tenant| tenant.metadata.name.as_deref())
                .collect::<Vec<_>>(),
            vec!["tenant-a", "tenant-b"]
        );
        assert!(
            reader
                .get(&ObjectRef::new("tenant-a").within("storage"))
                .is_none()
        );
        assert!(
            reader
                .get(&ObjectRef::new("tenant-b").within("storage"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn related_resource_apply_replays_previous_metadata_mapping() {
        let previous = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("owner-changed".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                resource_version: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let replacement = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("owner-changed".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-b")]),
                resource_version: Some("2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let events = stream::iter([
            Ok::<_, watcher::Error>(watcher::Event::Apply(partial_secret(previous))),
            Ok(watcher::Event::Apply(partial_secret(replacement))),
        ]);

        let emitted = relist_aware_touched_objects(events)
            .try_collect::<Vec<_>>()
            .await
            .expect("fixture events must be valid");
        let updated_tenants = emitted
            .iter()
            .skip(1)
            .flat_map(related_resource_metadata_refs)
            .map(|tenant| tenant.name)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            updated_tenants,
            BTreeSet::from(["tenant-a".to_string(), "tenant-b".to_string()])
        );
    }

    #[tokio::test]
    async fn related_resource_relist_replays_objects_missing_from_new_snapshot() {
        let deleted = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("deleted".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                resource_version: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut retained = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("retained".to_string()),
                namespace: Some("storage".to_string()),
                resource_version: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let first_retained = retained.clone();
        retained.metadata.resource_version = Some("2".to_string());
        let events = stream::iter([
            Ok::<_, watcher::Error>(watcher::Event::Init),
            Ok(watcher::Event::InitApply(partial_secret(deleted))),
            Ok(watcher::Event::InitApply(partial_secret(first_retained))),
            Ok(watcher::Event::InitDone),
            // An expired resource version restarts the watch. Kubernetes does not emit an
            // explicit Delete for objects that are absent from the replacement snapshot.
            Ok(watcher::Event::Init),
            Ok(watcher::Event::InitApply(partial_secret(retained))),
            Ok(watcher::Event::InitDone),
        ]);

        let emitted = relist_aware_touched_objects(events)
            .try_collect::<Vec<_>>()
            .await
            .expect("fixture events must be valid");
        let names = emitted
            .iter()
            .filter_map(|secret| secret.metadata.name.as_deref())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["deleted", "retained", "retained", "deleted"]);
        let replayed_delete = emitted.last().expect("missing object must be replayed");
        assert!(
            replayed_delete.metadata.resource_version.is_none(),
            "cached routing snapshots must discard resource versions"
        );
        assert_single_ref(
            &tenant_refs_for_secret(
                replayed_delete.clone(),
                &tenant_reference_index::TenantReferenceIndex::default(),
            ),
            "tenant-a",
            "storage",
        );
    }

    #[tokio::test]
    async fn related_resource_relist_replays_previous_metadata_mappings() {
        let previous_owner = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("owner-changed".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                resource_version: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let replacement_owner = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("owner-changed".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-b")]),
                resource_version: Some("2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let events = stream::iter([
            Ok::<_, watcher::Error>(watcher::Event::Init),
            Ok(watcher::Event::InitApply(partial_secret(previous_owner))),
            Ok(watcher::Event::InitDone),
            Ok(watcher::Event::Init),
            Ok(watcher::Event::InitApply(partial_secret(replacement_owner))),
            Ok(watcher::Event::InitDone),
        ]);

        let emitted = relist_aware_touched_objects(events)
            .try_collect::<Vec<_>>()
            .await
            .expect("fixture events must be valid");
        assert_eq!(emitted.len(), 3);

        let second_relist_tenants = emitted
            .iter()
            .skip(1)
            .flat_map(related_resource_metadata_refs)
            .map(|tenant| tenant.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            second_relist_tenants,
            BTreeSet::from(["tenant-a".to_string(), "tenant-b".to_string()])
        );
    }

    #[tokio::test]
    async fn related_resource_large_relist_retains_only_compact_routing_metadata() {
        const RESOURCE_COUNT: usize = 10_000;

        assert!(
            std::mem::size_of::<RelatedResourceSnapshot>()
                < std::mem::size_of::<metav1::ObjectMeta>(),
            "routing snapshots must remain smaller than full Kubernetes metadata"
        );

        let initial = stream::iter((0..RESOURCE_COUNT).map(|index| {
            let owner = (index == 1).then_some("tenant-a");
            Ok::<_, watcher::Error>(watcher::Event::InitApply(large_partial_secret(
                index, owner,
            )))
        }));
        let replacement = stream::iter((1..RESOURCE_COUNT).map(|index| {
            let owner = (index == 1).then_some("tenant-b");
            Ok::<_, watcher::Error>(watcher::Event::InitApply(large_partial_secret(
                index, owner,
            )))
        }));
        let events = stream::iter([Ok::<_, watcher::Error>(watcher::Event::Init)])
            .chain(initial)
            .chain(stream::iter([
                Ok(watcher::Event::InitDone),
                Ok(watcher::Event::Init),
            ]))
            .chain(replacement)
            .chain(stream::iter([Ok(watcher::Event::InitDone)]));

        let emitted = relist_aware_touched_objects(events)
            .try_collect::<Vec<_>>()
            .await
            .expect("large relist fixture must be valid");

        assert_eq!(emitted.len(), RESOURCE_COUNT * 2 + 1);
        assert!(emitted.iter().all(|resource| {
            resource.metadata.annotations.is_none()
                && resource.metadata.labels.is_none()
                && resource.metadata.resource_version.is_none()
                && resource.metadata.uid.is_none()
                && resource.metadata.finalizers.is_none()
        }));
        assert_eq!(
            emitted
                .iter()
                .filter(|resource| resource.metadata.name.as_deref() == Some("resource-00000"))
                .count(),
            2,
            "a resource absent from the replacement list must be replayed"
        );
        let changed_routes = emitted
            .iter()
            .filter(|resource| resource.metadata.name.as_deref() == Some("resource-00001"))
            .flat_map(related_resource_metadata_refs)
            .map(|tenant| tenant.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            changed_routes,
            BTreeSet::from(["tenant-a".to_string(), "tenant-b".to_string()])
        );
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

        let refs = tenant_refs_for_secret(
            secret,
            &tenant_reference_index::TenantReferenceIndex::default(),
        );

        assert_single_ref(&refs, "tenant-a", "storage");
    }

    #[test]
    fn secret_mapper_enqueues_every_tenant_referencing_a_shared_secret() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("shared-rpc-auth".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let index = tenant_reference_index::TenantReferenceIndex::default();
        for tenant in [
            tenant_referencing_secret("tenant-a", "storage", "shared-rpc-auth"),
            tenant_referencing_secret("tenant-b", "storage", "shared-rpc-auth"),
            tenant_referencing_secret("tenant-other-namespace", "other", "shared-rpc-auth"),
            tenant_referencing_secret("tenant-other-secret", "storage", "different-secret"),
        ] {
            index.apply_event(&watcher::Event::Apply(tenant));
        }

        let refs = tenant_refs_for_secret(secret, &index);

        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], ObjectRef::new("tenant-a").within("storage"));
        assert_eq!(refs[1], ObjectRef::new("tenant-b").within("storage"));
    }

    #[test]
    fn secret_mapper_ignores_legacy_routing_label_without_a_spec_reference() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("legacy-secret".to_string()),
                namespace: Some("storage".to_string()),
                labels: Some(BTreeMap::from([(
                    RUSTFS_TENANT_LABEL.to_string(),
                    "tenant-legacy".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_secret(
            secret,
            &tenant_reference_index::TenantReferenceIndex::default(),
        );

        assert!(refs.is_empty());
    }

    #[test]
    fn config_map_mapper_uses_tenant_owner_reference() {
        let owned = corev1::ConfigMap {
            metadata: metav1::ObjectMeta {
                name: Some("policy".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-policy")]),
                ..Default::default()
            },
            ..Default::default()
        };

        let refs = tenant_refs_for_config_map(
            owned,
            &tenant_reference_index::TenantReferenceIndex::default(),
        );
        assert_single_ref(&refs, "tenant-policy", "storage");
    }

    #[test]
    fn config_map_mapper_ignores_legacy_routing_label_without_a_spec_reference() {
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

        let refs = tenant_refs_for_config_map(
            labeled,
            &tenant_reference_index::TenantReferenceIndex::default(),
        );
        assert!(refs.is_empty());
    }

    #[test]
    fn config_map_mapper_uses_cached_tenant_references_without_resource_labels() {
        let config_map = corev1::ConfigMap {
            metadata: metav1::ObjectMeta {
                name: Some("shared-policy".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let index = tenant_reference_index::TenantReferenceIndex::default();
        for name in ["tenant-a", "tenant-b"] {
            let mut tenant = tenant_fixture(name, "storage");
            tenant
                .spec
                .policies
                .push(crate::types::v1alpha1::provisioning::ProvisioningPolicy {
                    name: "readwrite".to_string(),
                    document: crate::types::v1alpha1::provisioning::PolicyDocumentSource {
                        config_map_key_ref:
                            crate::types::v1alpha1::provisioning::ConfigMapKeyReference {
                                name: "shared-policy".to_string(),
                                key: "policy.json".to_string(),
                            },
                    },
                    ..Default::default()
                });
            index.apply_event(&watcher::Event::Apply(tenant));
        }

        let refs = tenant_refs_for_config_map(config_map, &index);

        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|reference| reference.name == "tenant-a"));
        assert!(refs.iter().any(|reference| reference.name == "tenant-b"));
    }

    #[test]
    fn secret_mapper_uses_cached_tenant_references_in_the_same_namespace() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("credentials".to_string()),
                namespace: Some("storage".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut matching = tenant_fixture("tenant-a", "storage");
        matching.spec.creds_secret = Some(corev1::LocalObjectReference {
            name: "credentials".to_string(),
        });
        let mut other_namespace = tenant_fixture("tenant-b", "other");
        other_namespace.spec.creds_secret = Some(corev1::LocalObjectReference {
            name: "credentials".to_string(),
        });
        let index = tenant_reference_index::TenantReferenceIndex::default();
        index.apply_event(&watcher::Event::Apply(matching));
        index.apply_event(&watcher::Event::Apply(other_namespace));

        let refs = tenant_refs_for_secret(secret, &index);

        assert_single_ref(&refs, "tenant-a", "storage");
    }

    #[test]
    fn secret_mapper_ignores_stale_label_when_an_index_reference_exists() {
        let secret = corev1::Secret {
            metadata: metav1::ObjectMeta {
                name: Some("credentials".to_string()),
                namespace: Some("storage".to_string()),
                labels: Some(BTreeMap::from([(
                    "rustfs.tenant".to_string(),
                    "tenant-stale".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut tenant = tenant_fixture("tenant-a", "storage");
        tenant.spec.creds_secret = Some(corev1::LocalObjectReference {
            name: "credentials".to_string(),
        });
        let index = tenant_reference_index::TenantReferenceIndex::default();
        index.apply_event(&watcher::Event::Apply(tenant));

        let refs = tenant_refs_for_secret(secret, &index);

        assert_single_ref(&refs, "tenant-a", "storage");
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
    fn legacy_rbac_mapper_uses_tenant_owner_reference() {
        let role = rbacv1::Role {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-role".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                ..Default::default()
            },
            ..Default::default()
        };
        let role_binding = rbacv1::RoleBinding {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-role-binding".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_single_ref(&tenant_refs_for_legacy_role(role), "tenant-a", "storage");
        assert_single_ref(
            &tenant_refs_for_legacy_role_binding(role_binding),
            "tenant-a",
            "storage",
        );
    }

    #[test]
    fn legacy_rbac_mapper_uses_tenant_label_for_orphan() {
        let role = rbacv1::Role {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-role".to_string()),
                namespace: Some("storage".to_string()),
                labels: Some(BTreeMap::from([(
                    RUSTFS_TENANT_LABEL.to_string(),
                    "tenant-a".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_single_ref(&tenant_refs_for_legacy_role(role), "tenant-a", "storage");
    }

    #[test]
    fn legacy_rbac_mapper_ignores_non_legacy_names() {
        let role = rbacv1::Role {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-custom-role".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                labels: Some(BTreeMap::from([(
                    RUSTFS_TENANT_LABEL.to_string(),
                    "tenant-a".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };
        let role_binding = rbacv1::RoleBinding {
            metadata: metav1::ObjectMeta {
                name: Some("tenant-a-role-binding-extra".to_string()),
                namespace: Some("storage".to_string()),
                owner_references: Some(vec![tenant_owner_ref("tenant-a")]),
                labels: Some(BTreeMap::from([(
                    RUSTFS_TENANT_LABEL.to_string(),
                    "tenant-a".to_string(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(tenant_refs_for_legacy_role(role).is_empty());
        assert!(tenant_refs_for_legacy_role_binding(role_binding).is_empty());
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

    #[test]
    fn tracked_crds_match_generated_schema() {
        let yaml = render_crds_yaml().expect("CRDs render to YAML");
        let (tenant, policy_binding) = yaml
            .split_once("---\n")
            .expect("Tenant and PolicyBinding CRDs should be separated");

        assert_eq!(
            tenant,
            include_str!("../deploy/rustfs-operator/crds/tenant-crd.yaml")
        );
        assert_eq!(
            policy_binding,
            include_str!("../deploy/rustfs-operator/crds/policybinding-crd.yaml")
        );
        assert_eq!(
            policy_binding,
            include_str!("../deploy/k8s-dev/policybinding-crd.yaml")
        );
    }

    #[test]
    fn chart_crd_names_match_generated_crds_without_duplicates() {
        let generated_names = parse_crds(&render_crds_yaml().expect("CRDs render to YAML"))
            .into_iter()
            .map(|crd| crd.metadata.name.expect("generated CRD should have a name"))
            .collect::<BTreeSet<_>>();
        let crd_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deploy/rustfs-operator/crds");
        let mut entries = fs::read_dir(&crd_dir)
            .expect("chart CRD directory should be readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("chart CRD directory entries should be readable");
        entries.sort_by_key(|entry| entry.path());

        let mut tracked_names = BTreeMap::new();
        for entry in entries {
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml")
            ) {
                continue;
            }

            let yaml = fs::read_to_string(&path).expect("tracked CRD should be readable");
            for crd in parse_crds(&yaml) {
                let name = crd.metadata.name.expect("tracked CRD should have a name");
                assert!(
                    tracked_names.insert(name.clone(), path.clone()).is_none(),
                    "chart contains duplicate CRD {name}"
                );
            }
        }

        assert_eq!(
            tracked_names.into_keys().collect::<BTreeSet<_>>(),
            generated_names,
            "chart CRD names must match the generated CRDs"
        );
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

    fn partial_secret(secret: corev1::Secret) -> PartialObjectMeta<corev1::Secret> {
        secret.metadata.into_response_partial::<corev1::Secret>()
    }

    fn large_partial_secret(
        index: usize,
        owner: Option<&str>,
    ) -> PartialObjectMeta<corev1::Secret> {
        metav1::ObjectMeta {
            name: Some(format!("resource-{index:05}")),
            namespace: Some("storage".to_string()),
            annotations: Some(BTreeMap::from([(
                "example.com/large".to_string(),
                "x".repeat(4 * 1024),
            )])),
            labels: Some(BTreeMap::from([(
                "example.com/unrelated".to_string(),
                "discard-me".to_string(),
            )])),
            owner_references: owner.map(|name| vec![tenant_owner_ref(name)]),
            resource_version: Some(format!("rv-{index}")),
            uid: Some(format!("uid-{index}")),
            finalizers: Some(vec!["example.com/finalizer".to_string()]),
            ..Default::default()
        }
        .into_response_partial::<corev1::Secret>()
    }

    fn tenant_fixture(name: &str, namespace: &str) -> Tenant {
        let mut tenant = Tenant::new(name, Default::default());
        tenant.metadata.namespace = Some(namespace.to_string());
        tenant
    }

    fn tenant_referencing_secret(name: &str, namespace: &str, secret_name: &str) -> Tenant {
        let mut tenant = tenant_fixture(name, namespace);
        tenant.spec.rpc_secret = Some(RpcSecretRef {
            name: secret_name.to_string(),
            key: "rpc-secret".to_string(),
        });
        tenant
    }

    fn assert_single_ref(refs: &[ObjectRef<Tenant>], name: &str, namespace: &str) {
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, name);
        assert_eq!(refs[0].namespace.as_deref(), Some(namespace));
    }
}
