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

//! OpenAPI documentation for RustFS Console API
//!
//! The api_* functions below are documentation stubs only; they are never called.
//! They exist solely for the #[utoipa::path] macro to generate the OpenAPI spec.

use axum::Json;
use utoipa::OpenApi;

use crate::console::models::auth::{LoginRequest, LoginResponse, SessionResponse};
use crate::console::models::cluster::{
    ClusterResourcesResponse, CreateNamespaceRequest, NamespaceItem, NamespaceListResponse,
    NodeInfo, NodeListResponse,
};
use crate::console::models::common::{
    ConsoleActionResponse, ConsoleErrorDetails, ConsoleErrorResponse,
};
use crate::console::models::event::{EventItem, EventListResponse};
use crate::console::models::pod::{
    ContainerInfo, ContainerState, DeletePodResponse, LogsQuery, PodCondition, PodDetails,
    PodListItem, PodListResponse, PodStatus, RestartPodRequest, VolumeInfo,
};
use crate::console::models::pool::{
    AddPoolRequest, AddPoolResponse, DeletePoolResponse, PoolDetails, PoolListResponse,
    ResourceList, ResourceRequirements,
};
use crate::console::models::tenant::{
    CreatePoolRequest, CreateTenantRequest, DeleteTenantResponse, EnvVar, LoggingConfig, PoolInfo,
    ServiceInfo, ServicePort, TenantCondition, TenantDetailsResponse, TenantListItem,
    TenantListQuery, TenantListResponse, TenantStateCountsResponse, TenantStatusSummary,
    TenantYAML, UpdateTenantRequest, UpdateTenantResponse,
};
use crate::console::models::topology::{
    TopologyCluster, TopologyClusterSummary, TopologyNamespace, TopologyNode,
    TopologyOverviewResponse, TopologyPod, TopologyPool, TopologyTenant, TopologyTenantSummary,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        api_login,
        api_logout,
        api_session,
        api_list_tenants,
        api_get_tenant_state_counts,
        api_create_tenant,
        api_list_tenants_by_ns,
        api_get_tenant_state_counts_by_ns,
        api_get_tenant,
        api_update_tenant,
        api_delete_tenant,
        api_get_tenant_yaml,
        api_put_tenant_yaml,
        api_list_pools,
        api_add_pool,
        api_delete_pool,
        api_list_pods,
        api_get_pod,
        api_delete_pod,
        api_restart_pod,
        api_get_pod_logs,
        api_stream_tenant_events,
        api_list_nodes,
        api_get_cluster_resources,
        api_list_namespaces,
        api_create_namespace,
        api_get_topology_overview,
    ),
    components(schemas(
        LoginRequest,
        LoginResponse,
        SessionResponse,
        ConsoleErrorResponse,
        ConsoleErrorDetails,
        ConsoleActionResponse,
        TenantListItem,
        TenantListResponse,
        TenantListQuery,
        TenantStateCountsResponse,
        TenantCondition,
        TenantStatusSummary,
        TenantDetailsResponse,
        CreateTenantRequest,
        CreatePoolRequest,
        PoolInfo,
        ServiceInfo,
        ServicePort,
        EnvVar,
        LoggingConfig,
        UpdateTenantRequest,
        UpdateTenantResponse,
        DeleteTenantResponse,
        TenantYAML,
        PoolDetails,
        PoolListResponse,
        AddPoolRequest,
        ResourceRequirements,
        ResourceList,
        AddPoolResponse,
        DeletePoolResponse,
        PodListItem,
        PodListResponse,
        PodDetails,
        PodStatus,
        PodCondition,
        ContainerInfo,
        ContainerState,
        VolumeInfo,
        RestartPodRequest,
        LogsQuery,
        EventItem,
        EventListResponse,
        NodeInfo,
        NodeListResponse,
        ClusterResourcesResponse,
        NamespaceItem,
        NamespaceListResponse,
        CreateNamespaceRequest,
        TopologyOverviewResponse,
        TopologyCluster,
        TopologyClusterSummary,
        TopologyNamespace,
        TopologyTenant,
        TopologyTenantSummary,
        TopologyPool,
        TopologyPod,
        TopologyNode,
    )),
    tags(
        (name = "auth", description = "Authentication"),
        (name = "tenants", description = "Tenant management"),
        (name = "pools", description = "Pool management"),
        (name = "pods", description = "Pod management"),
        (name = "events", description = "Event management"),
        (name = "cluster", description = "Cluster resources"),
        (name = "topology", description = "Cluster topology overview"),
    ),
    info(
        title = "RustFS Console API",
        version = "v1",
        description = "RustFS Operator Console REST API for managing RustFS storage clusters",
    ),
)]
pub struct ApiDoc;

