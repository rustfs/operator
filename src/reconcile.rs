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
use snafu::Snafu;
use std::sync::Arc;
use std::time::Duration;
use tracing::error;

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
        return Ok(Action::await_change());
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

    // 3. Create StatefulSets for each pool
    for pool in &latest_tenant.spec.pools {
        ctx.apply(&latest_tenant.new_statefulset(pool)?, &ns)
            .await?;
    }

    Ok(Action::await_change())
}

pub fn error_policy(_object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    error!("{:?}", error);
    Action::requeue(Duration::from_secs(5))
}
