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

// todo
// 1. 创建role/rolegroup/serviceaccount
// 2. 创建configmap
// 3. 创建service
// 4. 创建statfulset
pub async fn reconcile_rustfs(tenant: Arc<Tenant>, ctx: Arc<Context>) -> Result<Action, Error> {
    let ns = tenant.namespace()?;
    let latest_tenant = ctx.get::<Tenant>(&tenant.name(), &ns).await?;

    if latest_tenant.metadata.deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }

    let (role, service_account) = (
        ctx.apply(&latest_tenant.new_role(), &ns).await?,
        ctx.apply(&latest_tenant.new_service_account(), &ns).await?,
    );

    let service_account = ctx
        .apply(
            &latest_tenant.new_role_binding(&service_account, &role),
            &ns,
        )
        .await?;
    Ok(Action::await_change())
}

pub fn error_policy(_object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    error!("{:?}", error);
    Action::requeue(Duration::from_secs(5))
}