// --- Auth ---
#[utoipa::path(post, path = "/api/v1/login", request_body = LoginRequest, responses((status = 200, body = LoginResponse)), tag = "auth")]
fn api_login(_body: Json<LoginRequest>) -> Json<LoginResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(post, path = "/api/v1/logout", responses((status = 200)), tag = "auth")]
fn api_logout() {}

#[utoipa::path(get, path = "/api/v1/session", responses((status = 200, body = SessionResponse)), tag = "auth")]
fn api_session() -> Json<SessionResponse> {
    unimplemented!("Documentation only")
}

// --- Tenants ---
#[utoipa::path(
    get,
    path = "/api/v1/tenants",
    params(("state" = Option<String>, Query, description = "Filter by tenant state (case-insensitive)")),
    responses((status = 200, body = TenantListResponse)),
    tag = "tenants"
)]
fn api_list_tenants() -> Json<TenantListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/tenants/state-counts", responses((status = 200, body = TenantStateCountsResponse)), tag = "tenants")]
fn api_get_tenant_state_counts() -> Json<TenantStateCountsResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(post, path = "/api/v1/tenants", request_body = CreateTenantRequest, responses((status = 200, body = TenantListItem)), tag = "tenants")]
fn api_create_tenant(_body: Json<CreateTenantRequest>) -> Json<TenantListItem> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    get,
    path = "/api/v1/namespaces/{namespace}/tenants",
    params(
        ("namespace" = String, Path, description = "Namespace"),
        ("state" = Option<String>, Query, description = "Filter by tenant state (case-insensitive)")
    ),
    responses((status = 200, body = TenantListResponse)),
    tag = "tenants"
)]
fn api_list_tenants_by_ns() -> Json<TenantListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/state-counts", params(("namespace" = String, Path, description = "Namespace")), responses((status = 200, body = TenantStateCountsResponse)), tag = "tenants")]
fn api_get_tenant_state_counts_by_ns() -> Json<TenantStateCountsResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = TenantDetailsResponse)), tag = "tenants")]
fn api_get_tenant() -> Json<TenantDetailsResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(put, path = "/api/v1/namespaces/{namespace}/tenants/{name}", params(("namespace" = String, Path), ("name" = String, Path)), request_body = UpdateTenantRequest, responses((status = 200, body = UpdateTenantResponse)), tag = "tenants")]
fn api_update_tenant(_body: Json<UpdateTenantRequest>) -> Json<UpdateTenantResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(delete, path = "/api/v1/namespaces/{namespace}/tenants/{name}", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = DeleteTenantResponse)), tag = "tenants")]
fn api_delete_tenant() -> Json<DeleteTenantResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/yaml", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = TenantYAML)), tag = "tenants")]
fn api_get_tenant_yaml() -> Json<TenantYAML> {
    unimplemented!("Documentation only")
}

#[utoipa::path(put, path = "/api/v1/namespaces/{namespace}/tenants/{name}/yaml", params(("namespace" = String, Path), ("name" = String, Path)), request_body = TenantYAML, responses((status = 200, body = TenantYAML)), tag = "tenants")]
fn api_put_tenant_yaml(_body: Json<TenantYAML>) -> Json<TenantYAML> {
    unimplemented!("Documentation only")
}

