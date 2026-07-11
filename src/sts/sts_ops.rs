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

//! STS boundary:
//!   - temporary credentials and AssumeRole request composition/response parsing.
use super::helpers::{build_query_pairs, parse_assume_role_response};
use super::{
    ASSUME_ROLE_PATH, FORM_CONTENT_TYPE, RustfsAdminClient, RustfsClientError, STS_SIGNING_SERVICE,
};
use crate::sts::types::StsAssumeRoleCredentials;

impl RustfsAdminClient {
    // STS duties: temporary credentials and AssumeRole API call path.

    /// Send AssumeRole request to RustFS admin STS endpoint (`/`).
    pub async fn assume_role(
        &self,
        policy: Option<&str>,
        duration_seconds: u64,
    ) -> Result<StsAssumeRoleCredentials, RustfsClientError> {
        let mut params = vec![
            ("Version", Self::STS_VERSION.to_string()),
            ("Action", Self::STS_ACTION.to_string()),
            ("DurationSeconds", duration_seconds.to_string()),
        ];

        if let Some(policy) = policy {
            params.push(("Policy", policy.to_string()));
        }

        let body = build_query_pairs(
            &params
                .iter()
                .map(|(k, v)| (&k[..], &v[..]))
                .collect::<Vec<_>>(),
        );

        let path = ASSUME_ROLE_PATH;
        let signed = self.sign_request(
            "POST",
            path,
            "",
            &body,
            Some(FORM_CONTENT_TYPE),
            STS_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let response = self
            .http_client
            .post(format!("{}/", self.base_url.trim_end_matches('/')))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", FORM_CONTENT_TYPE)
            .body(body)
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

        parse_assume_role_response(&body).ok_or(RustfsClientError::ParseResponseFailed)
    }
}
