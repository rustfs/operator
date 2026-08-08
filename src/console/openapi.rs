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
use crate::console::models::encryption::{
    EncryptionInfoResponse, EncryptionUpdateResponse, LocalInfo, LocalMasterKeySecretRefInfo,
    SecurityContextInfo, SecurityContextUpdateResponse, UpdateEncryptionBackend,
    UpdateEncryptionRequest, UpdateLocalRequest, UpdateSecurityContextRequest, UpdateVaultRequest,
    VaultInfo,
};
use crate::console::models::event::{EventItem, EventListResponse};
use crate::console::models::pod::{
    ContainerInfo, ContainerState, DeletePodResponse, LogsQuery, PodCondition, PodDetails,
    PodListItem, PodListResponse, PodStatus, RestartPodRequest, VolumeInfo,
};
use crate::console::models::pool::{
    AddPoolRequest, AddPoolResponse, CancelPoolDecommissionRequest, DeletePoolResponse,
    PoolDecommissionRequestResponse, PoolDetails, PoolListResponse, ResourceList,
    ResourceRequirements, StartPoolDecommissionRequest,
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
use crate::types::v1alpha1::provisioning::{
    ConfigMapKeyReference, PolicyDocumentSource, ProvisioningBucket, ProvisioningDeletionPolicy,
    ProvisioningPolicy, ProvisioningUser, UserCredentialsSecretRef,
};
use crate::types::v1alpha1::status::provisioning::{
    ProvisioningItemState, ProvisioningItemStatus, ProvisioningPhase, ProvisioningStatus,
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
        api_create_tenant_from_yaml,
        api_list_tenants_by_ns,
        api_get_tenant_state_counts_by_ns,
        api_get_tenant,
        api_update_tenant,
        api_delete_tenant,
        api_get_tenant_yaml,
        api_put_tenant_yaml,
        api_get_encryption,
        api_update_encryption,
        api_get_security_context,
        api_update_security_context,
        api_list_pools,
        api_add_pool,
        api_delete_pool,
        api_start_pool_decommission,
        api_cancel_pool_decommission,
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
        ProvisioningStatus,
        ProvisioningPhase,
        ProvisioningItemStatus,
        ProvisioningItemState,
        ProvisioningPolicy,
        ProvisioningUser,
        UserCredentialsSecretRef,
        ProvisioningBucket,
        ProvisioningDeletionPolicy,
        PolicyDocumentSource,
        ConfigMapKeyReference,
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
        EncryptionInfoResponse,
        EncryptionUpdateResponse,
        VaultInfo,
        LocalInfo,
        LocalMasterKeySecretRefInfo,
        UpdateEncryptionRequest,
        UpdateEncryptionBackend,
        UpdateVaultRequest,
        UpdateLocalRequest,
        SecurityContextInfo,
        UpdateSecurityContextRequest,
        SecurityContextUpdateResponse,
        PoolDetails,
        PoolListResponse,
        AddPoolRequest,
        ResourceRequirements,
        ResourceList,
        AddPoolResponse,
        DeletePoolResponse,
        StartPoolDecommissionRequest,
        CancelPoolDecommissionRequest,
        PoolDecommissionRequestResponse,
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
        (name = "encryption", description = "Tenant encryption configuration"),
        (name = "security-context", description = "Tenant pod security context"),
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
#[utoipa::path(
    post,
    path = "/api/v1/login",
    request_body = LoginRequest,
    responses(
        (status = 200, body = LoginResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 429, body = ConsoleErrorResponse),
        (status = 503, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "auth"
)]
fn api_login(_body: Json<LoginRequest>) -> Json<LoginResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    post,
    path = "/api/v1/logout",
    responses(
        (status = 200, body = LoginResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "auth"
)]
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

#[utoipa::path(
    post,
    path = "/api/v1/tenants",
    request_body = CreateTenantRequest,
    responses(
        (status = 200, body = TenantListItem),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "tenants"
)]
fn api_create_tenant(_body: Json<CreateTenantRequest>) -> Json<TenantListItem> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    post,
    path = "/api/v1/tenants/yaml",
    request_body = TenantYAML,
    responses(
        (status = 200, body = TenantListItem),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "tenants"
)]
fn api_create_tenant_from_yaml(_body: Json<TenantYAML>) -> Json<TenantListItem> {
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

#[utoipa::path(
    put,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}",
    params(("namespace" = String, Path), ("name" = String, Path)),
    request_body = UpdateTenantRequest,
    responses(
        (status = 200, body = UpdateTenantResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "tenants"
)]
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

