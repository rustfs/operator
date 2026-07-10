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

pub(crate) const DEFAULT_PERSISTENCE_PATH: &str = "/data";
pub(crate) const LEGACY_LOCAL_KMS_KEY_DIR: &str = "/data/kms-keys";
pub(crate) const LOCAL_KMS_KEY_DIR_NAME: &str = ".kms-keys";

pub(crate) fn data_volume_mount_path(base_path: Option<&str>, shard: i32) -> String {
    let base_path = base_path.unwrap_or(DEFAULT_PERSISTENCE_PATH);
    format!("{}/rustfs{}", base_path.trim_end_matches('/'), shard)
}

pub(crate) fn default_local_kms_key_directory(base_path: Option<&str>) -> String {
    format!(
        "{}/{}",
        data_volume_mount_path(base_path, 0),
        LOCAL_KMS_KEY_DIR_NAME
    )
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersistenceConfig {
    #[x_kube(validation = Rule::new("self > 0").message("volumesPerServer must be greater than 0"))]
    #[x_kube(validation = Rule::new("self == oldSelf").message("volumesPerServer is immutable"))]
    pub volumes_per_server: i32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_claim_template: Option<corev1::PersistentVolumeClaimSpec>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[x_kube(validation = Rule::new("self != ''").message("path must be not empty when specified"))]
    pub path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<std::collections::BTreeMap<String, String>>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            volumes_per_server: 4, // Must be > 0 when serialized into a Tenant spec.
            volume_claim_template: None,
            path: None,
            labels: None,
            annotations: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{data_volume_mount_path, default_local_kms_key_directory};

    #[test]
    fn data_volume_mount_path_uses_default_base_path() {
        assert_eq!(data_volume_mount_path(None, 0), "/data/rustfs0");
        assert_eq!(data_volume_mount_path(None, 3), "/data/rustfs3");
    }

    #[test]
    fn data_volume_mount_path_trims_trailing_slash() {
        assert_eq!(
            data_volume_mount_path(Some("/mnt/rustfs/"), 1),
            "/mnt/rustfs/rustfs1"
        );
    }

    #[test]
    fn default_local_kms_key_directory_uses_first_data_volume() {
        assert_eq!(
            default_local_kms_key_directory(None),
            "/data/rustfs0/.kms-keys"
        );
        assert_eq!(
            default_local_kms_key_directory(Some("/mnt/rustfs")),
            "/mnt/rustfs/rustfs0/.kms-keys"
        );
    }
}
