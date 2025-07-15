// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::context::Context;
use crate::error::Error;
use crate::types::v1alpha1::tenant::Tenant;

use crate::types::v1alpha1::status::state::State;
use k8s_openapi::api::core::v1 as corev1;
use kube::runtime::events::EventType;

pub async fn check_or_create_io_service(
    mut tenant: Tenant,
    ctx: &Context,
) -> Result<Tenant, Error> {
    let svc = match ctx
        .get::<corev1::Service>("rustfs", &tenant.namespace()?)
        .await
    {
        Ok(svc) => svc,
        Err(e) if e.is_not_found() => {
            let new_tenant = ctx
                .update_status(&tenant, State::ProvisioningIOService, 0)
                .await?;

            // create a new service
            let svc = ctx
                .create(&new_tenant.new_io_service(), &new_tenant.namespace()?)
                .await?;

            ctx.record(
                &new_tenant,
                EventType::Normal,
                "ServiceCreated",
                "IO Service Created",
            )
            .await?;

            tenant = new_tenant;
            svc
        }
        e => e?,
    };

    // todo check the service is match or not.

    Ok(tenant)
}

pub async fn check_or_create_console_service(
    mut tenant: Tenant,
    ctx: &Context,
) -> Result<Tenant, Error> {
    let svc = match ctx
        .get::<corev1::Service>(&tenant.console_service_name(), &tenant.namespace()?)
        .await
    {
        Ok(svc) => svc,
        Err(e) if e.is_not_found() => {
            let new_tenant = ctx
                .update_status(&tenant, State::ProvisioningConsoleService, 0)
                .await?;

            // create a new service
            let svc = ctx
                .create(&new_tenant.new_console_service(), &new_tenant.namespace()?)
                .await?;

            ctx.record(
                &new_tenant,
                EventType::Normal,
                "ServiceCreated",
                "Console Service Created",
            )
            .await?;

            tenant = new_tenant;
            svc
        }
        e => e?,
    };

    // todo check the service is match or not.

    Ok(tenant)
}

pub async fn check_or_create_headless_service(
    mut tenant: Tenant,
    ctx: &Context,
) -> Result<Tenant, Error> {
    let svc = match ctx
        .get::<corev1::Service>(&tenant.headless_service_name(), &tenant.namespace()?)
        .await
    {
        Ok(svc) => svc,
        Err(e) if e.is_not_found() => {
            let new_tenant = ctx
                .update_status(&tenant, State::ProvisioningHeadlessService, 0)
                .await?;

            // create a new service
            let svc = ctx
                .create(&new_tenant.new_console_service(), &new_tenant.namespace()?)
                .await?;

            ctx.record(
                &new_tenant,
                EventType::Normal,
                "ServiceCreated",
                "Console Service Created",
            )
            .await?;

            tenant = new_tenant;
            svc
        }
        e => e?,
    };

    // todo check the service is match or not.

    Ok(tenant)
}
