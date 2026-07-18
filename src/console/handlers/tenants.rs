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

use super::validate_tenant_for_write;
use crate::console::{
    error::{self, Error, Result},
    json::ConsoleJson,
    models::tenant::*,
    state::Claims,
};
use crate::types::v1alpha1::{
    persistence::PersistenceConfig,
    pool::{Pool, validate_pool_shape_immutable},
    security_context::PodSecurityContextOverride,
    tenant::Tenant,
};
use axum::{
    Extension, Json,
    extract::{Path, Query},
};
use k8s_openapi::api::core::v1 as corev1;
use kube::{Api, Client, CustomResourceExt, Resource, ResourceExt, api::ListParams};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(serde::Deserialize)]
struct TenantManifestTypeMeta {
    #[serde(rename = "apiVersion")]
    api_version: Option<String>,
    kind: Option<String>,
}

static TENANT_OPENAPI_SCHEMA: LazyLock<std::result::Result<Value, String>> = LazyLock::new(|| {
    let crd = serde_json::to_value(Tenant::crd())
        .map_err(|error| format!("failed to serialize Tenant CRD: {error}"))?;
    let expected_version = Tenant::version(&());
    crd.pointer("/spec/versions")
        .and_then(Value::as_array)
        .and_then(|versions| {
            versions.iter().find(|version| {
                version.get("name").and_then(Value::as_str) == Some(expected_version.as_ref())
            })
        })
        .and_then(|version| version.pointer("/schema/openAPIV3Schema"))
        .cloned()
        .ok_or_else(|| {
            format!("Tenant CRD does not contain an OpenAPI schema for {expected_version}")
        })
});

// curl -s -X POST http://localhost:9090/api/v1/login \
//   -H "Content-Type: application/json" \
//   -d "{\"token\": \"$(kubectl create token rustfs-operator-console -n rustfs-system --duration=24h)\"}" \
//   -c cookies.txt

// curl -b cookies.txt http://localhost:9090/api/v1/tenants
pub async fn list_all_tenants(
    Query(query): Query<TenantListQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::all(client);

    let tenants = api
        .list(&ListParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

    let items = build_tenant_list_items(tenants.items, query.state.as_deref());

    Ok(Json(TenantListResponse { tenants: items }))
}

/// List tenants in one namespace.
pub async fn list_tenants_by_namespace(
    Path(namespace): Path<String>,
    Query(query): Query<TenantListQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let tenants = api
        .list(&ListParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

    let items = build_tenant_list_items(tenants.items, query.state.as_deref());

    Ok(Json(TenantListResponse { tenants: items }))
}

/// Count tenants by state across all namespaces.
pub async fn get_all_tenant_state_counts(
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantStateCountsResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::all(client);

    let tenants = api
        .list(&ListParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

    Ok(Json(summarize_tenant_states(&tenants.items)))
}

/// Count tenants by state in one namespace.
pub async fn get_tenant_state_counts_by_namespace(
    Path(namespace): Path<String>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantStateCountsResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);

    let tenants = api
        .list(&ListParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, "Tenants"))?;

    Ok(Json(summarize_tenant_states(&tenants.items)))
}

/// Full tenant detail including Services.
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

    // List tenant-scoped Services
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

    let status_summary = tenant_status_summary(&tenant);
    let conditions = tenant_conditions(&tenant);
    let next_actions = status_summary.next_actions.clone();
    let certificates = tenant_certificates(&tenant);
    let provisioning = tenant
        .status
        .as_ref()
        .map(|status| status.provisioning.clone())
        .unwrap_or_default();

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
        state: status_summary.current_state.clone(),
        status_summary,
        conditions,
        next_actions,
        certificates,
        provisioning,
        image: tenant.spec.image.clone(),
        mount_path: tenant.spec.mount_path.clone(),
        created_at: tenant
            .metadata
            .creation_timestamp
            .map(|ts| ts.0.to_rfc3339()),
        services: service_infos,
    }))
}

/// Create a Tenant CR (and namespace if missing).
pub async fn create_tenant(
    Extension(claims): Extension<Claims>,
    ConsoleJson(req): ConsoleJson<CreateTenantRequest>,
) -> Result<Json<TenantListItem>> {
    let tenant = tenant_from_create_request(req)?;
    let (name, namespace) = tenant_identity(&tenant)?;
    let name = name.to_string();
    let namespace = namespace.to_string();

    // Validate the complete request before creating any Kubernetes resource. A rejected Tenant
    // must not leave an empty Namespace behind.
    let client = create_client(&claims).await?;
    ensure_namespace_exists(&client, &namespace).await?;

    let api: Api<Tenant> = Api::namespaced(client, &namespace);
    let created = api
        .create(&Default::default(), &tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(tenant_to_list_item(created)))
}

