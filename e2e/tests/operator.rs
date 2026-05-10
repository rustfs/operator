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

use anyhow::{Result, ensure};
use kube::Api;
use rustfs_operator_e2e::framework::{assertions, config::E2eConfig, kube_client, live};

use operator::types::v1alpha1::tenant::Tenant;

#[tokio::test]
#[ignore = "requires a live Tenant; run through `make e2e-live-run`"]
async fn operator_live_tenant_is_ready_and_observed() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let client = kube_client::default_client().await?;
    let tenants: Api<Tenant> = kube_client::tenant_api(client, &config.test_namespace);
    let tenant = tenants.get(&config.tenant_name).await?;

    ensure!(
        assertions::current_state(&tenant) == Some("Ready"),
        "tenant {} in namespace {} is not Ready: {:?}",
        config.tenant_name,
        config.test_namespace,
        tenant.status
    );
    assertions::require_condition(&tenant, "Ready", "True")?;
    assertions::require_condition(&tenant, "Degraded", "False")?;
    assertions::require_observed_generation_current(&tenant)?;

    Ok(())
}
