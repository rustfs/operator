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

use axum::{
    Router,
    routing::{delete, get, post, put},
};

use crate::console::{handlers, state::AppState};

/// Login / session routes (partially unauthenticated)
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(handlers::auth::login))
        .route("/logout", post(handlers::auth::logout))
        .route("/session", get(handlers::auth::session_check))
}

/// Tenant CRUD, YAML, encryption, security context
pub fn tenant_routes() -> Router<AppState> {
    Router::new()
        .route("/tenants", get(handlers::tenants::list_all_tenants))
        .route(
            "/tenants/state-counts",
            get(handlers::tenants::get_all_tenant_state_counts),
        )
        .route("/tenants", post(handlers::tenants::create_tenant))
        .route(
            "/namespaces/:namespace/tenants",
            get(handlers::tenants::list_tenants_by_namespace),
        )
        .route(
            "/namespaces/:namespace/tenants/state-counts",
            get(handlers::tenants::get_tenant_state_counts_by_namespace),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            get(handlers::tenants::get_tenant_details),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            put(handlers::tenants::update_tenant),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            delete(handlers::tenants::delete_tenant),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/yaml",
            get(handlers::tenants::get_tenant_yaml),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/yaml",
            put(handlers::tenants::put_tenant_yaml),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/encryption",
            get(handlers::encryption::get_encryption),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/encryption",
            put(handlers::encryption::update_encryption),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/security-context",
            get(handlers::security_context::get_security_context),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/security-context",
            put(handlers::security_context::update_security_context),
        )
}

/// Pool list / add / delete under a tenant
pub fn pool_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/namespaces/:namespace/tenants/:name/pools",
            get(handlers::pools::list_pools),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools",
            post(handlers::pools::add_pool),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pools/:pool",
            delete(handlers::pools::delete_pool),
        )
}

/// Pod list, detail, delete, restart, logs
pub fn pod_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/namespaces/:namespace/tenants/:name/pods",
            get(handlers::pods::list_pods),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod",
            get(handlers::pods::get_pod_details),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod",
            delete(handlers::pods::delete_pod),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod/restart",
            post(handlers::pods::restart_pod),
        )
        .route(
            "/namespaces/:namespace/tenants/:name/pods/:pod/logs",
            get(handlers::pods::get_pod_logs),
        )
}

/// Kubernetes events for a tenant (SSE)
pub fn event_routes() -> Router<AppState> {
    Router::new().route(
        "/namespaces/:namespace/tenants/:tenant/events/stream",
        get(handlers::events::stream_tenant_events),
    )
}

/// Nodes, cluster capacity, namespaces
pub fn cluster_routes() -> Router<AppState> {
    Router::new()
        .route("/cluster/nodes", get(handlers::cluster::list_nodes))
        .route(
            "/cluster/resources",
            get(handlers::cluster::get_cluster_resources),
        )
        .route("/namespaces", get(handlers::cluster::list_namespaces))
        .route("/namespaces", post(handlers::cluster::create_namespace))
}

/// Topology overview for the dashboard
pub fn topology_routes() -> Router<AppState> {
    Router::new().route(
        "/topology/overview",
        get(handlers::topology::get_topology_overview),
    )
}
