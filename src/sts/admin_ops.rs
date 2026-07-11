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

//! Admin operations boundary:
//!   - user CRUD and policy queries/expression on `/rustfs/admin/v3/*`
//!   - only admin protocol semantics live here; transport/signing is delegated.

use std::collections::BTreeMap;

use super::helpers::{body_mentions_not_found, build_query_pairs, extract_canned_policy_document};
use super::{
    ADD_CANNED_POLICY_PATH, ADD_USER_PATH, ADMIN_SIGNING_SERVICE, INFO_CANNED_POLICY_PATH,
    JSON_CONTENT_TYPE, LIST_CANNED_POLICIES_PATH, RustfsAdminClient, RustfsClientError,
    RustfsServerInfo, SERVER_INFO_PATH, SET_POLICY_PATH, USER_INFO_PATH,
};
use reqwest::StatusCode;
use serde_json::Value;

impl RustfsAdminClient {
    // Admin duties: user and policy management APIs.
    // (Candidly scoped to tenant admin operations.)

    /// Query RustFS admin policy endpoint.
    pub async fn get_canned_policy(&self, policy_name: &str) -> Result<String, RustfsClientError> {
        if policy_name.trim().is_empty() {
            return Err(RustfsClientError::InvalidPolicyName);
        }

        let query = build_query_pairs(&[("name", policy_name)]);
        let path = INFO_CANNED_POLICY_PATH;
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let url = if query.is_empty() {
            url
        } else {
            format!("{url}?{query}")
        };

        let signed = self.sign_request("GET", path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::unexpected_response(response).await);
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        extract_canned_policy_document(&body)
    }

    /// Add or replace a RustFS canned policy through the admin API.
    pub async fn add_canned_policy(
        &self,
        policy_name: &str,
        policy_document: &str,
    ) -> Result<(), RustfsClientError> {
        if policy_name.trim().is_empty() {
            return Err(RustfsClientError::InvalidPolicyName);
        }
        serde_json::from_str::<Value>(policy_document)
            .map_err(|_| RustfsClientError::InvalidPolicyDocument)?;

        let query = build_query_pairs(&[("name", policy_name)]);
        let path = ADD_CANNED_POLICY_PATH;
        let url = format!("{}{}?{query}", self.base_url.trim_end_matches('/'), path);

        let signed = self.sign_request(
            "PUT",
            path,
            &query,
            policy_document,
            Some(JSON_CONTENT_TYPE),
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let response = self
            .http_client
            .put(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", JSON_CONTENT_TYPE)
            .body(policy_document.to_string())
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::unexpected_response(response).await);
        }

        Ok(())
    }

    pub async fn list_canned_policies(
        &self,
    ) -> Result<BTreeMap<String, String>, RustfsClientError> {
        let body = self
            .send_admin_request("GET", LIST_CANNED_POLICIES_PATH, "", "", None)
            .await?;
        let policies = serde_json::from_str::<BTreeMap<String, Value>>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)?;

        policies
            .into_iter()
            .map(|(name, policy)| {
                let raw = serde_json::to_string(&policy)
                    .map_err(|_| RustfsClientError::ParseResponseFailed)?;
                let policy = extract_canned_policy_document(&raw)
                    .map_err(|_| RustfsClientError::ParseResponseFailed)?;
                let policy = serde_json::from_str::<Value>(&policy)
                    .map_err(|_| RustfsClientError::ParseResponseFailed)?;

                serde_json::to_string(&policy)
                    .map(|document| (name, document))
                    .map_err(|_| RustfsClientError::ParseResponseFailed)
            })
            .collect()
    }

    pub async fn server_info(&self) -> Result<RustfsServerInfo, RustfsClientError> {
        let body = self
            .send_admin_request("GET", SERVER_INFO_PATH, "", "", None)
            .await?;
        serde_json::from_str::<RustfsServerInfo>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    pub async fn user_exists(&self, access_key: &str) -> Result<bool, RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }

        let query = build_query_pairs(&[("accessKey", access_key)]);
        let path = USER_INFO_PATH;
        let url = format!("{}{}?{query}", self.base_url.trim_end_matches('/'), path);
        let signed = self.sign_request("GET", path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(true);
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
            return Ok(false);
        }

        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    pub async fn add_user(
        &self,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }
        if secret_key.is_empty() {
            return Err(RustfsClientError::EmptyCredentialValue { key: "secretkey" });
        }

        let body = serde_json::json!({
            "secretKey": secret_key,
            "status": "enabled",
        })
        .to_string();
        let query = build_query_pairs(&[("accessKey", access_key)]);

        self.send_admin_request("PUT", ADD_USER_PATH, &query, &body, Some(JSON_CONTENT_TYPE))
            .await
            .map(|_| ())
    }

    pub async fn set_user_policy(
        &self,
        access_key: &str,
        policies: &[String],
    ) -> Result<(), RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }
        if policies.is_empty() || policies.iter().any(|policy| policy.trim().is_empty()) {
            return Err(RustfsClientError::InvalidPolicyName);
        }

        let policy_names = policies.join(",");
        let query = build_query_pairs(&[
            ("isGroup", "false"),
            ("policyName", policy_names.as_str()),
            ("userOrGroup", access_key),
        ]);

        self.send_admin_request("PUT", SET_POLICY_PATH, &query, "", None)
            .await
            .map(|_| ())
    }
}
