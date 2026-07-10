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

use async_trait::async_trait;
use axum::{
    Router,
    extract::{Form, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use k8s_openapi::api::authentication::v1::{TokenReview, TokenReviewSpec};
use kube::{Api, Client, api::ListParams};
use std::time::Instant;

use crate::console::state::AppState;
use crate::sts::binding;
use crate::sts::error::StsError;
use crate::sts::rustfs_client::{RustfsAdminClient, RustfsClientError};
use crate::sts::session_policy;
use crate::sts::token_review::{self, TokenReviewError};
use crate::sts::types::{
    AssumeRoleWithWebIdentityForm, StsParsedRequest, StsWebIdentityResponseContext, parse_sts_form,
    render_assume_role_with_web_identity_response, render_not_implemented_response,
};
use crate::{PolicyBinding, Tenant};

const DEFAULT_STS_AUDIENCE: &str = "sts.rustfs.com";

/// Build STS routes mounted at root path `/sts`.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/sts/:tenant_namespace/:tenant_name",
        post(assume_role_with_web_identity_for_tenant),
    )
}

/// Handle POST /sts/{tenantNamespace}/{tenantName}.
async fn assume_role_with_web_identity_for_tenant(
    State(state): State<AppState>,
    Path((tenant_namespace, tenant_name)): Path<(String, String)>,
    Form(form): Form<AssumeRoleWithWebIdentityForm>,
) -> Response {
    let started = Instant::now();
    let response = assume_role_with_web_identity(state, tenant_namespace, tenant_name, form).await;
    crate::metrics::record_sts_request(response.status().is_success(), started.elapsed());
    response
}

async fn assume_role_with_web_identity(
    state: AppState,
    tenant_namespace: String,
    tenant_name: String,
    form: AssumeRoleWithWebIdentityForm,
) -> Response {
    let parsed_request = match parse_sts_form(tenant_namespace, tenant_name, form) {
        Ok(parsed_request) => parsed_request,
        Err(error) => {
            tracing::warn!(
                error_code = %error.code(),
                "STS request failed request validation"
            );
            return xml_response(StatusCode::BAD_REQUEST, error.as_xml());
        }
    };

    let Some(kube_client) = state.kube_client.as_ref() else {
        tracing::info!(
            tenant_namespace = %parsed_request.tenant_namespace,
            "STS request accepted by compatibility mode: kube_client is not available"
        );
        return xml_response(
            StatusCode::NOT_IMPLEMENTED,
            render_not_implemented_response(),
        );
    };

    let runtime = RealStsRuntime {
        kube_client: kube_client.clone(),
    };

    process_assume_role_request(&runtime, parsed_request).await
}

#[async_trait]
trait StsRuntime {
    async fn authenticate_service_account(
        &self,
        token: &str,
        audience: &str,
    ) -> Result<token_review::ServiceAccountIdentity, StsError>;

    async fn list_policy_bindings(
        &self,
        tenant_namespace: &str,
    ) -> Result<Vec<PolicyBinding>, StsError>;

    async fn select_tenant(
        &self,
        tenant_namespace: &str,
        tenant_name: &str,
    ) -> Result<Tenant, StsError>;

    async fn create_rustfs_admin_client(
        &self,
        tenant: &Tenant,
    ) -> Result<RustfsAdminClient, StsError>;

    async fn fetch_canned_policy(
        &self,
        rustfs_client: &RustfsAdminClient,
        policy_name: &str,
    ) -> Result<String, RustfsClientError>;

    async fn assume_role(
        &self,
        rustfs_client: &RustfsAdminClient,
        policy: Option<&str>,
        duration_seconds: u64,
    ) -> Result<crate::sts::types::StsAssumeRoleCredentials, RustfsClientError>;
}

struct RealStsRuntime {
    kube_client: Client,
}

#[async_trait]
impl StsRuntime for RealStsRuntime {
    async fn authenticate_service_account(
        &self,
        token: &str,
        audience: &str,
    ) -> Result<token_review::ServiceAccountIdentity, StsError> {
        authenticate_service_account(&self.kube_client, token, audience).await
    }

    async fn list_policy_bindings(
        &self,
        tenant_namespace: &str,
    ) -> Result<Vec<PolicyBinding>, StsError> {
        list_policy_bindings(&self.kube_client, tenant_namespace).await
    }