// --- Pools ---
#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = PoolListResponse)), tag = "pools")]
fn api_list_pools() -> Json<PoolListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(post, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools", params(("namespace" = String, Path), ("name" = String, Path)), request_body = AddPoolRequest, responses((status = 200, body = AddPoolResponse)), tag = "pools")]
fn api_add_pool(_body: Json<AddPoolRequest>) -> Json<AddPoolResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    delete,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools/{pool}",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("pool" = String, Path)
    ),
    responses(
        (status = 200, body = DeletePoolResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "pools"
)]
fn api_delete_pool() -> Json<DeletePoolResponse> {
    unimplemented!("Documentation only")
}

// --- Pods ---
#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = PodListResponse)), tag = "pods")]
fn api_list_pods() -> Json<PodListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods/{pod}", params(("namespace" = String, Path), ("name" = String, Path), ("pod" = String, Path)), responses((status = 200, body = PodDetails)), tag = "pods")]
fn api_get_pod() -> Json<PodDetails> {
    unimplemented!("Documentation only")
}

#[utoipa::path(delete, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods/{pod}", params(("namespace" = String, Path), ("name" = String, Path), ("pod" = String, Path)), responses((status = 200, body = DeletePodResponse)), tag = "pods")]
fn api_delete_pod() -> Json<DeletePodResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(post, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods/{pod}/restart", params(("namespace" = String, Path), ("name" = String, Path), ("pod" = String, Path)), request_body = RestartPodRequest, responses((status = 200, body = DeletePodResponse)), tag = "pods")]
fn api_restart_pod(_body: Json<RestartPodRequest>) -> Json<DeletePodResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods/{pod}/logs", params(("namespace" = String, Path), ("name" = String, Path), ("pod" = String, Path), ("container" = Option<String>, Query), ("tail_lines" = Option<i64>, Query), ("timestamps" = Option<bool>, Query)), responses((status = 200, description = "Plain text log output", content_type = "text/plain")), tag = "pods")]
fn api_get_pod_logs() {}

// --- Events (SSE) ---
#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{tenant}/events/stream", params(("namespace" = String, Path), ("tenant" = String, Path)), responses((status = 200, description = "text/event-stream; `event: snapshot` + JSON EventListResponse; `event: stream_error` + JSON { message }", body = EventListResponse, content_type = "application/json")), tag = "events")]
fn api_stream_tenant_events() {
    unimplemented!("Documentation only")
}

// --- Cluster ---
#[utoipa::path(get, path = "/api/v1/cluster/nodes", responses((status = 200, body = NodeListResponse)), tag = "cluster")]
fn api_list_nodes() -> Json<NodeListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/cluster/resources", responses((status = 200, body = ClusterResourcesResponse)), tag = "cluster")]
fn api_get_cluster_resources() -> Json<ClusterResourcesResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(get, path = "/api/v1/namespaces", responses((status = 200, body = NamespaceListResponse)), tag = "cluster")]
fn api_list_namespaces() -> Json<NamespaceListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(post, path = "/api/v1/namespaces", request_body = CreateNamespaceRequest, responses((status = 200, body = NamespaceItem)), tag = "cluster")]
fn api_create_namespace(_body: Json<CreateNamespaceRequest>) -> Json<NamespaceItem> {
    unimplemented!("Documentation only")
}

// --- Topology ---
#[utoipa::path(get, path = "/api/v1/topology/overview", responses((status = 200, body = TopologyOverviewResponse)), tag = "topology")]
fn api_get_topology_overview() -> Json<TopologyOverviewResponse> {
    unimplemented!("Documentation only")
}

#[cfg(test)]
mod tests {
    use super::ApiDoc;
    use serde_json::Value;
    use utoipa::OpenApi;

    #[test]
    fn delete_pool_documents_standard_error_responses() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let responses = spec
            .pointer("/paths/~1api~1v1~1namespaces~1{namespace}~1tenants~1{name}~1pools~1{pool}/delete/responses")
            .expect("delete pool responses exist");

        for status in ["400", "401", "403", "404", "409", "500"] {
            let pointer = format!("/{status}/content/application~1json/schema/$ref");
            assert_eq!(
                responses.pointer(&pointer).and_then(Value::as_str),
                Some("#/components/schemas/ConsoleErrorResponse"),
                "status {status} should use ConsoleErrorResponse"
            );
        }
    }
}
