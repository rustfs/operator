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

use anyhow::{Context, Result, bail, ensure};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use k8s_openapi::api::core::v1 as corev1;
use kube::Api;
use operator::{
    console::state::AppState,
    sts::{rustfs_client::RustfsAdminClient, server::routes},
    types::v1alpha1::tenant::Tenant,
};
use rustfs_operator_e2e::framework::{
    artifacts::ArtifactCollector,
    cert_manager_tls as tls_e2e,
    config::E2eConfig,
    kube_client,
    kubectl::Kubectl,
    live,
    port_forward::{PortForwardGuard, PortForwardSpec},
    resources, wait,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tower::ServiceExt;

const VALID_WEB_IDENTITY_FORM: &str =
    "Version=2011-06-15&Action=AssumeRoleWithWebIdentity&WebIdentityToken=service-account-token";
const STS_LIVE_AUDIENCE: &str = "sts.rustfs.com";
const STS_LIVE_SERVICE_ACCOUNT: &str = "sts-e2e-workload";
const STS_LIVE_POLICY_BINDING: &str = "sts-e2e-binding";
const STS_LIVE_POLICY: &str = "sts-e2e-readonly";
const OPERATOR_STS_TLS_SECRET: &str = "sts-tls";
const PORT_FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(120);
const STS_LIVE_POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"RustfsStsE2eReadOnly","Effect":"Allow","Action":["s3:GetObject","s3:ListBucket"],"Resource":["arn:aws:s3:::*","arn:aws:s3:::*/*"]}]}"#;

#[tokio::test]
async fn sts_requires_explicit_namespace_and_tenant_route() {
    let app = routes().with_state(AppState::new("test-secret".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sts/rustfs-e2e")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(VALID_WEB_IDENTITY_FORM))
                .expect("request builds"),
        )
        .await
        .expect("handler responds");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sts_explicit_tenant_route_accepts_valid_form_shape() {
    let app = routes().with_state(AppState::new("test-secret".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sts/rustfs-e2e/e2e-tenant")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(VALID_WEB_IDENTITY_FORM))
                .expect("request builds"),
        )
        .await
        .expect("handler responds");

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = response_text(response).await;
    assert!(body.contains("<AssumeRoleWithWebIdentityResponse"));
    assert!(body.contains("<Code>NotImplemented</Code>"));
}

#[tokio::test]
async fn sts_explicit_tenant_route_returns_sts_xml_validation_errors() {
    let app = routes().with_state(AppState::new("test-secret".to_string()));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sts/rustfs-e2e/e2e-tenant")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "Version=2011-06-15&Action=AssumeRoleWithWebIdentity",
                ))
                .expect("request builds"),
        )
        .await
        .expect("handler responds");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_text(response).await;
    assert!(body.contains("<ErrorResponse"));
    assert!(body.contains("<Code>MissingParameter</Code>"));
    assert!(body.contains("WebIdentityToken"));
}

#[tokio::test]
#[ignore = "requires live Kubernetes TokenReview, PolicyBinding CRD, and RustFS STS; run through `make e2e-live-run`"]
async fn sts_live_tokenreview_policybinding_and_assume_role_succeeds() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let result = run_sts_live_flow(&config).await;

    if let Err(error) = &result {
        let collector = ArtifactCollector::new(&config.artifacts_dir);
        match collector.collect_kubernetes_snapshot(
            "sts_live_tokenreview_policybinding_and_assume_role_succeeds",
            &config,
        ) {
            Ok(report) => {
                eprintln!("collected e2e artifacts under {}", report.dir.display());
                eprintln!("{}", report.diagnosis);
            }
            Err(artifact_error) => {
                eprintln!("failed to collect e2e artifacts after {error}: {artifact_error}");
            }
        }
    }

    result
}

#[tokio::test]
#[ignore = "requires live Kubernetes TokenReview, PolicyBinding CRD, and operator STS TLS; run through `make e2e-live-run`"]
async fn sts_live_rejects_non_tls_tenant() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let result = run_sts_non_tls_rejection_flow(&config).await;

    if let Err(error) = &result {
        let collector = ArtifactCollector::new(&config.artifacts_dir);
        match collector.collect_kubernetes_snapshot("sts_live_rejects_non_tls_tenant", &config) {
            Ok(report) => {
                eprintln!("collected e2e artifacts under {}", report.dir.display());
                eprintln!("{}", report.diagnosis);
            }
            Err(artifact_error) => {
                eprintln!("failed to collect e2e artifacts after {error}: {artifact_error}");
            }
        }
    }

    result
}

