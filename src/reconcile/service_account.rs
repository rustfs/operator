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

pub async fn check_and_crate_service_account(tenant: &Tenant, ctx: &Context) -> Result<(), Error> {
    let sa = ctx
        .apply(tenant.new_service_account(), &tenant.namespace()?)
        .await?;
    let role = ctx.apply(tenant.new_role(), &tenant.namespace()?).await?;
    ctx.apply(tenant.new_role_binding(&sa, &role), &tenant.namespace()?)
        .await?;
    Ok(())
}
