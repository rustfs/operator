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
use std::collections::HashSet;

use crate::types::v1alpha1::persistence::PersistenceConfig;

const RUSTFS_ERASURE_SET_SIZES: &[usize] = &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
const RUSTFS_ERASURE_SET_DRIVE_COUNT_ENV: &str = "RUSTFS_ERASURE_SET_DRIVE_COUNT";
const RUSTFS_STORAGE_CLASS_STANDARD_ENV: &str = "RUSTFS_STORAGE_CLASS_STANDARD";
const RUSTFS_STORAGE_CLASS_RRS_ENV: &str = "RUSTFS_STORAGE_CLASS_RRS";
const DEFAULT_RRS_PARITY: usize = 1;

/// Kubernetes scheduling and placement configuration for pools.
/// Groups related scheduling fields for better code organization.
/// Uses #[serde(flatten)] to maintain flat YAML structure.
#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct SchedulingConfig {
    /// NodeSelector is a selector which must be true for the pod to fit on a node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<std::collections::BTreeMap<String, String>>,

    /// Affinity is a group of affinity scheduling rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affinity: Option<corev1::Affinity>,

    /// Tolerations allow pods to schedule onto nodes with matching taints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerations: Option<Vec<corev1::Toleration>>,

    /// TopologySpreadConstraints describes how pods should spread across topology domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_spread_constraints: Option<Vec<corev1::TopologySpreadConstraint>>,

    /// Resources describes the compute resource requirements for the pool's containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<corev1::ResourceRequirements>,

    /// PriorityClassName indicates the pod's priority. Overrides tenant-level priority class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.servers * self.persistence.volumesPerServer >= 4").
    message(Message::Expression(r#""pool " + self.name + " must have at least 4 total volumes (servers × volumesPerServer)""#.into())).
    reason(Reason::FieldValueInvalid))
]
#[x_kube(validation = Rule::new("self.servers != 3 || self.servers * self.persistence.volumesPerServer >= 6").
    message(Message::Expression(r#""pool " + self.name + " with 3 servers must have at least 6 volumes in total""#.into())).
    reason(Reason::FieldValueInvalid))
]
pub struct Pool {
    #[x_kube(validation = Rule::new("self != ''").message("pool name must be not empty"))]
    #[x_kube(validation = Rule::new("self.size() <= 63 && self.matches('^[a-z0-9]([-a-z0-9]*[a-z0-9])?$')").
        message("pool name must be a valid RFC 1123 label: lowercase alphanumeric or '-', start and end with alphanumeric, max 63 characters"))
    ]
    pub name: String,

    #[x_kube(validation = Rule::new("self > 0").message("servers must be greater than 0"))]
    #[x_kube(validation = Rule::new("self == oldSelf").message("servers is immutable"))]
    pub servers: i32,

    pub persistence: PersistenceConfig,

    /// Kubernetes scheduling and placement configuration.
    /// Flattened to maintain backward compatibility with YAML structure.
    #[serde(flatten)]
    pub scheduling: SchedulingConfig,
}

/// Validates total volume count (`servers * volumesPerServer`) for RustFS erasure coding.
/// Same rules as CRD CEL on [`Pool`] and the operator console API (`validate_pool_volumes`).
pub fn validate_pool_total_volumes(servers: i32, volumes_per_server: i32) -> Result<i32, String> {
    let total = servers * volumes_per_server;
    if servers <= 0 || volumes_per_server <= 0 {
        return Err("servers and volumes_per_server must be positive".to_string());
    }
    if servers == 2 && total < 4 {
        return Err("Pool with 2 servers must have at least 4 volumes in total".to_string());
    }
    if servers == 3 && total < 6 {
        return Err("Pool with 3 servers must have at least 6 volumes in total".to_string());
    }
    if total < 4 {
        return Err(format!(
            "Pool must have at least 4 total volumes (got {} servers × {} volumes = {})",
            servers, volumes_per_server, total
        ));
    }
    Ok(total)
}

