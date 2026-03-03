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
    Extension, Json,
    body::Body,
    extract::{Path, Query},
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use k8s_openapi::api::core::v1 as corev1;
use kube::{
    Api, Client, ResourceExt,
    api::{DeleteParams, ListParams, LogParams},
};
use crate::console::{
    error::{self, Error, Result},
    models::pod::*,
    state::Claims,
};

/// 校验 Pod 是否属于指定 Tenant（通过 rustfs.tenant 标签）
fn ensure_pod_belongs_to_tenant(
    pod: &corev1::Pod,
    tenant_name: &str,
    pod_name: &str,
) -> Result<()> {
    let pod_tenant = pod
        .metadata
        .labels
        .as_ref()
        .and_then(|l| l.get("rustfs.tenant").map(String::as_str));
    if pod_tenant != Some(tenant_name) {
        return Err(Error::NotFound {
            resource: format!("Pod '{}'", pod_name),
        });
    }
    Ok(())
}

/// 列出 Tenant 的所有 Pods
pub async fn list_pods(
    Path((namespace, tenant_name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PodListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Pod> = Api::namespaced(client, &namespace);

    // 查询带有 Tenant 标签的 Pods
    let pods = api
        .list(&ListParams::default().labels(&format!("rustfs.tenant={}", tenant_name)))
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pods for tenant '{}'", tenant_name)))?;

    let mut pod_list = Vec::new();

    for pod in pods.items {
        let name = pod.name_any();
        let status = pod.status.as_ref();
        let spec = pod.spec.as_ref();

        // 提取 Pool 名称（从 Pod 名称中解析）
        let pool = pod
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("rustfs.pool"))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        // Pod 阶段
        let phase = status
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        // 整体状态
        let pod_status = if let Some(status) = status {
            if let Some(conditions) = &status.conditions {
                if conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
                {
                    "Running"
                } else {
                    "NotReady"
                }
            } else {
                &phase
            }
        } else {
            "Unknown"
        };

        // 节点名称
        let node = spec.and_then(|s| s.node_name.clone());

        // 容器就绪状态
        let (ready_count, total_count) = if let Some(status) = status {
            let total = status
                .container_statuses
                .as_ref()
                .map(|c| c.len())
                .unwrap_or(0);
            let ready = status
                .container_statuses
                .as_ref()
                .map(|containers| containers.iter().filter(|c| c.ready).count())
                .unwrap_or(0);
            (ready, total)
        } else {
            (0, 0)
        };

        // 重启次数
        let restarts = status
            .and_then(|s| s.container_statuses.as_ref())
            .map(|containers| containers.iter().map(|c| c.restart_count).sum::<i32>())
            .unwrap_or(0);

        // 创建时间和 Age
        let created_at = pod
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|ts| ts.0.to_rfc3339());

        let age = pod
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|ts| {
                let duration = chrono::Utc::now().signed_duration_since(ts.0);
                format_duration(duration)
            })
            .unwrap_or_else(|| "Unknown".to_string());

        pod_list.push(PodListItem {
            name,
            pool,
            status: pod_status.to_string(),
            phase,
            node,
            ready: format!("{}/{}", ready_count, total_count),
            restarts,
            age,
            created_at,
        });
    }

    Ok(Json(PodListResponse { pods: pod_list }))
}