async fn response_text(response: axum::response::Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    String::from_utf8(body.to_vec()).expect("response body is utf8")
}

async fn run_sts_live_flow(config: &E2eConfig) -> Result<()> {
    let config = tls_e2e::managed_certificate_case_config(config);
    let kubectl = Kubectl::new(&config);
    tls_e2e::apply_managed_certificate_case_resources(&config)?;

    let kube_client = kube_client::default_client().await?;
    tls_e2e::wait_for_tenant_tls_ready(
        kube_client.clone(),
        &config.test_namespace,
        &config.tenant_name,
        tls_e2e::positive_cert_manager_tls_timeout(&config),
    )
    .await
    .context("wait for TLS-enabled Tenant for STS live e2e")?;
    tls_e2e::wait_for_certificate_ready(
        kube_client.clone(),
        &config.test_namespace,
        &tls_e2e::managed_certificate_name(&config),
        tls_e2e::positive_cert_manager_tls_timeout(&config),
    )
    .await
    .context("wait for operator-managed Tenant certificate")?;

    let tenants = kube_client::tenant_api(kube_client.clone(), &config.test_namespace);
    let tenant = wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout)
        .await
        .context("reuse Ready TLS Tenant for STS live e2e")?;

    kubectl
        .apply_yaml_command(sts_live_service_account_manifest(&config.test_namespace))
        .run_checked()
        .context("apply STS live ServiceAccount")?;
    kubectl
        .apply_yaml_command(sts_live_policy_binding_manifest(&config.test_namespace))
        .run_checked()
        .context("apply STS live PolicyBinding")?;

    ensure_rustfs_canned_policy(&config, &tenant, &kube_client).await?;
    let token = create_sts_live_service_account_token(&config)?;

    let (sts_url, sts_client, _sts_port_forward) =
        start_operator_sts_https_port_forward(&config, &kube_client).await?;

    let response = assume_role_with_web_identity(&sts_url, &config, &token, &sts_client).await?;
    assert_sts_live_response(&config, &response)?;

    Ok(())
}

async fn run_sts_non_tls_rejection_flow(config: &E2eConfig) -> Result<()> {
    resources::apply_smoke_tenant_resources(config).context("apply non-TLS smoke Tenant")?;
    let kubectl = Kubectl::new(config);
    let kube_client = kube_client::default_client().await?;
    let tenants = kube_client::tenant_api(kube_client.clone(), &config.test_namespace);
    wait::wait_for_tenant_ready(tenants, &config.tenant_name, config.timeout)
        .await
        .context("wait for non-TLS smoke Tenant")?;

    kubectl
        .apply_yaml_command(sts_live_service_account_manifest(&config.test_namespace))
        .run_checked()
        .context("apply STS live ServiceAccount")?;
    kubectl
        .apply_yaml_command(sts_live_policy_binding_manifest(&config.test_namespace))
        .run_checked()
        .context("apply STS live PolicyBinding")?;
    let token = create_sts_live_service_account_token(config)?;

    let (sts_url, sts_client, _sts_port_forward) =
        start_operator_sts_https_port_forward(config, &kube_client).await?;
    let (status, body) =
        assume_role_with_web_identity_response(&sts_url, config, &token, &sts_client).await?;

    ensure!(
        status == StatusCode::BAD_REQUEST,
        "non-TLS Tenant STS request should fail with BAD_REQUEST, got {status}:\n{body}"
    );
    ensure!(
        body.contains("<Code>InvalidParameterValue</Code>") && body.contains("tenantTls"),
        "non-TLS Tenant STS request should identify tenantTls as invalid:\n{body}"
    );

    Ok(())
}