/// Validate a pool name used in labels and RustFS peer DNS names.
pub fn validate_pool_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("pool name must be not empty".to_string());
    }
    if name.len() > 63 {
        return Err(format!(
            "pool name must be at most 63 characters, got {}",
            name.len()
        ));
    }

    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(
            "pool name must start with a lowercase alphanumeric character (a-z, 0-9)".to_string(),
        );
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(
            "pool name must end with a lowercase alphanumeric character (a-z, 0-9)".to_string(),
        );
    }
    for &b in bytes {
        if !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'-' {
            return Err(format!(
                "pool name contains invalid character '{}'; only lowercase alphanumeric and '-' are allowed",
                b as char
            ));
        }
    }

    Ok(())
}

pub fn validate_pool_collection(
    tenant_name: &str,
    pools: &[Pool],
    env: &[corev1::EnvVar],
) -> Result<(), String> {
    if pools.is_empty() {
        return Err("pools must be configured".to_string());
    }

    let mut names = HashSet::new();
    for pool in pools {
        validate_pool_name(&pool.name)?;
        if !names.insert(pool.name.as_str()) {
            return Err(format!("pool names must be unique: '{}'", pool.name));
        }
        validate_pool_total_volumes(pool.servers, pool.persistence.volumes_per_server)
            .map_err(|message| format!("pool '{}': {}", pool.name, message))?;
        validate_rustfs_peer_dns_label(tenant_name, pool)?;
    }

    validate_pool_erasure_compatibility(pools, env)
}

pub fn validate_pool_shape_immutable(existing: &[Pool], desired: &[Pool]) -> Result<(), String> {
    for desired_pool in desired {
        let Some(existing_pool) = existing
            .iter()
            .find(|existing_pool| existing_pool.name == desired_pool.name)
        else {
            continue;
        };

        if existing_pool.servers != desired_pool.servers {
            return Err(format!(
                "pool '{}' servers is immutable ({} -> {})",
                desired_pool.name, existing_pool.servers, desired_pool.servers
            ));
        }

        if existing_pool.persistence.volumes_per_server
            != desired_pool.persistence.volumes_per_server
        {
            return Err(format!(
                "pool '{}' volumesPerServer is immutable ({} -> {})",
                desired_pool.name,
                existing_pool.persistence.volumes_per_server,
                desired_pool.persistence.volumes_per_server
            ));
        }
    }

    Ok(())
}

fn validate_rustfs_peer_dns_label(tenant_name: &str, pool: &Pool) -> Result<(), String> {
    let max_ordinal = pool.servers.saturating_sub(1).max(0);
    let dns_label_len = tenant_name.len() + 1 + pool.name.len() + 1 + ordinal_digits(max_ordinal);

    if dns_label_len > 63 {
        return Err(format!(
            "pool '{}' makes RustFS peer DNS label too long: '{}-{}-<ordinal>' is {} characters at max ordinal {}, must be at most 63",
            pool.name, tenant_name, pool.name, dns_label_len, max_ordinal
        ));
    }

    Ok(())
}

fn ordinal_digits(value: i32) -> usize {
    value.to_string().len()
}

