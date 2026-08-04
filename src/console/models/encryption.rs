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

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::types::v1alpha1::security_context::{
    PodSecurityContextOverride, effective_run_as_non_root,
    security_context_pair_delegates_to_platform,
};
use k8s_openapi::api::core::v1 as corev1;

/// GET response – current encryption configuration for a Tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionInfoResponse {
    pub enabled: bool,
    pub backend: String,
    pub vault: Option<VaultInfo>,
    pub local: Option<LocalInfo>,
    pub kms_secret_name: Option<String>,
    pub default_key_id: Option<String>,
    pub security_context: Option<SecurityContextInfo>,
}

/// Vault endpoint only (token lives in `kmsSecret`).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub endpoint: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInfo {
    pub key_directory: Option<String>,
    pub master_key_secret_ref: Option<LocalMasterKeySecretRefInfo>,
    pub allow_insecure_dev_defaults: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalMasterKeySecretRefInfo {
    pub name: String,
    pub key: String,
}

/// Legacy Console form subset of the Tenant Pod security context.
///
/// The raw YAML editor is authoritative for seccomp, container, and Pool-level settings.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityContextInfo {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
    /// Effective value after applying Operator defaults, or unknown when platform admission owns it.
    #[schema(value_type = Option<bool>, nullable, required = false)]
    pub effective_run_as_non_root: Option<bool>,
    /// Whether an exact empty pair delegates unspecified values to platform admission.
    pub operator_defaults_delegated: bool,
}

impl SecurityContextInfo {
    pub(crate) fn from_contexts(
        pod: Option<&PodSecurityContextOverride>,
        container: Option<&corev1::SecurityContext>,
    ) -> Self {
        let run_as_user = pod.and_then(|context| context.run_as_user);
        let run_as_non_root = pod.and_then(|context| context.run_as_non_root);
        let operator_defaults_delegated =
            security_context_pair_delegates_to_platform(pod, container);

        Self {
            run_as_user,
            run_as_group: pod.and_then(|context| context.run_as_group),
            fs_group: pod.and_then(|context| context.fs_group),
            run_as_non_root,
            effective_run_as_non_root: (!operator_defaults_delegated)
                .then(|| effective_run_as_non_root(run_as_user, run_as_non_root)),
            operator_defaults_delegated,
        }
    }
}

/// PUT request – update encryption configuration.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateEncryptionRequest {
    pub enabled: bool,
    pub backend: Option<UpdateEncryptionBackend>,
    pub vault: Option<UpdateVaultRequest>,
    pub local: Option<UpdateLocalRequest>,
    pub kms_secret_name: Option<String>,
    pub default_key_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UpdateEncryptionBackend {
    Local,
    Vault,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateVaultRequest {
    pub endpoint: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateLocalRequest {
    pub key_directory: Option<String>,
    pub master_key_secret_ref: Option<LocalMasterKeySecretRefInfo>,
    pub allow_insecure_dev_defaults: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSecurityContextRequest {
    /// Omit to preserve the current value, send `null` to clear it, or send an integer to set it.
    #[serde(default)]
    #[schema(value_type = Option<i64>, nullable, required = false)]
    pub run_as_user: PatchField<i64>,
    /// Omit to preserve the current value, send `null` to clear it, or send an integer to set it.
    #[serde(default)]
    #[schema(value_type = Option<i64>, nullable, required = false)]
    pub run_as_group: PatchField<i64>,
    /// Omit to preserve the current value, send `null` to clear it, or send an integer to set it.
    #[serde(default)]
    #[schema(value_type = Option<i64>, nullable, required = false)]
    pub fs_group: PatchField<i64>,
    /// Omit to preserve the current value, send `null` to restore the Operator default, or send a boolean to set it.
    #[serde(default)]
    #[schema(value_type = Option<bool>, nullable, required = false)]
    pub run_as_non_root: PatchField<bool>,
}

/// A JSON Merge Patch field with distinct missing, null, and concrete-value states.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum PatchField<T> {
    /// The property was omitted and must not be modified.
    #[default]
    Missing,
    /// The property was explicitly set to JSON null and must be removed.
    Null,
    /// The property was explicitly assigned a value.
    Value(T),
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Option::<T>::deserialize(deserializer)?
            .map(Self::Value)
            .unwrap_or(Self::Null))
    }
}

/// SecurityContext update result.
#[derive(Debug, Serialize, ToSchema)]
pub struct SecurityContextUpdateResponse {
    pub success: bool,
    pub message: String,
}

