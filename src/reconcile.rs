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

use crate::context::Context;
use crate::types::v1alpha1::tenant::Tenant;
use crate::{context, types};
use kube::ResourceExt;
use kube::runtime::controller::Action;
use kube::runtime::events::EventType;
use snafu::Snafu;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error};

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(transparent)]
    Context { source: context::Error },

    #[snafu(transparent)]
    Types { source: types::error::Error },
}

pub async fn reconcile_rustfs(tenant: Arc<Tenant>, ctx: Arc<Context>) -> Result<Action, Error> {
    let ns = tenant.namespace()?;
    let latest_tenant = ctx.get::<Tenant>(&tenant.name(), &ns).await?;

    if latest_tenant.metadata.deletion_timestamp.is_some() {
        debug!(
            "tenant {} is deleted, deletion_timestamp is {:?}",
            tenant.name(),
            latest_tenant.metadata.deletion_timestamp
        );
        return Ok(Action::await_change());
    }

    // Validate credential Secret if configured
    // This only validates the Secret exists and has required keys.
    // Actual credential injection happens via secretKeyRef in the StatefulSet.
    if let Some(ref cfg) = latest_tenant.spec.creds_secret
        && !cfg.name.is_empty()
        && let Err(e) = ctx.validate_credential_secret(&latest_tenant).await
    {
        // Record event for credential validation failure
        let _ = ctx
            .record(
                &latest_tenant,
                EventType::Warning,
                "CredentialValidationFailed",
                &format!("Failed to validate credentials: {}", e),
            )
            .await;
        return Err(e.into());
    }

    // 1. Create RBAC resources (conditionally based on service account settings)
    let custom_sa = latest_tenant.spec.service_account_name.is_some();
    let create_rbac = latest_tenant
        .spec
        .create_service_account_rbac
        .unwrap_or(false);

    if !custom_sa || create_rbac {
        // Create Role
        let role = ctx.apply(&latest_tenant.new_role(), &ns).await?;

        if !custom_sa {
            // Create default ServiceAccount and bind it
            let service_account = ctx.apply(&latest_tenant.new_service_account(), &ns).await?;
            ctx.apply(
                &latest_tenant.new_role_binding(&service_account.name_any(), &role),
                &ns,
            )
            .await?;
        } else {
            // Use custom ServiceAccount and bind it
            let sa_name = latest_tenant.service_account_name();
            ctx.apply(&latest_tenant.new_role_binding(&sa_name, &role), &ns)
                .await?;
        }
    }

    // 2. Create Services
    ctx.apply(&latest_tenant.new_io_service(), &ns).await?;
    ctx.apply(&latest_tenant.new_console_service(), &ns).await?;
    ctx.apply(&latest_tenant.new_headless_service(), &ns)
        .await?;

    // 3. Create or update StatefulSets for each pool
    for pool in &latest_tenant.spec.pools {
        let ss_name = format!("{}-{}", latest_tenant.name(), pool.name);

        // Try to get existing StatefulSet
        match ctx.get::<k8s_openapi::api::apps::v1::StatefulSet>(&ss_name, &ns).await {
            Ok(existing_ss) => {
                // StatefulSet exists - check if update is needed
                debug!("StatefulSet {} exists, checking if update needed", ss_name);

                // First, validate that the update is safe (no immutable field changes)
                if let Err(e) = latest_tenant.validate_statefulset_update(&existing_ss, pool) {
                    error!("StatefulSet {} update validation failed: {}", ss_name, e);

                    // Record event for validation failure
                    let _ = ctx
                        .record(
                            &latest_tenant,
                            EventType::Warning,
                            "StatefulSetUpdateValidationFailed",
                            &format!("Cannot update StatefulSet {}: {}", ss_name, e),
                        )
                        .await;

                    return Err(e.into());
                }

                // Check if update is actually needed
                if latest_tenant.statefulset_needs_update(&existing_ss, pool)? {
                    debug!("StatefulSet {} needs update, applying changes", ss_name);

                    // Record event for update start
                    let _ = ctx
                        .record(
                            &latest_tenant,
                            EventType::Normal,
                            "StatefulSetUpdateStarted",
                            &format!("Updating StatefulSet {}", ss_name),
                        )
                        .await;

                    // Apply the update
                    ctx.apply(&latest_tenant.new_statefulset(pool)?, &ns).await?;

                    debug!("StatefulSet {} updated successfully", ss_name);
                } else {
                    debug!("StatefulSet {} is up to date, no changes needed", ss_name);
                }
            }
            Err(e) if e.to_string().contains("NotFound") => {
                // StatefulSet doesn't exist - create it
                debug!("StatefulSet {} not found, creating", ss_name);

                // Record event for creation
                let _ = ctx
                    .record(
                        &latest_tenant,
                        EventType::Normal,
                        "StatefulSetCreated",
                        &format!("Creating StatefulSet {}", ss_name),
                    )
                    .await;

                ctx.apply(&latest_tenant.new_statefulset(pool)?, &ns).await?;

                debug!("StatefulSet {} created successfully", ss_name);
            }
            Err(e) => {
                // Other error - propagate
                error!("Failed to get StatefulSet {}: {}", ss_name, e);
                return Err(e.into());
            }
        }
    }

    Ok(Action::await_change())
}