fn tenant_from_create_request(req: CreateTenantRequest) -> Result<Tenant> {
    // Validate tenant name is DNS-1035 compliant before hitting the K8s API
    if let Err(e) = crate::types::v1alpha1::tenant::validate_dns1035_label(&req.name) {
        return Err(Error::BadRequest {
            message: format!("{}", e),
        });
    }
    validate_namespace_name(&req.namespace).map_err(|message| Error::BadRequest { message })?;

    // Build Tenant object
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
            security_context: None,
            container_security_context: None,
            scheduling: Default::default(),
        })
        .collect();

    let security_context = req
        .security_context
        .as_ref()
        .map(|sc| PodSecurityContextOverride {
            run_as_user: sc.run_as_user,
            run_as_group: sc.run_as_group,
            fs_group: sc.fs_group,
            run_as_non_root: sc.run_as_non_root,
            seccomp_profile: None,
        });

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
            policies: req.policies.unwrap_or_default(),
            users: req.users.unwrap_or_default(),
            buckets: req.buckets.unwrap_or_default(),
            security_context,
            ..Default::default()
        },
        status: None,
    };
    validate_tenant_for_write(&tenant)?;
    Ok(tenant)
}

/// Create a Tenant from its complete YAML representation.
pub async fn create_tenant_from_yaml(
    Extension(claims): Extension<Claims>,
    ConsoleJson(req): ConsoleJson<TenantYAML>,
) -> Result<Json<TenantListItem>> {
    let tenant = parse_tenant_yaml_for_create(&req.yaml)?;
    let (name, namespace) = tenant_identity(&tenant)?;
    let name = name.to_string();
    let namespace = namespace.to_string();

    let client = create_client(&claims).await?;
    ensure_namespace_exists(&client, &namespace).await?;

    let api: Api<Tenant> = Api::namespaced(client.clone(), &namespace);
    let created = api
        .create(&Default::default(), &tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(tenant_to_list_item(created)))
}

/// Delete a Tenant CR.
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

/// Patch selected spec fields on a Tenant.
pub async fn update_tenant(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    ConsoleJson(req): ConsoleJson<UpdateTenantRequest>,
) -> Result<Json<UpdateTenantResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    // Load current object
    let mut tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;
    // Merge only provided fields
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

    if let Some(policies) = req.policies {
        tenant.spec.policies = policies;
        updated_fields.push("policies".to_string());
    }

    if let Some(users) = req.users {
        tenant.spec.users = users;
        updated_fields.push("users".to_string());
    }

    if let Some(buckets) = req.buckets {
        tenant.spec.buckets = buckets;
        updated_fields.push("buckets".to_string());
    }

    if updated_fields.is_empty() {
        return Err(Error::BadRequest {
            message: "No fields to update".to_string(),
        });
    }
    validate_tenant_for_write(&tenant)?;

    // Replace status-safe fields
    let updated_tenant = api
        .replace(&name, &Default::default(), &tenant)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(UpdateTenantResponse {
        success: true,
        message: format!("Tenant updated: {}", updated_fields.join(", ")),
        tenant: tenant_to_list_item(updated_tenant),
    }))
}

/// Return serialized Tenant manifest.
pub async fn get_tenant_yaml(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<TenantYAML>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let mut tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    // Remove managed fields to keep YAML readable (same as MinIO operator)
    tenant.metadata.managed_fields = None;

    let yaml_str = serde_yaml_ng::to_string(&tenant).map_err(|e| Error::InternalServer {
        message: format!("Failed to serialize Tenant to YAML: {}", e),
    })?;

    Ok(Json(TenantYAML { yaml: yaml_str }))
}

/// Apply raw YAML for a Tenant (server-side apply or replace).
pub async fn put_tenant_yaml(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    ConsoleJson(req): ConsoleJson<TenantYAML>,
) -> Result<Json<TenantYAML>> {
    let in_tenant = parse_tenant_yaml(&req.yaml)?;

    // Validate: name and namespace in YAML must match URL params
    let in_name = in_tenant.metadata.name.as_deref().unwrap_or_default();
    let in_ns = in_tenant.metadata.namespace.as_deref().unwrap_or_default();
    if !in_name.is_empty() && in_name != name {
        return Err(Error::BadRequest {
            message: format!(
                "Tenant name in YAML '{}' does not match URL '{}'",
                in_name, name
            ),
        });
    }
    if !in_ns.is_empty() && in_ns != namespace {
        return Err(Error::BadRequest {
            message: format!(
                "Tenant namespace in YAML '{}' does not match URL '{}'",
                in_ns, namespace
            ),
        });
    }

    // Validate: at least one pool
    if in_tenant.spec.pools.is_empty() {
        return Err(Error::BadRequest {
            message: "Tenant must have at least one pool".to_string(),
        });
    }

    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    // Get the current Tenant (to preserve resourceVersion and safe metadata)
    let mut current = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    apply_tenant_yaml_update(&mut current, in_tenant)?;

    let updated = api
        .replace(&name, &Default::default(), &current)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    // Return the updated Tenant YAML (clean, without managedFields)
    let mut clean = updated;
    clean.metadata.managed_fields = None;

    let yaml_str = serde_yaml_ng::to_string(&clean).map_err(|e| Error::InternalServer {
        message: format!("Failed to serialize Tenant to YAML: {}", e),
    })?;

    Ok(Json(TenantYAML { yaml: yaml_str }))
}

