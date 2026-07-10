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

/// AWS STS XML namespace for Operator-facing payloads.
pub const STS_XML_NAMESPACE: &str = "https://sts.amazonaws.com/doc/2011-06-15/";

/// Placeholder request id for deterministic test payloads.
pub const STS_REQUEST_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Validation and stub errors for STS request handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StsError {
    MissingParameter { parameter: &'static str },
    InvalidParameterValue { parameter: &'static str },
    InvalidIdentityToken,
    AccessDenied,
    UnsupportedRequestPolicy,
    TenantTlsClientCertificateUnsupported,
    InternalError,
    NotImplemented,
    MalformedPolicyDocument,
    PackedPolicyTooLarge,
}

impl StsError {
    /// STS-style short error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingParameter { .. } => "MissingParameter",
            Self::InvalidParameterValue { .. } => "InvalidParameterValue",
            Self::InvalidIdentityToken => "InvalidIdentityToken",
            Self::AccessDenied | Self::UnsupportedRequestPolicy => "AccessDenied",
            Self::TenantTlsClientCertificateUnsupported => "TenantTlsClientCertificateUnsupported",
            Self::InternalError => "InternalError",
            Self::NotImplemented => "NotImplemented",
            Self::MalformedPolicyDocument => "MalformedPolicyDocument",
            Self::PackedPolicyTooLarge => "PackedPolicyTooLarge",
        }
    }

    /// STS-style error message.
    pub fn message(&self) -> String {
        match self {
            Self::MissingParameter { parameter } => {
                format!("Missing required request body parameter {}.", parameter)
            }
            Self::InvalidParameterValue { parameter } => {
                format!("Invalid value for parameter {}.", parameter)
            }
            Self::InvalidIdentityToken => "The provided web identity token is invalid.".to_string(),
            Self::AccessDenied => {
                "The request is not authorized by a matching PolicyBinding.".to_string()
            }
            Self::UnsupportedRequestPolicy => {
                "Caller-supplied Policy request parameters are not supported by Operator STS."
                    .to_string()
            }
            Self::TenantTlsClientCertificateUnsupported => {
                "Operator STS does not support Tenants that require TLS client certificates."
                    .to_string()
            }
            Self::InternalError => "Internal server error.".to_string(),
            Self::NotImplemented => "This operation is not yet implemented.".to_string(),
            Self::MalformedPolicyDocument => "The policy document is malformed.".to_string(),
            Self::PackedPolicyTooLarge => {
                "The session policy is too long and must be less than or equal to 2048 bytes."
                    .to_string()
            }
        }
    }

    /// Render a full AWS STS error response document.
    pub fn as_xml(&self) -> String {
        render_sts_error_xml(self.code(), &self.message())
    }
}

/// Escape XML character data and attributes.
pub fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 16);
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub fn render_sts_error_xml(code: &str, message: &str) -> String {
    let message = escape_xml(message);
    let request_id = STS_REQUEST_ID;

    format!(
        "<ErrorResponse xmlns=\"{ns}\"><Error><Type>Sender</Type><Code>{code}</Code><Message>{message}</Message></Error><RequestId>{request_id}</RequestId></ErrorResponse>",
        ns = STS_XML_NAMESPACE,
    )
}