    async fn select_tenant(
        &self,
        tenant_namespace: &str,
        tenant_name: &str,
    ) -> Result<Tenant, StsError> {
        select_tenant(&self.kube_client, tenant_namespace, tenant_name).await
    }

    async fn create_rustfs_admin_client(
        &self,
        tenant: &Tenant,
    ) -> Result<RustfsAdminClient, StsError> {
        create_rustfs_admin_client(&self.kube_client, tenant).await
    }

    async fn fetch_canned_policy(
        &self,
        rustfs_client: &RustfsAdminClient,
        policy_name: &str,
    ) -> Result<String, RustfsClientError> {
        rustfs_client.get_canned_policy(policy_name).await
    }

    async fn assume_role(
        &self,
        rustfs_client: &RustfsAdminClient,
        policy: Option<&str>,
        duration_seconds: u64,
    ) -> Result<crate::sts::types::StsAssumeRoleCredentials, RustfsClientError> {
        rustfs_client.assume_role(policy, duration_seconds).await
    }
}

async fn process_assume_role_request(
    runtime: &(impl StsRuntime + Send + Sync),
    parsed_request: StsParsedRequest,
) -> Response {
    let sts_audience = operator_sts_audience();
    let identity = match runtime
        .authenticate_service_account(&parsed_request.web_identity_token, &sts_audience)
        .await
    {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error.code(),
                "TokenReview denied STS request"
            );
            return xml_response(StatusCode::BAD_REQUEST, error.as_xml());
        }
    };

    let policy_bindings = match runtime
        .list_policy_bindings(&parsed_request.tenant_namespace)
        .await
    {
        Ok(policy_bindings) => policy_bindings,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error.code(),
                "Failed listing PolicyBindings for STS authorization"
            );
            return xml_response(StatusCode::BAD_REQUEST, error.as_xml());
        }
    };

    let matching_bindings = binding::find_matching_bindings(
        &policy_bindings,
        &identity.namespace,
        &identity.service_account,
    );

    if matching_bindings.is_empty() {
        tracing::warn!(
            tenant_namespace = %parsed_request.tenant_namespace,
            tenant = %parsed_request.tenant_name,
            service_account_namespace = %identity.namespace,
            service_account = %identity.service_account,
            "No PolicyBinding matched service account for this STS request"
        );
        return xml_response(StatusCode::FORBIDDEN, StsError::AccessDenied.as_xml());
    }

    let tenant = match runtime
        .select_tenant(
            &parsed_request.tenant_namespace,
            &parsed_request.tenant_name,
        )
        .await
    {
        Ok(tenant) => tenant,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error.code(),
                "Failed selecting tenant for STS request"
            );
            return xml_response(StatusCode::BAD_REQUEST, error.as_xml());
        }
    };

    let rustfs_client = match runtime.create_rustfs_admin_client(&tenant).await {
        Ok(rustfs_client) => rustfs_client,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error.code(),
                "Failed creating RustFS admin client"
            );
            return xml_response(sts_error_status(&error), error.as_xml());
        }
    };

    let binding_policies =
        match resolve_binding_policies(runtime, &rustfs_client, &matching_bindings).await {
            Ok(binding_policies) => binding_policies,
            Err(error) => {
                tracing::warn!(
                    tenant_namespace = %parsed_request.tenant_namespace,
                    tenant = %parsed_request.tenant_name,
                    error = %error.code(),
                    "Failed resolving PolicyBinding policy documents"
                );
                let status = match error {
                    StsError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
                    StsError::AccessDenied => StatusCode::FORBIDDEN,
                    _ => StatusCode::BAD_REQUEST,
                };
                return xml_response(status, error.as_xml());
            }
        };

    let merged_policy = match session_policy::merge_session_policies(
        parsed_request.policy.as_deref(),
        &binding_policies,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error.code(),
                "Failed to build merged STS session policy"
            );
            return xml_response(sts_error_status(&error), error.as_xml());
        }
    };

    let credentials = match runtime
        .assume_role(
            &rustfs_client,
            merged_policy.as_deref(),
            parsed_request.duration_seconds,
        )
        .await
    {
        Ok(credentials) => credentials,
        Err(error) => {
            tracing::warn!(
                tenant_namespace = %parsed_request.tenant_namespace,
                tenant = %parsed_request.tenant_name,
                error = %error,
                "Failed calling RustFS AssumeRole"
            );
            return xml_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                StsError::InternalError.as_xml(),
            );
        }
    };

    let tenant_name = tenant
        .metadata
        .name
        .clone()
        .unwrap_or_else(|| parsed_request.tenant_name.clone());

    let binding_policy_count: usize = matching_bindings
        .iter()
        .map(|binding| binding.spec.policies.len())
        .sum();

    tracing::info!(
        tenant = %tenant_name,
        tenant_namespace = %parsed_request.tenant_namespace,
        service_account_namespace = %identity.namespace,
        service_account = %identity.service_account,
        action = %parsed_request.action,
        duration_seconds = parsed_request.duration_seconds,
        version = %parsed_request.version,
        request_policy = parsed_request.policy.is_some(),
        binding_policy_count,
        "STS request passed TokenReview + PolicyBinding authorization checks"
    );

    xml_response(
        StatusCode::OK,
        render_assume_role_with_web_identity_response(
            &credentials,
            &web_identity_response_context(
                &parsed_request,
                &identity,
                &sts_audience,
                &tenant_name,
                merged_policy.as_deref(),
                &credentials,
            ),
        ),
    )
}

