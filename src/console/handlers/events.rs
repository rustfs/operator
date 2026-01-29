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
use kube::{api::ListParams, Api, Client};
use snafu::ResultExt;

use crate::console::{
    error::{self, Error, Result},
    models::event::{EventItem, EventListResponse},
    state::Claims,
};

/// 列出 Tenant 相关的 Events
pub async fn list_tenant_events(
    Path((namespace, tenant)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<EventListResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<corev1::Event> = Api::namespaced(client, &namespace);

    // 查询与 Tenant 相关的 Events
    let events = api
        .list(&ListParams::default().fields(&format!("involvedObject.name={}", tenant)))
        .await
        .context(error::KubeApiSnafu)?;

    let items: Vec<EventItem> = events
        .items
        .into_iter()
        .map(|e| EventItem {
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
        })
        .collect();

    Ok(Json(EventListResponse { events: items }))
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
