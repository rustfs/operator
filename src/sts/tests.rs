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

//! Unit/integration tests for RustfsAdminClient split operation modules.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::{get, post, put},
};
use k8s_openapi::{ByteString, api::core::v1 as corev1};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

use super::{
    ADD_USER_PATH, ADMIN_SIGNING_SERVICE, CreateBucketResult, FORM_CONTENT_TYPE, JSON_CONTENT_TYPE,
    LIST_CANNED_POLICIES_PATH, MAX_UPSTREAM_ERROR_BODY_BYTES, POOLS_DECOMMISSION_PATH,
    POOLS_LIST_PATH, POOLS_STATUS_PATH, RustfsAdminClient, RustfsClientError, SERVER_INFO_PATH,
    SET_POLICY_PATH, STS_SIGNING_SERVICE, USER_INFO_PATH,
    helpers::{
        build_canonical_query, build_form_body, derive_signing_key, extract_canned_policy_document,
        extract_credentials, hmac_sha256_hex, parse_assume_role_response, sha256_hex,
    },
    tls_tenant_base_url,
};

const TEST_ACCESS_KEY: &str = "access";
const TEST_SECRET_KEY: &str = "secret";
const TEST_REGION: &str = "us-east-1";

#[test]
fn canonical_query_uses_sigv4_uri_encoding_and_encoded_sort_order() {
    let query = build_canonical_query(&[
        ("z", "a b~c/雪"),
        ("a~", "second"),
        ("a ", "first"),
        ("amp", "&="),
        ("dup", "z"),
        ("dup", "a"),
        ("empty", ""),
        ("雪", "key"),
    ]);

    assert_eq!(
        query,
        "%E9%9B%AA=key&a%20=first&amp=%26%3D&a~=second&dup=a&dup=z&empty=&z=a%20b~c%2F%E9%9B%AA"
    );
}

#[test]
fn form_body_keeps_html_form_encoding() {
    assert_eq!(
        build_form_body(&[("Policy", "a b~c/雪")]),
        "Policy=a+b%7Ec%2F%E9%9B%AA"
    );
}

#[test]
fn duplicate_query_values_match_independent_sigv4_verification() {
    let query =
        build_canonical_query(&[("dup", "z z"), ("dup", "a+a"), ("dup", "雪"), ("empty", "")]);
    assert_eq!(query, "dup=%E9%9B%AA&dup=a%2Ba&dup=z%20z&empty=");

    let client = RustfsAdminClient::new_with_base_url(
        "https://rustfs.example.test:9000",
        TEST_ACCESS_KEY,
        TEST_SECRET_KEY,
    );
    let signed = client
        .sign_request("GET", "/synthetic", &query, "", None, ADMIN_SIGNING_SERVICE)
        .unwrap();
    let request = CapturedRequest {
        method: "GET".to_string(),
        path: "/synthetic".to_string(),
        query,
        body: String::new(),
        host: "rustfs.example.test:9000".to_string(),
        content_type: String::new(),
        amz_date: signed.amz_date,
        payload_hash: signed.payload_hash,
        authorization: signed.authorization,
    };

    assert_sigv4_matches_wire(&request, ADMIN_SIGNING_SERVICE);
}

fn secret_with_fields(fields: Vec<(&str, &[u8])>) -> corev1::Secret {
    let mut data = BTreeMap::new();
    for (key, value) in fields {
        data.insert(key.to_string(), ByteString(value.to_vec()));
    }

    corev1::Secret {
        data: Some(data),
        ..Default::default()
    }
}

fn assert_oversized_upstream_body_hidden(err: RustfsClientError) {
    assert_eq!(
        err.to_string(),
        format!(
            "upstream returned 502 Bad Gateway: response body exceeded {MAX_UPSTREAM_ERROR_BODY_BYTES} bytes"
        )
    );
}

