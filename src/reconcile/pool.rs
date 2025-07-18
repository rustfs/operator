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
use crate::types::v1alpha1::status;
use crate::types::v1alpha1::tenant::Tenant;

use k8s_openapi::api::apps::v1;
use kube::core::object::HasStatus;
use kube::runtime::events::EventType;
use std::collections::HashSet;
use tracing::{info, warn};

pub async fn check_pool_decommission(mut tenant: Tenant, ctx: &Context) -> Result<Tenant, Error> {
    let pool_names: HashSet<_> = tenant
        .status()
        .map(|status| status.pools.iter().map(|p| p.ss_name.clone()).collect())
        .unwrap_or_default();

    let status_pool_len = tenant
        .status()
        .map(|status| status.pools.len())
        .unwrap_or_default();

    // duplicate name is not allowed
    if pool_names.len() < status_pool_len {
        let new_tenant = ctx
            .update_status(&tenant, status::state::State::NotOwned, 0)
            .await?;
        tenant = new_tenant;
    }

    if tenant.spec.pools.len() >= status_pool_len {
        return Ok(tenant);
    }

    let mut name_set = HashSet::with_capacity(tenant.spec.pools.len());
    // decommission triggered due to spec.pools being fewer than status.pools
    for (index, pool) in tenant.spec.pools.iter().enumerate() {
        if pool.name.is_empty() {
            warn!("decommission is not allowed because the name of spec.pool[{index}] is empty");
            ctx.update_status(&tenant, status::state::State::DecommissioningNotAllowed, 0)
                .await?;

            return Err(Error::StrError(
                "remove pool not allowed due to empty pool name".into(),
            ));
        }

        if name_set.contains(pool.name.as_str()) {
            warn!(
                "decommission is not allowed because the name of spec.pool[{index}] is duplicated"
            );
            return Err(Error::StrError(
                "remove pool not allowed due to duplicate pool name".into(),
            ));
        }

        name_set.insert(pool.name.as_str());
    }

    // if the name of status.pools not in spec.pools, remove the statefulset
    for pool_name in pool_names {
        if !name_set.contains(pool_name.as_str()) {
            ctx.record(
                &tenant,
                EventType::Normal,
                "PoolRemove",
                format!("pool {pool_name} removed").as_str(),
            )
            .await?;
            ctx.delete::<v1::StatefulSet>(&pool_name, &tenant.namespace()?)
                .await
                .or_else(|e| if e.is_not_found() { Ok(()) } else { Err(e) })?;

            info!("tenant:{}, pool {pool_name} removed", tenant.name());
        }
    }

    Ok(tenant)
}