async fn ensure_rustfs_canned_policy(
    config: &E2eConfig,
    tenant: &Tenant,
    kube_client: &kube::Client,
) -> Result<()> {
    let rustfs_port_forward_spec =
        PortForwardSpec::tenant_io(&config.test_namespace, &config.tenant_name);
    let rustfs_host = rustfs_service_dns(&config.test_namespace, &config.tenant_name);
    let rustfs_url = local_https_base_url(&rustfs_host, &rustfs_port_forward_spec);
    let mut rustfs_port_forward =
        PortForwardSpec::start_tenant_io(config).context("start RustFS tenant IO port-forward")?;
    let tenant_ca = RustfsAdminClient::load_tenant_tls_ca(kube_client, tenant)
        .await
        .context("load TLS Tenant CA")?
        .context("TLS Tenant should publish a CA Secret reference")?;
    let rustfs_probe_client = tls_client(
        &tenant_ca,
        &rustfs_host,
        rustfs_port_forward_spec.local_port,
    )?;
    wait_for_port_forward(&mut rustfs_port_forward, &rustfs_url, &rustfs_probe_client).await?;

    let credentials = RustfsAdminClient::load_tenant_credentials(kube_client, tenant)
        .await
        .context("load RustFS tenant credentials")?;
    let rustfs_admin = RustfsAdminClient::new_with_base_url_and_http_client(
        rustfs_url,
        credentials.access_key,
        credentials.secret_key,
        rustfs_probe_client,
    );

    rustfs_admin
        .add_canned_policy(STS_LIVE_POLICY, STS_LIVE_POLICY_DOCUMENT)
        .await
        .context("add RustFS canned policy for STS live e2e")?;

    Ok(())
}

fn create_sts_live_service_account_token(config: &E2eConfig) -> Result<String> {
    let output = Kubectl::new(config)
        .namespaced(&config.test_namespace)
        .command(vec![
            "create".to_string(),
            "token".to_string(),
            STS_LIVE_SERVICE_ACCOUNT.to_string(),
            format!("--audience={STS_LIVE_AUDIENCE}"),
            "--duration=10m".to_string(),
        ])
        .run_checked()
        .context("create projected ServiceAccount token for STS live e2e")?;
    let token = output.stdout.trim().to_string();

    ensure!(
        !token.is_empty(),
        "kubectl create token returned an empty token"
    );
    Ok(token)
}

async fn assume_role_with_web_identity(
    sts_url: &str,
    config: &E2eConfig,
    token: &str,
    client: &reqwest::Client,
) -> Result<String> {
    let (status, body) =
        assume_role_with_web_identity_response(sts_url, config, token, client).await?;

    ensure!(
        status.is_success(),
        "operator STS returned {status}:\n{body}"
    );

    Ok(body)
}

async fn assume_role_with_web_identity_response(
    sts_url: &str,
    config: &E2eConfig,
    token: &str,
    client: &reqwest::Client,
) -> Result<(StatusCode, String)> {
    let response = client
        .post(format!(
            "{}/sts/{}/{}",
            sts_url.trim_end_matches('/'),
            config.test_namespace,
            config.tenant_name
        ))
        .form(&[
            ("Version", "2011-06-15"),
            ("Action", "AssumeRoleWithWebIdentity"),
            ("WebIdentityToken", token),
            ("DurationSeconds", "900"),
        ])
        .send()
        .await
        .context("send AssumeRoleWithWebIdentity request to operator STS")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("read operator STS response body")?;

    Ok((status, body))
}

fn assert_sts_live_response(config: &E2eConfig, body: &str) -> Result<()> {
    let subject = format!(
        "<SubjectFromWebIdentityToken>system:serviceaccount:{}:{}</SubjectFromWebIdentityToken>",
        config.test_namespace, STS_LIVE_SERVICE_ACCOUNT
    );
    let audience = format!("<Audience>{STS_LIVE_AUDIENCE}</Audience>");
    let role_arn = format!(
        "arn:rustfs:sts::{}:assumed-role/{}/{}:{}",
        config.test_namespace, config.tenant_name, config.test_namespace, STS_LIVE_SERVICE_ACCOUNT
    );

    ensure!(
        body.contains("<AssumeRoleWithWebIdentityResponse"),
        "missing AssumeRoleWithWebIdentityResponse in operator STS success response"
    );
    ensure!(
        body.contains(&subject),
        "missing TokenReview subject in operator STS success response"
    );
    ensure!(
        body.contains(&audience),
        "missing TokenReview audience in operator STS success response"
    );
    ensure!(
        body.contains(&role_arn),
        "missing assumed role ARN in operator STS success response"
    );
    ensure!(
        body.contains("<AccessKeyId>"),
        "missing AccessKeyId in operator STS success response"
    );
    ensure!(
        body.contains("<SecretAccessKey>"),
        "missing SecretAccessKey in operator STS success response"
    );
    ensure!(
        body.contains("<SessionToken>"),
        "missing SessionToken in operator STS success response"
    );
    ensure!(
        body.contains("<Provider>kubernetes</Provider>"),
        "missing provider in operator STS success response"
    );

    Ok(())
}

