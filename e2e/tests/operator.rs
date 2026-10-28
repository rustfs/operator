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

use anyhow::{Context, Result, ensure};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{Client, config::Region, primitives::ByteStream, types::ServerSideEncryption};
use kube::Api;
use rustfs_operator_e2e::framework::{
    assertions, config::E2eConfig, kube_client, kubectl::Kubectl, live,
    port_forward::PortForwardSpec, resources, wait,
};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

use operator::types::v1alpha1::tenant::Tenant;

const CHECKPOINT_TEST_CRD: &str = "ownershipcheckpointtests.e2e.rustfs.com";
const CHECKPOINT_TEST_RESOURCE: &str = "ownershipcheckpointtests.e2e.rustfs.com";
const CHECKPOINT_TEST_NAME: &str = "ownership-checkpoint-contract";
const SSE_S3_MASTER_KEY_ENV: &str = "RUSTFS_SSE_S3_MASTER_KEY";
const SSE_S3_MASTER_KEY_SECRET_KEY: &str = "master-key";
// base64 of the 32 ASCII bytes "0123456789abcdef0123456789abcdef".
const SSE_S3_TEST_MASTER_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const SSE_S3_PROBE_OBJECT: &str = "sse-s3-probe.bin";
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

#[tokio::test]
#[ignore = "requires a dedicated live cluster; run through `make e2e-live-run`"]
async fn operator_live_sse_s3_bucket_encrypts_new_objects() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let kubectl = Kubectl::new(&config);
    let secret_name = format!("{}-sse-s3", config.tenant_name);
    let bucket = format!("e2e-sse-s3-{}", &Uuid::new_v4().simple().to_string()[..8]);

    let result = verify_sse_s3_bucket_encryption(&config, &kubectl, &secret_name, &bucket).await;
    let restore_result = restore_smoke_tenant(&config, &kubectl, &secret_name).await;

    result?;
    restore_result?;
    Ok(())
}

async fn verify_sse_s3_bucket_encryption(
    config: &E2eConfig,
    kubectl: &Kubectl,
    secret_name: &str,
    bucket: &str,
) -> Result<()> {
    kubectl
        .apply_yaml_command(sse_s3_secret_manifest(config, secret_name))
        .run_checked()?;
    kubectl
        .apply_yaml_command(sse_s3_tenant_manifest(config, secret_name, bucket)?)
        .run_checked()?;

    let client = kube_client::default_client().await?;
    let tenants: Api<Tenant> = kube_client::tenant_api(client, &config.test_namespace);
    wait::wait_for_tenant_ready(tenants.clone(), &config.tenant_name, config.timeout).await?;
    wait_for_bucket_encryption_applied(&tenants, &config.tenant_name, bucket, config.timeout)
        .await?;

    let spec =
        PortForwardSpec::tenant_io_on_available_port(&config.test_namespace, &config.tenant_name)?;
    let mut port_forward = spec
        .start_with_temp_log(kubectl)
        .context("start RustFS tenant IO port-forward")?;
    let s3 = s3_client(&spec.local_base_url()).await;

    let payload = format!("sse-s3 probe {bucket}\n").repeat(4096).into_bytes();
    wait::wait_until(
        "S3 PutObject through the tenant IO port-forward",
        Duration::from_secs(60),
        Duration::from_secs(2),
        || {
            let s3 = s3.clone();
            let payload = payload.clone();
            async move {
                Ok(s3
                    .put_object()
                    .bucket(bucket)
                    .key(SSE_S3_PROBE_OBJECT)
                    .body(ByteStream::from(payload))
                    .send()
                    .await
                    .ok())
            }
        },
    )
    .await?;
    port_forward.ensure_running()?;

    let head = s3
        .head_object()
        .bucket(bucket)
        .key(SSE_S3_PROBE_OBJECT)
        .send()
        .await
        .context("HeadObject on the SSE-S3 probe object")?;
    ensure!(
        head.server_side_encryption() == Some(&ServerSideEncryption::Aes256),
        "object written without SSE headers is not AES256-encrypted: {:?}",
        head.server_side_encryption()
    );

    let body = s3
        .get_object()
        .bucket(bucket)
        .key(SSE_S3_PROBE_OBJECT)
        .send()
        .await
        .context("GetObject on the SSE-S3 probe object")?
        .body
        .collect()
        .await?
        .into_bytes();
    ensure!(
        body.as_ref() == payload.as_slice(),
        "decrypted object differs from the uploaded payload ({} vs {} bytes)",
        body.len(),
        payload.len()
    );

    s3.delete_object()
        .bucket(bucket)
        .key(SSE_S3_PROBE_OBJECT)
        .send()
        .await
        .context("delete the SSE-S3 probe object")?;
    s3.delete_bucket()
        .bucket(bucket)
        .send()
        .await
        .context("delete the SSE-S3 probe bucket")?;
    Ok(())
}