fn apply_tenant_yaml_update(current: &mut Tenant, incoming: Tenant) -> Result<()> {
    if let Err(message) = validate_pool_shape_immutable(&current.spec.pools, &incoming.spec.pools) {
        return Err(Error::BadRequest { message });
    }

    // Only update safe fields. Apply annotations before validation because workload-security
    // acknowledgement annotations are intentionally bound to the incoming image reference.
    current.spec = incoming.spec;
    if let Some(labels) = incoming.metadata.labels {
        current.metadata.labels = Some(labels);
    }
    if let Some(annotations) = incoming.metadata.annotations {
        current.metadata.annotations = Some(annotations);
    }
    if let Some(finalizers) = incoming.metadata.finalizers {
        current.metadata.finalizers = Some(finalizers);
    }
    validate_tenant_for_write(current)
}

/// Build a client using the Kubernetes bearer token from session claims.
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

async fn ensure_namespace_exists(client: &Client, namespace: &str) -> Result<()> {
    let ns_api: Api<corev1::Namespace> = Api::all(client.clone());
    match ns_api.get(namespace).await {
        Ok(_) => return Ok(()),
        Err(kube::Error::Api(response)) if response.code == 404 => {}
        Err(error) => {
            return Err(error::map_kube_error(
                error,
                format!("Namespace '{namespace}'"),
            ));
        }
    }

    let ns = corev1::Namespace {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(namespace.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    match ns_api.create(&Default::default(), &ns).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(response))
            if response.code == 409 && response.reason == "AlreadyExists" =>
        {
            // Another request may have created the Namespace after our initial GET.
            // Re-read it so unrelated create conflicts are not silently accepted.
            ns_api
                .get(namespace)
                .await
                .map(|_| ())
                .map_err(|error| error::map_kube_error(error, format!("Namespace '{}'", namespace)))
        }
        Err(error) => Err(error::map_kube_error(
            error,
            format!("Namespace '{}'", namespace),
        )),
    }
}

fn tenant_openapi_schema() -> Result<&'static Value> {
    TENANT_OPENAPI_SCHEMA
        .as_ref()
        .map_err(|message| Error::InternalServer {
            message: message.clone(),
        })
}

fn collect_unknown_manifest_fields(
    value: &Value,
    schema: &Value,
    path: &str,
    unknown_fields: &mut Vec<String>,
) {
    if value.is_null()
        || schema
            .get("x-kubernetes-preserve-unknown-fields")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return;
    }

    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        let additional_properties = schema.get("additionalProperties");

        for (name, child) in object {
            // TypeMeta and ObjectMeta are implicit CRD fields and are validated by
            // their typed deserializers rather than openAPIV3Schema.
            if path.is_empty() && matches!(name.as_str(), "apiVersion" | "kind" | "metadata") {
                continue;
            }

            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            if let Some(child_schema) = properties.and_then(|properties| properties.get(name)) {
                collect_unknown_manifest_fields(child, child_schema, &child_path, unknown_fields);
                continue;
            }

            match additional_properties {
                Some(Value::Bool(true)) => {}
                Some(Value::Object(entries)) if entries.is_empty() => {}
                Some(child_schema @ Value::Object(_)) => collect_unknown_manifest_fields(
                    child,
                    child_schema,
                    &child_path,
                    unknown_fields,
                ),
                _ => unknown_fields.push(child_path),
            }
        }
        return;
    }

    if let Some(items) = value.as_array()
        && let Some(item_schema) = schema.get("items")
    {
        for (index, item) in items.iter().enumerate() {
            collect_unknown_manifest_fields(
                item,
                item_schema,
                &format!("{path}.{index}"),
                unknown_fields,
            );
        }
    }
}

fn normalized_ignored_path(path: &serde_ignored::Path<'_>) -> String {
    fn collect_segments(path: &serde_ignored::Path<'_>, segments: &mut Vec<String>) {
        match path {
            serde_ignored::Path::Root => {}
            serde_ignored::Path::Seq { parent, index } => {
                collect_segments(parent, segments);
                segments.push(index.to_string());
            }
            serde_ignored::Path::Map { parent, key } => {
                collect_segments(parent, segments);
                segments.push(key.clone());
            }
            serde_ignored::Path::Some { parent }
            | serde_ignored::Path::NewtypeStruct { parent }
            | serde_ignored::Path::NewtypeVariant { parent } => {
                collect_segments(parent, segments);
            }
        }
    }

    let mut segments = Vec::new();
    collect_segments(path, &mut segments);
    segments.join(".")
}

