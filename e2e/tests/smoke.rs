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

use anyhow::Result;
use rustfs_operator_e2e::framework::{
    artifacts::ArtifactCollector, config::E2eConfig, kube_client, kubectl::Kubectl, live,
    resources, storage, tools::required_tool_checks, wait,
};

#[test]
fn smoke_required_tool_inventory_is_defined() {
    assert!(required_tool_checks().len() >= 4);
}

#[test]
#[ignore = "requires a dedicated Kind cluster; run through `make e2e-smoke-live`"]
fn smoke_dedicated_context_is_active() -> Result<()> {
    let config = E2eConfig::from_env();

    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;
    Ok(())
}

#[test]
#[ignore = "requires deployed operator components; run through `make e2e-smoke-live`"]
fn smoke_operator_and_console_deployments_are_ready() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let kubectl = Kubectl::new(&config).namespaced(&config.operator_namespace);
    kubectl
        .command([
            "rollout",
            "status",
            "deployment/rustfs-operator",
            "--timeout=180s",
        ])
        .run_checked()?;
    kubectl
        .command([
            "rollout",
            "status",
            "deployment/rustfs-operator-console",
            "--timeout=180s",
        ])
        .run_checked()?;

    Ok(())
}

#[tokio::test]
#[ignore = "creates storage, credentials, and a Tenant; run through `make e2e-smoke-live`"]
async fn smoke_apply_tenant_and_wait_ready() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let result = async {
        storage::prepare_local_storage(&config)?;
        resources::apply_smoke_tenant_resources(&config)?;

        let client = kube_client::default_client().await?;
        let tenants = kube_client::tenant_api(client, &config.test_namespace);
        wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout).await?;
        Ok(())
    }
    .await;

    if let Err(error) = &result {
        let collector = ArtifactCollector::new(&config.artifacts_dir);
        if let Err(artifact_error) =
            collector.collect_kubernetes_snapshot("smoke_apply_tenant_and_wait_ready", &config)
        {
            eprintln!("failed to collect e2e artifacts after {error}: {artifact_error}");
        }
    }

    result
}