fn web_identity_response_context(
    parsed_request: &StsParsedRequest,
    identity: &token_review::ServiceAccountIdentity,
    audience: &str,
    tenant_name: &str,
    merged_policy: Option<&str>,
    credentials: &crate::sts::types::StsAssumeRoleCredentials,
) -> StsWebIdentityResponseContext {
    let subject = format!(
        "system:serviceaccount:{}:{}",
        identity.namespace, identity.service_account
    );
    let role_session_name = format!("{}:{}", identity.namespace, identity.service_account);
    let packed_policy_size = merged_policy
        .map(|policy| {
            let size = policy.len() * 100;
            size.div_ceil(session_policy::MAX_SESSION_POLICY_SIZE)
                .min(100) as u8
        })
        .unwrap_or(0);

    StsWebIdentityResponseContext {
        subject,
        audience: audience.to_string(),
        provider: "kubernetes".to_string(),
        assumed_role_arn: format!(
            "arn:rustfs:sts::{}:assumed-role/{}/{}",
            parsed_request.tenant_namespace, tenant_name, role_session_name
        ),
        assumed_role_id: format!("{}:{}", credentials.access_key_id, role_session_name),
        packed_policy_size,
    }
}

fn operator_sts_audience() -> String {
    std::env::var("OPERATOR_STS_AUDIENCE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STS_AUDIENCE.to_string())
}

async fn resolve_binding_policies<R: StsRuntime + Send + Sync>(
    runtime: &R,
    rustfs_client: &RustfsAdminClient,
    matching_bindings: &[PolicyBinding],
) -> Result<Vec<String>, StsError> {
    let mut policies = Vec::new();
    let mut referenced_policy_count = 0usize;

    for binding in matching_bindings {
        if binding.spec.policies.is_empty() {
            tracing::warn!(
                policy_binding = binding.metadata.name.as_deref().unwrap_or("<unknown>"),
                "PolicyBinding matched service account but does not reference any policies"
            );
        }

        for policy_name in &binding.spec.policies {
            referenced_policy_count += 1;

            let raw_policy = match runtime
                .fetch_canned_policy(rustfs_client, policy_name)
                .await
            {
                Ok(policy) => policy,
                Err(error) => {
                    tracing::warn!(
                        policy = %policy_name,
                        error = %error,
                        "Failed fetching PolicyBinding policy; skipping policy"
                    );
                    continue;
                }
            };

            match session_policy::normalize_policy_for_merge(&raw_policy) {
                Ok(compact) => policies.push(compact),
                Err(error) => {
                    tracing::warn!(
                        policy = %policy_name,
                        error = %error.code(),
                        "Invalid PolicyBinding policy document; skipping policy"
                    );
                    continue;
                }
            }
        }
    }

    if policies.is_empty() {
        tracing::warn!(
            matched_binding_count = matching_bindings.len(),
            referenced_policy_count,
            "No valid PolicyBinding policy documents were resolved"
        );
        return Err(StsError::AccessDenied);
    }

    Ok(policies)
}

async fn authenticate_service_account(
    client: &Client,
    token: &str,
    audience: &str,
) -> Result<token_review::ServiceAccountIdentity, StsError> {
    let request = TokenReview {
        metadata: Default::default(),
        spec: TokenReviewSpec {
            audiences: Some(vec![audience.to_string()]),
            token: Some(token.to_string()),
        },
        status: None,
    };

    let api: Api<TokenReview> = Api::all(client.clone());
    let token_review = api
        .create(&kube::api::PostParams::default(), &request)
        .await
        .map_err(|_| StsError::InternalError)?;

    let status = token_review
        .status
        .as_ref()
        .ok_or(StsError::InvalidIdentityToken)?;

    token_review::extract_service_account_identity_for_audience(status, Some(audience))
        .map_err(map_token_review_error)
}

fn map_token_review_error(error: TokenReviewError) -> StsError {
    match error {
        TokenReviewError::MissingTokenReview => StsError::InvalidIdentityToken,
        TokenReviewError::NotAuthenticated => StsError::InvalidIdentityToken,
        TokenReviewError::MissingAudience => StsError::InvalidIdentityToken,
        TokenReviewError::InvalidAudience => StsError::InvalidIdentityToken,
        TokenReviewError::MissingUsername => StsError::InvalidIdentityToken,
        TokenReviewError::InvalidUsername => StsError::InvalidIdentityToken,
        TokenReviewError::InvalidUsernameFormat => StsError::InvalidIdentityToken,
    }
}

async fn list_policy_bindings(
    client: &Client,
    tenant_namespace: &str,
) -> Result<Vec<PolicyBinding>, StsError> {
    let api: Api<PolicyBinding> = Api::namespaced(client.clone(), tenant_namespace);
    api.list(&ListParams::default())
        .await
        .map(|list| list.items)
        .map_err(|_| StsError::InternalError)
}

async fn select_tenant(
    client: &Client,
    tenant_namespace: &str,
    tenant_name: &str,
) -> Result<Tenant, StsError> {
    let api: Api<Tenant> = Api::namespaced(client.clone(), tenant_namespace);

    api.get(tenant_name).await.map_err(|error| match error {
        kube::Error::Api(api_error) if api_error.code == 404 => StsError::InvalidParameterValue {
            parameter: "tenantName",
        },
        _ => StsError::InternalError,
    })
}

async fn create_rustfs_admin_client(
    client: &Client,
    tenant: &Tenant,
) -> Result<RustfsAdminClient, StsError> {
    if !tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
        return Err(StsError::InvalidParameterValue {
            parameter: "tenantTls",
        });
    }

    let credentials = RustfsAdminClient::load_tenant_credentials(client, tenant)
        .await
        .map_err(|_| StsError::InternalError)?;

    RustfsAdminClient::from_tls_tenant_for_sts(client, tenant, credentials)
        .await
        .map_err(map_rustfs_client_creation_error)
}