pub fn error_policy(_object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    error!("error_policy: {:?}", error);

    // todo: update tenant status (issue #42)

    // Use different requeue strategies based on error type:
    // - User-fixable errors (credentials, validation): Longer intervals to reduce spam
    // - Transient errors (API, network): Shorter intervals for quick recovery
    match error {
        Error::Context { source } => match source {
            // Credential validation errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            context::Error::CredentialSecretNotFound { .. }
            | context::Error::CredentialSecretMissingKey { .. }
            | context::Error::CredentialSecretInvalidEncoding { .. }
            | context::Error::CredentialSecretTooShort { .. } => {
                Action::requeue(Duration::from_secs(60))
            }

            // Kubernetes API errors - might be transient (network, API server issues)
            // Use shorter requeue for faster recovery
            context::Error::Kube { .. } | context::Error::Record { .. } => {
                Action::requeue(Duration::from_secs(5))
            }

            // Other context errors - use moderate requeue
            _ => Action::requeue(Duration::from_secs(15)),
        },

        // Type errors - validation issues, use moderate requeue
        Error::Types { source } => match source {
            // Immutable field modification errors - require user intervention
            // Use 60-second requeue to reduce event/log spam while user fixes the issue
            types::error::Error::ImmutableFieldModified { .. } => {
                Action::requeue(Duration::from_secs(60))
            }

            // Other type errors - use moderate requeue
            _ => Action::requeue(Duration::from_secs(15)),
        },
    }
}

#[cfg(test)]
mod tests {
    // Test 10: RBAC creation logic - default behavior
    #[test]
    fn test_should_create_rbac_default() {
        let tenant = crate::tests::create_test_tenant(None, None);

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(!custom_sa, "Should not have custom SA");
        assert!(
            !create_rbac,
            "createServiceAccountRbac should default to false"
        );
        assert!(should_create_rbac, "Should create RBAC when no custom SA");
    }

    // Test 11: RBAC creation logic - custom SA with createServiceAccountRbac=true
    #[test]
    fn test_should_create_rbac_custom_sa_with_rbac() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(true));

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(create_rbac, "createServiceAccountRbac should be true");
        assert!(
            should_create_rbac,
            "Should create RBAC when explicitly requested"
        );
    }

    // Test 12: RBAC creation logic - custom SA with createServiceAccountRbac=false
    #[test]
    fn test_should_skip_rbac_custom_sa_without_rbac() {
        let tenant =
            crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), Some(false));

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(!create_rbac, "createServiceAccountRbac should be false");
        assert!(!should_create_rbac, "Should skip RBAC creation");
    }

    // Test 13: RBAC creation logic - custom SA with createServiceAccountRbac=None (default)
    #[test]
    fn test_should_skip_rbac_custom_sa_default() {
        let tenant = crate::tests::create_test_tenant(Some("my-custom-sa".to_string()), None);

        let custom_sa = tenant.spec.service_account_name.is_some();
        let create_rbac = tenant.spec.create_service_account_rbac.unwrap_or(false);
        let should_create_rbac = !custom_sa || create_rbac;

        assert!(custom_sa, "Should have custom SA");
        assert!(
            !create_rbac,
            "createServiceAccountRbac should default to false"
        );
        assert!(
            !should_create_rbac,
            "Should skip RBAC when None treated as false"
        );
    }

    // Test 14: Service account determination in reconcile logic
    #[test]
    fn test_determine_sa_name_in_reconcile() {
        // Test default behavior
        let tenant_default = crate::tests::create_test_tenant(None, None);
        let sa_name = tenant_default.service_account_name();
        assert_eq!(sa_name, "test-tenant-sa");

        // Test custom SA
        let tenant_custom = crate::tests::create_test_tenant(Some("custom-sa".to_string()), None);
        let sa_name_custom = tenant_custom.service_account_name();
        assert_eq!(sa_name_custom, "custom-sa");
    }
}