async fn restore_smoke_tenant(
    config: &E2eConfig,
    kubectl: &Kubectl,
    secret_name: &str,
) -> Result<()> {
    resources::apply_smoke_tenant_resources(config)?;
    let client = kube_client::default_client().await?;
    let tenants: Api<Tenant> = kube_client::tenant_api(client, &config.test_namespace);
    wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout).await?;
    kubectl
        .clone()
        .namespaced(&config.test_namespace)
        .command(["delete", "secret", secret_name, "--ignore-not-found=true"])
        .run_checked()?;
    Ok(())
}

fn sse_s3_secret_manifest(config: &E2eConfig, secret_name: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {secret_name}
  namespace: {namespace}
type: Opaque
stringData:
  {key}: {value}
"#,
        namespace = config.test_namespace,
        key = SSE_S3_MASTER_KEY_SECRET_KEY,
        value = SSE_S3_TEST_MASTER_KEY,
    )
}

fn sse_s3_tenant_manifest(config: &E2eConfig, secret_name: &str, bucket: &str) -> Result<String> {
    let mut tenant = serde_json::to_value(resources::smoke_tenant_template(config).build())?;
    let spec = tenant["spec"]
        .as_object_mut()
        .context("smoke Tenant has no spec")?;
    spec.entry("env")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("smoke Tenant spec.env is not a list")?
        .push(json!({
            "name": SSE_S3_MASTER_KEY_ENV,
            "valueFrom": {
                "secretKeyRef": {"name": secret_name, "key": SSE_S3_MASTER_KEY_SECRET_KEY}
            }
        }));
    spec.insert(
        "buckets".to_string(),
        json!([{"name": bucket, "encryption": {"mode": "SSE-S3"}}]),
    );
    Ok(serde_yaml_ng::to_string(&tenant)?)
}

async fn wait_for_bucket_encryption_applied(
    tenants: &Api<Tenant>,
    tenant_name: &str,
    bucket: &str,
    timeout: Duration,
) -> Result<()> {
    wait::wait_until(
        &format!("bucket {bucket} SSE-S3 encryption to be applied"),
        timeout,
        Duration::from_secs(5),
        || async move {
            let tenant = tenants.get(tenant_name).await?;
            let applied = tenant.status.as_ref().is_some_and(|status| {
                status.provisioning.buckets.iter().any(|item| {
                    item.item.name == bucket
                        && item.item.state == "Ready"
                        && item.encryption_desired_hash.is_some()
                        && item.encryption_desired_hash == item.encryption_last_applied_hash
                })
            });
            Ok(applied.then_some(()))
        },
    )
    .await
}

async fn s3_client(endpoint: &str) -> Client {
    let (access_key, secret_key) = resources::test_credentials();
    let credentials = Credentials::new(access_key, secret_key, None, None, "rustfs-e2e-sse-s3");
    let shared_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(credentials)
        .endpoint_url(endpoint)
        .load()
        .await;
    Client::from_conf(
        aws_sdk_s3::config::Builder::from(&shared_config)
            .force_path_style(true)
            .build(),
    )
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