#[test]
fn tls_tenant_base_url_uses_custom_cluster_domain() {
    let mut tenant = crate::tests::create_test_tenant(None, None);
    tenant.metadata.name = Some("prod-rustfs".to_string());
    tenant.metadata.namespace = Some("mse".to_string());

    assert_eq!(
        tls_tenant_base_url(&tenant, "k8s.mse.cloud").unwrap(),
        "https://prod-rustfs-hl.mse.svc.k8s.mse.cloud:9000"
    );
}

#[test]
fn extract_credentials_reports_missing_access_key() {
    let secret = secret_with_fields(vec![("secretkey", b"sekret")]);

    let err = extract_credentials(secret.data.as_ref()).expect_err("expected missing access key");
    assert!(matches!(
        err,
        RustfsClientError::MissingCredentialKey { key: "accesskey" }
    ));
}

#[test]
fn extract_credentials_reports_non_utf8_access_key() {
    let secret = secret_with_fields(vec![("accesskey", &[0xff, 0xfe]), ("secretkey", b"sekret")]);

    let err = extract_credentials(secret.data.as_ref()).expect_err("expected invalid utf8");
    assert!(matches!(
        err,
        RustfsClientError::InvalidCredentialValue { key: "accesskey" }
    ));
}

#[test]
fn extract_credentials_reports_missing_secret_key() {
    let secret = secret_with_fields(vec![("accesskey", b"access")]);

    let err = extract_credentials(secret.data.as_ref()).expect_err("expected missing secret key");
    assert!(matches!(
        err,
        RustfsClientError::MissingCredentialKey { key: "secretkey" }
    ));
}

#[test]
fn extract_credentials_reports_non_utf8_secret_key() {
    let secret = secret_with_fields(vec![("accesskey", b"access"), ("secretkey", &[0xff, 0xfe])]);

    let err = extract_credentials(secret.data.as_ref()).expect_err("expected invalid utf8");
    assert!(matches!(
        err,
        RustfsClientError::InvalidCredentialValue { key: "secretkey" }
    ));
}

#[test]
fn extract_credentials_reports_empty_secret_key() {
    let secret = secret_with_fields(vec![("accesskey", b"abc"), ("secretkey", b"")]);

    let err = extract_credentials(secret.data.as_ref()).expect_err("expected empty secret key");
    assert!(matches!(
        err,
        RustfsClientError::EmptyCredentialValue { key: "secretkey" }
    ));
}

#[test]
fn parse_assume_role_xml_success_and_failure() {
    let body_ok = "<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleResult><Credentials><AccessKeyId>AKI</AccessKeyId><SecretAccessKey>SEC</SecretAccessKey><SessionToken>TOKEN</SessionToken><Expiration>2026-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
    let parsed =
        parse_assume_role_response(body_ok).expect("valid assume role response should parse");

    assert_eq!(parsed.access_key_id, "AKI");
    assert_eq!(parsed.secret_access_key, "SEC");
    assert_eq!(parsed.session_token, "TOKEN");
    assert_eq!(parsed.expiration, "2026-01-01T00:00:00Z");

    assert!(parse_assume_role_response("<NotFound />").is_none());
}

#[test]
fn unexpected_status_includes_upstream_xml_error_summary() {
    let err = RustfsClientError::unexpected_status_with_body(
        StatusCode::BAD_REQUEST,
        r#"<Error><Code>InvalidRequest</Code><Message>invalid resource: unknown &quot;*&quot;</Message><RequestId>abc</RequestId></Error>"#,
    );

    let message = err.to_string();
    assert_eq!(
        message,
        r#"upstream returned 400 Bad Request: InvalidRequest: invalid resource: unknown "*""#
    );
    assert!(!message.contains("<Error>"));
}

#[test]
fn unexpected_status_includes_upstream_json_error_summary() {
    let err = RustfsClientError::unexpected_status_with_body(
        StatusCode::BAD_REQUEST,
        r#"{"code":"InvalidRequest","message":"policy Resource must use ARN form"}"#,
    );

    assert_eq!(
        err.to_string(),
        "upstream returned 400 Bad Request: InvalidRequest: policy Resource must use ARN form"
    );
}

