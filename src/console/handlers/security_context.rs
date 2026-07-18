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
    models::encryption::{PatchField, SecurityContextInfo, UpdateSecurityContextRequest},
    state::Claims,
};
use crate::types::v1alpha1::security_context::PodSecurityContextOverride;
use crate::types::v1alpha1::tenant::Tenant;
use axum::{Extension, Json, extract::Path};
use kube::{Api, Client};

/// GET /namespaces/:namespace/tenants/:name/security-context
///
/// Returns the legacy Console form subset. Advanced settings remain available via raw YAML.
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

    Ok(Json(security_context_info(
        tenant.spec.security_context.as_ref(),
    )))
}

fn security_context_info(
    security_context: Option<&PodSecurityContextOverride>,
) -> SecurityContextInfo {
    SecurityContextInfo::from_override(security_context)
}

fn apply_patch_field<T: Copy>(target: &mut Option<T>, field: &PatchField<T>) {
    match field {
        PatchField::Missing => {}
        PatchField::Null => *target = None,
        PatchField::Value(value) => *target = Some(*value),
    }
}

fn apply_validated_security_context_update(
    tenant: &mut Tenant,
    body: &UpdateSecurityContextRequest,
) -> Result<bool> {
    let has_updates = !matches!(body.run_as_user, PatchField::Missing)
        || !matches!(body.run_as_group, PatchField::Missing)
        || !matches!(body.fs_group, PatchField::Missing)
        || !matches!(body.run_as_non_root, PatchField::Missing);
    if !has_updates {
        return Ok(false);
    }

    let context = tenant.spec.security_context.get_or_insert_default();
    apply_patch_field(&mut context.run_as_user, &body.run_as_user);
    apply_patch_field(&mut context.run_as_group, &body.run_as_group);
    apply_patch_field(&mut context.fs_group, &body.fs_group);
    apply_patch_field(&mut context.run_as_non_root, &body.run_as_non_root);
    validate_tenant_for_write(tenant)?;

    Ok(true)
}

/// PUT /namespaces/:namespace/tenants/:name/security-context
///
/// Updates only legacy Pod-level fields without replacing advanced security settings.
pub async fn update_security_context(
    Path((namespace, name)): Path<(String, String)>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<UpdateSecurityContextRequest>,
) -> Result<Json<SecurityContextUpdateResponse>> {
    let client = create_client(&claims).await?;
    let api: Api<Tenant> = Api::namespaced(client, &namespace);

    // Read, validate, and replace with resourceVersion optimistic concurrency. Retrying the full
    // sequence prevents two individually valid partial updates from racing into an invalid
    // runAsUser/runAsNonRoot combination.
    const MAX_RETRIES: u32 = 3;
    let mut last_conflict = None;
    for _ in 0..MAX_RETRIES {
        let mut tenant = api
            .get(&name)
            .await
            .map_err(|e| error::map_kube_error(e, format!("Tenant '{}'", name)))?;

        if !apply_validated_security_context_update(&mut tenant, &body)? {
            return Ok(Json(SecurityContextUpdateResponse {
                success: true,
                message: "SecurityContext updated".to_string(),
            }));
        }

        match api.replace(&name, &Default::default(), &tenant).await {
            Ok(_) => {
                return Ok(Json(SecurityContextUpdateResponse {
                    success: true,
                    message: "SecurityContext updated".to_string(),
                }));
            }
            Err(error) => {
                let mapped = error::map_kube_error(error, format!("Tenant '{}'", name));
                if !matches!(&mapped, Error::Conflict { .. }) {
                    return Err(mapped);
                }
                last_conflict = Some(mapped);
            }
        }
    }

    Err(last_conflict.unwrap_or_else(|| Error::Conflict {
        message: "Resource was modified by another request, please retry".to_string(),
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

#[cfg(test)]
mod tests {
    use super::{apply_validated_security_context_update, security_context_info};
    use crate::console::error::Error;
    use crate::console::models::encryption::{PatchField, UpdateSecurityContextRequest};
    use crate::types::v1alpha1::security_context::PodSecurityContextOverride;

    #[test]
    fn legacy_root_context_reports_effective_non_root_false() {
        let context = PodSecurityContextOverride {
            run_as_user: Some(0),
            run_as_non_root: None,
            ..Default::default()
        };

        let info = security_context_info(Some(&context));

        assert_eq!(info.run_as_user, Some(0));
        assert_eq!(info.run_as_non_root, None);
        assert!(!info.effective_run_as_non_root);
    }

    #[test]
    fn explicit_non_root_setting_wins_in_console_response() {
        let context = PodSecurityContextOverride {
            run_as_user: Some(0),
            run_as_non_root: Some(true),
            ..Default::default()
        };

        let info = security_context_info(Some(&context));

        assert_eq!(info.run_as_non_root, Some(true));
        assert!(info.effective_run_as_non_root);
    }

    #[test]
    fn absent_context_reports_default_without_inventing_raw_values() {
        let info = security_context_info(None);

        assert_eq!(info.run_as_user, None);
        assert_eq!(info.run_as_non_root, None);
        assert!(info.effective_run_as_non_root);
    }

    #[test]
    fn partial_update_preserves_advanced_and_untouched_fields() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        tenant.spec.security_context = Some(PodSecurityContextOverride {
            run_as_user: Some(10_001),
            run_as_group: Some(10_002),
            seccomp_profile: Some(k8s_openapi::api::core::v1::SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let changed = apply_validated_security_context_update(
            &mut tenant,
            &UpdateSecurityContextRequest {
                run_as_user: PatchField::Null,
                run_as_group: PatchField::Missing,
                fs_group: PatchField::Value(20_001),
                run_as_non_root: PatchField::Value(false),
            },
        )
        .expect("valid partial update");

        assert!(changed);
        let context = tenant.spec.security_context.expect("security context");
        assert_eq!(context.run_as_user, None);
        assert_eq!(context.run_as_group, Some(10_002));
        assert_eq!(context.fs_group, Some(20_001));
        assert_eq!(context.run_as_non_root, Some(false));
        assert_eq!(
            context.seccomp_profile.map(|profile| profile.type_),
            Some("RuntimeDefault".to_string())
        );
    }

    #[test]
    fn empty_update_skips_validation_and_kubernetes_write() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let changed = apply_validated_security_context_update(
            &mut tenant,
            &UpdateSecurityContextRequest {
                run_as_user: PatchField::Missing,
                run_as_group: PatchField::Missing,
                fs_group: PatchField::Missing,
                run_as_non_root: PatchField::Missing,
            },
        )
        .expect("empty update should be a no-op");

        assert!(!changed);
        assert!(tenant.spec.security_context.is_none());
    }

    #[test]
    fn contradictory_root_update_is_rejected_before_write() {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let error = apply_validated_security_context_update(
            &mut tenant,
            &UpdateSecurityContextRequest {
                run_as_user: PatchField::Value(0),
                run_as_group: PatchField::Missing,
                fs_group: PatchField::Missing,
                run_as_non_root: PatchField::Value(true),
            },
        )
        .expect_err("UID 0 with runAsNonRoot=true must not be persisted");

        assert!(matches!(
            error,
            Error::BadRequest { message }
                if message.contains("UID 0") && message.contains("explicitly true")
        ));
    }
}
