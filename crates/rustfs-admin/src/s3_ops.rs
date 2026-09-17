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
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesCData, BytesRef, BytesStart, BytesText, Event};
use reqwest::StatusCode;

const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";
const MAX_BUCKET_CONFIGURATION_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_BUCKET_LIFECYCLE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketVersioningState {
    Unversioned,
    Enabled,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketVersioningUpdate {
    Enabled,
    Suspended,
}

impl BucketVersioningUpdate {
    const fn as_s3_str(self) -> &'static str {
        match self {
            Self::Enabled => "Enabled",
            Self::Suspended => "Suspended",
        }
    }

    fn to_xml(self) -> String {
        let status = self.as_s3_str();
        format!(
            "<VersioningConfiguration xmlns=\"{S3_XML_NAMESPACE}\"><Status>{status}</Status></VersioningConfiguration>"
        )
    }
}

impl From<BucketVersioningUpdate> for BucketVersioningState {
    fn from(update: BucketVersioningUpdate) -> Self {
        match update {
            BucketVersioningUpdate::Enabled => Self::Enabled,
            BucketVersioningUpdate::Suspended => Self::Suspended,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketObjectLockRetentionMode {
    Governance,
    Compliance,
}

impl BucketObjectLockRetentionMode {
    const fn as_s3_str(self) -> &'static str {
        match self {
            Self::Governance => "GOVERNANCE",
            Self::Compliance => "COMPLIANCE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketObjectLockRetentionPeriod {
    Days(i32),
    Years(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketObjectLockDefaultRetention {
    pub mode: BucketObjectLockRetentionMode,
    pub period: BucketObjectLockRetentionPeriod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketObjectLockConfiguration {
    pub default_retention: Option<BucketObjectLockDefaultRetention>,
}

impl BucketObjectLockConfiguration {
    pub fn enabled_without_default_retention() -> Self {
        Self {
            default_retention: None,
        }
    }

    pub fn to_xml(&self) -> String {
        let mut xml = format!(
            "<ObjectLockConfiguration xmlns=\"{S3_XML_NAMESPACE}\"><ObjectLockEnabled>Enabled</ObjectLockEnabled>"
        );
        if let Some(retention) = self.default_retention {
            xml.push_str("<Rule><DefaultRetention><Mode>");
            xml.push_str(retention.mode.as_s3_str());
            xml.push_str("</Mode>");
            match retention.period {
                BucketObjectLockRetentionPeriod::Days(days) => {
                    xml.push_str("<Days>");
                    xml.push_str(&days.to_string());
                    xml.push_str("</Days>");
                }
                BucketObjectLockRetentionPeriod::Years(years) => {
                    xml.push_str("<Years>");
                    xml.push_str(&years.to_string());
                    xml.push_str("</Years>");
                }
            }
            xml.push_str("</DefaultRetention></Rule>");
        }
        xml.push_str("</ObjectLockConfiguration>");
        xml
    }
}

#[derive(Debug)]
struct StrictXmlElement {
    name: String,
    text: String,
    children: Vec<StrictXmlElement>,
}

impl StrictXmlElement {
    fn child(&self, name: &str) -> Result<Option<&Self>, ()> {
        let mut matches = self.children.iter().filter(|child| child.name == name);
        let child = matches.next();
        if matches.next().is_some() {
            return Err(());
        }
        Ok(child)
    }

    fn validate_children(&self, allowed: &[&str]) -> Result<(), ()> {
        if self.text.trim().is_empty()
            && self
                .children
                .iter()
                .all(|child| allowed.contains(&child.name.as_str()))
        {
            Ok(())
        } else {
            Err(())
        }
    }

    fn leaf_text(&self) -> Result<&str, ()> {
        if self.children.is_empty() && !self.text.trim().is_empty() {
            Ok(self.text.trim())
        } else {
            Err(())
        }
    }
}

fn parse_bucket_versioning_xml(xml: &str) -> Result<BucketVersioningState, RustfsClientError> {
    let root = parse_strict_configuration_xml(xml, "VersioningConfiguration")
        .map_err(|()| RustfsClientError::InvalidBucketVersioningResponse)?;
    root.validate_children(&["Status"])
        .map_err(|()| RustfsClientError::InvalidBucketVersioningResponse)?;
    let Some(status) = root
        .child("Status")
        .map_err(|()| RustfsClientError::InvalidBucketVersioningResponse)?
    else {
        return Ok(BucketVersioningState::Unversioned);
    };
    match status
        .leaf_text()
        .map_err(|()| RustfsClientError::InvalidBucketVersioningResponse)?
    {
        "Enabled" => Ok(BucketVersioningState::Enabled),
        "Suspended" => Ok(BucketVersioningState::Suspended),
        _ => Err(RustfsClientError::InvalidBucketVersioningResponse),
    }
}

fn parse_bucket_object_lock_xml(
    xml: &str,
) -> Result<BucketObjectLockConfiguration, RustfsClientError> {
    let invalid = || RustfsClientError::InvalidObjectLockConfigurationResponse;
    let root =
        parse_strict_configuration_xml(xml, "ObjectLockConfiguration").map_err(|()| invalid())?;
    root.validate_children(&["ObjectLockEnabled", "Rule"])
        .map_err(|()| invalid())?;
    let enabled = root
        .child("ObjectLockEnabled")
        .map_err(|()| invalid())?
        .ok_or_else(invalid)?;
    if enabled.leaf_text().map_err(|()| invalid())? != "Enabled" {
        return Err(invalid());
    }

    let Some(rule) = root.child("Rule").map_err(|()| invalid())? else {
        return Ok(BucketObjectLockConfiguration::enabled_without_default_retention());
    };
    rule.validate_children(&["DefaultRetention"])
        .map_err(|()| invalid())?;
    let retention = rule
        .child("DefaultRetention")
        .map_err(|()| invalid())?
        .ok_or_else(invalid)?;
    retention
        .validate_children(&["Mode", "Days", "Years"])
        .map_err(|()| invalid())?;

    let mode = match retention
        .child("Mode")
        .map_err(|()| invalid())?
        .ok_or_else(invalid)?
        .leaf_text()
        .map_err(|()| invalid())?
    {
        "GOVERNANCE" => BucketObjectLockRetentionMode::Governance,
        "COMPLIANCE" => BucketObjectLockRetentionMode::Compliance,
        _ => return Err(invalid()),
    };
    let days = retention.child("Days").map_err(|()| invalid())?;
    let years = retention.child("Years").map_err(|()| invalid())?;
    let period = match (days, years) {
        (Some(days), None) => BucketObjectLockRetentionPeriod::Days(
            parse_positive_i32(days.leaf_text().map_err(|()| invalid())?).ok_or_else(invalid)?,
        ),
        (None, Some(years)) => BucketObjectLockRetentionPeriod::Years(
            parse_positive_i32(years.leaf_text().map_err(|()| invalid())?).ok_or_else(invalid)?,
        ),
        _ => return Err(invalid()),
    };

    Ok(BucketObjectLockConfiguration {
        default_retention: Some(BucketObjectLockDefaultRetention { mode, period }),
    })
}

fn parse_positive_i32(value: &str) -> Option<i32> {
    value.parse::<i32>().ok().filter(|value| *value > 0)
}

fn parse_strict_configuration_xml(xml: &str, expected_root: &str) -> Result<StrictXmlElement, ()> {
    let mut reader = Reader::from_str(xml);
    let mut stack = Vec::<StrictXmlElement>::new();
    let mut root = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                validate_configuration_attributes(&element)?;
                stack.push(StrictXmlElement {
                    name: configuration_element_local_name(element.name().as_ref())?,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(element)) => {
                validate_configuration_attributes(&element)?;
                attach_configuration_element(
                    &mut stack,
                    &mut root,
                    StrictXmlElement {
                        name: configuration_element_local_name(element.name().as_ref())?,
                        text: String::new(),
                        children: Vec::new(),
                    },
                )?;
            }
            Ok(Event::End(element)) => {
                let current = stack.pop().ok_or(())?;
                if current.name != configuration_element_local_name(element.name().as_ref())? {
                    return Err(());
                }
                attach_configuration_element(&mut stack, &mut root, current)?;
            }
            Ok(Event::Text(text)) => {
                let value = text.xml10_content().map_err(|_| ())?;
                append_configuration_text(&mut stack, &value)?;
            }
            Ok(Event::CData(text)) => {
                let value = text.xml10_content().map_err(|_| ())?;
                append_configuration_text(&mut stack, &value)?;
            }
            Ok(Event::GeneralRef(_)) | Ok(Event::DocType(_)) => return Err(()),
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::Eof) => break,
            Err(_) => return Err(()),
        }
    }

    if !stack.is_empty() {
        return Err(());
    }
    let root = root.ok_or(())?;
    if root.name == expected_root {
        Ok(root)
    } else {
        Err(())
    }
}

fn append_configuration_text(stack: &mut [StrictXmlElement], value: &str) -> Result<(), ()> {
    let Some(current) = stack.last_mut() else {
        return if value.trim().is_empty() {
            Ok(())
        } else {
            Err(())
        };
    };
    current.text.push_str(value);
    Ok(())
}

fn attach_configuration_element(
    stack: &mut [StrictXmlElement],
    root: &mut Option<StrictXmlElement>,
    element: StrictXmlElement,
) -> Result<(), ()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(element);
        return Ok(());
    }
    if root.is_some() {
        return Err(());
    }
    *root = Some(element);
    Ok(())
}

fn validate_configuration_attributes(element: &BytesStart<'_>) -> Result<(), ()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| ())?;
        let name = attribute.key.as_ref();
        if name != b"xmlns" && !name.starts_with(b"xmlns:") {
            return Err(());
        }
    }
    Ok(())
}

fn configuration_element_local_name(name: &[u8]) -> Result<String, ()> {
    let name = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    std::str::from_utf8(name)
        .map(str::to_string)
        .map_err(|_| ())
}

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
    let mut elements = Vec::<CanonicalElement>::new();
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
                } else if let Some(parent) = elements.last_mut() {
                    parent.start_child(&mut canonical);
                }
                append_start_element(&mut canonical, &element, &reader)?;
                elements.push(CanonicalElement::new(name));
            }
            Ok(Event::Empty(element)) => {
                let name = element_local_name(element.name().as_ref())?;
                if elements.is_empty() {
                    if saw_root || name != "LifecycleConfiguration" {
                        return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                    }
                    saw_root = true;
                } else if let Some(parent) = elements.last_mut() {
                    parent.start_child(&mut canonical);
                }
                append_start_element(&mut canonical, &element, &reader)?;
                canonical.push_str("</");
                canonical.push_str(&name);
                canonical.push('>');
            }
            Ok(Event::End(element)) => {
                let name = element_local_name(element.name().as_ref())?;
                let Some(mut current) = elements.pop() else {
                    return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                };
                if current.name != name {
                    return Err(RustfsClientError::InvalidLifecycleConfigurationResponse);
                }
                current.finish(&mut canonical);
                canonical.push_str("</");
                canonical.push_str(&name);
                canonical.push('>');
            }
            Ok(Event::Text(text)) => {
                append_canonical_text(
                    &mut canonical,
                    elements.last_mut(),
                    decode_text(&text)?,
                    false,
                )?;
            }
            Ok(Event::CData(text)) => {
                append_canonical_text(
                    &mut canonical,
                    elements.last_mut(),
                    decode_cdata(&text)?,
                    true,
                )?;
            }
            Ok(Event::GeneralRef(reference)) => {
                append_canonical_text(
                    &mut canonical,
                    elements.last_mut(),
                    decode_reference(&reference)?,
                    true,
                )?;
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::PI(_)) => {}
            Ok(Event::DocType(_)) => {
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

struct CanonicalElement {
    name: String,
    has_child: bool,
    has_text: bool,
    pending_whitespace: String,
}

impl CanonicalElement {
    fn new(name: String) -> Self {
        Self {
            name,
            has_child: false,
            has_text: false,
            pending_whitespace: String::new(),
        }
    }

    fn start_child(&mut self, canonical: &mut String) {
        self.has_child = true;
        if self.has_text {
            canonical.push_str(&self.pending_whitespace);
        }
        self.pending_whitespace.clear();
    }

    fn finish(&mut self, canonical: &mut String) {
        if !self.has_child || self.has_text {
            canonical.push_str(&self.pending_whitespace);
        }
    }
}

fn append_canonical_text(
    canonical: &mut String,
    element: Option<&mut CanonicalElement>,
    text: String,
    explicit_text: bool,
) -> Result<(), RustfsClientError> {
    let Some(element) = element else {
        return if !explicit_text && is_xml_whitespace(&text) {
            Ok(())
        } else {
            Err(RustfsClientError::InvalidLifecycleConfigurationResponse)
        };
    };

    let is_whitespace = is_xml_whitespace(&text);
    let text = escape_xml(&text);
    if is_whitespace && !explicit_text {
        element.pending_whitespace.push_str(&text);
    } else {
        canonical.push_str(&element.pending_whitespace);
        element.pending_whitespace.clear();
        canonical.push_str(&text);
        element.has_text = true;
    }
    Ok(())
}

fn is_xml_whitespace(text: &str) -> bool {
    text.chars()
        .all(|character| matches!(character, ' ' | '\t' | '\r' | '\n'))
}

fn decode_text(text: &BytesText<'_>) -> Result<String, RustfsClientError> {
    text.xml10_content()
        .map(|text| text.into_owned())
        .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)
}

