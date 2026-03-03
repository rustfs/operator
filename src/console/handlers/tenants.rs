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

use axum::{Extension, Json, extract::Path};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, ResourceExt, api::ListParams};
use crate::console::{
    error::{self, Error, Result},
    models::tenant::*,
    state::Claims,
};
use crate::types::v1alpha1::{persistence::PersistenceConfig, pool::Pool, tenant::Tenant};

// curl -s -X POST http://localhost:9090/api/v1/login \
//   -H "Content-Type: application/json" \
//   -d "{\"token\": \"$(kubectl create token rustfs-operator-console -n rustfs-system --duration=24h)\"}" \
//   -c cookies.txt

// curl -b cookies.txt http://localhost:9090/api/v1/tenants
pub async fn list_all_tenants(
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::all(client);

    let tenants = api
        .list(&ListParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

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
            created_at: t.metadata.creation_timestamp.map(|ts| ts.0.to_rfc3339()),
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
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

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
            created_at: t.metadata.creation_timestamp.map(|ts| ts.0.to_rfc3339()),
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

    let tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    // 获取 Services
    let svc_api: Api<corev1::Service> = Api::namespaced(client, &namespace);
    let services = svc_api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", name)))
        .await
        .map_err(|e| error::map_kube_error(e, format!("Services for tenant '{}'", name)))?;

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
        ns_api
            .create(&Default::default(), &ns)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Namespace '{}'", req.namespace)))?;
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
                            vec![(
                                "storage".to_string(),
                                k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                    p.storage_size,
                                ),
                            )]
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
            creds_secret: req
                .creds_secret
                .map(|name| corev1::LocalObjectReference { name }),
            ..Default::default()
        },
        status: None,
    };

    let api: Api<Tenant> = Api::namespaced(client, &req.namespace);
    let created = api
        .create(&Default::default(), &tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", req.name)))?;

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
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(DeleteTenantResponse {
        success: true,
        message: format!("Tenant {} deleted successfully", name),
    }))
}

/// 更新 Tenant
pub async fn update_tenant(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<UpdateTenantRequest>,
) -> Result<Json<UpdateTenantResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    // 获取当前 Tenant
    let mut tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    // 应用更新（仅更新提供的字段）
    let mut updated_fields = Vec::new();

    if let Some(image) = req.image {
        tenant.spec.image = Some(image.clone());
        updated_fields.push(format!("image={}", image));
    }

    if let Some(mount_path) = req.mount_path {
        tenant.spec.mount_path = Some(mount_path.clone());
        updated_fields.push(format!("mount_path={}", mount_path));
    }

    if let Some(env_vars) = req.env {
        tenant.spec.env = env_vars
            .into_iter()
            .map(|e| corev1::EnvVar {
                name: e.name,
                value: e.value,
                ..Default::default()
            })
            .collect();
        updated_fields.push("env".to_string());
    }

    if let Some(creds_secret) = req.creds_secret {
        if creds_secret.is_empty() {
            tenant.spec.creds_secret = None;
            updated_fields.push("creds_secret=<removed>".to_string());
        } else {
            tenant.spec.creds_secret = Some(corev1::LocalObjectReference {
                name: creds_secret.clone(),
            });
            updated_fields.push(format!("creds_secret={}", creds_secret));
        }
    }

    if let Some(pod_mgmt_policy) = req.pod_management_policy {
        use crate::types::v1alpha1::k8s::PodManagementPolicy;
        tenant.spec.pod_management_policy = match pod_mgmt_policy.as_str() {
            "OrderedReady" => Some(PodManagementPolicy::OrderedReady),
            "Parallel" => Some(PodManagementPolicy::Parallel),
            _ => {
                return Err(Error::BadRequest {
                    message: format!(
                        "Invalid pod_management_policy '{}', must be 'OrderedReady' or 'Parallel'",
                        pod_mgmt_policy
                    ),
                });
            }
        };
        updated_fields.push(format!("pod_management_policy={}", pod_mgmt_policy));
    }

    if let Some(image_pull_policy) = req.image_pull_policy {
        use crate::types::v1alpha1::k8s::ImagePullPolicy;
        tenant.spec.image_pull_policy = match image_pull_policy.as_str() {
            "Always" => Some(ImagePullPolicy::Always),
            "IfNotPresent" => Some(ImagePullPolicy::IfNotPresent),
            "Never" => Some(ImagePullPolicy::Never),
            _ => {
                return Err(Error::BadRequest {
                    message: format!(
                        "Invalid image_pull_policy '{}', must be 'Always', 'IfNotPresent', or 'Never'",
                        image_pull_policy
                    ),
                });
            }
        };
        updated_fields.push(format!("image_pull_policy={}", image_pull_policy));
    }

    if let Some(logging) = req.logging {
        use crate::types::v1alpha1::logging::{LoggingConfig, LoggingMode};

        let mode = match logging.log_type.as_str() {
            "stdout" => LoggingMode::Stdout,
            "emptyDir" => LoggingMode::EmptyDir,
            "persistent" => LoggingMode::Persistent,
            _ => {
                return Err(Error::BadRequest {
                    message: format!(
                        "Invalid logging type '{}', must be 'stdout', 'emptyDir', or 'persistent'",
                        logging.log_type
                    ),
                });
            }
        };

        tenant.spec.logging = Some(LoggingConfig {
            mode,
            storage_size: logging.volume_size,
            storage_class: logging.storage_class,
            mount_path: None,
        });
        updated_fields.push(format!("logging={}", logging.log_type));
    }

    if updated_fields.is_empty() {
        return Err(Error::BadRequest {
            message: "No fields to update".to_string(),
        });
    }

    // 提交更新
    let updated_tenant = api
        .replace(&name, &Default::default(), &tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(UpdateTenantResponse {
        success: true,
        message: format!("Tenant updated: {}", updated_fields.join(", ")),
        tenant: TenantListItem {
            name: updated_tenant.name_any(),
            namespace: updated_tenant.namespace().unwrap_or_default(),
            pools: updated_tenant
                .spec
                .pools
                .iter()
                .map(|p| PoolInfo {
                    name: p.name.clone(),
                    servers: p.servers,
                    volumes_per_server: p.persistence.volumes_per_server,
                })
                .collect(),
            state: updated_tenant
                .status
                .as_ref()
                .map(|s| s.current_state.to_string())
                .unwrap_or_else(|| "Unknown".to_string()),
            created_at: updated_tenant
                .metadata
                .creation_timestamp
                .map(|ts| ts.0.to_rfc3339()),
        },
    }))
}

/// 创建 Kubernetes 客户端
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
