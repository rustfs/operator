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

//! S3 boundary:
//!   - bucket lifecycle methods (create/lookup features)
//!   - request semantics for S3-style object storage operations.

use reqwest::StatusCode;

use super::helpers::{
    body_mentions_not_found, bucket_already_exists, build_query_pairs, create_bucket_body,
};
use super::{ADMIN_SIGNING_SERVICE, CreateBucketResult, RustfsAdminClient, RustfsClientError};

impl RustfsAdminClient {
    // S3 duties: bucket operations exposed by the RustFS/S3-compatible endpoint.

    pub async fn create_bucket(
        &self,
        bucket: &str,
        region: Option<&str>,
        object_lock: bool,
    ) -> Result<CreateBucketResult, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let body = create_bucket_body(region);
        let content_type = (!body.is_empty()).then_some("application/xml");
        let mut extra_headers = Vec::new();
        if let Some(content_type) = content_type {
            extra_headers.push(("content-type", content_type));
        }
        if object_lock {
            extra_headers.push(("x-amz-bucket-object-lock-enabled", "true"));
        }
        let signed = self.sign_request_with_extra_headers(
            "PUT",
            &path,
            "",
            &body,
            ADMIN_SIGNING_SERVICE,
            &extra_headers,
        )?;
        let host = self.host()?;

        let mut request = self
            .http_client
            .put(format!("{}{}", self.base_url.trim_end_matches('/'), path))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host);

        for (name, value) in &extra_headers {
            request = request.header(*name, *value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(CreateBucketResult::Created);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if bucket_already_exists(status, &body) {
            return Ok(CreateBucketResult::AlreadyExists);
        }

        Err(RustfsClientError::unexpected_status_with_body(
            status, &body,
        ))
    }

    pub async fn bucket_object_lock_enabled(
        &self,
        bucket: &str,
    ) -> Result<bool, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_query_pairs(&[("object-lock", "")]);
        let signed = self.sign_request("GET", &path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .get(format!(
                "{}{}?{query}",
                self.base_url.trim_end_matches('/'),
                path
            ))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status == StatusCode::NOT_FOUND || body_mentions_not_found(&body) {
                return Ok(false);
            }
            return Err(RustfsClientError::unexpected_status_with_body(
                status, &body,
            ));
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;
        Ok(body.contains("<ObjectLockEnabled>Enabled</ObjectLockEnabled>"))
    }
}
