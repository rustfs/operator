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

use crate::console::{
    error::{self, Error, Result},
    models::encryption::{SecurityContextInfo, UpdateSecurityContextRequest},
    state::Claims,
};
use crate::types::v1alpha1::encryption::PodSecurityContextOverride;
use crate::types::v1alpha1::tenant::Tenant;
use axum::{Extension, Json, extract::Path};
use kube::api::{Patch, PatchParams};
use kube::{Api, Client};

/// GET /namespaces/:namespace/tenants/:name/security-context
pub async fn get_security_context(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<SecurityContextInfo>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    let info = tenant.spec.security_context.as_ref().map_or_else(
        || SecurityContextInfo {
            run_as_user: None,
            run_as_group: None,
            fs_group: None,
            run_as_non_root: None,
        },
        |sc| SecurityContextInfo {
            run_as_user: sc.run_as_user,
            run_as_group: sc.run_as_group,
            fs_group: sc.fs_group,
            run_as_non_root: sc.run_as_non_root,
        },
    );

    Ok(Json(info))
}

/// PUT /namespaces/:namespace/tenants/:name/security-context
pub async fn update_security_context(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<UpdateSecurityContextRequest>,
) -> Result<Json<SecurityContextUpdateResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    let _tenant = api
        .get(&name)
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    let security_context = PodSecurityContextOverride {
        run_as_user: body.run_as_user,
        run_as_group: body.run_as_group,
        fs_group: body.fs_group,
        run_as_non_root: body.run_as_non_root,
    };

    let patch = serde_json::json!({
        "spec": {
            "securityContext": serde_json::to_value(&security_context).map_err(|e| Error::Json { source: e })?
        }
    });

    api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

    Ok(Json(SecurityContextUpdateResponse {
        success: true,
        message: "SecurityContext updated".to_string(),
    }))
}

#[derive(Debug, serde::Serialize)]
pub struct SecurityContextUpdateResponse {
    pub success: bool,
    pub message: String,
}

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
