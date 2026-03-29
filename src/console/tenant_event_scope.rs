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

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1 as corev1;
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

/// Label selector `rustfs.tenant=<tenant>` — must match [`crate::console::handlers::pods::list_pods`].
pub fn tenant_label_selector(tenant: &str) -> String {
    format!("rustfs.tenant={}", tenant)
}

/// Allowed `(kind, name)` pairs for `Event.involvedObject` in this tenant scope.
#[derive(Debug, Clone)]
pub struct TenantEventScope {
    /// Tenant name (scope metadata; filtering uses [`Self::involved`]).
    #[allow(dead_code)]
    pub tenant_name: String,
    /// `(involvedObject.kind, involvedObject.name)` using API kind strings.
    pub involved: HashSet<(String, String)>,
}

impl TenantEventScope {
    /// Returns true if this event's regarding object is in scope.
    pub fn matches_involved(&self, ev: &corev1::Event) -> bool {
        let obj = &ev.involved_object;
        let kind = obj.kind.clone().unwrap_or_default();
        let name = obj.name.clone().unwrap_or_default();
        if kind.is_empty() || name.is_empty() {
            return false;
        }
        self.involved.contains(&(kind, name))
    }
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

/// Filter namespace events to scope, dedupe, sort newest first, cap at [`MAX_EVENTS_SNAPSHOT`].
pub fn merge_namespace_events(scope: &TenantEventScope, raw: Vec<corev1::Event>) -> Vec<EventItem> {
    let filtered: Vec<corev1::Event> = raw
        .into_iter()
        .filter(|e| scope.matches_involved(e))
        .collect();

    // Dedupe by uid
    let mut by_uid: HashMap<String, corev1::Event> = HashMap::new();
    let mut no_uid: Vec<corev1::Event> = Vec::new();
    for e in filtered {
        if let Some(uid) = e.metadata.uid.clone() {
            by_uid.insert(uid, e);
        } else {
            no_uid.push(e);
        }
    }
    let mut merged: Vec<corev1::Event> = by_uid.into_values().collect();

    // Weak dedupe for events without uid
    let mut seen_weak: HashSet<(String, String, String, String, String)> = HashSet::new();
    for e in no_uid {
        let weak = weak_dedup_key(&e);
        if seen_weak.insert(weak) {
            merged.push(e);
        }
    }

    merged.sort_by_key(|b| Reverse(event_sort_key(b)));
    merged.truncate(MAX_EVENTS_SNAPSHOT);
    merged.into_iter().map(core_event_to_item).collect()
}

fn weak_dedup_key(e: &corev1::Event) -> (String, String, String, String, String) {
    let kind = e.involved_object.kind.clone().unwrap_or_default();
    let name = e.involved_object.name.clone().unwrap_or_default();
    let reason = e.reason.clone().unwrap_or_default();
    let first = e
        .first_timestamp
        .as_ref()
        .map(|t| t.0.to_rfc3339())
        .unwrap_or_default();
    let msg = e.message.clone().unwrap_or_default();
    (kind, name, reason, first, msg)
}

fn event_sort_key(e: &corev1::Event) -> chrono::DateTime<chrono::Utc> {
    if let Some(ref et) = e.event_time {
        return et.0;
    }
    if let Some(ref lt) = e.last_timestamp {
        return lt.0;
    }
    if let Some(ref ft) = e.first_timestamp {
        return ft.0;
    }
    chrono::DateTime::from_timestamp(0, 0).unwrap_or_else(chrono::Utc::now)
}

fn core_event_to_item(e: corev1::Event) -> EventItem {
    EventItem {
        event_type: e.type_.unwrap_or_default(),
        reason: e.reason.unwrap_or_default(),
        message: e.message.unwrap_or_default(),
        involved_object: format!(
            "{}/{}",
            e.involved_object.kind.unwrap_or_default(),
            e.involved_object.name.unwrap_or_default()
        ),
        first_timestamp: e.first_timestamp.map(|ts| ts.0.to_rfc3339()),
        last_timestamp: e.last_timestamp.map(|ts| ts.0.to_rfc3339()),
        count: e.count.unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn mk_event(kind: &str, name: &str, uid: Option<&str>) -> corev1::Event {
        corev1::Event {
            involved_object: corev1::ObjectReference {
                kind: Some(kind.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            metadata: ObjectMeta {
                uid: uid.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tenant_kind_required_for_tenant_name() {
        let scope = TenantEventScope {
            tenant_name: "t1".to_string(),
            involved: HashSet::from([
                (TENANT_CR_KIND.to_string(), "t1".to_string()),
                ("Pod".to_string(), "p1".to_string()),
            ]),
        };
        let raw = vec![
            mk_event("Pod", "other", Some("a")),
            mk_event("ConfigMap", "t1", Some("b")),
            mk_event(TENANT_CR_KIND, "t1", Some("c")),
        ];
        let items = merge_namespace_events(&scope, raw);
        let objs: Vec<&str> = items.iter().map(|i| i.involved_object.as_str()).collect();
        assert!(objs.contains(&"Tenant/t1"));
        assert!(!objs.iter().any(|s| *s == "Pod/other"));
        assert!(!objs.iter().any(|s| *s == "ConfigMap/t1"));
    }
}
