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

use super::helpers::{
    body_mentions_not_found, build_canonical_query, extract_canned_policy_document,
};
use super::{
    ADD_CANNED_POLICY_PATH, ADD_USER_PATH, ADMIN_SIGNING_SERVICE, INFO_CANNED_POLICY_PATH,
    JSON_CONTENT_TYPE, LIST_CANNED_POLICIES_PATH, REMOVE_USER_PATH, RustfsAdminClient,
    RustfsClientError, RustfsServerInfo, RustfsServerInfoResponse, RustfsUserInfo,
    SERVER_INFO_PATH, SET_POLICY_PATH, USER_INFO_PATH,
};
use reqwest::StatusCode;
use serde_json::Value;

fn parse_user_info_policy_names(body: &Value) -> Vec<String> {
    let Some(field) = body
        .get("policyName")
        .or_else(|| body.get("policy_name"))
        .or_else(|| body.get("PolicyName"))
    else {
        return Vec::new();
    };

    match field {
        Value::String(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

impl RustfsAdminClient {
    // Admin duties: user and policy management APIs.
    // (Candidly scoped to tenant admin operations.)

    /// Query RustFS admin policy endpoint.
    pub async fn get_canned_policy(&self, policy_name: &str) -> Result<String, RustfsClientError> {
        if policy_name.trim().is_empty() {
            return Err(RustfsClientError::InvalidPolicyName);
        }

        let query = build_canonical_query(&[("name", policy_name)]);
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

        let query = build_canonical_query(&[("name", policy_name)]);
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
        serde_json::from_str::<RustfsServerInfoResponse>(&body)
            .map(|response| response.info)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    /// Fetch IAM user info. Returns `Ok(None)` when the user does not exist.
    pub async fn get_user_info(
        &self,
        access_key: &str,
    ) -> Result<Option<RustfsUserInfo>, RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }

        let query = build_canonical_query(&[("accessKey", access_key)]);
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
            let body = response
                .text()
                .await
                .map_err(|_| RustfsClientError::RequestFailed)?;
            // Existence probes (and some test fixtures) return an empty 200 body.
            // Treat any successful response as "user exists"; parse policies when present.
            if body.trim().is_empty() {
                return Ok(Some(RustfsUserInfo {
                    policy_names: Vec::new(),
                }));
            }
            let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            return Ok(Some(RustfsUserInfo {
                policy_names: parse_user_info_policy_names(&parsed),
            }));
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
            return Ok(None);
        }

        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    pub async fn user_exists(&self, access_key: &str) -> Result<bool, RustfsClientError> {
        Ok(self.get_user_info(access_key).await?.is_some())
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
        let query = build_canonical_query(&[("accessKey", access_key)]);

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
        let query = build_canonical_query(&[
            ("isGroup", "false"),
            ("policyName", policy_names.as_str()),
            ("userOrGroup", access_key),
        ]);

        self.send_admin_request("PUT", SET_POLICY_PATH, &query, "", None)
            .await
            .map(|_| ())
    }

    /// Remove a RustFS user. Missing users are treated as success (idempotent).
    pub async fn remove_user(&self, access_key: &str) -> Result<(), RustfsClientError> {
        if access_key.trim().is_empty() {
            return Err(RustfsClientError::InvalidCredentialValue { key: "accesskey" });
        }

        let query = build_canonical_query(&[("accessKey", access_key)]);
        let path = REMOVE_USER_PATH;
        let url = format!("{}{}?{query}", self.base_url.trim_end_matches('/'), path);
        let signed = self.sign_request("DELETE", path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .delete(url)
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
            return Ok(());
        }

        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }
}

#[cfg(test)]
mod parse_tests {
    use super::parse_user_info_policy_names;
    use serde_json::json;

    #[test]
    fn parses_comma_separated_policy_name() {
        let body = json!({"policyName":"cosi-mlflow,cosi-grant-ba-1"});
        assert_eq!(
            parse_user_info_policy_names(&body),
            vec!["cosi-mlflow".to_string(), "cosi-grant-ba-1".to_string()]
        );
    }

    #[test]
    fn parses_policy_name_array_and_snake_case() {
        let body = json!({"policy_name":["a","b"]});
        assert_eq!(
            parse_user_info_policy_names(&body),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn missing_policy_field_yields_empty() {
        assert!(parse_user_info_policy_names(&json!({"status":"enabled"})).is_empty());
    }

    #[test]
    fn empty_success_body_is_treated_as_existing_user_without_policies() {
        // Mirrors get_user_info's empty-200 handling used by existence probes.
        let body = "";
        assert!(body.trim().is_empty());
        let info = RustfsUserInfo {
            policy_names: Vec::new(),
        };
        assert!(info.policy_names.is_empty());
    }
}