#[utoipa::path(
    put,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/yaml",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path)
    ),
    request_body = TenantYAML,
    responses(
        (status = 200, body = TenantYAML),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "tenants"
)]
fn api_put_tenant_yaml(_body: Json<TenantYAML>) -> Json<TenantYAML> {
    unimplemented!("Documentation only")
}

// --- Encryption ---
#[utoipa::path(
    get,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/encryption",
    params(
        ("namespace" = String, Path, description = "Tenant namespace"),
        ("name" = String, Path, description = "Tenant name")
    ),
    responses(
        (status = 200, body = EncryptionInfoResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "encryption"
)]
fn api_get_encryption() -> Json<EncryptionInfoResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    put,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/encryption",
    params(
        ("namespace" = String, Path, description = "Tenant namespace"),
        ("name" = String, Path, description = "Tenant name")
    ),
    request_body = UpdateEncryptionRequest,
    responses(
        (status = 200, body = EncryptionUpdateResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "encryption"
)]
fn api_update_encryption(_body: Json<UpdateEncryptionRequest>) -> Json<EncryptionUpdateResponse> {
    unimplemented!("Documentation only")
}

// --- Security Context ---
#[utoipa::path(
    get,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/security-context",
    params(
        ("namespace" = String, Path, description = "Tenant namespace"),
        ("name" = String, Path, description = "Tenant name")
    ),
    responses(
        (status = 200, body = SecurityContextInfo),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "security-context"
)]
fn api_get_security_context() -> Json<SecurityContextInfo> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    put,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/security-context",
    params(
        ("namespace" = String, Path, description = "Tenant namespace"),
        ("name" = String, Path, description = "Tenant name")
    ),
    request_body = UpdateSecurityContextRequest,
    responses(
        (status = 200, body = SecurityContextUpdateResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "security-context"
)]
fn api_update_security_context(
    _body: Json<UpdateSecurityContextRequest>,
) -> Json<SecurityContextUpdateResponse> {
    unimplemented!("Documentation only")
}

// --- Pools ---
#[utoipa::path(get, path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools", params(("namespace" = String, Path), ("name" = String, Path)), responses((status = 200, body = PoolListResponse)), tag = "pools")]
fn api_list_pools() -> Json<PoolListResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    post,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools",
    params(("namespace" = String, Path), ("name" = String, Path)),
    request_body = AddPoolRequest,
    responses(
        (status = 200, body = AddPoolResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "pools"
)]
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

#[utoipa::path(
    post,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools/{pool}/decommission",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("pool" = String, Path)
    ),
    request_body = StartPoolDecommissionRequest,
    responses(
        (status = 200, body = PoolDecommissionRequestResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "pools"
)]
fn api_start_pool_decommission(
    _body: Json<StartPoolDecommissionRequest>,
) -> Json<PoolDecommissionRequestResponse> {
    unimplemented!("Documentation only")
}

