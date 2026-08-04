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

//! Core duties: shared request building, signing and host resolution helpers used by all ops.
use chrono::Utc;
use url::Url;

use super::helpers::{derive_signing_key, hmac_sha256_hex, sha256_hex};
use super::{ADMIN_SIGNING_SERVICE, RustfsAdminClient, RustfsClientError, SignedRequest};

impl RustfsAdminClient {
    pub(super) async fn send_admin_request(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &str,
        content_type: Option<&str>,
    ) -> Result<String, RustfsClientError> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        let url = if query.is_empty() {
            url
        } else {
            format!("{url}?{query}")
        };

        let signed = self.sign_request(
            method,
            path,
            query,
            body,
            content_type,
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let builder = match method {
            "GET" => self.http_client.get(url),
            "POST" => self.http_client.post(url),
            "PUT" => self.http_client.put(url),
            _ => return Err(RustfsClientError::RequestBuildFailed),
        }
        .header("x-amz-date", &signed.amz_date)
        .header("x-amz-content-sha256", &signed.payload_hash)
        .header("authorization", &signed.authorization)
        .header("host", host);

        let builder = if let Some(content_type) = content_type {
            builder.header("content-type", content_type)
        } else {
            builder
        };
        let builder = if body.is_empty() {
            builder
        } else {
            builder.body(body.to_string())
        };

        let response = builder
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if !response.status().is_success() {
            return Err(RustfsClientError::unexpected_response(response).await);
        }

        response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)
    }

    pub(super) fn sign_request(
        &self,
        method: &str,
        path: &str,
        canonical_query: &str,
        payload: &str,
        content_type: Option<&str>,
        service: &str,
    ) -> Result<SignedRequest, RustfsClientError> {
        let extra_headers = content_type
            .map(|content_type| vec![("content-type", content_type)])
            .unwrap_or_default();
        self.sign_request_with_extra_headers(
            method,
            path,
            canonical_query,
            payload,
            service,
            &extra_headers,
        )
    }

    pub(super) fn sign_request_with_extra_headers(
        &self,
        method: &str,
        path: &str,
        canonical_query: &str,
        payload: &str,
        service: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<SignedRequest, RustfsClientError> {
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let payload_hash = sha256_hex(payload.as_bytes());

        let host = self.host()?;
        let mut signed_headers = vec![
            ("host", host.as_str()),
            ("x-amz-content-sha256", payload_hash.as_str()),
            ("x-amz-date", amz_date.as_str()),
        ];

        signed_headers.extend(extra_headers.iter().copied());
        signed_headers.sort_by_key(|(name, _)| *name);

        let canonical_headers: String = signed_headers
            .iter()
            .map(|(key, value)| format!("{}:{}\n", key.to_ascii_lowercase(), value.trim()))
            .collect();
        let mut signed_header_names = String::new();
        for (index, (name, _)) in signed_headers.iter().enumerate() {
            if index > 0 {
                signed_header_names.push(';');
            }
            signed_header_names.push_str(&name.to_ascii_lowercase());
        }

        let canonical_request = format!(
            "{method}\n{path}\n{canonical_query}\n{canonical_headers}\n{signed_header_names}\n{payload_hash}",
        );

        let credential_scope = format!("{date_stamp}/{}/{service}/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );

        let signing_key = derive_signing_key(&self.secret_key, &date_stamp, &self.region, service)?;
        let signature = hmac_sha256_hex(&signing_key, &string_to_sign)?;
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_header_names, signature
        );

        Ok(SignedRequest {
            amz_date,
            payload_hash,
            authorization,
        })
    }

    pub(super) fn host(&self) -> Result<String, RustfsClientError> {
        let parsed =
            Url::parse(&self.base_url).map_err(|_| RustfsClientError::RequestBuildFailed)?;
        let mut host = parsed
            .host_str()
            .map(str::to_string)
            .or_else(|| parsed.host().map(|h| h.to_string()))
            .ok_or(RustfsClientError::RequestBuildFailed)?;
        if let Some(port) = parsed.port() {
            host.push(':');
            host.push_str(&port.to_string());
        }
        Ok(host)
    }
}
