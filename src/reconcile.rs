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

pub(crate) mod pool;
pub(crate) mod service;
pub(crate) mod service_account;

use crate::context::Context;
use crate::error::Error;
use crate::types::v1alpha1::status::state::State;
use crate::types::v1alpha1::tenant::Tenant;
use kube::runtime::controller::Action;
use std::sync::Arc;

pub async fn reconcile(tenant: Arc<Tenant>, ctx: Arc<Context>) -> Result<Action, Error> {
    let mut latest_tenant = ctx
        .get::<Tenant>(&tenant.name(), &tenant.namespace()?)
        .await?;

    if latest_tenant.metadata.deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }

    // service
    latest_tenant = check_and_create_service(latest_tenant, &ctx).await?;

    // check counts
    check_tenant_count(&latest_tenant, &ctx).await?;

    // service account\role\role binding
    service_account::check_and_crate_service_account(&latest_tenant, &ctx).await?;

    Ok(Action::await_change())
}

async fn check_and_create_service(
    mut latest_tenant: Tenant,
    ctx: &Context,
) -> Result<Tenant, Error> {
    latest_tenant = pool::check_pool_decommission(latest_tenant, ctx).await?;
    latest_tenant = service::check_or_create_io_service(latest_tenant, ctx).await?;
    latest_tenant = service::check_or_create_console_service(latest_tenant, ctx).await?;
    latest_tenant = service::check_or_create_headless_service(latest_tenant, ctx).await?;

    Ok(latest_tenant)
}

async fn check_tenant_count(tenant: &Tenant, ctx: &Context) -> Result<(), Error> {
    let all_tenant = ctx.list::<Tenant>(&tenant.namespace()?).await?;

    // if one tenant is Initialized
    if all_tenant
        .items
        .iter()
        .filter_map(|t| t.status.as_ref())
        .any(|status| status.current_state == State::Initialized.to_string())
    {
        ctx.update_status(tenant, State::MultipleTenantsExist, 0)
            .await?;
        return Err(Error::MultiError);
    }

    Ok(())
}