#[test]
fn unexpected_status_redacts_sensitive_upstream_error_summary() {
    let err = RustfsClientError::unexpected_status_with_body(
        StatusCode::BAD_REQUEST,
        r#"{"code":"InvalidRequest","message":"secretkey: SK_TEST clientSecret: oidc-secret SecretAccessKey: SK_STS AccessKeyId: AKIA_STS <SecretAccessKey>SK_XML</SecretAccessKey> <AccessKeyId>AKIA_XML</AccessKeyId>"}"#,
    );

    let message = err.to_string();
    assert!(message.contains("secretkey: <redacted>"));
    assert!(message.contains("clientSecret: <redacted>"));
    assert!(message.contains("SecretAccessKey: <redacted>"));
    assert!(message.contains("AccessKeyId: <redacted>"));
    assert!(message.contains("<SecretAccessKey><redacted></SecretAccessKey>"));
    assert!(message.contains("<AccessKeyId><redacted></AccessKeyId>"));
    assert!(!message.contains("SK_TEST"));
    assert!(!message.contains("oidc-secret"));
    assert!(!message.contains("SK_STS"));
    assert!(!message.contains("AKIA_STS"));
    assert!(!message.contains("SK_XML"));
    assert!(!message.contains("AKIA_XML"));
}

#[test]
fn unexpected_status_hides_truncated_unstructured_response_body() {
    let retained_body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES);
    let err = RustfsClientError::unexpected_status_with_limited_body(
        StatusCode::BAD_GATEWAY,
        &retained_body,
        true,
    );

    assert_eq!(
        err.to_string(),
        format!(
            "upstream returned 502 Bad Gateway: response body exceeded {MAX_UPSTREAM_ERROR_BODY_BYTES} bytes"
        )
    );
}

#[tokio::test]
async fn unexpected_response_preserves_exact_limit_unstructured_response_body() {
    let body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES);
    let router = Router::new().route(
        ADD_USER_PATH,
        put(move || {
            let body = body.clone();
            async move { (StatusCode::BAD_GATEWAY, body) }
        }),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let err = client
        .add_user("app-user", "secret123")
        .await
        .expect_err("exact limit body should still report the retained body");

    let message = err.to_string();
    assert!(message.contains("upstream returned 502 Bad Gateway"));
    assert!(!message.contains("response body exceeded"));

    server.abort();
}

#[tokio::test]
async fn unexpected_response_hides_over_limit_unstructured_response_body() {
    let body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES + 1);
    let router = Router::new().route(
        ADD_USER_PATH,
        put(move || {
            let body = body.clone();
            async move { (StatusCode::BAD_GATEWAY, body) }
        }),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let err = client
        .add_user("app-user", "secret123")
        .await
        .expect_err("oversized body should be hidden");

    assert_oversized_upstream_body_hidden(err);

    server.abort();
}

#[derive(Clone, Default)]
struct Capture {
    method: Arc<Mutex<String>>,
    path: Arc<Mutex<String>>,
    query: Arc<Mutex<String>>,
    body: Arc<Mutex<String>>,
    host: Arc<Mutex<String>>,
    content_type: Arc<Mutex<String>>,
    amz_date: Arc<Mutex<String>>,
    payload_hash: Arc<Mutex<String>>,
    authorization: Arc<Mutex<String>>,
    object_lock_header: Arc<Mutex<String>>,
}

#[derive(Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    query: String,
    body: String,
    host: String,
    content_type: String,
    amz_date: String,
    payload_hash: String,
    authorization: String,
}

impl Capture {
    async fn request(&self) -> CapturedRequest {
        CapturedRequest {
            method: self.method.lock().await.clone(),
            path: self.path.lock().await.clone(),
            query: self.query.lock().await.clone(),
            body: self.body.lock().await.clone(),
            host: self.host.lock().await.clone(),
            content_type: self.content_type.lock().await.clone(),
            amz_date: self.amz_date.lock().await.clone(),
            payload_hash: self.payload_hash.lock().await.clone(),
            authorization: self.authorization.lock().await.clone(),
        }
    }
}