/// Generic success response.
#[derive(Debug, Serialize, ToSchema)]
pub struct EncryptionUpdateResponse {
    pub success: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::{
        PatchField, SecurityContextInfo, UpdateEncryptionRequest, UpdateSecurityContextRequest,
    };
    use crate::types::v1alpha1::security_context::PodSecurityContextOverride;
    use k8s_openapi::api::core::v1 as corev1;

    #[test]
    fn security_context_info_keeps_raw_value_and_serializes_effective_value() {
        let context = PodSecurityContextOverride {
            run_as_user: Some(0),
            run_as_non_root: None,
            ..Default::default()
        };

        let info = SecurityContextInfo::from_contexts(Some(&context), None);
        let json = serde_json::to_value(&info).expect("SecurityContextInfo should serialize");

        assert_eq!(info.run_as_non_root, None);
        assert_eq!(info.effective_run_as_non_root, Some(false));
        assert!(!info.operator_defaults_delegated);
        assert!(json["runAsNonRoot"].is_null());
        assert_eq!(json["effectiveRunAsNonRoot"], serde_json::json!(false));
        assert_eq!(json["operatorDefaultsDelegated"], serde_json::json!(false));
    }

    #[test]
    fn delegated_security_context_serializes_an_unknown_effective_value() {
        let pod = PodSecurityContextOverride::default();
        let container = corev1::SecurityContext::default();

        let info = SecurityContextInfo::from_contexts(Some(&pod), Some(&container));
        let json = serde_json::to_value(&info).expect("SecurityContextInfo should serialize");

        assert_eq!(info.effective_run_as_non_root, None);
        assert!(info.operator_defaults_delegated);
        assert!(json["effectiveRunAsNonRoot"].is_null());
        assert_eq!(json["operatorDefaultsDelegated"], serde_json::json!(true));
    }

    #[test]
    fn security_context_update_distinguishes_missing_null_and_value() {
        let request: UpdateSecurityContextRequest = serde_json::from_value(serde_json::json!({
            "runAsUser": null,
            "runAsGroup": 20_001,
            "runAsNonRoot": false
        }))
        .expect("security context update should deserialize");

        assert_eq!(request.run_as_user, PatchField::Null);
        assert_eq!(request.run_as_group, PatchField::Value(20_001));
        assert_eq!(request.fs_group, PatchField::Missing);
        assert_eq!(request.run_as_non_root, PatchField::Value(false));
    }

    #[test]
    fn security_context_update_preserves_all_omitted_fields() {
        let request: UpdateSecurityContextRequest = serde_json::from_value(serde_json::json!({}))
            .expect("empty security context update should deserialize");

        assert_eq!(request.run_as_user, PatchField::Missing);
        assert_eq!(request.run_as_group, PatchField::Missing);
        assert_eq!(request.fs_group, PatchField::Missing);
        assert_eq!(request.run_as_non_root, PatchField::Missing);
    }

    #[test]
    fn security_context_update_rejects_unknown_fields() {
        let result: Result<UpdateSecurityContextRequest, _> =
            serde_json::from_value(serde_json::json!({ "runAsUsr": 20_001 }));

        let error = match result {
            Ok(_) => panic!("unknown security context fields must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown field `runAsUsr`"));
    }

    #[test]
    fn encryption_update_accepts_supported_wire_contracts() {
        for value in [
            serde_json::json!({ "enabled": false }),
            serde_json::json!({
                "enabled": true,
                "backend": "vault",
                "vault": { "endpoint": "https://vault.example.com" },
                "kmsSecretName": "vault-token",
                "defaultKeyId": "tenant-key"
            }),
            serde_json::json!({
                "enabled": true,
                "backend": "local",
                "local": {
                    "keyDirectory": "/var/lib/rustfs/kms",
                    "masterKeySecretRef": { "name": "local-kms", "key": "master-key" },
                    "allowInsecureDevDefaults": false
                }
            }),
        ] {
            serde_json::from_value::<UpdateEncryptionRequest>(value)
                .expect("supported encryption request should deserialize");
        }
    }

    #[test]
    fn encryption_update_rejects_unknown_backends_and_fields() {
        for value in [
            serde_json::json!({ "enabled": true, "backend": "valut" }),
            serde_json::json!({ "enabled": true, "enabeld": true }),
            serde_json::json!({
                "enabled": true,
                "backend": "vault",
                "vault": { "endpoint": "https://vault.example.com", "endpont": "typo" }
            }),
            serde_json::json!({
                "enabled": true,
                "backend": "local",
                "local": { "allowInsecureDevDefault": true }
            }),
        ] {
            serde_json::from_value::<UpdateEncryptionRequest>(value)
                .expect_err("unknown encryption values and fields must be rejected");
        }
    }
}