/// 删除 Pod
pub async fn delete_pod(
    Path((namespace, tenant_name, pod_name)): Path<(String, String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DeletePodResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Pod> = Api::namespaced(client, &namespace);

    let pod = api
        .get(&pod_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;
    ensure_pod_belongs_to_tenant(&pod, &tenant_name, &pod_name)?;

    api.delete(&pod_name, &DeleteParams::default())
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;

    Ok(Json(DeletePodResponse {
        success: true,
        message: format!(
            "Pod '{}' deletion initiated. StatefulSet will recreate it.",
            pod_name
        ),
    }))
}

/// 重启 Pod（通过删除实现）
pub async fn restart_pod(
    Path((namespace, tenant_name, pod_name)): Path<(String, String, String)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<RestartPodRequest>,
) -> Result<Json<DeletePodResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Pod> = Api::namespaced(client, &namespace);

    let pod = api
        .get(&pod_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;
    ensure_pod_belongs_to_tenant(&pod, &tenant_name, &pod_name)?;

    // 删除 Pod，StatefulSet 控制器会自动重建
    let delete_params = if req.force {
        DeleteParams {
            grace_period_seconds: Some(0),
            ..Default::default()
        }
    } else {
        DeleteParams::default()
    };

    api.delete(&pod_name, &delete_params)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;

    Ok(Json(DeletePodResponse {
        success: true,
        message: format!(
            "Pod '{}' restart initiated. StatefulSet will recreate it.",
            pod_name
        ),
    }))
}

/// 获取 Pod 详情
pub async fn get_pod_details(
    Path((namespace, tenant_name, pod_name)): Path<(String, String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PodDetails>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Pod> = Api::namespaced(client, &namespace);

    let pod = api
        .get(&pod_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;
    ensure_pod_belongs_to_tenant(&pod, &tenant_name, &pod_name)?;

    // 提取详细信息
    let pool = pod
        .metadata
        .labels
        .as_ref()
        .and_then(|l| l.get("rustfs.pool"))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    let status_info = pod.status.as_ref();
    let spec = pod.spec.as_ref();

    // 构建状态
    let status = PodStatus {
        phase: status_info
            .and_then(|s| s.phase.clone())
            .unwrap_or_else(|| "Unknown".to_string()),
        conditions: status_info
            .and_then(|s| s.conditions.as_ref())
            .map(|conditions| {
                conditions
                    .iter()
                    .map(|c| PodCondition {
                        type_: c.type_.clone(),
                        status: c.status.clone(),
                        reason: c.reason.clone(),
                        message: c.message.clone(),
                        last_transition_time: c
                            .last_transition_time
                            .as_ref()
                            .map(|t| t.0.to_rfc3339()),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        host_ip: status_info.and_then(|s| s.host_ip.clone()),
        pod_ip: status_info.and_then(|s| s.pod_ip.clone()),
        start_time: status_info
            .and_then(|s| s.start_time.as_ref())
            .map(|t| t.0.to_rfc3339()),
    };

    // 容器信息
    let containers = if let Some(container_statuses) =
        status_info.and_then(|s| s.container_statuses.as_ref())
    {
        container_statuses
            .iter()
            .map(|cs| {
                let state = if let Some(running) =
                    &cs.state.as_ref().and_then(|s| s.running.as_ref())
                {
                    ContainerState::Running {
                        started_at: running.started_at.as_ref().map(|t| t.0.to_rfc3339()),
                    }
                } else if let Some(waiting) = &cs.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                    ContainerState::Waiting {
                        reason: waiting.reason.clone(),
                        message: waiting.message.clone(),
                    }
                } else if let Some(terminated) =
                    &cs.state.as_ref().and_then(|s| s.terminated.as_ref())
                {
                    ContainerState::Terminated {
                        reason: terminated.reason.clone(),
                        exit_code: terminated.exit_code,
                        finished_at: terminated.finished_at.as_ref().map(|t| t.0.to_rfc3339()),
                    }
                } else {
                    ContainerState::Waiting {
                        reason: Some("Unknown".to_string()),
                        message: None,
                    }
                };

                ContainerInfo {
                    name: cs.name.clone(),
                    image: cs.image.clone(),
                    ready: cs.ready,
                    restart_count: cs.restart_count,
                    state,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Volume 信息
    let volumes = spec
        .and_then(|s| s.volumes.as_ref())
        .map(|vols| {
            vols.iter()
                .map(|v| {
                    let volume_type = if v.persistent_volume_claim.is_some() {
                        "PersistentVolumeClaim"
                    } else if v.empty_dir.is_some() {
                        "EmptyDir"
                    } else if v.config_map.is_some() {
                        "ConfigMap"
                    } else if v.secret.is_some() {
                        "Secret"
                    } else {
                        "Other"
                    };

                    VolumeInfo {
                        name: v.name.clone(),
                        volume_type: volume_type.to_string(),
                        claim_name: v
                            .persistent_volume_claim
                            .as_ref()
                            .map(|pvc| pvc.claim_name.clone()),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(PodDetails {
        name: pod.name_any(),
        namespace: pod.namespace().unwrap_or_default(),
        pool,
        status,
        containers,
        volumes,
        node: spec.and_then(|s| s.node_name.clone()),
        ip: status_info.and_then(|s| s.pod_ip.clone()),
        labels: pod.metadata.labels.unwrap_or_default(),
        annotations: pod.metadata.annotations.unwrap_or_default(),
        created_at: pod.metadata.creation_timestamp.map(|ts| ts.0.to_rfc3339()),
    }))
}

/// 获取 Pod 日志（流式传输）
pub async fn get_pod_logs(
    Path((namespace, tenant_name, pod_name)): Path<(String, String, String)>,
    Query(query): Query<LogsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Response> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Pod> = Api::namespaced(client, &namespace);

    let pod = api
        .get(&pod_name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;
    ensure_pod_belongs_to_tenant(&pod, &tenant_name, &pod_name)?;

    // 构建日志参数
    let mut log_params = LogParams {
        container: query.container,
        follow: query.follow,
        tail_lines: Some(query.tail_lines),
        timestamps: query.timestamps,
        ..Default::default()
    };

    // since_time 校验：仅当时间不晚于当前时间时使用
    if let Some(since_time) = &query.since_time {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(since_time) {
            let dt_utc = dt.with_timezone(&chrono::Utc);
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(dt_utc);
            if duration.num_seconds() >= 0 {
                log_params.since_seconds = Some(duration.num_seconds());
            }
            // 若 since_time 在未来，忽略该参数（不设置 since_seconds）
        }
    }

    // 获取日志流
    let log_stream = api
        .log_stream(&pod_name, &log_params)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Pod '{}'", pod_name)))?;

    // 将字节流转换为可用的 Body
    // kube-rs 返回的是 impl AsyncBufRead，我们需要逐行读取并转换为字节流
    use futures::io::AsyncBufReadExt;
    let lines = log_stream.lines();

    // 转换为字节流
    let byte_stream = lines.map_ok(|line| format!("{}\n", line).into_bytes());

    // 返回流式响应
    Ok(Body::from_stream(byte_stream).into_response())
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

/// 格式化时间间隔
fn format_duration(duration: chrono::Duration) -> String {
    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    if days > 0 {
        format!("{}d{}h", days, hours)
    } else if hours > 0 {
        format!("{}h{}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m", minutes)
    } else {
        format!("{}s", duration.num_seconds())
    }
}
