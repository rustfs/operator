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

use serde_json::{Map, Value};

use crate::sts::error::StsError;

const DEFAULT_POLICY_VERSION: &str = "2012-10-17";
pub const MAX_SESSION_POLICY_SIZE: usize = 2048;

#[derive(Debug, Clone)]
struct ParsedPolicy {
    version: String,
    statements: Vec<Value>,
}

/// Parse a policy payload and return a normalized compact policy string.
pub fn normalize_policy_for_merge(raw: &str) -> Result<String, StsError> {
    let policy = parse_policy(raw)?;

    let mut merged = Map::new();
    merged.insert("Version".to_string(), Value::String(policy.version));
    merged.insert("Statement".to_string(), Value::Array(policy.statements));

    serde_json::to_string(&Value::Object(merged)).map_err(|_| StsError::MalformedPolicyDocument)
}

/// Build the inline session policy from PolicyBinding policies.
pub fn merge_session_policies(
    request_policy: Option<&str>,
    binding_policies: &[String],
) -> Result<Option<String>, StsError> {
    if request_policy.is_some() {
        return Err(StsError::UnsupportedRequestPolicy);
    }

    let mut statements = Vec::<Value>::new();
    let mut version: Option<String> = None;

    for raw_policy in binding_policies {
        let policy = parse_policy(raw_policy)?;
        if version.is_none() {
            version = Some(policy.version);
        }
        statements.extend(policy.statements);
    }

    if statements.is_empty() {
        return Ok(None);
    }

    compact_policy(
        version.unwrap_or_else(|| DEFAULT_POLICY_VERSION.to_string()),
        statements,
    )
    .map(Some)
}

fn parse_policy(raw: &str) -> Result<ParsedPolicy, StsError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(StsError::MalformedPolicyDocument);
    }

    let raw_policy =
        serde_json::from_str::<Value>(raw).map_err(|_| StsError::MalformedPolicyDocument)?;
    let object = raw_policy
        .as_object()
        .ok_or(StsError::MalformedPolicyDocument)?;

    let version = object
        .get("Version")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_POLICY_VERSION)
        .to_string();

    let statement = object
        .get("Statement")
        .ok_or(StsError::MalformedPolicyDocument)?;
    let statements = match statement {
        Value::Array(values) => {
            if values.is_empty() {
                return Err(StsError::MalformedPolicyDocument);
            }
            values.clone()
        }
        Value::Object(object) => vec![Value::Object(object.clone())],
        _ => return Err(StsError::MalformedPolicyDocument),
    };

    Ok(ParsedPolicy {
        version,
        statements,
    })
}

fn compact_policy(version: String, statements: Vec<Value>) -> Result<String, StsError> {
    let mut merged = Map::new();
    merged.insert("Version".to_string(), Value::String(version));
    merged.insert("Statement".to_string(), Value::Array(statements));

    let compacted = serde_json::to_string(&Value::Object(merged))
        .map_err(|_| StsError::MalformedPolicyDocument)?;

    if compacted.len() > MAX_SESSION_POLICY_SIZE {
        return Err(StsError::PackedPolicyTooLarge);
    }

    Ok(compacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(statements: &[String]) -> String {
        let statement_lines = statements
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",");

        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement_lines}]}}")
    }

    fn allow_statement(sid: &str, action: &str, resource: &str) -> String {
        format!(
            "{{\"Sid\":\"{sid}\",\"Effect\":\"Allow\",\"Action\":\"{action}\",\"Resource\":\"{resource}\"}}"
        )
    }

    #[test]
    fn normalize_policy_for_merge_rejects_empty_policy() {
        assert!(matches!(
            normalize_policy_for_merge(""),
            Err(StsError::MalformedPolicyDocument)
        ));
    }

    #[test]
    fn normalize_policy_for_merge_rejects_malformed_json() {
        assert!(matches!(
            normalize_policy_for_merge("{\"Version\": \"2012-10-17\""),
            Err(StsError::MalformedPolicyDocument)
        ));
    }

    #[test]
    fn normalize_policy_for_merge_rejects_missing_statements() {
        let without_statements = "{\"Version\":\"2012-10-17\"}";
        assert!(matches!(
            normalize_policy_for_merge(without_statements),
            Err(StsError::MalformedPolicyDocument)
        ));
    }

    #[test]
    fn merge_binding_policies_without_request_keeps_compact_shape() {
        let binding_policy = policy(&[
            allow_statement("BindingRead", "s3:GetObject", "arn:aws:s3:::bucket-a/*"),
            allow_statement("BindingList", "s3:ListBucket", "arn:aws:s3:::bucket-a"),
        ]);

        let merged = merge_session_policies(None, &[binding_policy]).expect("merge should succeed");
        let merged = merged.expect("merged policy should exist");

        let value = serde_json::from_str::<Value>(&merged).expect("merged policy is json");
        assert_eq!(value["Version"], Value::String("2012-10-17".to_string()));
        let statements = value["Statement"]
            .as_array()
            .expect("merged policy should contain statement array");
        assert_eq!(statements.len(), 2);
        assert!(merged.len() <= MAX_SESSION_POLICY_SIZE);
    }

    #[test]
    fn merge_rejects_caller_supplied_request_policy() {
        let request_policy = policy(&[allow_statement(
            "RequestRead",
            "s3:GetObject",
            "arn:aws:s3:::bucket-a/*",
        )]);
        let binding_policy = policy(&[allow_statement(
            "BindingRead",
            "s3:GetObject",
            "arn:aws:s3:::bucket-a/*",
        )]);

        let error = merge_session_policies(Some(&request_policy), &[binding_policy])
            .expect_err("caller policy must not be accepted until subset evaluation exists");

        assert!(matches!(error, StsError::UnsupportedRequestPolicy));
    }

    #[test]
    fn merge_session_policy_returns_none_for_empty_inputs() {
        let merged = merge_session_policies(None, &[]).expect("merge should succeed");
        assert!(merged.is_none());
    }

    #[test]
    fn merge_rejects_request_policy_without_binding_upper_bound() {
        let request_policy = policy(&[allow_statement("RequestAll", "s3:*", "*")]);

        let error = merge_session_policies(Some(&request_policy), &[])
            .expect_err("request policy without a binding upper bound must be rejected");

        assert!(matches!(error, StsError::UnsupportedRequestPolicy));
    }

    #[test]
    fn merge_rejects_oversized_inline_policy() {
        let long_sid = "A".repeat(4000);
        let long_statement = policy(&[allow_statement(&long_sid, "s3:GetObject", "*")]);

        let err = merge_session_policies(None, &[long_statement])
            .expect_err("policy should be too large");
        assert!(matches!(err, StsError::PackedPolicyTooLarge));
    }

    #[test]
    fn normalize_policy_for_merge_rejects_empty_statement_array() {
        let no_statements = "{\"Version\":\"2012-10-17\",\"Statement\":[]}";
        assert!(matches!(
            normalize_policy_for_merge(no_statements),
            Err(StsError::MalformedPolicyDocument)
        ));
    }
}