fn decode_cdata(text: &BytesCData<'_>) -> Result<String, RustfsClientError> {
    text.xml10_content()
        .map(|text| text.into_owned())
        .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)
}

fn decode_reference(reference: &BytesRef<'_>) -> Result<String, RustfsClientError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?
    {
        return Ok(character.to_string());
    }

    let name = reference
        .xml10_content()
        .map_err(|_| RustfsClientError::InvalidLifecycleConfigurationResponse)?;
    resolve_xml_entity(&name)
        .map(str::to_string)
        .ok_or(RustfsClientError::InvalidLifecycleConfigurationResponse)
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

async fn read_bucket_configuration_response(
    mut response: reqwest::Response,
    invalid: fn() -> RustfsClientError,
) -> Result<String, RustfsClientError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RustfsClientError::RequestFailed)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BUCKET_CONFIGURATION_RESPONSE_BYTES {
            return Err(invalid());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| invalid())
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

    pub async fn get_bucket_versioning(
        &self,
        bucket: &str,
    ) -> Result<BucketVersioningState, RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("versioning", "")]);
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
            return Err(RustfsClientError::unexpected_response(response).await);
        }
        let body = read_bucket_configuration_response(response, || {
            RustfsClientError::InvalidBucketVersioningResponse
        })
        .await?;
        parse_bucket_versioning_xml(&body)
    }

    pub async fn put_bucket_versioning(
        &self,
        bucket: &str,
        update: BucketVersioningUpdate,
    ) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("versioning", "")]);
        let body = update.to_xml();
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
            Ok(())
        } else {
            Err(RustfsClientError::unexpected_response(response).await)
        }
    }

    pub async fn get_bucket_object_lock_configuration(
        &self,
        bucket: &str,
    ) -> Result<Option<BucketObjectLockConfiguration>, RustfsClientError> {
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

        if response.status().is_success() {
            let body = read_bucket_configuration_response(response, || {
                RustfsClientError::InvalidObjectLockConfigurationResponse
            })
            .await?;
            return parse_bucket_object_lock_xml(&body).map(Some);
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

    pub async fn put_bucket_object_lock_configuration(
        &self,
        bucket: &str,
        configuration: &BucketObjectLockConfiguration,
    ) -> Result<(), RustfsClientError> {
        if bucket.trim().is_empty() {
            return Err(RustfsClientError::RequestBuildFailed);
        }

        let path = format!("/{bucket}");
        let query = build_canonical_query(&[("object-lock", "")]);
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
            Ok(())
        } else {
            Err(RustfsClientError::unexpected_response(response).await)
        }
    }

    pub async fn bucket_object_lock_enabled(
        &self,
        bucket: &str,
    ) -> Result<bool, RustfsClientError> {
        self.get_bucket_object_lock_configuration(bucket)
            .await
            .map(|configuration| configuration.is_some())
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
    fn bucket_versioning_xml_parses_all_s3_states() {
        assert_eq!(
            BucketVersioningUpdate::Enabled.to_xml(),
            "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Status>Enabled</Status></VersioningConfiguration>"
        );
        assert_eq!(
            BucketVersioningUpdate::Suspended.to_xml(),
            "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Status>Suspended</Status></VersioningConfiguration>"
        );
        assert_eq!(
            parse_bucket_versioning_xml("<VersioningConfiguration/>").unwrap(),
            BucketVersioningState::Unversioned
        );
        assert_eq!(
            parse_bucket_versioning_xml(
                "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Status>Enabled</Status></VersioningConfiguration>",
            )
            .unwrap(),
            BucketVersioningState::Enabled
        );
        assert_eq!(
            parse_bucket_versioning_xml(
                "<s3:VersioningConfiguration xmlns:s3=\"http://s3.amazonaws.com/doc/2006-03-01/\"><s3:Status>Suspended</s3:Status></s3:VersioningConfiguration>",
            )
            .unwrap(),
            BucketVersioningState::Suspended
        );
    }

    #[test]
    fn bucket_versioning_xml_rejects_unknown_or_duplicate_fields() {
        assert!(matches!(
            parse_bucket_versioning_xml(
                "<VersioningConfiguration><Status>Disabled</Status></VersioningConfiguration>",
            ),
            Err(RustfsClientError::InvalidBucketVersioningResponse)
        ));
        assert!(matches!(
            parse_bucket_versioning_xml(
                "<VersioningConfiguration><Status>Enabled</Status><Status>Enabled</Status></VersioningConfiguration>",
            ),
            Err(RustfsClientError::InvalidBucketVersioningResponse)
        ));
        assert!(matches!(
            parse_bucket_versioning_xml(
                "<VersioningConfiguration><Unknown/></VersioningConfiguration>",
            ),
            Err(RustfsClientError::InvalidBucketVersioningResponse)
        ));
        assert!(matches!(
            parse_bucket_versioning_xml(
                "<VersioningConfiguration source=\"untrusted\"><Status>Enabled</Status></VersioningConfiguration>",
            ),
            Err(RustfsClientError::InvalidBucketVersioningResponse)
        ));
    }

    #[test]
    fn object_lock_xml_round_trips_days_and_enabled_only() {
        for (mode, s3_mode) in [
            (BucketObjectLockRetentionMode::Governance, "GOVERNANCE"),
            (BucketObjectLockRetentionMode::Compliance, "COMPLIANCE"),
        ] {
            let configuration = BucketObjectLockConfiguration {
                default_retention: Some(BucketObjectLockDefaultRetention {
                    mode,
                    period: BucketObjectLockRetentionPeriod::Days(30),
                }),
            };
            assert_eq!(
                configuration.to_xml(),
                format!(
                    "<ObjectLockConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>{s3_mode}</Mode><Days>30</Days></DefaultRetention></Rule></ObjectLockConfiguration>"
                )
            );
            let parsed = parse_bucket_object_lock_xml(&configuration.to_xml()).unwrap();
            assert_eq!(parsed, configuration);
        }

        let enabled_only = BucketObjectLockConfiguration::enabled_without_default_retention();
        assert_eq!(
            parse_bucket_object_lock_xml(&enabled_only.to_xml()).unwrap(),
            enabled_only
        );
    }

    #[test]
    fn object_lock_xml_accepts_years_but_rejects_ambiguous_retention() {
        let years = "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>GOVERNANCE</Mode><Years>2</Years></DefaultRetention></Rule></ObjectLockConfiguration>";
        assert_eq!(
            parse_bucket_object_lock_xml(years).unwrap(),
            BucketObjectLockConfiguration {
                default_retention: Some(BucketObjectLockDefaultRetention {
                    mode: BucketObjectLockRetentionMode::Governance,
                    period: BucketObjectLockRetentionPeriod::Years(2),
                }),
            }
        );

        let ambiguous = "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>GOVERNANCE</Mode><Days>30</Days><Years>1</Years></DefaultRetention></Rule></ObjectLockConfiguration>";
        assert!(matches!(
            parse_bucket_object_lock_xml(ambiguous),
            Err(RustfsClientError::InvalidObjectLockConfigurationResponse)
        ));

        let unknown = "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Unknown/></ObjectLockConfiguration>";
        assert!(matches!(
            parse_bucket_object_lock_xml(unknown),
            Err(RustfsClientError::InvalidObjectLockConfigurationResponse)
        ));
    }

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
    fn lifecycle_canonicalization_preserves_leaf_whitespace_and_decodes_text() {
        let prefix_with_one_space = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix> </Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let prefix_with_two_spaces = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix>  </Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let escaped_text = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix>logs/&apos;</Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let literal_text = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix>logs/'</Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let cdata_text = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix><![CDATA[logs/']]></Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let numeric_reference = concat!(
            "<LifecycleConfiguration><Rule><Filter><Prefix>logs/&#39;</Prefix></Filter>",
            "<ID>all</ID><Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let non_xml_whitespace = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter>\u{a0}<ID>all</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let no_interstitial_text = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter><ID>all</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let explicit_whitespace = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter><![CDATA[ ]]><ID>all</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );
        let referenced_whitespace = concat!(
            "<LifecycleConfiguration><Rule><Filter></Filter>&#32;<ID>all</ID>",
            "<Status>Enabled</Status></Rule></LifecycleConfiguration>"
        );

        assert_ne!(
            canonicalize_bucket_lifecycle_xml(prefix_with_one_space).unwrap(),
            canonicalize_bucket_lifecycle_xml(prefix_with_two_spaces).unwrap()
        );
        assert_eq!(
            canonicalize_bucket_lifecycle_xml(escaped_text).unwrap(),
            canonicalize_bucket_lifecycle_xml(literal_text).unwrap()
        );
        assert_eq!(
            canonicalize_bucket_lifecycle_xml(literal_text).unwrap(),
            canonicalize_bucket_lifecycle_xml(cdata_text).unwrap()
        );
        assert_eq!(
            canonicalize_bucket_lifecycle_xml(literal_text).unwrap(),
            canonicalize_bucket_lifecycle_xml(numeric_reference).unwrap()
        );
        assert_ne!(
            canonicalize_bucket_lifecycle_xml(non_xml_whitespace).unwrap(),
            canonicalize_bucket_lifecycle_xml(no_interstitial_text).unwrap()
        );
        assert_ne!(
            canonicalize_bucket_lifecycle_xml(explicit_whitespace).unwrap(),
            canonicalize_bucket_lifecycle_xml(no_interstitial_text).unwrap()
        );
        assert_eq!(
            canonicalize_bucket_lifecycle_xml(explicit_whitespace).unwrap(),
            canonicalize_bucket_lifecycle_xml(referenced_whitespace).unwrap()
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