fn parse_tenant_yaml_for_create(yaml: &str) -> Result<Tenant> {
    let mut tenant = parse_tenant_yaml(yaml)?;
    let (name, namespace) = tenant_identity(&tenant)?;

    crate::types::v1alpha1::tenant::validate_dns1035_label(name).map_err(|error| {
        Error::BadRequest {
            message: error.to_string(),
        }
    })?;
    validate_namespace_name(namespace).map_err(|message| Error::BadRequest { message })?;
    validate_tenant_for_write(&tenant)?;

    sanitize_tenant_for_create(&mut tenant);
    Ok(tenant)
}

fn parse_tenant_yaml(yaml: &str) -> Result<Tenant> {
    let type_meta: TenantManifestTypeMeta =
        serde_yaml_ng::from_str(yaml).map_err(|error| Error::BadRequest {
            message: format!("Invalid Tenant YAML: {error}"),
        })?;

    let expected_api_version = Tenant::api_version(&());
    if type_meta.api_version.as_deref() != Some(expected_api_version.as_ref()) {
        return Err(Error::BadRequest {
            message: format!("apiVersion must be '{expected_api_version}'"),
        });
    }
    let expected_kind = Tenant::kind(&());
    if type_meta.kind.as_deref() != Some(expected_kind.as_ref()) {
        return Err(Error::BadRequest {
            message: format!("kind must be '{expected_kind}'"),
        });
    }

    let value: serde_json::Value =
        serde_yaml_ng::from_str(yaml).map_err(|error| Error::BadRequest {
            message: format!("Invalid Tenant YAML: {error}"),
        })?;
    let mut unknown_fields = Vec::new();
    collect_unknown_manifest_fields(&value, tenant_openapi_schema()?, "", &mut unknown_fields);
    let tenant = serde_ignored::deserialize(value, |path| {
        let path = normalized_ignored_path(&path);
        // ObjectMeta is implicit in the CRD OpenAPI schema, so its typed
        // deserializer is the source of truth for unknown metadata fields.
        // The generated schema walker covers spec, status, and root fields.
        if path == "metadata" || path.starts_with("metadata.") {
            unknown_fields.push(path);
        }
    })
    .map_err(|error| Error::BadRequest {
        message: format!("Invalid Tenant YAML: {error}"),
    })?;

    if !unknown_fields.is_empty() {
        unknown_fields.sort();
        unknown_fields.dedup();
        return Err(Error::BadRequest {
            message: format!(
                "Unknown field(s) in Tenant YAML: {}",
                unknown_fields.join(", ")
            ),
        });
    }

    Ok(tenant)
}

fn tenant_identity(tenant: &Tenant) -> Result<(&str, &str)> {
    let name = tenant
        .metadata
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::BadRequest {
            message: "metadata.name is required".to_string(),
        })?;
    let namespace = tenant
        .metadata
        .namespace
        .as_deref()
        .filter(|namespace| !namespace.is_empty())
        .ok_or_else(|| Error::BadRequest {
            message: "metadata.namespace is required".to_string(),
        })?;

    Ok((name, namespace))
}

fn validate_namespace_name(namespace: &str) -> std::result::Result<(), String> {
    if namespace.len() > 63 {
        return Err("metadata.namespace must be at most 63 characters".to_string());
    }

    let bytes = namespace.as_bytes();
    if !bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
    {
        return Err(
            "metadata.namespace must be a valid DNS-1123 label (lowercase alphanumeric or '-')"
                .to_string(),
        );
    }

    Ok(())
}

fn sanitize_tenant_for_create(tenant: &mut Tenant) {
    tenant.status = None;
    tenant.metadata.creation_timestamp = None;
    tenant.metadata.deletion_grace_period_seconds = None;
    tenant.metadata.deletion_timestamp = None;
    tenant.metadata.generation = None;
    tenant.metadata.managed_fields = None;
    tenant.metadata.resource_version = None;
    tenant.metadata.self_link = None;
    tenant.metadata.uid = None;
}

fn build_tenant_list_items(
    tenants: Vec<Tenant>,
    state_filter: Option<&str>,
) -> Vec<TenantListItem> {
    tenants
        .into_iter()
        .filter_map(|t| {
            let item = tenant_to_list_item(t);
            if state_matches_filter(&item.state, state_filter) {
                Some(item)
            } else {
                None
            }
        })
        .collect()
}

fn tenant_state(t: &Tenant) -> String {
    tenant_status_summary(t).current_state
}