fn request_header(req: &Request<Body>, name: &str) -> String {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn capture_signed_request(capture: &Capture, req: Request<Body>) {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let host = request_header(&req, "host");
    let content_type = request_header(&req, "content-type");
    let amz_date = request_header(&req, "x-amz-date");
    let payload_hash = request_header(&req, "x-amz-content-sha256");
    let authorization = request_header(&req, "authorization");
    let body = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();

    *capture.method.lock().await = method;
    *capture.path.lock().await = path;
    *capture.query.lock().await = query;
    *capture.body.lock().await = body;
    *capture.host.lock().await = host;
    *capture.content_type.lock().await = content_type;
    *capture.amz_date.lock().await = amz_date;
    *capture.payload_hash.lock().await = payload_hash;
    *capture.authorization.lock().await = authorization;
}

fn assert_sigv4_matches_wire(request: &CapturedRequest, service: &str) {
    let calculated_payload_hash = sha256_hex(request.body.as_bytes());
    assert_eq!(request.payload_hash, calculated_payload_hash);

    let signed_header_names = if request.content_type.is_empty() {
        "host;x-amz-content-sha256;x-amz-date"
    } else {
        "content-type;host;x-amz-content-sha256;x-amz-date"
    };
    let mut canonical_headers = String::new();
    if !request.content_type.is_empty() {
        canonical_headers.push_str("content-type:");
        canonical_headers.push_str(request.content_type.trim());
        canonical_headers.push('\n');
    }
    canonical_headers.push_str("host:");
    canonical_headers.push_str(request.host.trim());
    canonical_headers.push_str("\nx-amz-content-sha256:");
    canonical_headers.push_str(request.payload_hash.trim());
    canonical_headers.push_str("\nx-amz-date:");
    canonical_headers.push_str(request.amz_date.trim());
    canonical_headers.push('\n');

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        request.method,
        request.path,
        request.query,
        canonical_headers,
        signed_header_names,
        request.payload_hash
    );
    let date_stamp = request
        .amz_date
        .get(..8)
        .expect("x-amz-date must start with YYYYMMDD");
    let credential_scope = format!("{date_stamp}/{TEST_REGION}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        request.amz_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes())
    );
    let signing_key =
        derive_signing_key(TEST_SECRET_KEY, date_stamp, TEST_REGION, service).unwrap();
    let signature = hmac_sha256_hex(&signing_key, &string_to_sign).unwrap();
    let expected_authorization = format!(
        "AWS4-HMAC-SHA256 Credential={TEST_ACCESS_KEY}/{credential_scope}, SignedHeaders={signed_header_names}, Signature={signature}"
    );

    assert_eq!(request.authorization, expected_authorization);
}