#[utoipa::path(
    post,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/pools/{pool}/decommission/cancel",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("pool" = String, Path)
    ),
    request_body = CancelPoolDecommissionRequest,
    responses(
        (status = 200, body = PoolDecommissionRequestResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "pools"
)]
fn api_cancel_pool_decommission(
    _body: Json<CancelPoolDecommissionRequest>,
) -> Json<PoolDecommissionRequestResponse> {
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

#[utoipa::path(
    post,
    path = "/api/v1/namespaces/{namespace}/tenants/{name}/pods/{pod}/restart",
    params(
        ("namespace" = String, Path),
        ("name" = String, Path),
        ("pod" = String, Path)
    ),
    request_body = RestartPodRequest,
    responses(
        (status = 200, body = DeletePodResponse),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 404, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "pods"
)]
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

#[utoipa::path(
    post,
    path = "/api/v1/namespaces",
    request_body = CreateNamespaceRequest,
    responses(
        (status = 200, body = NamespaceItem),
        (status = 400, body = ConsoleErrorResponse),
        (status = 413, body = ConsoleErrorResponse),
        (status = 415, body = ConsoleErrorResponse),
        (status = 422, body = ConsoleErrorResponse),
        (status = 401, body = ConsoleErrorResponse),
        (status = 403, body = ConsoleErrorResponse),
        (status = 409, body = ConsoleErrorResponse),
        (status = 500, body = ConsoleErrorResponse)
    ),
    tag = "cluster"
)]
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
    fn every_json_request_documents_the_console_rejection_envelope() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let paths = spec
            .pointer("/paths")
            .and_then(Value::as_object)
            .expect("OpenAPI paths exist");
        let mut request_operations = 0;

        for (path, path_item) in paths {
            for method in ["post", "put", "patch"] {
                let Some(operation) = path_item.get(method) else {
                    continue;
                };
                if operation.get("requestBody").is_none() {
                    continue;
                }
                request_operations += 1;

                for status in ["400", "413", "415", "422"] {
                    assert_eq!(
                        operation
                            .pointer(&format!(
                                "/responses/{status}/content/application~1json/schema/$ref"
                            ))
                            .and_then(Value::as_str),
                        Some("#/components/schemas/ConsoleErrorResponse"),
                        "{method} {path} status {status} should use ConsoleErrorResponse"
                    );
                }
            }
        }

        assert_eq!(request_operations, 12);
    }

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

    #[test]
    fn tenant_api_documents_provisioning_fields() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let schemas = spec
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .expect("schemas exist");

        assert!(schemas.contains_key("ProvisioningStatus"));
        assert!(schemas.contains_key("UserCredentialsSecretRef"));
        assert_eq!(
            spec.pointer("/components/schemas/TenantDetailsResponse/properties/provisioning/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/ProvisioningStatus")
        );
        assert_eq!(
            spec.pointer("/components/schemas/CreateTenantRequest/properties/policies/items/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/ProvisioningPolicy")
        );
        assert_eq!(
            spec.pointer("/components/schemas/UpdateTenantRequest/properties/buckets/items/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/ProvisioningBucket")
        );
        assert_eq!(
            spec.pointer(
                "/components/schemas/ProvisioningUser/properties/credsSecret/oneOf/1/$ref",
            )
            .and_then(Value::as_str),
            Some("#/components/schemas/UserCredentialsSecretRef")
        );
    }

    #[test]
    fn tenant_yaml_create_route_is_documented() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let operation = spec
            .pointer("/paths/~1api~1v1~1tenants~1yaml/post")
            .expect("Tenant YAML create path should exist");

        assert_eq!(
            operation
                .pointer("/requestBody/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/TenantYAML")
        );
        assert_eq!(
            operation
                .pointer("/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/TenantListItem")
        );
        for status in ["400", "413", "415", "422"] {
            assert_eq!(
                operation
                    .pointer(&format!(
                        "/responses/{status}/content/application~1json/schema/$ref"
                    ))
                    .and_then(Value::as_str),
                Some("#/components/schemas/ConsoleErrorResponse"),
                "status {status} should use ConsoleErrorResponse"
            );
        }
    }

    #[test]
    fn encryption_routes_and_strict_request_schema_are_documented() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let operation = spec
            .pointer("/paths/~1api~1v1~1namespaces~1{namespace}~1tenants~1{name}~1encryption")
            .expect("encryption path should exist");

        assert_eq!(
            operation
                .pointer("/get/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/EncryptionInfoResponse")
        );
        assert!(
            spec.pointer("/components/schemas/SecurityContextInfo")
                .is_some(),
            "nested encryption response schemas should be registered"
        );
        assert_eq!(
            operation
                .pointer("/put/requestBody/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/UpdateEncryptionRequest")
        );
        assert_eq!(
            operation
                .pointer("/put/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/EncryptionUpdateResponse")
        );
        assert_eq!(
            operation.pointer("/put/tags/0").and_then(Value::as_str),
            Some("encryption")
        );
        for status in ["400", "413", "415", "422"] {
            assert_eq!(
                operation
                    .pointer(&format!(
                        "/put/responses/{status}/content/application~1json/schema/$ref"
                    ))
                    .and_then(Value::as_str),
                Some("#/components/schemas/ConsoleErrorResponse"),
                "status {status} should use ConsoleErrorResponse"
            );
        }

        assert_eq!(
            spec.pointer("/components/schemas/UpdateEncryptionBackend/enum")
                .and_then(Value::as_array),
            Some(&vec![
                Value::String("local".to_string()),
                Value::String("vault".to_string()),
            ])
        );
        for schema in [
            "UpdateEncryptionRequest",
            "UpdateVaultRequest",
            "UpdateLocalRequest",
            "LocalMasterKeySecretRefInfo",
        ] {
            assert_eq!(
                spec.pointer(&format!(
                    "/components/schemas/{schema}/additionalProperties"
                ))
                .and_then(Value::as_bool),
                Some(false),
                "{schema} should reject unknown fields"
            );
        }
    }

    #[test]
    fn security_context_routes_are_documented() {
        let spec = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI spec serializes");
        let operation = spec
            .pointer("/paths/~1api~1v1~1namespaces~1{namespace}~1tenants~1{name}~1security-context")
            .expect("security-context path should exist");

        assert_eq!(
            operation
                .pointer("/get/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/SecurityContextInfo")
        );
        assert_eq!(
            operation
                .pointer("/put/requestBody/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/UpdateSecurityContextRequest")
        );
        assert_eq!(
            operation
                .pointer("/put/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/SecurityContextUpdateResponse")
        );
        let effective_types = spec
            .pointer(
                "/components/schemas/SecurityContextInfo/properties/effectiveRunAsNonRoot/type",
            )
            .and_then(Value::as_array)
            .expect("effective runAsNonRoot should use OpenAPI nullable types");
        assert!(
            effective_types
                .iter()
                .filter_map(Value::as_str)
                .any(|value| value == "boolean")
                && effective_types
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|value| value == "null"),
            "platform delegation makes the effective runAsNonRoot value unknown"
        );
        assert_eq!(
            spec.pointer(
                "/components/schemas/SecurityContextInfo/properties/operatorDefaultsDelegated/type"
            )
            .and_then(Value::as_str),
            Some("boolean")
        );
        assert_eq!(
            operation.pointer("/put/tags/0").and_then(Value::as_str),
            Some("security-context")
        );
        assert!(
            spec.pointer("/tags")
                .and_then(Value::as_array)
                .is_some_and(|tags| tags.iter().any(|tag| {
                    tag.get("name").and_then(Value::as_str) == Some("security-context")
                })),
            "security-context tag metadata should be registered"
        );
        for status in ["400", "413", "415", "422"] {
            assert_eq!(
                operation
                    .pointer(&format!(
                        "/put/responses/{status}/content/application~1json/schema/$ref"
                    ))
                    .and_then(Value::as_str),
                Some("#/components/schemas/ConsoleErrorResponse"),
                "status {status} should use ConsoleErrorResponse"
            );
        }
    }
}
