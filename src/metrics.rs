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

use crate::http_admission::{AdmissionEndpoint, AdmissionReason};
use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[derive(Default)]
struct Metrics {
    reconcile_total: Mutex<BTreeMap<String, u64>>,
    reconcile_duration: Mutex<BTreeMap<String, DurationSummary>>,
    reconcile_requeues_total: Mutex<BTreeMap<String, u64>>,
    reconcile_inflight: AtomicU64,
    operator_leader: AtomicU64,
    sts_requests_total: Mutex<BTreeMap<String, u64>>,
    sts_request_duration: Mutex<BTreeMap<String, DurationSummary>>,
    unauthenticated_admission_rejections_total: Mutex<BTreeMap<AdmissionKey, u64>>,
    unauthenticated_requests_total: Mutex<BTreeMap<UnauthenticatedRequestKey, u64>>,
    http_requests_total: Mutex<BTreeMap<HttpKey, u64>>,
    http_request_duration: Mutex<BTreeMap<HttpKey, DurationSummary>>,
    tenant_monitor_polls_total: Mutex<BTreeMap<String, u64>>,
    tenant_monitor_poll_duration: Mutex<BTreeMap<String, DurationSummary>>,
    tenant_storage: Mutex<BTreeMap<TenantKey, TenantStorageSnapshot>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HttpKey {
    component: String,
    method: String,
    status: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AdmissionKey {
    endpoint: &'static str,
    reason: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct UnauthenticatedRequestKey {
    endpoint: &'static str,
    outcome: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnauthenticatedRequestOutcome {
    Success,
    Error,
    Rejected(AdmissionReason),
}

impl UnauthenticatedRequestOutcome {
    pub(crate) fn from_status(status: StatusCode) -> Self {
        if status.is_success() {
            Self::Success
        } else {
            Self::Error
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Rejected(_) => "rejected",
        }
    }

    fn rejection_reason(self) -> Option<AdmissionReason> {
        match self {
            Self::Rejected(reason) => Some(reason),
            Self::Success | Self::Error => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TenantKey {
    namespace: String,
    tenant: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct DurationSummary {
    count: u64,
    sum_seconds: f64,
}

impl DurationSummary {
    fn observe(&mut self, duration: Duration) {
        self.count += 1;
        self.sum_seconds += duration.as_secs_f64();
    }
}

#[derive(Clone, Debug, Default)]
struct TenantStorageSnapshot {
    poll_success: bool,
    last_poll_timestamp_seconds: u64,
    online_drives: u64,
    offline_drives: u64,
    healing_drives: u64,
    raw_capacity_bytes: u64,
    raw_used_bytes: u64,
    object_usage_bytes: u64,
    write_quorum_drives: u64,
    healthy: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TenantStorageMetrics {
    pub online_drives: u64,
    pub offline_drives: u64,
    pub healing_drives: u64,
    pub raw_capacity_bytes: u64,
    pub raw_used_bytes: u64,
    pub object_usage_bytes: u64,
    pub write_quorum_drives: u64,
    pub healthy: bool,
}

fn metrics() -> &'static Metrics {
    static METRICS: OnceLock<Metrics> = OnceLock::new();
    METRICS.get_or_init(|| Metrics {
        unauthenticated_admission_rejections_total: Mutex::new(
            initial_admission_rejection_counters(),
        ),
        unauthenticated_requests_total: Mutex::new(initial_unauthenticated_request_counters()),
        ..Metrics::default()
    })
}

fn initial_admission_rejection_counters() -> BTreeMap<AdmissionKey, u64> {
    let mut counters = BTreeMap::new();
    for endpoint in [AdmissionEndpoint::Sts, AdmissionEndpoint::ConsoleLogin] {
        for reason in [
            AdmissionReason::RateLimit,
            AdmissionReason::ConcurrencyLimit,
            AdmissionReason::BodyTooLarge,
            AdmissionReason::BodyReadFailure,
            AdmissionReason::Timeout,
        ] {
            counters.insert(
                AdmissionKey {
                    endpoint: endpoint.as_str(),
                    reason: reason.as_str(),
                },
                0,
            );
        }
    }
    counters
}

fn initial_unauthenticated_request_counters() -> BTreeMap<UnauthenticatedRequestKey, u64> {
    let mut counters = BTreeMap::new();
    for endpoint in [AdmissionEndpoint::Sts, AdmissionEndpoint::ConsoleLogin] {
        for outcome in ["success", "error", "rejected"] {
            counters.insert(
                UnauthenticatedRequestKey {
                    endpoint: endpoint.as_str(),
                    outcome,
                },
                0,
            );
        }
    }
    counters
}

pub fn set_operator_leader(is_leader: bool) {
    metrics()
        .operator_leader
        .store(u64::from(is_leader), Ordering::Relaxed);
}

pub fn reconcile_started() -> Instant {
    metrics().reconcile_inflight.fetch_add(1, Ordering::Relaxed);
    Instant::now()
}

pub fn reconcile_finished(success: bool, duration: Duration) {
    let result = result_label(success);
    metrics().reconcile_inflight.fetch_sub(1, Ordering::Relaxed);
    increment_string_counter(&metrics().reconcile_total, result);
    observe_string_duration(&metrics().reconcile_duration, result, duration);
}

pub fn record_reconcile_requeue(duration: Duration) {
    let delay = duration.as_secs().to_string();
    increment_string_counter(&metrics().reconcile_requeues_total, &delay);
}

pub fn record_sts_request(success: bool, duration: Duration) {
    let result = result_label(success);
    increment_string_counter(&metrics().sts_requests_total, result);
    observe_string_duration(&metrics().sts_request_duration, result, duration);
    #[cfg(test)]
    let _ = TEST_REQUEST_METRICS.try_with(|captured| {
        captured.borrow_mut().sts_request_results.push(success);
    });
}

pub(crate) fn record_unauthenticated_request(
    endpoint: AdmissionEndpoint,
    outcome: UnauthenticatedRequestOutcome,
) {
    let key = UnauthenticatedRequestKey {
        endpoint: endpoint.as_str(),
        outcome: outcome.as_str(),
    };
    let mut counters = metrics()
        .unauthenticated_requests_total
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counters.entry(key).or_default() += 1;
    drop(counters);

    if let Some(reason) = outcome.rejection_reason() {
        let key = AdmissionKey {
            endpoint: endpoint.as_str(),
            reason: reason.as_str(),
        };
        let mut counters = metrics()
            .unauthenticated_admission_rejections_total
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *counters.entry(key).or_default() += 1;
    }

    #[cfg(test)]
    let _ = TEST_REQUEST_METRICS.try_with(|captured| {
        captured
            .borrow_mut()
            .unauthenticated_requests
            .push((endpoint, outcome));
    });
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TestRequestMetrics {
    pub(crate) sts_request_results: Vec<bool>,
    pub(crate) unauthenticated_requests: Vec<(AdmissionEndpoint, UnauthenticatedRequestOutcome)>,
}

#[cfg(test)]
tokio::task_local! {
    static TEST_REQUEST_METRICS: std::cell::RefCell<TestRequestMetrics>;
}

#[cfg(test)]
pub(crate) async fn capture_request_metrics<F>(future: F) -> (F::Output, TestRequestMetrics)
where
    F: std::future::Future,
{
    TEST_REQUEST_METRICS
        .scope(
            std::cell::RefCell::new(TestRequestMetrics::default()),
            async {
                let output = future.await;
                let captured = TEST_REQUEST_METRICS.with(|metrics| metrics.borrow().clone());
                (output, captured)
            },
        )
        .await
}

pub fn record_http_request(component: &str, method: &str, status: StatusCode, duration: Duration) {
    let key = HttpKey {
        component: component.to_string(),
        method: http_method_label(method).to_string(),
        status: status_class(status).to_string(),
    };

    {
        let mut counters = metrics()
            .http_requests_total
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *counters.entry(key.clone()).or_default() += 1;
    }

    let mut summaries = metrics()
        .http_request_duration
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    summaries.entry(key).or_default().observe(duration);
}

pub async fn record_console_http(request: Request, next: Next) -> Response {
    record_component_http("console", request, next).await
}

pub async fn record_operator_http(request: Request, next: Next) -> Response {
    record_component_http("operator", request, next).await
}

async fn record_component_http(component: &'static str, request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_string();
    let started = Instant::now();
    let response = next.run(request).await;
    record_http_request(component, &method, response.status(), started.elapsed());
    response
}

pub fn record_tenant_monitor_poll(result: &str, duration: Duration) {
    increment_string_counter(&metrics().tenant_monitor_polls_total, result);
    observe_string_duration(&metrics().tenant_monitor_poll_duration, result, duration);
}

pub fn record_tenant_monitor_skipped(namespace: &str, tenant: &str, duration: Duration) {
    record_tenant_monitor_poll("skipped", duration);
    update_tenant_storage_snapshot(
        namespace,
        tenant,
        TenantStorageSnapshot {
            poll_success: false,
            last_poll_timestamp_seconds: unix_timestamp_seconds(),
            ..Default::default()
        },
    );
}

pub fn record_tenant_storage(namespace: &str, tenant: &str, storage: TenantStorageMetrics) {
    update_tenant_storage_snapshot(
        namespace,
        tenant,
        TenantStorageSnapshot {
            poll_success: true,
            last_poll_timestamp_seconds: unix_timestamp_seconds(),
            online_drives: storage.online_drives,
            offline_drives: storage.offline_drives,
            healing_drives: storage.healing_drives,
            raw_capacity_bytes: storage.raw_capacity_bytes,
            raw_used_bytes: storage.raw_used_bytes,
            object_usage_bytes: storage.object_usage_bytes,
            write_quorum_drives: storage.write_quorum_drives,
            healthy: storage.healthy,
        },
    );
}

pub fn record_tenant_storage_error(namespace: &str, tenant: &str) {
    let key = TenantKey {
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
    };
    let mut snapshots = metrics()
        .tenant_storage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let snapshot = snapshots.entry(key).or_default();
    snapshot.poll_success = false;
    snapshot.last_poll_timestamp_seconds = unix_timestamp_seconds();
}

pub fn prune_tenant_storage(active_tenants: &[(String, String)]) {
    let active_tenants = active_tenants
        .iter()
        .map(|(namespace, tenant)| TenantKey {
            namespace: namespace.clone(),
            tenant: tenant.clone(),
        })
        .collect::<BTreeSet<_>>();

    let mut snapshots = metrics()
        .tenant_storage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    snapshots.retain(|key, _| active_tenants.contains(key));
}

pub async fn handler() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
    );
    (headers, render())
}

pub fn render() -> String {
    let mut output = String::new();

    render_string_counter(
        &mut output,
        "rustfs_operator_reconcile_total",
        "Total number of Tenant reconcile attempts by result.",
        "result",
        &metrics().reconcile_total,
    );
    render_string_duration_summary(
        &mut output,
        "rustfs_operator_reconcile_duration_seconds",
        "Tenant reconcile handler duration by result.",
        "result",
        &metrics().reconcile_duration,
    );
    render_gauge(
        &mut output,
        "rustfs_operator_reconcile_inflight",
        "Number of reconcile handlers currently running.",
        metrics().reconcile_inflight.load(Ordering::Relaxed) as f64,
    );
    render_string_counter(
        &mut output,
        "rustfs_operator_reconcile_requeues_total",
        "Total number of error-policy reconcile requeues by delay in seconds.",
        "delay_seconds",
        &metrics().reconcile_requeues_total,
    );
    render_gauge(
        &mut output,
        "rustfs_operator_leader",
        "Whether this process is the active operator leader.",
        metrics().operator_leader.load(Ordering::Relaxed) as f64,
    );

    render_string_counter(
        &mut output,
        "rustfs_operator_sts_requests_total",
        "Total number of operator STS requests by result.",
        "result",
        &metrics().sts_requests_total,
    );
    render_string_duration_summary(
        &mut output,
        "rustfs_operator_sts_request_duration_seconds",
        "Operator STS request duration by result.",
        "result",
        &metrics().sts_request_duration,
    );
    render_admission_rejection_counter(&mut output);
    render_unauthenticated_request_counter(&mut output);

    render_http_counter(&mut output);
    render_http_duration_summary(&mut output);

    render_string_counter(
        &mut output,
        "rustfs_operator_tenant_monitor_polls_total",
        "Total number of tenant storage monitor polls by result.",
        "result",
        &metrics().tenant_monitor_polls_total,
    );
    render_string_duration_summary(
        &mut output,
        "rustfs_operator_tenant_monitor_poll_duration_seconds",
        "Tenant storage monitor poll duration by result.",
        "result",
        &metrics().tenant_monitor_poll_duration,
    );
    render_tenant_storage(&mut output);

    output
}

fn render_admission_rejection_counter(output: &mut String) {
    let name = "rustfs_operator_unauthenticated_admission_rejections_total";
    output.push_str(&format!(
        "# HELP {name} Total number of unauthenticated endpoint requests rejected by admission control.\n# TYPE {name} counter\n"
    ));
    let counters = metrics()
        .unauthenticated_admission_rejections_total
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (key, value) in counters.iter() {
        output.push_str(&format!(
            "{name}{{{}}} {}\n",
            labels(&[("endpoint", key.endpoint), ("reason", key.reason)]),
            value
        ));
    }
}

fn render_unauthenticated_request_counter(output: &mut String) {
    let name = "rustfs_operator_unauthenticated_requests_total";
    output.push_str(&format!(
        "# HELP {name} Total number of requests to unauthenticated control-plane-backed endpoints.\n# TYPE {name} counter\n"
    ));
    let counters = metrics()
        .unauthenticated_requests_total
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (key, value) in counters.iter() {
        output.push_str(&format!(
            "{name}{{{}}} {}\n",
            labels(&[("endpoint", key.endpoint), ("outcome", key.outcome)]),
            value
        ));
    }
}

fn update_tenant_storage_snapshot(namespace: &str, tenant: &str, snapshot: TenantStorageSnapshot) {
    let key = TenantKey {
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
    };
    let mut snapshots = metrics()
        .tenant_storage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    snapshots.insert(key, snapshot);
}

fn increment_string_counter(counters: &Mutex<BTreeMap<String, u64>>, label_value: &str) {
    let mut counters = counters
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counters.entry(label_value.to_string()).or_default() += 1;
}

fn observe_string_duration(
    summaries: &Mutex<BTreeMap<String, DurationSummary>>,
    label_value: &str,
    duration: Duration,
) {
    let mut summaries = summaries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    summaries
        .entry(label_value.to_string())
        .or_default()
        .observe(duration);
}

fn render_string_counter(
    output: &mut String,
    name: &str,
    help: &str,
    label_name: &str,
    counters: &Mutex<BTreeMap<String, u64>>,
) {
    output.push_str(&format!("# HELP {name} {help}\n# TYPE {name} counter\n"));
    let counters = counters
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (label_value, value) in counters.iter() {
        output.push_str(&format!(
            "{name}{{{}}} {}\n",
            labels(&[(label_name, label_value)]),
            value
        ));
    }
}

fn render_string_duration_summary(
    output: &mut String,
    name: &str,
    help: &str,
    label_name: &str,
    summaries: &Mutex<BTreeMap<String, DurationSummary>>,
) {
    output.push_str(&format!("# HELP {name} {help}\n# TYPE {name} summary\n"));
    let summaries = summaries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (label_value, summary) in summaries.iter() {
        let label = labels(&[(label_name, label_value)]);
        output.push_str(&format!(
            "{name}_count{{{label}}} {}\n{name}_sum{{{label}}} {:.6}\n",
            summary.count, summary.sum_seconds
        ));
    }
}

fn render_gauge(output: &mut String, name: &str, help: &str, value: f64) {
    output.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {:.6}\n",
        value
    ));
}

fn render_http_counter(output: &mut String) {
    let name = "rustfs_operator_http_requests_total";
    output.push_str(&format!(
        "# HELP {name} Total number of HTTP requests served by operator components.\n# TYPE {name} counter\n"
    ));
    let counters = metrics()
        .http_requests_total
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (key, value) in counters.iter() {
        output.push_str(&format!(
            "{name}{{{}}} {}\n",
            labels(&[
                ("component", &key.component),
                ("method", &key.method),
                ("status", &key.status),
            ]),
            value
        ));
    }
}

fn render_http_duration_summary(output: &mut String) {
    let name = "rustfs_operator_http_request_duration_seconds";
    output.push_str(&format!(
        "# HELP {name} HTTP request duration served by operator components.\n# TYPE {name} summary\n"
    ));
    let summaries = metrics()
        .http_request_duration
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (key, summary) in summaries.iter() {
        let label = labels(&[
            ("component", &key.component),
            ("method", &key.method),
            ("status", &key.status),
        ]);
        output.push_str(&format!(
            "{name}_count{{{label}}} {}\n{name}_sum{{{label}}} {:.6}\n",
            summary.count, summary.sum_seconds
        ));
    }
}

fn render_tenant_storage(output: &mut String) {
    let snapshots = metrics()
        .tenant_storage
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_storage_poll_success",
        "Whether the last tenant storage poll succeeded.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, if snapshot.poll_success { 1.0 } else { 0.0 })),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_storage_last_poll_timestamp_seconds",
        "Unix timestamp of the last tenant storage poll.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.last_poll_timestamp_seconds as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_drives_online",
        "Number of RustFS drives reported online for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.online_drives as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_drives_offline",
        "Number of RustFS drives reported offline for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.offline_drives as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_drives_healing",
        "Number of RustFS drives currently healing for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.healing_drives as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_raw_capacity_bytes",
        "Raw RustFS capacity in bytes for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.raw_capacity_bytes as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_raw_used_bytes",
        "Raw RustFS used capacity in bytes for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.raw_used_bytes as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_object_usage_bytes",
        "Object usage in bytes reported by RustFS for a tenant.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.object_usage_bytes as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_erasure_write_quorum_drives",
        "Estimated write quorum drive count implied by RustFS erasure parity.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, snapshot.write_quorum_drives as f64)),
    );
    render_tenant_gauge_family(
        output,
        "rustfs_operator_tenant_storage_healthy",
        "Whether tenant storage is online, not healing, and satisfies write quorum.",
        snapshots
            .iter()
            .map(|(key, snapshot)| (key, if snapshot.healthy { 1.0 } else { 0.0 })),
    );
}

fn render_tenant_gauge_family<'a>(
    output: &mut String,
    name: &str,
    help: &str,
    values: impl Iterator<Item = (&'a TenantKey, f64)>,
) {
    output.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
    for (key, value) in values {
        output.push_str(&format!(
            "{name}{{{}}} {:.6}\n",
            labels(&[("namespace", &key.namespace), ("tenant", &key.tenant)]),
            value
        ));
    }
}

fn labels(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}=\"{}\"", escape_label_value(value)))
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_label_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

fn http_method_label(method: &str) -> &'static str {
    match method {
        "CONNECT" => "CONNECT",
        "DELETE" => "DELETE",
        "GET" => "GET",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        "PATCH" => "PATCH",
        "POST" => "POST",
        "PUT" => "PUT",
        "TRACE" => "TRACE",
        _ => "OTHER",
    }
}

fn result_label(success: bool) -> &'static str {
    if success { "success" } else { "error" }
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware};
    use tower::ServiceExt;

    #[test]
    fn labels_escape_prometheus_special_characters() {
        assert_eq!(
            labels(&[("tenant", "a\"b\\c\nd")]),
            "tenant=\"a\\\"b\\\\c\\nd\""
        );
    }

    #[test]
    fn status_class_groups_http_codes() {
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::INTERNAL_SERVER_ERROR), "5xx");
    }

    #[test]
    fn http_method_labels_are_bounded() {
        for method in [
            "CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE",
        ] {
            assert_eq!(http_method_label(method), method);
        }
        assert_eq!(http_method_label("CUSTOM-A"), "OTHER");
        assert_eq!(http_method_label("CUSTOM-B"), "OTHER");
    }

    #[tokio::test]
    async fn component_http_metrics_group_custom_methods_as_other() {
        let console_method = "CONSOLE-CUSTOM-METHOD";
        let console = Router::new()
            .fallback(|| async { StatusCode::NO_CONTENT })
            .layer(middleware::from_fn(record_console_http));
        let console_response = match console.oneshot(request_with_method(console_method)).await {
            Ok(response) => response,
            Err(error) => match error {},
        };

        let operator_method = "OPERATOR-CUSTOM-METHOD";
        let operator = Router::new()
            .fallback(|| async { StatusCode::NO_CONTENT })
            .layer(middleware::from_fn(record_operator_http));
        let operator_response = match operator.oneshot(request_with_method(operator_method)).await {
            Ok(response) => response,
            Err(error) => match error {},
        };

        assert_eq!(console_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(operator_response.status(), StatusCode::NO_CONTENT);

        let rendered = render();
        assert!(rendered.contains(
            "rustfs_operator_http_requests_total{component=\"console\",method=\"OTHER\",status=\"2xx\"}"
        ));
        assert!(rendered.contains(
            "rustfs_operator_http_requests_total{component=\"operator\",method=\"OTHER\",status=\"2xx\"}"
        ));
        assert!(!rendered.contains(console_method));
        assert!(!rendered.contains(operator_method));
    }

    fn request_with_method(method: &str) -> Request {
        Request::builder()
            .method(method)
            .uri("/")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("custom HTTP method should build a request: {error}"))
    }

    #[test]
    fn admission_rejection_metrics_use_fixed_labels() {
        record_unauthenticated_request(
            AdmissionEndpoint::Sts,
            UnauthenticatedRequestOutcome::Rejected(AdmissionReason::RateLimit),
        );

        assert!(render().contains(
            "rustfs_operator_unauthenticated_admission_rejections_total{endpoint=\"sts\",reason=\"rate_limit\"}"
        ));
    }

    #[test]
    fn unauthenticated_request_metrics_use_fixed_endpoint_and_outcome_labels() {
        record_unauthenticated_request(
            AdmissionEndpoint::ConsoleLogin,
            UnauthenticatedRequestOutcome::Success,
        );
        record_unauthenticated_request(
            AdmissionEndpoint::Sts,
            UnauthenticatedRequestOutcome::Rejected(AdmissionReason::Timeout),
        );

        let rendered = render();
        assert!(rendered.contains(
            "rustfs_operator_unauthenticated_requests_total{endpoint=\"console_login\",outcome=\"success\"}"
        ));
        assert!(rendered.contains(
            "rustfs_operator_unauthenticated_requests_total{endpoint=\"sts\",outcome=\"rejected\"}"
        ));
    }

    #[test]
    fn unauthenticated_request_outcome_keeps_downstream_errors_distinct() {
        assert_eq!(
            UnauthenticatedRequestOutcome::from_status(StatusCode::OK),
            UnauthenticatedRequestOutcome::Success
        );
        assert_eq!(
            UnauthenticatedRequestOutcome::from_status(StatusCode::BAD_REQUEST),
            UnauthenticatedRequestOutcome::Error
        );
        assert_eq!(
            UnauthenticatedRequestOutcome::from_status(StatusCode::INTERNAL_SERVER_ERROR),
            UnauthenticatedRequestOutcome::Error
        );
    }

    #[test]
    fn unauthenticated_metric_series_have_zero_baselines() {
        let rejection_counters = initial_admission_rejection_counters();
        let request_counters = initial_unauthenticated_request_counters();

        assert_eq!(rejection_counters.len(), 10);
        assert!(rejection_counters.values().all(|value| *value == 0));
        assert_eq!(request_counters.len(), 6);
        assert!(request_counters.values().all(|value| *value == 0));
    }

    #[test]
    fn prune_tenant_storage_removes_absent_snapshots() {
        let active = "prune-active";
        let stale = "prune-stale";
        let namespace = "prune-namespace";

        record_tenant_storage(
            namespace,
            active,
            TenantStorageMetrics {
                online_drives: 4,
                healthy: true,
                ..Default::default()
            },
        );
        record_tenant_storage(
            namespace,
            stale,
            TenantStorageMetrics {
                online_drives: 4,
                healthy: true,
                ..Default::default()
            },
        );

        prune_tenant_storage(&[(namespace.to_string(), active.to_string())]);

        let rendered = render();
        assert!(rendered.contains("tenant=\"prune-active\""));
        assert!(!rendered.contains("tenant=\"prune-stale\""));
    }
}