fn validate_pool_erasure_compatibility(
    pools: &[Pool],
    env: &[corev1::EnvVar],
) -> Result<(), String> {
    let set_drive_count = rustfs_erasure_set_drive_count_from_env(env)?;
    let pool_layouts = pools
        .iter()
        .map(|pool| {
            rustfs_drives_per_set(
                pool.servers,
                pool.persistence.volumes_per_server,
                set_drive_count,
            )
            .map(|drives_per_set| (pool, drives_per_set))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let Some((first_pool, first_drives_per_set)) = pool_layouts.first() else {
        return Ok(());
    };

    let standard_parity = rustfs_standard_parity_from_env(env, *first_drives_per_set)?;
    let rrs_parity = rustfs_rrs_parity_from_env(env, *first_drives_per_set)?;

    if standard_parity > 0 && rrs_parity > 0 && standard_parity < rrs_parity {
        return Err(format!(
            "{RUSTFS_STORAGE_CLASS_STANDARD_ENV} parity {} must be greater than or equal to {RUSTFS_STORAGE_CLASS_RRS_ENV} parity {}",
            standard_parity, rrs_parity
        ));
    }

    validate_parity_for_pool(
        first_pool,
        *first_drives_per_set,
        standard_parity,
        "STANDARD",
    )?;
    validate_parity_for_pool(first_pool, *first_drives_per_set, rrs_parity, "RRS")?;

    for (pool, drives_per_set) in pool_layouts.iter().skip(1) {
        validate_parity_for_pool(pool, *drives_per_set, standard_parity, "STANDARD")?;
        validate_parity_for_pool(pool, *drives_per_set, rrs_parity, "RRS")?;
    }

    Ok(())
}

fn validate_parity_for_pool(
    pool: &Pool,
    drives_per_set: usize,
    parity: usize,
    storage_class: &str,
) -> Result<(), String> {
    if drives_per_set == 0 {
        return Err(format!(
            "pool '{}' has invalid drivesPerSet {}, must be greater than zero",
            pool.name, drives_per_set
        ));
    }

    let max_parity = drives_per_set / 2;
    if parity > max_parity {
        return Err(format!(
            "pool '{}' has RustFS drivesPerSet {}, but {} parity {} exceeds the maximum {}",
            pool.name, drives_per_set, storage_class, parity, max_parity
        ));
    }
    Ok(())
}

fn rustfs_erasure_set_drive_count_from_env(
    env: &[corev1::EnvVar],
) -> Result<Option<usize>, String> {
    let Some(value) = literal_env_value(env, RUSTFS_ERASURE_SET_DRIVE_COUNT_ENV)? else {
        return Ok(None);
    };

    let set_drive_count = value.parse::<usize>().map_err(|_| {
        format!(
            "{RUSTFS_ERASURE_SET_DRIVE_COUNT_ENV} must be a non-negative integer, got '{}'",
            value
        )
    })?;

    Ok((set_drive_count > 0).then_some(set_drive_count))
}

fn rustfs_standard_parity_from_env(
    env: &[corev1::EnvVar],
    set_drive_count: usize,
) -> Result<usize, String> {
    let parity = literal_env_value(env, RUSTFS_STORAGE_CLASS_STANDARD_ENV)?
        .map(parse_storage_class_parity)
        .transpose()?;

    Ok(parity.unwrap_or_else(|| rustfs_default_parity_count(set_drive_count)))
}

fn rustfs_rrs_parity_from_env(
    env: &[corev1::EnvVar],
    set_drive_count: usize,
) -> Result<usize, String> {
    let parity = literal_env_value(env, RUSTFS_STORAGE_CLASS_RRS_ENV)?
        .map(parse_storage_class_parity)
        .transpose()?;

    Ok(parity.unwrap_or(if set_drive_count == 1 {
        0
    } else {
        DEFAULT_RRS_PARITY
    }))
}

fn literal_env_value<'a>(env: &'a [corev1::EnvVar], name: &str) -> Result<Option<&'a str>, String> {
    let Some(var) = env.iter().rev().find(|var| var.name == name) else {
        return Ok(None);
    };

    if var.value_from.is_some() {
        return Err(format!(
            "{name} is configured with value_from, which is not supported during admission validation"
        ));
    }

    Ok(var.value.as_deref())
}

fn parse_storage_class_parity(value: &str) -> Result<usize, String> {
    let Some((scheme, parity)) = value.split_once(':') else {
        return Err(format!(
            "invalid storage class '{}': expected 'EC:<parity>'",
            value
        ));
    };
    if scheme != "EC" {
        return Err(format!(
            "invalid storage class '{}': only EC scheme is supported",
            value
        ));
    }
    parity.parse::<usize>().map_err(|_| {
        format!(
            "invalid storage class '{}': parity must be a non-negative integer",
            value
        )
    })
}

