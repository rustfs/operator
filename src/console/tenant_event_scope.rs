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

//! Discover which `(involvedObject.kind, involvedObject.name)` pairs belong to a Tenant for event aggregation (PRD §4.1).
//! Lists use [`k8s_openapi::api::events::v1::Event`] (`events.k8s.io/v1`) with per-resource field selectors instead of listing all namespace events.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt};
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::events::v1 as eventsv1;
use kube::{Api, Client, ResourceExt, api::ListParams};

use crate::console::{
    error::{self, Result},
    models::event::EventItem,
};
use crate::types::v1alpha1::tenant::Tenant;

/// `involvedObject.kind` for the Tenant CR (matches CRD `names.kind`).
pub const TENANT_CR_KIND: &str = "Tenant";

/// Default max events per SSE snapshot (PRD §5.1).
pub const MAX_EVENTS_SNAPSHOT: usize = 200;

/// Concurrent `events.k8s.io` list calls (per regarding kind+name pair).
const EVENT_LIST_CONCURRENCY: usize = 16;

/// Label selector `rustfs.tenant=<tenant>` — must match [`crate::console::handlers::pods::list_pods`].
pub fn tenant_label_selector(tenant: &str) -> String {
    format!("rustfs.tenant={}", tenant)
}

/// Allowed `(kind, name)` pairs for `Event.regarding` in this tenant scope.
#[derive(Debug, Clone)]
pub struct TenantEventScope {
    /// Tenant name (scope metadata).
    #[allow(dead_code)]
    pub tenant_name: String,
    /// `(regarding.kind, regarding.name)` using API kind strings.
    pub involved: HashSet<(String, String)>,
}