fn map_rustfs_client_creation_error(error: RustfsClientError) -> StsError {
    match error {
        RustfsClientError::TenantTlsRequired => StsError::InvalidParameterValue {
            parameter: "tenantTls",
        },
        RustfsClientError::TenantTlsClientCertificateRequired => {
            StsError::TenantTlsClientCertificateUnsupported
        }
        _ => StsError::InternalError,
    }
}

fn xml_response(status_code: StatusCode, body: String) -> Response {
    (
        status_code,
        [(header::CONTENT_TYPE, "application/xml")],
        body,
    )
        .into_response()
}

fn sts_error_status(error: &StsError) -> StatusCode {
    match error {
        StsError::AccessDenied | StsError::UnsupportedRequestPolicy => StatusCode::FORBIDDEN,
        StsError::InvalidParameterValue { .. }
        | StsError::TenantTlsClientCertificateUnsupported
        | StsError::MissingParameter { .. }
        | StsError::MalformedPolicyDocument
        | StsError::PackedPolicyTooLarge
        | StsError::InvalidIdentityToken => StatusCode::BAD_REQUEST,
        StsError::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        StsError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::sts::types::{STS_API_VERSION, STS_WEB_IDENTITY_ACTION, StsAssumeRoleCredentials};
    use crate::types::v1alpha1::policy_binding::{PolicyBindingApplication, PolicyBindingSpec};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn namespace_only_route_is_not_registered() {
        let app = routes().with_state(AppState::new("test-secret".to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sts/tenant-a")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "Version=2011-06-15&Action=AssumeRoleWithWebIdentity&WebIdentityToken=abc",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn valid_explicit_tenant_route_returns_not_implemented_if_runtime_is_unavailable() {
        let app = routes().with_state(AppState::new("test-secret".to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sts/tenant-a/rustfs-a")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "Version=2011-06-15&Action=AssumeRoleWithWebIdentity&WebIdentityToken=abc",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn invalid_request_returns_bad_request_xml_error() {
        let app = routes().with_state(AppState::new("test-secret".to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sts/tenant-a/rustfs-a")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "Version=2011-06-15&Action=AssumeRoleWithWebIdentity",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("<ErrorResponse"));
        assert!(text.contains("<Code>MissingParameter</Code>"));
    }

    #[tokio::test]
    async fn sts_handler_rejects_invalid_tokenreview_identity() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Err(StsError::InvalidIdentityToken))),
            policy_bindings: Mutex::new(Some(Ok(vec![]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::new()),
            assume_role_result: Mutex::new(Some(Ok(StsAssumeRoleCredentials {
                access_key_id: "ak".to_string(),
                secret_access_key: "sk".to_string(),
                session_token: "token".to_string(),
                expiration: "2024-01-01T00:00:00Z".to_string(),
            }))),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: None,
            policy: None,
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "rustfs-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn sts_handler_requires_matching_policy_binding() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "other-ns".to_string(),
                        serviceaccount: "other-sa".to_string(),
                    },
                    policies: vec!["policy-a".to_string()],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::new()),
            assume_role_result: Mutex::new(Some(Ok(StsAssumeRoleCredentials {
                access_key_id: "ak".to_string(),
                secret_access_key: "sk".to_string(),
                session_token: "token".to_string(),
                expiration: "2024-01-01T00:00:00Z".to_string(),
            }))),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: None,
            policy: None,
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "rustfs-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn sts_handler_uses_binding_policy_without_request_policy_and_succeeds() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "workload-sa".to_string(),
                    },
                    policies: vec!["policy-binding".to_string()],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::from([
                Ok("{\"Version\": \"2012-10-17\", \"Statement\": [{\"Sid\":\"b1\",\"Effect\":\"Allow\"}]}".to_string()),
            ])),
            assume_role_result: Mutex::new(Some(Ok(StsAssumeRoleCredentials {
                access_key_id: "ak".to_string(),
                secret_access_key: "sk".to_string(),
                session_token: "session".to_string(),
                expiration: "2024-01-01T00:00:00Z".to_string(),
            }))),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: Some("3600".to_string()),
            policy: None,
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "tenant-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert!(text.contains("<AssumeRoleWithWebIdentityResponse"));
        assert!(text.contains("<AccessKeyId>ak</AccessKeyId>"));

        let assume_role_policies = runtime.assume_role_policies.lock().unwrap();
        assert_eq!(assume_role_policies.len(), 1);
        let policy = assume_role_policies[0]
            .as_ref()
            .expect("binding session policy should be passed to RustFS AssumeRole");
        assert!(policy.contains("\"Sid\":\"b1\""));
    }

    #[tokio::test]
    async fn sts_handler_rejects_caller_supplied_request_policy() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "workload-sa".to_string(),
                    },
                    policies: vec!["policy-binding".to_string()],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::from([
                Ok("{\"Version\": \"2012-10-17\", \"Statement\": [{\"Sid\":\"binding-read\",\"Effect\":\"Allow\",\"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::bucket-a/*\"}]}".to_string()),
            ])),
            assume_role_result: Mutex::new(Some(Err(RustfsClientError::RequestFailed))),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: Some("3600".to_string()),
            policy: Some("{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"request-admin\",\"Effect\":\"Allow\",\"Action\":\"s3:*\",\"Resource\":\"*\"}] }".to_string()),
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "tenant-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert!(text.contains("<Code>AccessDenied</Code>"));
        assert!(text.contains("Policy request parameters are not supported"));
        assert!(runtime.assume_role_policies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sts_handler_rejects_malformed_caller_policy_without_assume_role() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "workload-sa".to_string(),
                    },
                    policies: vec!["policy-binding".to_string()],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::from([
                Ok("{\"Version\": \"2012-10-17\", \"Statement\": [{\"Sid\":\"binding-read\",\"Effect\":\"Allow\",\"Action\":\"s3:GetObject\",\"Resource\":\"arn:aws:s3:::bucket-a/*\"}]}".to_string()),
            ])),
            assume_role_result: Mutex::new(Some(Err(RustfsClientError::RequestFailed))),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: Some("3600".to_string()),
            policy: Some("{".to_string()),
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "tenant-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert!(text.contains("<Code>AccessDenied</Code>"));
        assert!(text.contains("Policy request parameters are not supported"));
        assert!(runtime.assume_role_policies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sts_handler_rejects_matching_binding_without_valid_policies() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "workload-sa".to_string(),
                    },
                    policies: vec![],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Ok(RustfsAdminClient::new_with_base_url(
                "http://127.0.0.1:1",
                "access-key",
                "secret-key",
            )))),
            fetch_policy_results: Mutex::new(VecDeque::new()),
            assume_role_result: Mutex::new(None),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: Some("3600".to_string()),
            policy: None,
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "rustfs-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn sts_handler_rejects_mtls_tenant_with_clear_bad_request() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(Some(Ok(token_review::ServiceAccountIdentity {
                namespace: "tenant-a".to_string(),
                service_account: "workload-sa".to_string(),
            }))),
            policy_bindings: Mutex::new(Some(Ok(vec![PolicyBinding {
                metadata: Default::default(),
                spec: PolicyBindingSpec {
                    application: PolicyBindingApplication {
                        namespace: "tenant-a".to_string(),
                        serviceaccount: "workload-sa".to_string(),
                    },
                    policies: vec!["policy-a".to_string()],
                },
                status: None,
            }]))),
            tenant: Mutex::new(Some(Ok(crate::types::v1alpha1::tenant::Tenant {
                metadata: Default::default(),
                spec: Default::default(),
                status: None,
            }))),
            create_client: Mutex::new(Some(Err(StsError::TenantTlsClientCertificateUnsupported))),
            fetch_policy_results: Mutex::new(VecDeque::new()),
            assume_role_result: Mutex::new(None),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let form = AssumeRoleWithWebIdentityForm {
            version: Some(STS_API_VERSION.to_string()),
            action: Some(STS_WEB_IDENTITY_ACTION.to_string()),
            web_identity_token: Some("sa-token".to_string()),
            duration_seconds: Some("3600".to_string()),
            policy: None,
        };
        let parsed = parse_sts_form("tenant-a".to_string(), "rustfs-a".to_string(), form).unwrap();

        let response = process_assume_role_request(&runtime, parsed).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("<Code>TenantTlsClientCertificateUnsupported</Code>"));
        assert!(text.contains("does not support Tenants that require TLS client certificates"));
    }

    #[test]
    fn mtls_tenant_client_error_maps_to_explicit_sts_error() {
        let error =
            map_rustfs_client_creation_error(RustfsClientError::TenantTlsClientCertificateRequired);

        assert_eq!(error, StsError::TenantTlsClientCertificateUnsupported);
        assert_eq!(sts_error_status(&error), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn resolve_binding_policies_skips_invalid_binding_policies() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(None),
            policy_bindings: Mutex::new(None),
            tenant: Mutex::new(None),
            create_client: Mutex::new(None),
            fetch_policy_results: Mutex::new(VecDeque::from([
                Ok("{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"good\",\"Effect\":\"Allow\"}]}".to_string()),
                Ok("{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()),
                Err(RustfsClientError::RequestFailed),
            ])),
            assume_role_result: Mutex::new(None),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let client = RustfsAdminClient::new_with_base_url("http://127.0.0.1:1", "access", "secret");
        let bindings = vec![PolicyBinding {
            metadata: Default::default(),
            spec: PolicyBindingSpec {
                application: PolicyBindingApplication {
                    namespace: "tenant-a".to_string(),
                    serviceaccount: "sa".to_string(),
                },
                policies: vec![
                    "policy-good".to_string(),
                    "policy-empty".to_string(),
                    "policy-fail".to_string(),
                ],
            },
            status: None,
        }];

        let policies = super::resolve_binding_policies(&runtime, &client, &bindings)
            .await
            .expect("valid referenced policies should allow the STS request");

        assert_eq!(policies.len(), 1);
        assert!(policies[0].contains("\"Sid\":\"good\""));
    }

    #[tokio::test]
    async fn resolve_binding_policies_rejects_when_no_valid_binding_policy_exists() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(None),
            policy_bindings: Mutex::new(None),
            tenant: Mutex::new(None),
            create_client: Mutex::new(None),
            fetch_policy_results: Mutex::new(VecDeque::from([Err(
                RustfsClientError::RequestFailed,
            )])),
            assume_role_result: Mutex::new(None),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let client = RustfsAdminClient::new_with_base_url("http://127.0.0.1:1", "access", "secret");
        let bindings = vec![PolicyBinding {
            metadata: Default::default(),
            spec: PolicyBindingSpec {
                application: PolicyBindingApplication {
                    namespace: "tenant-a".to_string(),
                    serviceaccount: "sa".to_string(),
                },
                policies: vec!["policy-fail".to_string()],
            },
            status: None,
        }];

        let error = super::resolve_binding_policies(&runtime, &client, &bindings)
            .await
            .expect_err("no valid binding policy must reject the STS request");

        assert!(matches!(error, StsError::AccessDenied));
    }

    #[tokio::test]
    async fn resolve_binding_policies_rejects_empty_binding_policy_lists() {
        let runtime = MockStsRuntime {
            identity: Mutex::new(None),
            policy_bindings: Mutex::new(None),
            tenant: Mutex::new(None),
            create_client: Mutex::new(None),
            fetch_policy_results: Mutex::new(VecDeque::new()),
            assume_role_result: Mutex::new(None),
            assume_role_policies: Mutex::new(Vec::new()),
        };

        let client = RustfsAdminClient::new_with_base_url("http://127.0.0.1:1", "access", "secret");
        let bindings = vec![PolicyBinding {
            metadata: Default::default(),
            spec: PolicyBindingSpec {
                application: PolicyBindingApplication {
                    namespace: "tenant-a".to_string(),
                    serviceaccount: "sa".to_string(),
                },
                policies: vec![],
            },
            status: None,
        }];

        let error = super::resolve_binding_policies(&runtime, &client, &bindings)
            .await
            .expect_err("empty binding policy list must reject the STS request");

        assert!(matches!(error, StsError::AccessDenied));
    }

    struct MockStsRuntime {
        identity: Mutex<Option<Result<token_review::ServiceAccountIdentity, StsError>>>,
        policy_bindings: Mutex<Option<Result<Vec<PolicyBinding>, StsError>>>,
        tenant: Mutex<Option<Result<Tenant, StsError>>>,
        create_client: Mutex<Option<Result<RustfsAdminClient, StsError>>>,
        fetch_policy_results: Mutex<VecDeque<Result<String, RustfsClientError>>>,
        assume_role_result:
            Mutex<Option<Result<crate::sts::types::StsAssumeRoleCredentials, RustfsClientError>>>,
        assume_role_policies: Mutex<Vec<Option<String>>>,
    }

    #[async_trait]
    impl StsRuntime for MockStsRuntime {
        async fn authenticate_service_account(
            &self,
            _token: &str,
            _audience: &str,
        ) -> Result<token_review::ServiceAccountIdentity, StsError> {
            self.identity
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(StsError::InternalError))
        }

        async fn list_policy_bindings(
            &self,
            _tenant_namespace: &str,
        ) -> Result<Vec<PolicyBinding>, StsError> {
            self.policy_bindings
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(StsError::InternalError))
        }

        async fn select_tenant(
            &self,
            _tenant_namespace: &str,
            _tenant_name: &str,
        ) -> Result<Tenant, StsError> {
            self.tenant
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(StsError::InternalError))
        }

        async fn create_rustfs_admin_client(
            &self,
            _tenant: &Tenant,
        ) -> Result<RustfsAdminClient, StsError> {
            self.create_client
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(StsError::InternalError))
        }

        async fn fetch_canned_policy(
            &self,
            _rustfs_client: &RustfsAdminClient,
            _policy_name: &str,
        ) -> Result<String, RustfsClientError> {
            self.fetch_policy_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(RustfsClientError::RequestFailed))
        }

        async fn assume_role(
            &self,
            _rustfs_client: &RustfsAdminClient,
            policy: Option<&str>,
            _duration_seconds: u64,
        ) -> Result<crate::sts::types::StsAssumeRoleCredentials, RustfsClientError> {
            self.assume_role_policies
                .lock()
                .unwrap()
                .push(policy.map(ToString::to_string));
            self.assume_role_result
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(RustfsClientError::RequestFailed))
        }
    }
}
