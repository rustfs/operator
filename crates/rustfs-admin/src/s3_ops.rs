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

use super::helpers::{
    BucketConflictKind, body_mentions_not_found, bucket_conflict_kind, build_canonical_query,
    create_bucket_body, escape_xml, is_absent_resource,
};
use super::{ADMIN_SIGNING_SERVICE, CreateBucketResult, RustfsAdminClient, RustfsClientError};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use reqwest::StatusCode;

const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";
const MAX_BUCKET_LIFECYCLE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketLifecycleRuleStatus {
    Enabled,
    Disabled,
}

impl BucketLifecycleRuleStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLifecycleExpiration {
    pub days: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLifecycleAbortIncompleteMultipartUpload {
    pub days_after_initiation: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLifecycleRule {
    pub id: String,
    pub status: BucketLifecycleRuleStatus,
    pub prefix: String,
    pub expiration: Option<BucketLifecycleExpiration>,
    pub abort_incomplete_multipart_upload: Option<BucketLifecycleAbortIncompleteMultipartUpload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketLifecycleConfiguration {
    pub rules: Vec<BucketLifecycleRule>,
}

impl BucketLifecycleConfiguration {
    pub fn to_xml(&self) -> String {
        let mut rules = self.rules.iter().collect::<Vec<_>>();
        rules.sort_unstable_by(|left, right| left.id.cmp(&right.id));

        let mut xml = format!("<LifecycleConfiguration xmlns=\"{S3_XML_NAMESPACE}\">");
        for rule in rules {
            xml.push_str("<Rule>");
            if let Some(abort) = &rule.abort_incomplete_multipart_upload {
                xml.push_str("<AbortIncompleteMultipartUpload><DaysAfterInitiation>");
                xml.push_str(&abort.days_after_initiation.to_string());
                xml.push_str("</DaysAfterInitiation></AbortIncompleteMultipartUpload>");
            }
            if let Some(expiration) = &rule.expiration {
                xml.push_str("<Expiration><Days>");
                xml.push_str(&expiration.days.to_string());
                xml.push_str("</Days></Expiration>");
            }
            xml.push_str("<Filter>");
            if !rule.prefix.is_empty() {
                xml.push_str("<Prefix>");
                xml.push_str(&escape_xml(&rule.prefix));
                xml.push_str("</Prefix>");
            }
            xml.push_str("</Filter><ID>");
            xml.push_str(&escape_xml(&rule.id));
            xml.push_str("</ID><Status>");
            xml.push_str(rule.status.as_str());
            xml.push_str("</Status></Rule>");
        }
        xml.push_str("</LifecycleConfiguration>");
        xml
    }
}

/// Produces a stable representation for lifecycle ownership checks. Namespace declarations,
/// formatting whitespace, and empty-element spelling are normalized, while unknown lifecycle
/// elements and attributes remain part of the result so the operator cannot silently adopt them.
pub fn canonicalize_bucket_lifecycle_xml(xml: &str) -> Result<String, RustfsClientError> {
    let mut reader = Reader::from_str(xml);
    let mut canonical = String::new();
    let mut elements = Vec::new();
    let mut saw_root = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = element_local_name(element.name().as_ref())?;
                if elements.is_empty() {
                    if saw_root || name != "LifecycleConfiguration" {
                        return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                    }
                    saw_root = true;
                }
                append_start_element(&mut canonical, &element, &reader)?;
                elements.push(name);
            }
            Ok(Event::Empty(element)) => {
                let name = element_local_name(element.name().as_ref())?;
                if elements.is_empty() {
                    if saw_root || name != "LifecycleConfiguration" {
                        return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                    }
                    saw_root = true;
                }
                append_start_element(&mut canonical, &element, &reader)?;
                canonical.push_str("</");
                canonical.push_str(&name);
                canonical.push('>');
            }
            Ok(Event::End(element)) => {
                let name = element_local_name(element.name().as_ref())?;
                if elements.pop().as_deref() != Some(name.as_str()) {
                    return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                }
                canonical.push_str("</");
                canonical.push_str(&name);
                canonical.push('>');
            }
            Ok(Event::Text(text)) => {
                let text = std::str::from_utf8(text.as_ref())
                    .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?;
                if !text.trim().is_empty() {
                    if elements.is_empty() {
                        return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                    }
                    canonical.push_str(text);
                }
            }
            Ok(Event::CData(text)) => {
                if elements.is_empty() {
                    return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                }
                let text = std::str::from_utf8(text.as_ref())
                    .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?;
                canonical.push_str("<![CDATA[");
                canonical.push_str(text);
                canonical.push_str("]]>");
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::DocType(_) | Event::GeneralRef(_)) => {
                return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(RustfsClientError::InvalidLifecycleConfigurationResponse),
        }
    }

    if !saw_root || !elements.is_empty() {
        return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
    }
    Ok(canonical)
}

async fn read_bucket_lifecycle_response(
    mut response: reqwest::Response,
) -> Result<String, RustfsClientError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RustfsClientError::RequestFailed)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BUCKET_LIFECYCLE_RESPONSE_BYTES {
            return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)
}

