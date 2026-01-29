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

use axum::{routing::{delete, get, post}, Router};

use crate::console::{handlers, state::AppState};

/// 认证路由
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(handlers::auth::login))
        .route("/logout", post(handlers::auth::logout))
        .route("/session", get(handlers::auth::session_check))
}

/// Tenant 管理路由
pub fn tenant_routes() -> Router<AppState> {
    Router::new()
        .route("/tenants", get(handlers::tenants::list_all_tenants))
        .route("/tenants", post(handlers::tenants::create_tenant))
        .route(
            "/namespaces/:namespace/tenants",
            get(handlers::tenants::list_tenants_by_namespace),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            get(handlers::tenants::get_tenant_details),
        )
        .route(
            "/namespaces/:namespace/tenants/:name",
            delete(handlers::tenants::delete_tenant),
        )
}

/// 事件管理路由
pub fn event_routes() -> Router<AppState> {
    Router::new().route(
        "/namespaces/:namespace/tenants/:tenant/events",
        get(handlers::events::list_tenant_events),
    )
}

/// 集群资源路由
pub fn cluster_routes() -> Router<AppState> {
    Router::new()
        .route("/cluster/nodes", get(handlers::cluster::list_nodes))
        .route("/cluster/resources", get(handlers::cluster::get_cluster_resources))
        .route("/namespaces", get(handlers::cluster::list_namespaces))
        .route("/namespaces", post(handlers::cluster::create_namespace))
}