#[tokio::test]
async fn assume_role_request_targets_root_path_and_action_is_assume_role() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new().route(
            "/",
            post(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    capture_signed_request(&c, req).await;

                    let response =
                        "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>AKI</AccessKeyId><SecretAccessKey>SEC</SecretAccessKey><SessionToken>TOKEN</SessionToken><Expiration>2026-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
                    (StatusCode::OK, response)
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    let creds = client
        .assume_role(Some(r#"{"Statement":[{"Resource":"a b~+/雪"}]}"#), 3600)
        .await
        .unwrap();
    assert_eq!(creds.access_key_id, "AKI");

    let request = capture.request().await;
    assert_eq!(request.path, "/");
    assert_eq!(
        request.body,
        "Action=AssumeRole&DurationSeconds=3600&Policy=%7B%22Statement%22%3A%5B%7B%22Resource%22%3A%22a+b%7E%2B%2F%E9%9B%AA%22%7D%5D%7D&Version=2011-06-15"
    );
    assert!(request.query.is_empty());
    assert_eq!(request.content_type, FORM_CONTENT_TYPE);
    assert_sigv4_matches_wire(&request, STS_SIGNING_SERVICE);

    server.abort();
}

#[tokio::test]
async fn info_canned_policy_uses_expected_path_and_query() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
            .route(
                "/rustfs/admin/v3/info-canned-policy",
                get(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        let path = req.uri().path().to_string();
                        let query = req.uri().query().unwrap_or("").to_string();
                        let authorization = req
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("")
                            .to_string();

                        *c.path.lock().await = path;
                        *c.query.lock().await = query;
                        *c.authorization.lock().await = authorization;

                        (
                            StatusCode::OK,
                            "{\"policy_name\":\"tenant-policy\",\"policy\":{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"allow\",\"Effect\":\"Allow\"}]}}",
                        )
                    },
                ),
            )
            .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    let policy = client.get_canned_policy("tenant-policy").await.unwrap();
    let policy_value = serde_json::from_str::<Value>(&policy).unwrap();
    assert_eq!(policy_value["Version"], "2012-10-17");
    assert_eq!(policy_value["Statement"][0]["Sid"], "allow");

    assert_eq!(
        &*capture.path.lock().await,
        "/rustfs/admin/v3/info-canned-policy"
    );
    assert!(capture.query.lock().await.contains("name=tenant-policy"));
    assert!(
        capture
            .authorization
            .lock()
            .await
            .contains("/s3/aws4_request")
    );

    server.abort();
}

#[tokio::test]
async fn list_canned_policies_extracts_policy_document_and_canonicalizes_json() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            LIST_CANNED_POLICIES_PATH,
            get(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    let path = req.uri().path().to_string();
                    let query = req.uri().query().unwrap_or("").to_string();

                    *c.path.lock().await = path;
                    *c.query.lock().await = query;

                    (
                        StatusCode::OK,
                        serde_json::json!({
                            "tenant-policy": {
                                "policy_name":"tenant-policy",
                                "policy":{
                                    "Statement": [{
                                        "Resource": "arn:aws:s3:::tenant",
                                        "Effect": "Allow",
                                        "Action": "s3:GetObject"
                                    }],
                                    "Version":"2012-10-17"
                                }
                            },
                            "inline-policy": {
                                "Version": "2012-10-17",
                                "Statement": [{
                                    "Sid": "inline",
                                    "Action": "s3:ListBucket",
                                    "Effect": "Allow",
                                    "Resource": ["arn:aws:s3:::tenant*"]
                                }]
                            }
                        })
                        .to_string(),
                    )
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let policies = client.list_canned_policies().await.unwrap();

    let tenant_policy = serde_json::from_str::<Value>(&policies["tenant-policy"]).unwrap();
    assert_eq!(tenant_policy["Version"], "2012-10-17");
    assert_eq!(tenant_policy["Statement"][0]["Action"], "s3:GetObject");

    let inline_policy = serde_json::from_str::<Value>(&policies["inline-policy"]).unwrap();
    assert_eq!(inline_policy["Version"], "2012-10-17");
    assert_eq!(inline_policy["Statement"][0]["Sid"], "inline");
    assert_eq!(&*capture.path.lock().await, LIST_CANNED_POLICIES_PATH);
    assert!(capture.query.lock().await.is_empty());

    server.abort();
}

#[tokio::test]
async fn add_canned_policy_uses_expected_path_query_body_and_admin_signing() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            "/rustfs/admin/v3/add-canned-policy",
            put(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    let path = req.uri().path().to_string();
                    let query = req.uri().query().unwrap_or("").to_string();
                    let authorization = req
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body = String::from_utf8(body_bytes.to_vec()).unwrap();

                    *c.path.lock().await = path;
                    *c.query.lock().await = query;
                    *c.authorization.lock().await = authorization;
                    *c.body.lock().await = body;

                    StatusCode::OK
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;

    client
        .add_canned_policy("tenant-policy", policy)
        .await
        .unwrap();

    assert_eq!(
        &*capture.path.lock().await,
        "/rustfs/admin/v3/add-canned-policy"
    );
    assert!(capture.query.lock().await.contains("name=tenant-policy"));
    assert_eq!(&*capture.body.lock().await, policy);
    assert!(
        capture
            .authorization
            .lock()
            .await
            .contains("/s3/aws4_request")
    );

    server.abort();
}

#[tokio::test]
async fn add_canned_policy_reports_upstream_policy_parse_error() {
    let router = Router::new().route(
        "/rustfs/admin/v3/add-canned-policy",
        put(|| async {
            (
                StatusCode::BAD_REQUEST,
                r#"<Error><Code>InvalidRequest</Code><Message>invalid resource: unknown &quot;*&quot;</Message></Error>"#,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let policy = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#;
    let err = client
        .add_canned_policy("tenant-policy", policy)
        .await
        .expect_err("invalid RustFS policy should include upstream parse details");

    let message = err.to_string();
    assert!(message.contains("upstream returned 400 Bad Request"));
    assert!(message.contains(r#"InvalidRequest: invalid resource: unknown "*""#));
    assert!(!message.contains("<Error>"));

    server.abort();
}

#[tokio::test]
async fn server_info_uses_expected_path_and_parses_wrapped_health_fields() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            SERVER_INFO_PATH,
            get(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    let path = req.uri().path().to_string();
                    let authorization = req
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();

                    *c.path.lock().await = path;
                    *c.authorization.lock().await = authorization;

                    (
                        StatusCode::OK,
                        serde_json::json!({
                            "info": {
                                "usage": {"size": 42},
                                "backend": {
                                    "onlineDisks": 3,
                                    "offlineDisks": 1,
                                    "standardSCParity": 2,
                                    "totalSets": [1],
                                    "totalDrivesPerSet": [4]
                                },
                                "pools": {
                                    "0": {
                                        "0": {
                                            "rawUsage": 100,
                                            "rawCapacity": 400,
                                            "usage": 50,
                                            "objectsCount": 2,
                                            "healDisks": 1
                                        }
                                    }
                                }
                            },
                            "admin_discovery": {
                                "runtimeCapabilities": "/rustfs/admin/v4/runtime/capabilities",
                                "clusterSnapshot": "/rustfs/admin/v4/cluster/snapshot",
                                "extensionsCatalog": "/rustfs/admin/v4/extensions/catalog"
                            },
                        })
                        .to_string(),
                    )
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let info = client.server_info().await.unwrap();

    let backend = info.backend.unwrap();
    assert_eq!(backend.online_disks, 3);
    assert_eq!(backend.offline_disks, 1);
    assert_eq!(backend.standard_sc_parity, Some(2));
    assert_eq!(info.usage.unwrap().size, 42);
    assert_eq!(info.pools.unwrap()["0"]["0"].raw_capacity, 400);
    assert_eq!(&*capture.path.lock().await, SERVER_INFO_PATH);
    assert!(
        capture
            .authorization
            .lock()
            .await
            .contains("/s3/aws4_request")
    );

    server.abort();
}

#[tokio::test]
async fn list_pools_parses_current_rustfs_pool_shape() {
    let router = Router::new().route(
            POOLS_LIST_PATH,
            get(|| async {
                (
                    StatusCode::OK,
                    r#"[{"id":1,"cmdline":"http://tenant-pool-a-{0...3}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...3}","lastUpdate":"2026-05-20T00:00:00Z","totalSize":100,"currentSize":50,"usedSize":25,"used":25.0,"status":"running","decommissionInfo":{"startTime":"2026-05-20T00:00:00Z","complete":false,"failed":false,"canceled":false,"objectsDecommissioned":7,"objectsDecommissionedFailed":1,"bytesDecommissioned":9,"bytesDecommissionedFailed":2}}]"#,
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    let pools = client.list_pools().await.unwrap();

    assert_eq!(pools[0].id, 1);
    assert_eq!(pools[0].status, "running");
    assert_eq!(
        pools[0]
            .decommission
            .as_ref()
            .and_then(|info| info.objects_decommissioned),
        Some(7)
    );

    server.abort();
}

#[tokio::test]
async fn pool_decommission_start_uses_by_id_query_and_admin_signing() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            POOLS_DECOMMISSION_PATH,
            post(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    *c.path.lock().await = req.uri().path().to_string();
                    *c.query.lock().await = req.uri().query().unwrap_or("").to_string();
                    *c.authorization.lock().await = req
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();

                    StatusCode::OK
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    client.start_pool_decommission_by_id("1").await.unwrap();

    assert_eq!(&*capture.path.lock().await, POOLS_DECOMMISSION_PATH);
    assert_eq!(&*capture.query.lock().await, "by-id=true&pool=1");
    assert!(
        capture
            .authorization
            .lock()
            .await
            .contains("/s3/aws4_request")
    );

    server.abort();
}

#[tokio::test]
async fn pool_status_uses_by_id_query_and_parses_decommission_info() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
            .route(
                POOLS_STATUS_PATH,
                get(
                    move |State(c): State<Capture>, req: Request<Body>| async move {
                        *c.path.lock().await = req.uri().path().to_string();
                        *c.query.lock().await = req.uri().query().unwrap_or("").to_string();

                        (
                            StatusCode::OK,
                            r#"{"id":1,"cmdline":"http://tenant-pool-a-{0...3}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...3}","lastUpdate":"2026-05-20T00:00:00Z","decommissionInfo":{"startTime":"2026-05-20T00:00:00Z","complete":true,"failed":false,"canceled":false,"objectsDecommissioned":10,"objectsDecommissionedFailed":0,"bytesDecommissioned":20,"bytesDecommissionedFailed":0}}"#,
                        )
                    },
                ),
            )
            .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    let status = client.pool_status_by_id("1").await.unwrap();

    assert_eq!(status.id, 1);
    assert_eq!(&*capture.path.lock().await, POOLS_STATUS_PATH);
    assert_eq!(&*capture.query.lock().await, "by-id=true&pool=1");
    assert_eq!(
        status.decommission.and_then(|info| info.complete),
        Some(true)
    );

    server.abort();
}

#[tokio::test]
async fn add_user_uses_expected_path_query_and_body() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            ADD_USER_PATH,
            put(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    capture_signed_request(&c, req).await;
                    StatusCode::OK
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    client.add_user("app user~+/雪", "secret123").await.unwrap();

    let request = capture.request().await;
    assert_eq!(request.path, ADD_USER_PATH);
    assert_eq!(request.query, "accessKey=app%20user~%2B%2F%E9%9B%AA");
    assert_eq!(
        request.body,
        r#"{"secretKey":"secret123","status":"enabled"}"#
    );
    assert_eq!(request.content_type, JSON_CONTENT_TYPE);
    assert_sigv4_matches_wire(&request, ADMIN_SIGNING_SERVICE);

    server.abort();
}

#[tokio::test]
async fn user_exists_limits_unexpected_error_response_body() {
    let body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES + 1);
    let router = Router::new().route(
        USER_INFO_PATH,
        get(move || {
            let body = body.clone();
            async move { (StatusCode::BAD_GATEWAY, body) }
        }),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let err = client
        .user_exists("app-user")
        .await
        .expect_err("unexpected user lookup error should hide oversized body");

    assert_oversized_upstream_body_hidden(err);

    server.abort();
}

#[tokio::test]
async fn set_user_policy_uses_single_authoritative_mapping_call() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            SET_POLICY_PATH,
            put(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    *c.path.lock().await = req.uri().path().to_string();
                    *c.query.lock().await = req.uri().query().unwrap_or("").to_string();
                    StatusCode::OK
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    client
        .set_user_policy(
            "app-user",
            &["app-readwrite".to_string(), "diagnostics".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(&*capture.path.lock().await, SET_POLICY_PATH);
    assert_eq!(
        &*capture.query.lock().await,
        "isGroup=false&policyName=app-readwrite%2Cdiagnostics&userOrGroup=app-user"
    );

    server.abort();
}

#[tokio::test]
async fn set_user_policy_rejects_empty_policy_list() {
    let client = RustfsAdminClient::new_with_base_url("http://127.0.0.1:1", "access", "secret");

    let err = client
        .set_user_policy("app-user", &[])
        .await
        .expect_err("empty policy list should be rejected before request");

    assert!(matches!(err, RustfsClientError::InvalidPolicyName));
}

#[tokio::test]
async fn bucket_object_lock_enabled_parses_enabled_response() {
    let router = Router::new().route(
            "/app-data",
            get(|req: Request<Body>| async move {
                assert_eq!(req.uri().query().unwrap_or(""), "object-lock=");
                (
                    StatusCode::OK,
                    "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled></ObjectLockConfiguration>",
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");

    assert!(client.bucket_object_lock_enabled("app-data").await.unwrap());

    server.abort();
}

#[tokio::test]
async fn bucket_object_lock_enabled_limits_unexpected_error_response_body() {
    let body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES + 1);
    let router = Router::new().route(
        "/app-data",
        get(move |req: Request<Body>| {
            let body = body.clone();
            async move {
                assert_eq!(req.uri().query().unwrap_or(""), "object-lock=");
                (StatusCode::BAD_GATEWAY, body)
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let err = client
        .bucket_object_lock_enabled("app-data")
        .await
        .expect_err("unexpected object-lock error should hide oversized body");

    assert_oversized_upstream_body_hidden(err);

    server.abort();
}

#[tokio::test]
async fn create_bucket_sends_object_lock_header_and_region_body() {
    let capture = Capture::default();
    let route_capture = capture.clone();

    let router = Router::new()
        .route(
            "/app-data",
            put(
                move |State(c): State<Capture>, req: Request<Body>| async move {
                    *c.path.lock().await = req.uri().path().to_string();
                    *c.object_lock_header.lock().await = req
                        .headers()
                        .get("x-amz-bucket-object-lock-enabled")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    *c.body.lock().await = String::from_utf8(body_bytes.to_vec()).unwrap();
                    StatusCode::OK
                },
            ),
        )
        .with_state(route_capture.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let result = client
        .create_bucket("app-data", Some("us-west-2"), true)
        .await
        .unwrap();

    assert_eq!(result, CreateBucketResult::Created);
    assert_eq!(&*capture.path.lock().await, "/app-data");
    assert_eq!(&*capture.object_lock_header.lock().await, "true");
    assert!(
        capture
            .body
            .lock()
            .await
            .contains("<LocationConstraint>us-west-2</LocationConstraint>")
    );

    server.abort();
}

#[tokio::test]
async fn create_bucket_limits_unexpected_error_response_body() {
    let body = "x".repeat(MAX_UPSTREAM_ERROR_BODY_BYTES + 1);
    let router = Router::new().route(
        "/app-data",
        put(move || {
            let body = body.clone();
            async move { (StatusCode::BAD_GATEWAY, body) }
        }),
    );

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let client = RustfsAdminClient::new_with_base_url(format!("http://{addr}"), "access", "secret");
    let err = client
        .create_bucket("app-data", None, false)
        .await
        .expect_err("unexpected bucket create error should hide oversized body");

    assert_oversized_upstream_body_hidden(err);

    server.abort();
}

#[test]
fn extract_canned_policy_document_accepts_raw_policy_document() {
    let raw_policy =
        "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"raw\",\"Effect\":\"Allow\"}]}";

    let policy = extract_canned_policy_document(raw_policy).unwrap();

    let policy_value = serde_json::from_str::<Value>(&policy).unwrap();
    assert_eq!(policy_value["Version"], "2012-10-17");
    assert_eq!(policy_value["Statement"][0]["Sid"], "raw");
}
