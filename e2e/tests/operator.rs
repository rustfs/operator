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
use rustfs_operator_e2e::framework::{
    assertions, config::E2eConfig, kube_client, kubectl::Kubectl, live,
};
use serde_json::{Value, json};

use operator::types::v1alpha1::tenant::Tenant;

const CHECKPOINT_TEST_CRD: &str = "ownershipcheckpointtests.e2e.rustfs.com";
const CHECKPOINT_TEST_RESOURCE: &str = "ownershipcheckpointtests.e2e.rustfs.com";
const CHECKPOINT_TEST_NAME: &str = "ownership-checkpoint-contract";
const CHECKPOINT_TEST_CRD_YAML: &str = r#"
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: ownershipcheckpointtests.e2e.rustfs.com
spec:
  group: e2e.rustfs.com
  scope: Namespaced
  names:
    plural: ownershipcheckpointtests
    singular: ownershipcheckpointtest
    kind: OwnershipCheckpointTest
  versions:
    - name: v1
      served: true
      storage: true
      schema:
        openAPIV3Schema:
          type: object
          properties:
            spec:
              type: object
            status:
              type: object
              properties:
                marker:
                  type: string
                users:
                  type: array
                  items:
                    type: object
                    required:
                      - name
                      - state
                    properties:
                      name:
                        type: string
                      state:
                        type: string
      subresources:
        status: {}
"#;

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

#[tokio::test]
#[ignore = "requires a dedicated live cluster; run through `make e2e-live-run`"]
async fn operator_live_status_subresource_enforces_cas_and_pruning() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let kubectl = Kubectl::new(&config);
    kubectl
        .command([
            "delete",
            "crd",
            CHECKPOINT_TEST_CRD,
            "--ignore-not-found=true",
        ])
        .run_checked()?;
    let result = verify_status_subresource_contract(&kubectl, &config.test_namespace);
    let cleanup_result = kubectl
        .command([
            "delete",
            "crd",
            CHECKPOINT_TEST_CRD,
            "--ignore-not-found=true",
        ])
        .run_checked();

    result?;
    cleanup_result?;
    Ok(())
}

fn verify_status_subresource_contract(kubectl: &Kubectl, namespace: &str) -> Result<()> {
    kubectl
        .apply_yaml_command(CHECKPOINT_TEST_CRD_YAML)
        .run_checked()?;
    kubectl
        .command([
            "wait".to_string(),
            "--for=condition=Established".to_string(),
            format!("crd/{CHECKPOINT_TEST_CRD}"),
            "--timeout=60s".to_string(),
        ])
        .run_checked()?;

    let namespaced = kubectl.clone().namespaced(namespace);
    namespaced
        .create_yaml_command(format!(
            r#"
apiVersion: e2e.rustfs.com/v1
kind: OwnershipCheckpointTest
metadata:
  name: {CHECKPOINT_TEST_NAME}
spec: {{}}
"#
        ))
        .run_checked()?;
    let created = namespaced
        .command([
            "get",
            CHECKPOINT_TEST_RESOURCE,
            CHECKPOINT_TEST_NAME,
            "-o",
            "json",
        ])
        .run_checked()?;
    let created: Value = serde_json::from_str(&created.stdout)?;
    let initial_resource_version = created["metadata"]["resourceVersion"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("test resource did not receive a resourceVersion"))?;

    let winner_patch = json!({
        "metadata": { "resourceVersion": initial_resource_version },
        "status": {
            "marker": "winner",
            "users": [{
                "name": "app-user",
                "state": "Pending",
                "ownership": {
                    "state": "PendingCreate",
                    "tenantUid": "tenant-uid",
                },
            }],
        },
    })
    .to_string();
    let winner = namespaced
        .command([
            "patch".to_string(),
            CHECKPOINT_TEST_RESOURCE.to_string(),
            CHECKPOINT_TEST_NAME.to_string(),
            "--subresource=status".to_string(),
            "--type=merge".to_string(),
            "-p".to_string(),
            winner_patch,
            "-o".to_string(),
            "json".to_string(),
        ])
        .run_checked()?;
    let winner: Value = serde_json::from_str(&winner.stdout)?;
    ensure!(
        winner["status"]["users"][0].get("ownership").is_none(),
        "the API server preserved an ownership field omitted by the CRD schema"
    );

    let stale_patch = json!({
        "metadata": { "resourceVersion": initial_resource_version },
        "status": { "marker": "loser" },
    })
    .to_string();
    let stale = namespaced
        .command([
            "patch".to_string(),
            CHECKPOINT_TEST_RESOURCE.to_string(),
            CHECKPOINT_TEST_NAME.to_string(),
            "--subresource=status".to_string(),
            "--type=merge".to_string(),
            "-p".to_string(),
            stale_patch,
        ])
        .run()?;
    ensure!(
        stale.code != Some(0),
        "the API server accepted a status patch with a stale resourceVersion"
    );
    let stale_output = format!("{}\n{}", stale.stdout, stale.stderr).to_ascii_lowercase();
    ensure!(
        stale_output.contains("conflict") || stale_output.contains("object has been modified"),
        "the stale status patch failed without a Kubernetes conflict: {stale_output}"
    );

    let current = namespaced
        .command([
            "get",
            CHECKPOINT_TEST_RESOURCE,
            CHECKPOINT_TEST_NAME,
            "-o",
            "json",
        ])
        .run_checked()?;
    let current: Value = serde_json::from_str(&current.stdout)?;
    ensure!(
        current["status"]["marker"] == "winner",
        "the rejected stale patch changed the persisted status"
    );

    Ok(())
}