/// Load Pod / StatefulSet / PVC names and Tenant CR row — same discovery rules as `list_pods` / `list_pools`.
pub async fn discover_tenant_event_scope(
    client: &Client,
    namespace: &str,
    tenant: &str,
) -> Result<TenantEventScope> {
    let tenant_api: Api<Tenant> = Api::namespaced(client.clone(), namespace);
    let t = tenant_api
        .get(tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", tenant)))?;

    let mut involved: HashSet<(String, String)> = HashSet::new();
    involved.insert((TENANT_CR_KIND.to_string(), tenant.to_string()));

    for pool in &t.spec.pools {
        let ss_name = format!("{}-{}", tenant, pool.name);
        involved.insert(("StatefulSet".to_string(), ss_name));
    }

    let pod_api: Api<corev1::Pod> = Api::namespaced(client.clone(), namespace);
    let pods = pod_api
        .list(&ListParams::default().labels(&tenant_label_selector(tenant)))
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pods for tenant '{}'", tenant)))?;
    for p in pods.items {
        involved.insert(("Pod".to_string(), p.name_any()));
    }

    let pvc_api: Api<corev1::PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    let pvcs = pvc_api
        .list(&ListParams::default().labels(&tenant_label_selector(tenant)))
        .await
        .map_err(|e| {
            error::map_kube_error(e, format!("PersistentVolumeClaims for tenant '{}'", tenant))
        })?;
    for pvc in pvcs.items {
        involved.insert(("PersistentVolumeClaim".to_string(), pvc.name_any()));
    }

    Ok(TenantEventScope {
        tenant_name: tenant.to_string(),
        involved,
    })
}

/// List [`eventsv1::Event`] for [`TenantEventScope`] using `regarding.kind` + `regarding.name` field selectors (parallel, bounded concurrency).
pub async fn list_scoped_events_v1(
    client: &Client,
    namespace: &str,
    scope: &TenantEventScope,
) -> Result<Vec<eventsv1::Event>> {
    let api: Api<eventsv1::Event> = Api::namespaced(client.clone(), namespace);
    let pairs: Vec<(String, String)> = scope.involved.iter().cloned().collect();

    let results: Vec<_> = stream::iter(pairs)
        .map(|(kind, name)| {
            let api = api.clone();
            async move {
                let field_selector = format!("regarding.kind={},regarding.name={}", kind, name);
                api.list(&ListParams::default().fields(&field_selector).limit(500))
                    .await
            }
        })
        .buffer_unordered(EVENT_LIST_CONCURRENCY)
        .collect()
        .await;

    let mut all = Vec::new();
    for res in results {
        let list = res.map_err(|e| {
            error::map_kube_error(e, format!("Events for tenant '{}'", scope.tenant_name))
        })?;
        all.extend(list.items);
    }

    Ok(all)
}

/// Dedupe, sort newest first, cap at [`MAX_EVENTS_SNAPSHOT`], map to [`EventItem`].
pub fn merge_events_v1(raw: Vec<eventsv1::Event>) -> Vec<EventItem> {
    // Dedupe by uid
    let mut by_uid: HashMap<String, eventsv1::Event> = HashMap::new();
    let mut no_uid: Vec<eventsv1::Event> = Vec::new();
    for e in raw {
        if let Some(uid) = e.metadata.uid.clone() {
            by_uid.insert(uid, e);
        } else {
            no_uid.push(e);
        }
    }
    let mut merged: Vec<eventsv1::Event> = by_uid.into_values().collect();

    let mut seen_weak: HashSet<(String, String, String, String, String)> = HashSet::new();
    for e in no_uid {
        let weak = weak_dedup_key_v1(&e);
        if seen_weak.insert(weak) {
            merged.push(e);
        }
    }

    merged.sort_by_key(|b| Reverse(event_v1_sort_key(b)));
    merged.truncate(MAX_EVENTS_SNAPSHOT);
    merged.into_iter().map(events_v1_to_item).collect()
}

fn weak_dedup_key_v1(e: &eventsv1::Event) -> (String, String, String, String, String) {
    let kind = e
        .regarding
        .as_ref()
        .and_then(|r| r.kind.as_ref())
        .cloned()
        .unwrap_or_default();
    let name = e
        .regarding
        .as_ref()
        .and_then(|r| r.name.as_ref())
        .cloned()
        .unwrap_or_default();
    let reason = e.reason.clone().unwrap_or_default();
    let first = e
        .deprecated_first_timestamp
        .as_ref()
        .map(|t| t.0.to_rfc3339())
        .unwrap_or_default();
    let msg = e.note.clone().unwrap_or_default();
    (kind, name, reason, first, msg)
}

fn event_v1_sort_key(e: &eventsv1::Event) -> chrono::DateTime<chrono::Utc> {
    if let Some(ref et) = e.event_time {
        return et.0;
    }
    if let Some(ref s) = e.series {
        return s.last_observed_time.0;
    }
    if let Some(ref lt) = e.deprecated_last_timestamp {
        return lt.0;
    }
    if let Some(ref ft) = e.deprecated_first_timestamp {
        return ft.0;
    }
    chrono::DateTime::from_timestamp(0, 0).unwrap_or_else(chrono::Utc::now)
}

fn events_v1_to_item(e: eventsv1::Event) -> EventItem {
    let kind = e
        .regarding
        .as_ref()
        .and_then(|r| r.kind.clone())
        .unwrap_or_default();
    let name = e
        .regarding
        .as_ref()
        .and_then(|r| r.name.clone())
        .unwrap_or_default();
    let count = e
        .series
        .as_ref()
        .map(|s| s.count)
        .or(e.deprecated_count)
        .unwrap_or(0);
    EventItem {
        event_type: e.type_.unwrap_or_default(),
        reason: e.reason.unwrap_or_default(),
        message: e.note.unwrap_or_default(),
        involved_object: format!("{}/{}", kind, name),
        first_timestamp: e.deprecated_first_timestamp.map(|ts| ts.0.to_rfc3339()),
        last_timestamp: e
            .deprecated_last_timestamp
            .map(|ts| ts.0.to_rfc3339())
            .or_else(|| e.event_time.map(|ts| ts.0.to_rfc3339())),
        count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn mk_event_v1(kind: &str, name: &str, uid: Option<&str>) -> eventsv1::Event {
        eventsv1::Event {
            regarding: Some(corev1::ObjectReference {
                kind: Some(kind.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            }),
            metadata: ObjectMeta {
                uid: uid.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn merge_dedupes_identical_uid() {
        let raw = vec![
            mk_event_v1("Pod", "p1", Some("uid-a")),
            mk_event_v1("Pod", "p1", Some("uid-a")),
        ];
        let items = merge_events_v1(raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].involved_object, "Pod/p1");
    }
}
