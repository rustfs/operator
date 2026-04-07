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

use std::convert::Infallible;
use std::result::Result as StdResult;
use std::time::Duration;

use crate::console::{
    error::{Error, Result},
    models::event::EventListResponse,
    state::Claims,
    tenant_event_scope::{discover_tenant_event_scope, list_scoped_events_v1, merge_events_v1},
};
use axum::{
    Extension,
    extract::Path,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::StreamExt;
use k8s_openapi::api::events::v1 as eventsv1;
use kube::{
    Api, Client,
    runtime::{WatchStreamExt, watcher},
};
use tokio_stream::wrappers::ReceiverStream;

/// SSE stream of merged tenant-scoped Kubernetes events (PRD §5.1).
///
/// Uses the same `session` cookie JWT as other console routes. Payloads use named SSE events:
/// - `snapshot`: JSON [`EventListResponse`]
/// - `stream_error`: JSON `{"message":"..."}` (watch/snapshot failures)
pub async fn stream_tenant_events(
    Path((namespace, tenant)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Sse<ReceiverStream<StdResult<Event, Infallible>>>> {
    let client = create_client(&claims).await?;
    // Preflight: fail the HTTP request if snapshot cannot be built (avoids 200 + empty SSE).
    let first_json = build_snapshot_json(&client, &namespace, &tenant).await?;
    let (tx, rx) = tokio::sync::mpsc::channel::<StdResult<Event, Infallible>>(16);
    let ns = namespace.clone();
    let tenant_name = tenant.clone();

    tokio::spawn(async move {
        if let Err(e) = run_event_sse_loop(client, ns, tenant_name, tx, first_json).await {
            tracing::warn!("Tenant events SSE ended with error: {}", e);
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

fn snapshot_sse_event(json: String) -> Event {
    Event::default().event("snapshot").data(json)
}

fn stream_error_sse_event(message: &str) -> Event {
    let payload = serde_json::json!({ "message": message }).to_string();
    Event::default().event("stream_error").data(payload)
}

async fn run_event_sse_loop(
    client: Client,
    namespace: String,
    tenant: String,
    tx: tokio::sync::mpsc::Sender<StdResult<Event, Infallible>>,
    first_json: String,
) -> Result<()> {
    if tx.send(Ok(snapshot_sse_event(first_json))).await.is_err() {
        return Ok(());
    }

    let event_api: Api<eventsv1::Event> = Api::namespaced(client.clone(), &namespace);
    let mut watch = watcher(event_api, watcher::Config::default())
        .default_backoff()
        .boxed();
    let mut scope_tick = tokio::time::interval(Duration::from_secs(30));
    scope_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = scope_tick.tick() => {
                match build_snapshot_json(&client, &namespace, &tenant).await {
                    Ok(json) => {
                        if tx.send(Ok(snapshot_sse_event(json))).await.is_err() {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("tenant events snapshot failed: {}", e);
                        let msg = e.to_string();
                        if tx.send(Ok(stream_error_sse_event(&msg))).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
            ev = watch.next() => {
                match ev {
                    Some(Ok(_)) => {
                        match build_snapshot_json(&client, &namespace, &tenant).await {
                            Ok(json) => {
                                if tx.send(Ok(snapshot_sse_event(json))).await.is_err() {
                                    return Ok(());
                                }
                            }
                            Err(e) => {
                                tracing::warn!("tenant events snapshot failed: {}", e);
                                let msg = e.to_string();
                                if tx.send(Ok(stream_error_sse_event(&msg))).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!("Kubernetes Event watch error: {}", e);
                        let msg = format!("Kubernetes Event watch error: {}", e);
                        if tx.send(Ok(stream_error_sse_event(&msg))).await.is_err() {
                            return Ok(());
                        }
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}

async fn build_snapshot_json(client: &Client, namespace: &str, tenant: &str) -> Result<String> {
    let scope = discover_tenant_event_scope(client, namespace, tenant).await?;
    let raw = list_scoped_events_v1(client, namespace, &scope).await?;
    let items = merge_events_v1(raw);
    let body = EventListResponse { events: items };
    serde_json::to_string(&body).map_err(|e| Error::Json { source: e })
}

/// Build a client impersonating the session token.
async fn create_client(claims: &Claims) -> Result<Client> {
    let mut config = kube::Config::infer()
        .await
        .map_err(|e| Error::InternalServer {
            message: format!("Failed to load kubeconfig: {}", e),
        })?;

    config.auth_info.token = Some(claims.k8s_token.clone().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}