async fn start_operator_sts_https_port_forward(
    config: &E2eConfig,
    kube_client: &kube::Client,
) -> Result<(String, reqwest::Client, PortForwardGuard)> {
    let sts_port_forward_spec = PortForwardSpec::operator_sts(&config.operator_namespace);
    let sts_host = operator_sts_service_dns(&config.operator_namespace);
    let sts_url = local_https_base_url(&sts_host, &sts_port_forward_spec);
    let mut sts_port_forward =
        PortForwardSpec::start_operator_sts(config).context("start operator STS port-forward")?;
    let sts_ca = load_secret_key(
        kube_client,
        &config.operator_namespace,
        OPERATOR_STS_TLS_SECRET,
        "ca.crt",
    )
    .await
    .context("load operator STS CA")?;
    let sts_client = tls_client(&sts_ca, &sts_host, sts_port_forward_spec.local_port)?;
    wait_for_port_forward(&mut sts_port_forward, &sts_url, &sts_client).await?;

    Ok((sts_url, sts_client, sts_port_forward))
}

async fn wait_for_port_forward(
    port_forward: &mut PortForwardGuard,
    base_url: &str,
    client: &reqwest::Client,
) -> Result<()> {
    let deadline = Instant::now() + PORT_FORWARD_READY_TIMEOUT;

    loop {
        port_forward.ensure_running()?;
        let last_error = match client.get(base_url).send().await {
            Ok(_) => return Ok(()),
            Err(error) => error.to_string(),
        };

        if Instant::now() >= deadline {
            bail!(
                "port-forward not ready after {:?}; last error: {}; command: {}; log {}:\n{}",
                PORT_FORWARD_READY_TIMEOUT,
                last_error,
                port_forward.command_display(),
                port_forward.log_path().display(),
                port_forward.log_contents()
            );
        }

        sleep(Duration::from_secs(1)).await;
    }
}

fn local_https_base_url(host: &str, spec: &PortForwardSpec) -> String {
    format!("https://{host}:{}", spec.local_port)
}

fn rustfs_service_dns(namespace: &str, tenant_name: &str) -> String {
    format!("{tenant_name}-io.{namespace}.svc")
}

fn operator_sts_service_dns(namespace: &str) -> String {
    format!("rustfs-operator-sts.{namespace}.svc")
}

fn tls_client(ca_pem: &[u8], host: &str, local_port: u16) -> Result<reqwest::Client> {
    operator::install_rustls_crypto_provider();

    let certs = reqwest::Certificate::from_pem_bundle(ca_pem)
        .context("parse CA PEM bundle for TLS client")?;
    let local_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, local_port));
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .resolve(host, local_addr);
    for cert in certs {
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder.build()?)
}

async fn load_secret_key(
    kube_client: &kube::Client,
    namespace: &str,
    secret_name: &str,
    key: &str,
) -> Result<Vec<u8>> {
    let api: Api<corev1::Secret> = Api::namespaced(kube_client.clone(), namespace);
    let secret = api
        .get(secret_name)
        .await
        .with_context(|| format!("load Secret {namespace}/{secret_name}"))?;

    secret
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .map(|bytes| bytes.0.clone())
        .filter(|bytes| !bytes.is_empty())
        .with_context(|| format!("Secret {namespace}/{secret_name} missing non-empty key {key}"))
}

fn sts_live_service_account_manifest(namespace: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: ServiceAccount
metadata:
  name: {STS_LIVE_SERVICE_ACCOUNT}
  namespace: {namespace}
"#
    )
}

fn sts_live_policy_binding_manifest(namespace: &str) -> String {
    format!(
        r#"apiVersion: sts.rustfs.com/v1alpha1
kind: PolicyBinding
metadata:
  name: {STS_LIVE_POLICY_BINDING}
  namespace: {namespace}
spec:
  application:
    namespace: {namespace}
    serviceaccount: {STS_LIVE_SERVICE_ACCOUNT}
  policies:
    - {STS_LIVE_POLICY}
"#
    )
}
