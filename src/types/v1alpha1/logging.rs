//  Copyright 2025 RustFS Team
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;

/// Logging configuration for RustFS Tenant
///
/// Defines how RustFS outputs logs. Following cloud-native best practices,
/// the default mode is Stdout, which allows Kubernetes to collect and manage logs.
///
/// **Important Note on Storage System Logs**:
/// RustFS is a storage system, and its logs should NOT be stored in RustFS itself
/// to avoid circular dependencies during startup. The recommended approach is:
/// - Stdout mode (default): Logs collected by Kubernetes, no dependencies
/// - EmptyDir mode: Temporary local storage for debugging
/// - Persistent mode: Only if external storage (Ceph/NFS/Cloud) is available
///
/// **Why not RustFS self-storage?**
/// During startup, RustFS needs to write logs before its S3 API is available,
/// creating a chicken-and-egg problem. Startup logs cannot be written to a
/// system that hasn't started yet.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    /// Logging mode: stdout, emptyDir, or persistent
    ///
    /// - stdout: Output logs to stdout/stderr (default, recommended for cloud-native)
    /// - emptyDir: Write logs to an emptyDir volume (temporary, lost on Pod restart)
    /// - persistent: Write logs to a PersistentVolumeClaim (persisted across restarts)
    #[serde(default = "default_logging_mode")]
    pub mode: LoggingMode,

    /// Storage size for persistent logs (only used when mode=persistent)
    /// Defaults to 5Gi if not specified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_size: Option<String>,

    /// Storage class for persistent logs (only used when mode=persistent)
    /// If not specified, uses the cluster's default StorageClass
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,

    /// Custom mount path for log directory
    /// Defaults to /logs if not specified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<String>,
}

/// Logging mode for RustFS
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Display)]
#[serde(rename_all = "lowercase")]
pub enum LoggingMode {
    /// Output logs to stdout/stderr (cloud-native, recommended)
    ///
    /// Logs are collected by Kubernetes and can be viewed with kubectl logs.
    /// Can be integrated with log aggregation systems (Loki, ELK, etc.).
    /// This is the ONLY mode that works during RustFS startup without dependencies.
    Stdout,

    /// Write logs to emptyDir volume (temporary storage)
    ///
    /// Useful for debugging. Logs are lost when Pod restarts.
    /// Uses local disk, no external dependencies.
    EmptyDir,

    /// Write logs to PersistentVolumeClaim (persistent storage)
    ///
    /// **Warning**: Requires an external StorageClass to provide PVCs.
    /// Only use this when:
    /// - The cluster has existing storage (Ceph/NFS/Cloud) independent of RustFS
    /// - You need persistent logs separate from RustFS data volumes
    ///
    /// **Do NOT use RustFS itself as storage for these logs** - this creates
    /// a circular dependency where RustFS startup logs cannot be written because
    /// RustFS S3 API hasn't started yet.
    Persistent,
}

fn default_logging_mode() -> LoggingMode {
    LoggingMode::Stdout
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            mode: LoggingMode::Stdout,
            storage_size: None,
            storage_class: None,
            mount_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_logging_config() {
        let config = LoggingConfig::default();
        assert_eq!(config.mode, LoggingMode::Stdout);
        assert_eq!(config.storage_size, None);
    }

    #[test]
    fn test_persistent_logging_config() {
        let config = LoggingConfig {
            mode: LoggingMode::Persistent,
            storage_size: Some("10Gi".to_string()),
            storage_class: Some("fast-ssd".to_string()),
            mount_path: None,
        };
        assert_eq!(config.mode, LoggingMode::Persistent);
        assert_eq!(config.storage_size, Some("10Gi".to_string()));
    }
}