fn state_matches_filter(state: &str, state_filter: Option<&str>) -> bool {
    match state_filter {
        Some(filter) => canonical_console_state_filter(Some(filter)).is_some_and(|filter| {
            canonical_console_state(Some(state)).eq_ignore_ascii_case(&filter)
        }),
        None => true,
    }
}

fn summarize_tenant_states(tenants: &[Tenant]) -> TenantStateCountsResponse {
    let mut counts = std::collections::BTreeMap::new();
    for tenant in tenants {
        let state = tenant_state(tenant);
        *counts.entry(state).or_insert(0) += 1;
    }

    TenantStateCountsResponse {
        total: tenants.len() as u32,
        counts,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_tenant_yaml_update, ensure_namespace_exists, parse_tenant_yaml,
        parse_tenant_yaml_for_create, state_matches_filter, tenant_from_create_request,
    };
    use crate::console::error::Error;
    use crate::console::models::tenant::{CreatePoolRequest, CreateTenantRequest};
    use crate::types::v1alpha1::tenant::RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION;
    use kube::{Client, client::Body};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::service_fn;

    const MINIMAL_TENANT_YAML: &str = r#"
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: legacy-root
  namespace: storage
spec:
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
"#;

    fn minimal_create_request(image: Option<&str>) -> CreateTenantRequest {
        CreateTenantRequest {
            name: "tenant-a".to_string(),
            namespace: "storage".to_string(),
            pools: vec![CreatePoolRequest {
                name: "pool-0".to_string(),
                servers: 1,
                volumes_per_server: 1,
                storage_size: "1Gi".to_string(),
                storage_class: None,
            }],
            image: image.map(str::to_string),
            mount_path: None,
            creds_secret: None,
            policies: None,
            users: None,
            buckets: None,
            security_context: None,
        }
    }

    #[test]
    fn json_create_rejects_invalid_tenant_before_kubernetes_work() {
        let error = tenant_from_create_request(minimal_create_request(Some(
            "rustfs/rustfs:1.0.0-alpha.99",
        )))
        .expect_err("an incompatible image must be rejected during request preparation");

        assert!(matches!(
            error,
            Error::BadRequest { message } if message.contains("Tokio io_uring")
        ));
    }

    #[test]
    fn json_create_validates_namespace_before_kubernetes_work() {
        let mut request = minimal_create_request(Some("rustfs/rustfs:1.0.0-beta.10"));
        request.namespace = "Storage_Team".to_string();

        let error = tenant_from_create_request(request)
            .expect_err("an invalid namespace must be rejected during request preparation");

        assert!(matches!(
            error,
            Error::BadRequest { message } if message.contains("metadata.namespace")
        ));
    }

    #[test]
    fn raw_yaml_update_validates_image_and_acknowledgement_from_the_same_request() {
        let mut current = crate::tests::create_test_tenant(None, None);
        current.spec.image = Some("rustfs/rustfs:1.0.0-beta.10".to_string());

        let image = "registry.example.com/rustfs/rustfs@sha256:0123456789abcdef";
        let mut incoming = current.clone();
        incoming.spec.image = Some(image.to_string());
        incoming
            .metadata
            .annotations
            .get_or_insert_default()
            .insert(
                RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION.to_string(),
                image.to_string(),
            );

        apply_tenant_yaml_update(&mut current, incoming)
            .expect("an incoming image-bound acknowledgement should validate atomically");
        assert_eq!(current.spec.image.as_deref(), Some(image));
        assert_eq!(
            current
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get(RUNTIME_DEFAULT_IMAGE_ACK_ANNOTATION))
                .map(String::as_str),
            Some(image)
        );
    }

    #[test]
    fn state_filter_is_case_insensitive_for_known_states() {
        assert!(state_matches_filter("Ready", Some("ready")));
        assert!(state_matches_filter("Reconciling", Some("updating")));
        assert!(state_matches_filter("Blocked", Some("blocked")));
    }

    #[test]
    fn unknown_filter_value_does_not_match_unknown_state() {
        assert!(!state_matches_filter("Unknown", Some("foo")));
    }

    #[test]
    fn yaml_create_preserves_complete_security_context_configuration() {
        let tenant = parse_tenant_yaml_for_create(
            r#"
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: legacy-root
  namespace: storage
  labels:
    team: storage
  annotations:
    example.com/note: preserved
  finalizers:
    - example.com/finalizer
  ownerReferences:
    - apiVersion: v1
      kind: ConfigMap
      name: tenant-owner
      uid: owner-uid
  resourceVersion: "42"
  uid: tenant-uid
  generation: 7
  creationTimestamp: "2026-07-18T00:00:00Z"
  deletionTimestamp: "2026-07-19T00:00:00Z"
  deletionGracePeriodSeconds: 30
  selfLink: /apis/rustfs.com/v1alpha1/namespaces/storage/tenants/legacy-root
  managedFields:
    - manager: kubectl
      operation: Apply
      apiVersion: rustfs.com/v1alpha1
      fieldsType: FieldsV1
spec:
  image: rustfs/rustfs:1.0.0-alpha.99
  securityContext:
    runAsUser: 0
    runAsGroup: 0
    fsGroup: 0
    seccompProfile:
      type: Localhost
      localhostProfile: profiles/rustfs.json
  containerSecurityContext:
    runAsUser: 0
    allowPrivilegeEscalation: true
    readOnlyRootFilesystem: false
    capabilities:
      add: [SYS_ADMIN]
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
      securityContext:
        runAsUser: 0
        runAsGroup: 0
        fsGroup: 0
        seccompProfile:
          type: Localhost
          localhostProfile: profiles/pool-rustfs.json
      containerSecurityContext:
        runAsUser: 0
        allowPrivilegeEscalation: true
        capabilities:
          add: [SYS_ADMIN]
status:
  currentState: Ready
  availableReplicas: 1
  pools: []
"#,
        )
        .expect("complete Tenant YAML should parse");

        let tenant_pod = tenant
            .spec
            .security_context
            .as_ref()
            .expect("Tenant Pod security context is preserved");
        assert_eq!(tenant_pod.run_as_user, Some(0));
        assert_eq!(tenant_pod.run_as_non_root, None);
        assert_eq!(
            tenant_pod
                .seccomp_profile
                .as_ref()
                .and_then(|profile| profile.localhost_profile.as_deref()),
            Some("profiles/rustfs.json")
        );

        let tenant_container = tenant
            .spec
            .container_security_context
            .as_ref()
            .expect("Tenant container security context is preserved");
        assert_eq!(tenant_container.run_as_user, Some(0));
        assert_eq!(tenant_container.allow_privilege_escalation, Some(true));
        assert_eq!(
            tenant_container
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.add.as_ref()),
            Some(&vec!["SYS_ADMIN".to_string()])
        );

        let pool = &tenant.spec.pools[0];
        let pool_pod = pool
            .security_context
            .as_ref()
            .expect("Pool Pod security context is preserved");
        assert_eq!(pool_pod.run_as_user, Some(0));
        assert_eq!(pool_pod.run_as_non_root, None);
        assert_eq!(
            pool_pod
                .seccomp_profile
                .as_ref()
                .and_then(|profile| profile.localhost_profile.as_deref()),
            Some("profiles/pool-rustfs.json")
        );

        let pool_container = pool
            .container_security_context
            .as_ref()
            .expect("Pool container security context is preserved");
        assert_eq!(pool_container.run_as_user, Some(0));
        assert_eq!(pool_container.allow_privilege_escalation, Some(true));
        assert_eq!(
            pool_container
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.add.as_ref()),
            Some(&vec!["SYS_ADMIN".to_string()])
        );

        assert_eq!(
            tenant
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get("team"))
                .map(String::as_str),
            Some("storage")
        );
        assert_eq!(
            tenant
                .metadata
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get("example.com/note"))
                .map(String::as_str),
            Some("preserved")
        );
        assert!(tenant.metadata.finalizers.is_some());
        assert!(tenant.metadata.owner_references.is_some());
    }

    #[test]
    fn yaml_create_strips_status_and_server_owned_metadata() {
        let yaml = r#"
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: legacy-root
  namespace: storage
  resourceVersion: "42"
  uid: tenant-uid
  generation: 7
  creationTimestamp: "2026-07-18T00:00:00Z"
  deletionTimestamp: "2026-07-19T00:00:00Z"
  deletionGracePeriodSeconds: 30
  selfLink: /apis/rustfs.com/v1alpha1/namespaces/storage/tenants/legacy-root
  managedFields:
    - manager: kubectl
      operation: Apply
      apiVersion: rustfs.com/v1alpha1
      fieldsType: FieldsV1
spec:
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
status:
  currentState: Ready
  availableReplicas: 1
  pools: []
"#;

        let tenant = parse_tenant_yaml_for_create(yaml).expect("Tenant YAML should parse");

        assert!(tenant.status.is_none());
        assert!(tenant.metadata.resource_version.is_none());
        assert!(tenant.metadata.uid.is_none());
        assert!(tenant.metadata.generation.is_none());
        assert!(tenant.metadata.creation_timestamp.is_none());
        assert!(tenant.metadata.deletion_timestamp.is_none());
        assert!(tenant.metadata.deletion_grace_period_seconds.is_none());
        assert!(tenant.metadata.self_link.is_none());
        assert!(tenant.metadata.managed_fields.is_none());
    }

    #[test]
    fn yaml_create_rejects_wrong_type_metadata_and_missing_identity() {
        let cases = [
            (
                MINIMAL_TENANT_YAML.replace("rustfs.com/v1alpha1", "v1"),
                "apiVersion",
            ),
            (MINIMAL_TENANT_YAML.replace("Tenant", "ConfigMap"), "kind"),
            (
                MINIMAL_TENANT_YAML.replace("  name: legacy-root\n", ""),
                "metadata.name",
            ),
            (
                MINIMAL_TENANT_YAML.replace("  namespace: storage\n", ""),
                "metadata.namespace",
            ),
            (
                MINIMAL_TENANT_YAML
                    .replace("metadata:\n  name: legacy-root\n  namespace: storage\n", ""),
                "metadata",
            ),
        ];

        for (yaml, expected) in cases {
            let message = bad_request_message(&yaml);
            assert!(
                message.contains(expected),
                "expected '{expected}' in '{message}'"
            );
        }
    }

    #[test]
    fn yaml_create_rejects_invalid_namespace_and_empty_pools() {
        let invalid_namespace = MINIMAL_TENANT_YAML.replace("storage", "Storage_Team");
        assert!(bad_request_message(&invalid_namespace).contains("metadata.namespace"));

        let empty_pools = MINIMAL_TENANT_YAML.replace(
            "  pools:\n    - name: pool-0\n      servers: 1\n      persistence:\n        volumesPerServer: 1\n",
            "  pools: []\n",
        );
        assert!(bad_request_message(&empty_pools).contains("pools must be configured"));
    }

    #[test]
    fn yaml_create_rejects_incompatible_or_invalid_security_profiles() {
        let incompatible_image = MINIMAL_TENANT_YAML.replace(
            "spec:\n  pools:",
            "spec:\n  image: rustfs/rustfs:1.0.0-alpha.99\n  pools:",
        );
        assert!(bad_request_message(&incompatible_image).contains("Tokio io_uring"));

        let invalid_profile = MINIMAL_TENANT_YAML.replace(
            "spec:\n  pools:",
            "spec:\n  securityContext:\n    seccompProfile:\n      type: RuntimeDefaut\n  pools:",
        );
        assert!(bad_request_message(&invalid_profile).contains("must be RuntimeDefault"));
    }

    #[test]
    fn yaml_create_rejects_unknown_nested_field_with_exact_path() {
        let persistence_yaml = MINIMAL_TENANT_YAML.replace(
            "        volumesPerServer: 1",
            "        volumesPerServer: 1\n        volumePerSever: 2",
        );

        assert_eq!(
            bad_request_message(&persistence_yaml),
            "Unknown field(s) in Tenant YAML: spec.pools.0.persistence.volumePerSever"
        );

        let security_context_yaml = MINIMAL_TENANT_YAML.replace(
            "spec:\n",
            "spec:\n  containerSecurityContex:\n    runAsUser: 1000\n",
        );
        assert_eq!(
            bad_request_message(&security_context_yaml),
            "Unknown field(s) in Tenant YAML: spec.containerSecurityContex"
        );

        let flattened_pool_yaml = MINIMAL_TENANT_YAML.replace(
            "      servers: 1",
            "      servers: 1\n      unknownPoolSetting: true",
        );
        assert_eq!(
            bad_request_message(&flattened_pool_yaml),
            "Unknown field(s) in Tenant YAML: spec.pools.0.unknownPoolSetting"
        );
    }

    #[test]
    fn yaml_create_reports_optional_object_unknown_field_once_without_wrapper_segments() {
        let yaml = MINIMAL_TENANT_YAML.replace(
            "spec:\n",
            "spec:\n  containerSecurityContext:\n    runAsUser: 1000\n    unknownSecurity: true\n",
        );

        let message = bad_request_message(&yaml);
        assert_eq!(
            message,
            "Unknown field(s) in Tenant YAML: spec.containerSecurityContext.unknownSecurity"
        );
        assert!(!message.contains('?'));
        assert_eq!(message.matches("unknownSecurity").count(), 1);
    }

    #[test]
    fn yaml_create_reports_metadata_array_unknown_field_without_wrapper_segments() {
        let yaml = MINIMAL_TENANT_YAML.replace(
            "  namespace: storage",
            "  namespace: storage\n  ownerReferences:\n    - apiVersion: v1\n      kind: ConfigMap\n      name: tenant-owner\n      uid: owner-uid\n      unknownOwnerField: true",
        );

        let message = bad_request_message(&yaml);
        assert_eq!(
            message,
            "Unknown field(s) in Tenant YAML: metadata.ownerReferences.0.unknownOwnerField"
        );
        assert!(!message.contains('?'));
        assert_eq!(message.matches("unknownOwnerField").count(), 1);
    }

    #[test]
    fn yaml_create_allows_arbitrary_keys_in_schema_maps() {
        let yaml = MINIMAL_TENANT_YAML.replace(
            "      servers: 1",
            "      servers: 1\n      nodeSelector:\n        topology.kubernetes.io/zone: zone-a\n      resources:\n        requests:\n          example.com/device: '1'",
        );

        let tenant = parse_tenant_yaml_for_create(&yaml)
            .expect("schema additionalProperties maps should accept arbitrary keys");
        let scheduling = &tenant.spec.pools[0].scheduling;
        assert_eq!(
            scheduling
                .node_selector
                .as_ref()
                .and_then(|selector| selector.get("topology.kubernetes.io/zone"))
                .map(String::as_str),
            Some("zone-a")
        );
    }

    #[test]
    fn raw_yaml_update_rejects_unknown_top_level_field_with_exact_path() {
        let yaml = MINIMAL_TENANT_YAML.replace("kind: Tenant", "kind: Tenant\nunknownRoot: true");

        assert_eq!(
            bad_parse_request_message(&yaml),
            "Unknown field(s) in Tenant YAML: unknownRoot"
        );
    }

    #[tokio::test]
    async fn namespace_create_accepts_verified_already_exists_race() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let service = service_fn({
            let request_count = Arc::clone(&request_count);
            move |request: http::Request<Body>| {
                let request_number = request_count.fetch_add(1, Ordering::SeqCst);
                async move {
                    let (status, body) = match request_number {
                        0 => {
                            assert_eq!(request.method(), http::Method::GET);
                            assert_eq!(request.uri().path(), "/api/v1/namespaces/storage");
                            (
                                http::StatusCode::NOT_FOUND,
                                serde_json::json!({
                                    "status": "Failure",
                                    "message": "namespaces \"storage\" not found",
                                    "reason": "NotFound",
                                    "code": 404
                                }),
                            )
                        }
                        1 => {
                            assert_eq!(request.method(), http::Method::POST);
                            assert_eq!(request.uri().path(), "/api/v1/namespaces");
                            (
                                http::StatusCode::CONFLICT,
                                serde_json::json!({
                                    "status": "Failure",
                                    "message": "namespaces \"storage\" already exists",
                                    "reason": "AlreadyExists",
                                    "code": 409
                                }),
                            )
                        }
                        2 => {
                            assert_eq!(request.method(), http::Method::GET);
                            assert_eq!(request.uri().path(), "/api/v1/namespaces/storage");
                            (
                                http::StatusCode::OK,
                                serde_json::json!({
                                    "apiVersion": "v1",
                                    "kind": "Namespace",
                                    "metadata": { "name": "storage" }
                                }),
                            )
                        }
                        other => panic!("unexpected request number {other}"),
                    };

                    Ok::<_, std::convert::Infallible>(
                        http::Response::builder()
                            .status(status)
                            .body(Body::from(
                                serde_json::to_vec(&body).expect("response body should serialize"),
                            ))
                            .expect("response should build"),
                    )
                }
            }
        });
        let client = Client::new(service, "default");

        ensure_namespace_exists(&client, "storage")
            .await
            .expect("AlreadyExists race should be treated as success after verification");

        assert_eq!(request_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn raw_yaml_parser_allows_omitted_identity_but_validates_type_metadata() {
        let without_identity = MINIMAL_TENANT_YAML.replace(
            "metadata:\n  name: legacy-root\n  namespace: storage\n",
            "metadata: {}\n",
        );
        let tenant = parse_tenant_yaml(&without_identity)
            .expect("raw update YAML may omit metadata name and namespace");
        assert!(tenant.metadata.name.is_none());
        assert!(tenant.metadata.namespace.is_none());

        let wrong_kind = without_identity.replace("kind: Tenant", "kind: ConfigMap");
        assert!(bad_parse_request_message(&wrong_kind).contains("kind"));
    }

    #[test]
    fn serialized_tenant_round_trips_through_raw_yaml_parser() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let yaml = serde_yaml_ng::to_string(&tenant).expect("Tenant should serialize to YAML");

        assert!(yaml.contains("apiVersion: rustfs.com/v1alpha1"));
        assert!(yaml.contains("kind: Tenant"));

        let parsed = parse_tenant_yaml(&yaml).expect("GET YAML should be accepted by PUT parser");
        assert_eq!(parsed.metadata.name, tenant.metadata.name);
        assert_eq!(parsed.metadata.namespace, tenant.metadata.namespace);
        assert_eq!(parsed.spec.pools.len(), tenant.spec.pools.len());
    }

    fn bad_request_message(yaml: &str) -> String {
        match parse_tenant_yaml_for_create(yaml) {
            Err(Error::BadRequest { message }) => message,
            result => panic!("expected BadRequest, got {result:?}"),
        }
    }

    fn bad_parse_request_message(yaml: &str) -> String {
        match parse_tenant_yaml(yaml) {
            Err(Error::BadRequest { message }) => message,
            result => panic!("expected BadRequest, got {result:?}"),
        }
    }
}