pub fn rustfs_drives_per_set(
    servers: i32,
    volumes_per_server: i32,
    set_drive_count: Option<usize>,
) -> Result<usize, String> {
    let total_volumes = validate_pool_total_volumes(servers, volumes_per_server)? as usize;
    let volume_pattern_size = volumes_per_server as usize;

    // Matches the current RustFS ellipsis layout calculation for the operator's
    // generated host{0...n}/rustfs{0...m} pattern.
    let set_counts = RUSTFS_ERASURE_SET_SIZES
        .iter()
        .copied()
        .filter(|set_size| total_volumes.is_multiple_of(*set_size))
        .filter(|set_size| pattern_is_symmetric(volume_pattern_size, *set_size))
        .collect::<Vec<_>>();

    if set_counts.is_empty() {
        return Err(format!(
            "pool layout with {} servers × {} volumesPerServer = {} total volumes is not divisible by any RustFS supported erasure set size with symmetric volume distribution",
            servers, volumes_per_server, total_volumes
        ));
    }

    if let Some(set_drive_count) = set_drive_count {
        if !set_counts.contains(&set_drive_count) {
            return Err(format!(
                "{RUSTFS_ERASURE_SET_DRIVE_COUNT_ENV}={} is not valid for pool layout {} servers × {} volumesPerServer; acceptable values are {:?}",
                set_drive_count, servers, volumes_per_server, set_counts
            ));
        }
        return Ok(set_drive_count);
    }

    Ok(common_set_drive_count(total_volumes, &set_counts))
}

fn pattern_is_symmetric(pattern_size: usize, set_size: usize) -> bool {
    if pattern_size > set_size {
        pattern_size.is_multiple_of(set_size)
    } else {
        set_size.is_multiple_of(pattern_size)
    }
}

fn common_set_drive_count(divisible_size: usize, set_counts: &[usize]) -> usize {
    if divisible_size < set_counts[set_counts.len() - 1] {
        return divisible_size;
    }

    let mut prev_d = divisible_size / set_counts[0];
    let mut set_size = 0;
    for &count in set_counts {
        if divisible_size.is_multiple_of(count) {
            let d = divisible_size / count;
            if d <= prev_d {
                prev_d = d;
                set_size = count;
            }
        }
    }
    set_size
}

