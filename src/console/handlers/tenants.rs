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

use axum::{extract::Path, Extension, Json};
use k8s_openapi::api::core::v1 as corev1;
use kube::{api::ListParams, Api, Client, ResourceExt};
use snafu::ResultExt;

use crate::console::{
    error::{self, Error, Result},
    models::tenant::*,
    state::Claims,
};
use crate::types::v1alpha1::{persistence::PersistenceConfig, pool::Pool, tenant::Tenant};

/// 列出所有 Tenants
pub async fn list_all_tenants(Extension(claims): Extension<Claims>) -> Result<Json<TenantListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::all(client);

    let tenants = api
        .list(&ListParams::default())
        .await
        .context(error::KubeApiSnafu)?;

    let items: Vec<TenantListItem> = tenants
        .items
        .into_iter()
        .map(|t| TenantListItem {
            name: t.name_any(),
            namespace: t.namespace().unwrap_or_default(),
            pools: t
                .spec
                .pools
                .iter()
                .map(|p| PoolInfo {
                    name: p.name.clone(),
                    servers: p.servers,
                    volumes_per_server: p.persistence.volumes_per_server,
                })
                .collect(),
            state: t
                .status
                .as_ref()
                .map(|s| s.current_state.to_string())
                .unwrap_or_else(|| "Unknown".to_string()),
            created_at: t
                .metadata
                .creation_timestamp
                .map(|ts| ts.0.to_rfc3339()),
        })
        .collect();

    Ok(Json(TenantListResponse { tenants: items }))
}

/// 按命名空间列出 Tenants
pub async fn list_tenants_by_namespace(
    Path(namespace): Path<String>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let tenants = api
        .list(&ListParams::default())
        .await
        .context(error::KubeApiSnafu)?;

    let items: Vec<TenantListItem> = tenants
        .items
        .into_iter()
        .map(|t| TenantListItem {
            name: t.name_any(),
            namespace: t.namespace().unwrap_or_default(),
            pools: t
                .spec
                .pools
                .iter()
                .map(|p| PoolInfo {
                    name: p.name.clone(),
                    servers: p.servers,
                    volumes_per_server: p.persistence.volumes_per_server,
                })
                .collect(),
            state: t
                .status
                .as_ref()
                .map(|s| s.current_state.to_string())
                .unwrap_or_else(|| "Unknown".to_string()),
            created_at: t
                .metadata
                .creation_timestamp
                .map(|ts| ts.0.to_rfc3339()),
        })
        .collect();

    Ok(Json(TenantListResponse { tenants: items }))
}

/// 获取 Tenant 详情
pub async fn get_tenant_details(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantDetailsResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);

    let tenant = api.get(&name).await.context(error::KubeApiSnafu)?;

    // 获取 Services
    let svc_api: Api<corev1::Service> = Api::namespaced(client, &namespace);
    let services = svc_api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", name)))
        .await
        .context(error::KubeApiSnafu)?;

    let service_infos: Vec<ServiceInfo> = services
        .items
        .into_iter()
        .map(|svc| ServiceInfo {
            name: svc.name_any(),
            service_type: svc
                .spec
                .as_ref()
                .and_then(|s| s.type_.clone())
                .unwrap_or_default(),
            ports: svc
                .spec
                .as_ref()
                .map(|s| {
                    s.ports
                        .as_ref()
                        .map(|ports| {
                            ports
                                .iter()
                                .map(|p| ServicePort {
                                    name: p.name.clone().unwrap_or_default(),
                                    port: p.port,
                                    target_port: p
                                        .target_port
                                        .as_ref()
                                        .map(|tp| match tp {
                                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i) => i.to_string(),
                                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s) => s.clone(),
                                        })
                                        .unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
        })
        .collect();

    Ok(Json(TenantDetailsResponse {
        name: tenant.name_any(),
        namespace: tenant.namespace().unwrap_or_default(),
        pools: tenant
            .spec
            .pools
            .iter()
            .map(|p| PoolInfo {
                name: p.name.clone(),
                servers: p.servers,
                volumes_per_server: p.persistence.volumes_per_server,
            })
            .collect(),
        state: tenant
            .status
            .as_ref()
            .map(|s| s.current_state.to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        image: tenant.spec.image.clone(),
        mount_path: tenant.spec.mount_path.clone(),
        created_at: tenant
            .metadata
            .creation_timestamp
            .map(|ts| ts.0.to_rfc3339()),
        services: service_infos,
    }))
}

/// 创建 Tenant
pub async fn create_tenant(
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateTenantRequest>,
) -> Result<Json<TenantListItem>> {
    let client = create_client(&claims).await?;

    // 检查 Namespace 是否存在
    let ns_api: Api<corev1::Namespace> = Api::all(client.clone());
    let ns_exists = ns_api.get(&req.namespace).await.is_ok();

    // 如果不存在则创建
    if !ns_exists {
        let ns = corev1::Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(req.namespace.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        ns_api.create(&Default::default(), &ns).await.context(error::KubeApiSnafu)?;
    }

    // 构造 Tenant CRD
    let pools: Vec<Pool> = req
        .pools
        .into_iter()
        .map(|p| Pool {
            name: p.name,
            servers: p.servers,
            persistence: PersistenceConfig {
                volumes_per_server: p.volumes_per_server,
                volume_claim_template: Some(corev1::PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(corev1::VolumeResourceRequirements {
                        requests: Some(
                            vec![("storage".to_string(), k8s_openapi::apimachinery::pkg::api::resource::Quantity(p.storage_size))]
                                .into_iter()
                                .collect(),
                        ),
                        ..Default::default()
                    }),
                    storage_class_name: p.storage_class,
                    ..Default::default()
                }),
                path: None,
                labels: None,
                annotations: None,
            },
            scheduling: Default::default(),
        })
        .collect();

    let tenant = Tenant {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(req.namespace.clone()),
            ..Default::default()
        },
        spec: crate::types::v1alpha1::tenant::TenantSpec {
            pools,
            image: req.image,
            mount_path: req.mount_path,
            creds_secret: req.creds_secret.map(|name| corev1::LocalObjectReference { name }),
            ..Default::default()
        },
        status: None,
    };

    let api: Api<Tenant> = Api::namespaced(client, &req.namespace);
    let created = api
        .create(&Default::default(), &tenant)
        .await
        .context(error::KubeApiSnafu)?;

    Ok(Json(TenantListItem {
        name: created.name_any(),
        namespace: created.namespace().unwrap_or_default(),
        pools: created
            .spec
            .pools
            .iter()
            .map(|p| PoolInfo {
                name: p.name.clone(),
                servers: p.servers,
                volumes_per_server: p.persistence.volumes_per_server,
            })
            .collect(),
        state: "Creating".to_string(),
        created_at: created
            .metadata
            .creation_timestamp
            .map(|ts| ts.0.to_rfc3339()),
    }))
}

/// 删除 Tenant
pub async fn delete_tenant(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DeleteTenantResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    api.delete(&name, &Default::default())
        .await
        .context(error::KubeApiSnafu)?;

    Ok(Json(DeleteTenantResponse {
        success: true,
        message: format!("Tenant {} deleted successfully", name),
    }))
}

/// 创建 Kubernetes 客户端
async fn create_client(claims: &Claims) -> Result<Client> {
    let mut config = kube::Config::infer().await.map_err(|e| Error::InternalServer {
        message: format!("Failed to load kubeconfig: {}", e),
    })?;

    config.auth_info.token = Some(claims.k8s_token.clone().into());

    Client::try_from(config).map_err(|e| Error::InternalServer {
        message: format!("Failed to create K8s client: {}", e),
    })
}