fn append_start_element(
    canonical: &mut String,
    element: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
) -> Result<(), RustfsClientError> {
    let name = element_local_name(element.name().as_ref())?;
    let attributes = element
        .attributes()
        .map(|attribute| {
            let attribute =
                attribute.map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?;
            if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
                return Ok(None);
            }
            let key = element_local_name(attribute.key.as_ref())?;
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?;
            Ok(Some((key, value.into_owned())))
        })
        .collect::<Result<Vec<_>, RustfsClientError>>()?;
    let mut attributes = attributes.into_iter().flatten().collect::<Vec<_>>();
    attributes.sort_unstable();

    canonical.push('<');
    canonical.push_str(&name);
    for (key, value) in attributes {
        canonical.push(' ');
        canonical.push_str(&key);
        canonical.push_str("=\"");
        canonical.push_str(&escape_xml(&value));
        canonical.push('"');
    }
    canonical.push('>');
    Ok(())
}

fn element_local_name(name: &[u8]) -> Result<String, RustfsClientError> {
    let name = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    std::str::from_utf8(name)
        .map(str::to_string)
        .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)
}

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
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        match bucket_conflict_kind(status, &body) {
            Some(BucketConflictKind::OwnedByYou) => {
                return Ok(CreateBucketResult::AlreadyOwnedByYou);
            }
            Some(BucketConflictKind::OwnedByOther) => return Ok(CreateBucketResult::AlreadyExists),
            None => {}
        }

        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    /// Delete a bucket. Missing buckets are treated as success (idempotent).
    pub async fn delete_bucket(&self, bucket: &str) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let signed = self.sign_request("DELETE", &path, "", "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .delete(format!("{}{}", self.base_url.trim_end_matches('/'), path))
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

    pub async fn bucket_object_lock_enabled(
        &self,
        bucket: &str,
    ) -> Result<bool, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("object-lock", "")]);
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
            let (body, truncated) = RustfsClientError::limited_response_body(response).await;
            if is_absent_resource(status, &body) {
                return Ok(false);
            }
            return Err(RustfsClientError::unexpected_status_with_limited_body(
                status, &body, truncated,
            ));
        }

        let body = response
            .text()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;
        Ok(body.contains("<ObjectLockEnabled>Enabled</ObjectLockEnabled>"))
    }

    pub async fn put_bucket_policy(
        &self,
        bucket: &str,
        policy: &str,
    ) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() || policy.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("policy", "")]);
        let signed = self.sign_request(
            "PUT",
            &path,
            &query,
            policy,
            Some("application/json"),
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;

        let response = self
            .http_client
            .put(format!(
                "{}{}?{query}",
                self.base_url.trim_end_matches('/'),
                path
            ))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", "application/json")
            .body(policy.to_string())
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(());
        }

        Err(RustfsClientError::unexpected_response(response).await)
    }

    pub async fn get_bucket_policy(
        &self,
        bucket: &str,
    ) -> Result<Option<String>, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("policy", "")]);
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

        if response.status().is_success() {
            let body = response
                .text()
                .await
                .map_err(|_| RustfsClientError::RequestFailed)?;
            let trimmed = body.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            return Ok(Some(body));
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if is_absent_resource(status, &body) {
            return Ok(None);
        }
        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    pub async fn delete_bucket_policy(&self, bucket: &str) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("policy", "")]);
        let signed = self.sign_request("DELETE", &path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;

        let response = self
            .http_client
            .delete(format!(
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

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if is_absent_resource(status, &body) {
            return Ok(());
        }
        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    pub async fn put_bucket_lifecycle_configuration(
        &self,
        bucket: &str,
        configuration: &BucketLifecycleConfiguration,
    ) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() || configuration.rules.is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("lifecycle", "")]);
        let body = configuration.to_xml();
        let signed = self.sign_request(
            "PUT",
            &path,
            &query,
            &body,
            Some("application/xml"),
            ADMIN_SIGNING_SERVICE,
        )?;
        let host = self.host()?;
        let response = self
            .http_client
            .put(format!(
                "{}{}?{query}",
                self.base_url.trim_end_matches('/'),
                path
            ))
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.payload_hash)
            .header("authorization", &signed.authorization)
            .header("host", host)
            .header("content-type", "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|_| RustfsClientError::RequestFailed)?;

        if response.status().is_success() {
            return Ok(());
        }
        Err(RustfsClientError::unexpected_response(response).await)
    }

    pub async fn get_bucket_lifecycle_configuration(
        &self,
        bucket: &str,
    ) -> Result<Option<String>, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("lifecycle", "")]);
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

        if response.status().is_success() {
            let body = read_bucket_lifecycle_response(response).await?;
            if body.trim().is_empty() {
                return Ok(None);
            }
            return Ok(Some(body));
        }

        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if is_absent_resource(status, &body) {
            return Ok(None);
        }
        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }

    pub async fn delete_bucket_lifecycle_configuration(
        &self,
        bucket: &str,
    ) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("lifecycle", "")]);
        let signed = self.sign_request("DELETE", &path, &query, "", None, ADMIN_SIGNING_SERVICE)?;
        let host = self.host()?;
        let response = self
            .http_client
            .delete(format!(
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

        if response.status().is_success() {
            return Ok(());
        }
        let status = response.status();
        let (body, truncated) = RustfsClientError::limited_response_body(response).await;
        if is_absent_resource(status, &body) {
            return Ok(());
        }
        Err(RustfsClientError::unexpected_status_with_limited_body(
            status, &body, truncated,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_xml_is_stable_and_escapes_values() {
        let configuration = BucketLifecycleConfiguration {
            rules: vec![
                BucketLifecycleRule {
                    id: "z<&".to_string(),
                    status: BucketLifecycleRuleStatus::Enabled,
                    prefix: "logs/<".to_string(),
                    expiration: Some(BucketLifecycleExpiration { days: 30 }),
                    abort_incomplete_multipart_upload: None,
                },
                BucketLifecycleRule {
                    id: "a".to_string(),
                    status: BucketLifecycleRuleStatus::Disabled,
                    prefix: String::new(),
                    expiration: None,
                    abort_incomplete_multipart_upload: Some(
                        BucketLifecycleAbortIncompleteMultipartUpload {
                            days_after_initiation: 1,
                        },
                    ),
                },
            ],
        };

        let xml = configuration.to_xml();
        assert!(xml.find("<ID>a</ID>").unwrap() < xml.find("<ID>z&lt;&amp;</ID>").unwrap());
        assert!(xml.contains("<Filter></Filter><ID>a</ID>"));
        assert!(xml.contains("<Prefix>logs/&lt;</Prefix>"));
    }

    #[test]
    fn lifecycle_xml_matches_rustfs_get_response_shape() {
        let configuration = BucketLifecycleConfiguration {
            rules: vec![
                BucketLifecycleRule {
                    id: "expire-old-logs".to_string(),
                    status: BucketLifecycleRuleStatus::Enabled,
                    prefix: "logs/".to_string(),
                    expiration: Some(BucketLifecycleExpiration { days: 30 }),
                    abort_incomplete_multipart_upload: None,
                },
                BucketLifecycleRule {
                    id: "cleanup-incomplete-uploads".to_string(),
                    status: BucketLifecycleRuleStatus::Enabled,
                    prefix: String::new(),
                    expiration: None,
                    abort_incomplete_multipart_upload: Some(
                        BucketLifecycleAbortIncompleteMultipartUpload {
                            days_after_initiation: 1,
                        },
                    ),
                },
            ],
        };
        let rustfs_get_fixture = concat!(
            "<LifecycleConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
            "<Rule><AbortIncompleteMultipartUpload><DaysAfterInitiation>1</DaysAfterInitiation>",
            "</AbortIncompleteMultipartUpload><Filter></Filter>",
            "<ID>cleanup-incomplete-uploads</ID><Status>Enabled</Status></Rule>",
            "<Rule><Expiration><Days>30</Days></Expiration>",
            "<Filter><Prefix>logs/</Prefix></Filter><ID>expire-old-logs</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );

        assert_eq!(
            canonicalize_bucket_lifecycle_xml(&configuration.to_xml()).unwrap(),
            canonicalize_bucket_lifecycle_xml(rustfs_get_fixture).unwrap()
        );
    }

    #[test]
    fn lifecycle_canonicalization_normalizes_transport_formatting() {
        let compact = concat!(
            "<LifecycleConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
            "<Rule><Filter></Filter><ID>all</ID><Status>Enabled</Status></Rule>",
            "</LifecycleConfiguration>"
        );
        let formatted = concat!(
            "<?xml version=\"1.0\"?>\n",
            "<s3:LifecycleConfiguration xmlns:s3=\"http://s3.amazonaws.com/doc/2006-03-01/\">\n",
            "  <s3:Rule><s3:Filter/><s3:ID>all</s3:ID><s3:Status>Enabled</s3:Status></s3:Rule>\n",
            "</s3:LifecycleConfiguration>"
        );

        assert_eq!(
            canonicalize_bucket_lifecycle_xml(compact).unwrap(),
            canonicalize_bucket_lifecycle_xml(formatted).unwrap()
        );
    }

    #[test]
    fn lifecycle_canonicalization_preserves_unknown_actions() {
        let desired = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter><ID>all</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let live = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter><ID>all</ID>",
            "<Status>Enabled</Status><Transition><Days>7</Days>",
            "<StorageClass>WARM</StorageClass></Transition></Rule></LifecycleConfiguration>"
        );

        assert_ne!(
            canonicalize_bucket_lifecycle_xml(desired).unwrap(),
            canonicalize_bucket_lifecycle_xml(live).unwrap()
        );
    }

    #[test]
    fn lifecycle_canonicalization_rejects_malformed_or_wrong_root_xml() {
        assert!(canonicalize_bucket_lifecycle_xml("<Rule></Rule>").is_err());
        assert!(
            canonicalize_bucket_lifecycle_xml(
                "<LifecycleConfiguration><Rule></LifecycleConfiguration>",
            )
            .is_err()
        );
    }
}
