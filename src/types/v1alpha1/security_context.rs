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

use k8s_openapi::api::core::v1 as corev1;
use kube::KubeSchema;
use serde::{Deserialize, Serialize};

/// Largest UID/GID accepted by Kubernetes workload security contexts.
pub(crate) const MAX_KUBERNETES_ID: i64 = i32::MAX as i64;

/// Resolves the effective `runAsNonRoot` value used by generated RustFS Pods.
///
/// Explicit configuration always wins. Otherwise UID 0 preserves legacy root
/// behavior, while every other UID uses the Operator's secure default.
pub(crate) fn effective_run_as_non_root(run_as_user: Option<i64>, explicit: Option<bool>) -> bool {
    explicit.unwrap_or(run_as_user != Some(0))
}

/// Pod SecurityContext overrides for RustFS pods.
///
/// Overrides the operator defaults (`runAsUser` / `runAsGroup` / `fsGroup` = 10001,
/// `runAsNonRoot` = true, and `seccompProfile.type` = `RuntimeDefault`).
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct PodSecurityContextOverride {
    /// UID to run the container process as.
    #[schemars(range(min = 0, max = 2147483647))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_user: Option<i64>,

    /// GID to run the container process as.
    #[schemars(range(min = 0, max = 2147483647))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_group: Option<i64>,

    /// GID applied to all volumes mounted in the Pod (`fsGroup`).
    #[schemars(range(min = 0, max = 2147483647))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group: Option<i64>,

    /// Enforce non-root execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_as_non_root: Option<bool>,

    /// Seccomp profile applied to all containers in the Pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seccomp_profile: Option<corev1::SeccompProfile>,
}

#[cfg(test)]
mod tests {
    use super::effective_run_as_non_root;

    #[test]
    fn effective_run_as_non_root_preserves_legacy_uid_zero() {
        assert!(!effective_run_as_non_root(Some(0), None));
        assert!(effective_run_as_non_root(Some(10_001), None));
        assert!(effective_run_as_non_root(None, None));
    }

    #[test]
    fn effective_run_as_non_root_prefers_explicit_configuration() {
        assert!(effective_run_as_non_root(Some(0), Some(true)));
        assert!(!effective_run_as_non_root(Some(10_001), Some(false)));
    }
}
