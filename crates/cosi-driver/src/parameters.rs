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

//! BucketClass / BucketAccessClass parameter parsing (Rook-style).

use std::collections::HashMap;

use snafu::Snafu;

use crate::policy::AccessPolicy;

pub const PARAM_SECRET_NAME: &str = "objectStoreUserSecretName";
pub const PARAM_SECRET_NAMESPACE: &str = "objectStoreUserSecretNamespace";
pub const PARAM_ENDPOINT: &str = "endpoint";
pub const PARAM_REGION: &str = "region";
pub const PARAM_TLS_CA_CM_NAME: &str = "tlsCAConfigMapName";
pub const PARAM_TLS_CA_CM_NAMESPACE: &str = "tlsCAConfigMapNamespace";
pub const PARAM_POLICY: &str = "policy";

#[derive(Debug, Snafu)]
pub enum ParameterError {
    #[snafu(display("missing required parameter `{name}`"))]
    Missing { name: &'static str },
    #[snafu(display("parameter `{name}` must not be empty"))]
    Empty { name: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendParameters {
    pub secret_name: String,
    pub secret_namespace: String,
    pub endpoint: String,
    pub region: Option<String>,
    pub tls_ca_configmap_name: Option<String>,
    pub tls_ca_configmap_namespace: Option<String>,
    pub access_policy: AccessPolicy,
}

impl BackendParameters {
    pub fn from_map(params: &HashMap<String, String>) -> Result<Self, ParameterError> {
        Ok(Self {
            secret_name: required(params, PARAM_SECRET_NAME)?,
            secret_namespace: required(params, PARAM_SECRET_NAMESPACE)?,
            endpoint: required(params, PARAM_ENDPOINT)?,
            region: optional(params, PARAM_REGION),
            tls_ca_configmap_name: optional(params, PARAM_TLS_CA_CM_NAME),
            tls_ca_configmap_namespace: optional(params, PARAM_TLS_CA_CM_NAMESPACE),
            access_policy: AccessPolicy::parse(params.get(PARAM_POLICY).map(String::as_str)),
        })
    }
}

fn required(
    params: &HashMap<String, String>,
    name: &'static str,
) -> Result<String, ParameterError> {
    let value = params
        .get(name)
        .ok_or(ParameterError::Missing { name })?
        .trim();
    if value.is_empty() {
        return Err(ParameterError::Empty { name });
    }
    Ok(value.to_string())
}

fn optional(params: &HashMap<String, String>, name: &str) -> Option<String> {
    params
        .get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_parameters() {
        let mut map = HashMap::new();
        map.insert(PARAM_SECRET_NAME.to_string(), "creds".into());
        map.insert(PARAM_SECRET_NAMESPACE.to_string(), "ns".into());
        map.insert(
            PARAM_ENDPOINT.to_string(),
            "http://tenant-io.ns.svc:9000".into(),
        );
        map.insert(PARAM_POLICY.to_string(), "readonly".into());

        let parsed = BackendParameters::from_map(&map).expect("parse");
        assert_eq!(parsed.secret_name, "creds");
        assert_eq!(parsed.access_policy, AccessPolicy::Readonly);
        assert_eq!(parsed.endpoint, "http://tenant-io.ns.svc:9000");
    }

    #[test]
    fn rejects_missing_endpoint() {
        let mut map = HashMap::new();
        map.insert(PARAM_SECRET_NAME.to_string(), "creds".into());
        map.insert(PARAM_SECRET_NAMESPACE.to_string(), "ns".into());
        let err = BackendParameters::from_map(&map).expect_err("missing endpoint");
        assert!(matches!(
            err,
            ParameterError::Missing {
                name: PARAM_ENDPOINT
            }
        ));
    }
}