fn rustfs_default_parity_count(drives_per_set: usize) -> usize {
    match drives_per_set {
        1 => 0,
        2 | 3 => 1,
        4 | 5 => 2,
        6 | 7 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_storage_class_parity, rustfs_drives_per_set, validate_pool_collection,
        validate_pool_name, validate_pool_total_volumes,
    };
    use crate::types::v1alpha1::persistence::PersistenceConfig;
    use crate::types::v1alpha1::pool::Pool;
    use k8s_openapi::api::core::v1 as corev1;

    #[test]
    fn rejects_non_positive_inputs() {
        assert!(validate_pool_total_volumes(0, 4).is_err());
        assert!(validate_pool_total_volumes(4, 0).is_err());
    }

    #[test]
    fn rejects_total_under_four_when_not_caught_by_special_cases() {
        assert!(validate_pool_total_volumes(1, 3).is_err());
        assert!(validate_pool_total_volumes(1, 2).is_err());
    }

    #[test]
    fn four_servers_one_volume_each_is_ok() {
        assert_eq!(validate_pool_total_volumes(4, 1).unwrap(), 4);
    }

    #[test]
    fn two_servers_need_at_least_four_total() {
        assert!(validate_pool_total_volumes(2, 1).is_err());
        assert_eq!(validate_pool_total_volumes(2, 2).unwrap(), 4);
    }

    #[test]
    fn three_servers_need_at_least_six_total() {
        assert!(validate_pool_total_volumes(3, 1).is_err());
        assert_eq!(validate_pool_total_volumes(3, 2).unwrap(), 6);
    }

    #[test]
    fn accepts_common_valid_configs() {
        assert_eq!(validate_pool_total_volumes(1, 4).unwrap(), 4);
        assert_eq!(validate_pool_total_volumes(4, 1).unwrap(), 4);
        assert_eq!(validate_pool_total_volumes(2, 2).unwrap(), 4);
    }

    #[test]
    fn validates_pool_name_as_rfc1123_label() {
        assert!(validate_pool_name("pool-0").is_ok());
        assert!(validate_pool_name("0-pool").is_ok());
        assert!(validate_pool_name("Pool-0").is_err());
        assert!(validate_pool_name("-pool").is_err());
        assert!(validate_pool_name("pool-").is_err());
    }

    #[test]
    fn derives_rustfs_drives_per_set_for_operator_pool_pattern() {
        assert_eq!(rustfs_drives_per_set(4, 4, None).unwrap(), 16);
        assert_eq!(rustfs_drives_per_set(8, 4, None).unwrap(), 16);
        assert_eq!(rustfs_drives_per_set(2, 2, None).unwrap(), 4);
        assert_eq!(rustfs_drives_per_set(3, 2, None).unwrap(), 6);
    }

    #[test]
    fn rejects_pool_layout_rustfs_cannot_split_into_sets() {
        assert!(rustfs_drives_per_set(17, 1, None).is_err());
    }

    #[test]
    fn rejects_incompatible_pool_parity() {
        let pools = vec![test_pool("pool-0", 4, 4), test_pool("pool-1", 2, 2)];

        let err = validate_pool_collection("tenant", &pools, &[]).unwrap_err();

        assert!(err.contains("STANDARD parity 4 exceeds the maximum 2"));
    }

    #[test]
    fn rejects_parity_exceeding_half_for_small_drive_set() {
        let pools = vec![test_pool("pool-0", 2, 4)];
        let env = vec![
            corev1::EnvVar {
                name: "RUSTFS_ERASURE_SET_DRIVE_COUNT".to_string(),
                value: Some("2".to_string()),
                ..Default::default()
            },
            corev1::EnvVar {
                name: "RUSTFS_STORAGE_CLASS_STANDARD".to_string(),
                value: Some("EC:2".to_string()),
                ..Default::default()
            },
        ];

        let err = validate_pool_collection("tenant", &pools, &env).unwrap_err();
        assert!(err.contains("STANDARD parity 2 exceeds the maximum 1"));
    }

    #[test]
    fn honors_literal_erasure_env_for_pool_compatibility() {
        let pools = vec![test_pool("pool-0", 4, 4), test_pool("pool-1", 2, 2)];
        let env = vec![corev1::EnvVar {
            name: "RUSTFS_ERASURE_SET_DRIVE_COUNT".to_string(),
            value: Some("4".to_string()),
            ..Default::default()
        }];

        assert!(validate_pool_collection("tenant", &pools, &env).is_ok());
    }

    #[test]
    fn rejects_erasure_layout_validation_with_value_from_env_vars() {
        let pools = vec![test_pool("pool-0", 4, 4)];
        let env = vec![corev1::EnvVar {
            name: "RUSTFS_STORAGE_CLASS_STANDARD".to_string(),
            value_from: Some(corev1::EnvVarSource {
                secret_key_ref: Some(corev1::SecretKeySelector {
                    name: "legacy-rustfs-config".to_string(),
                    key: "standard-parity".to_string(),
                    optional: Some(false),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }];

        let err = validate_pool_collection("tenant", &pools, &env).unwrap_err();
        assert!(err.contains("not supported during admission validation"));
    }

    #[test]
    fn honors_literal_standard_storage_class_env_for_pool_compatibility() {
        let pools = vec![test_pool("pool-0", 4, 4), test_pool("pool-1", 2, 2)];
        let env = vec![corev1::EnvVar {
            name: "RUSTFS_STORAGE_CLASS_STANDARD".to_string(),
            value: Some("EC:2".to_string()),
            ..Default::default()
        }];

        assert!(validate_pool_collection("tenant", &pools, &env).is_ok());
    }

    #[test]
    fn validates_storage_class_parity_format() {
        assert_eq!(parse_storage_class_parity("EC:2").unwrap(), 2);
        assert!(parse_storage_class_parity("INVALID:2").is_err());
        assert!(parse_storage_class_parity("EC:not-a-number").is_err());
    }

    #[test]
    fn rejects_too_long_rustfs_peer_dns_labels() {
        let pools = vec![test_pool(
            "pool-name-that-makes-the-peer-label-too-long",
            4,
            4,
        )];

        let err = validate_pool_collection("tenant-name-that-is-already-quite-long", &pools, &[])
            .unwrap_err();

        assert!(err.contains("RustFS peer DNS label too long"));
    }

    fn test_pool(name: &str, servers: i32, volumes_per_server: i32) -> Pool {
        Pool {
            name: name.to_string(),
            servers,
            persistence: PersistenceConfig {
                volumes_per_server,
                ..Default::default()
            },
            scheduling: Default::default(),
        }
    }
}
