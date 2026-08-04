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

use crate::{
    metrics::{self, TenantStorageMetrics},
    sts::rustfs_client::{RustfsAdminClient, RustfsServerInfo},
    types::v1alpha1::tenant::Tenant,
};
use futures::{StreamExt, stream};
use kube::{Api, Client, api::ListParams};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

const DEFAULT_MONITOR_INTERVAL: Duration = Duration::from_secs(300);
const MAX_CONCURRENT_TENANT_POLLS: usize = 4;
const TENANT_LIST_PAGE_SIZE: u32 = 500;

pub fn is_enabled() -> bool {
    env_bool("OPERATOR_TENANT_MONITOR_ENABLED", true)
}

pub fn interval() -> Duration {
    match std::env::var("OPERATOR_TENANT_MONITOR_INTERVAL_SECONDS") {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(seconds) if seconds > 0 => Duration::from_secs(seconds),
            Ok(_) | Err(_) => {
                warn!(
                    value,
                    "invalid OPERATOR_TENANT_MONITOR_INTERVAL_SECONDS value, using default"
                );
                DEFAULT_MONITOR_INTERVAL
            }
        },
        Err(_) => DEFAULT_MONITOR_INTERVAL,
    }
}

pub async fn run(client: Client, cancel: CancellationToken, cluster_domain: String) {
    let interval = interval();
    info!(
        interval_seconds = interval.as_secs(),
        "tenant storage monitor started"
    );

    loop {
        poll_all_tenants(client.clone(), &cluster_domain).await;

        tokio::select! {
            _ = cancel.cancelled() => {
                info!("tenant storage monitor cancellation requested");
                break;
            }
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

async fn poll_all_tenants(client: Client, cluster_domain: &str) {
    let started = Instant::now();
    let tenants = match list_all_tenants(client.clone()).await {
        Ok(tenants) => tenants,
        Err(error) => {
            warn!(%error, "tenant storage monitor failed listing tenants");
            metrics::record_tenant_monitor_poll("error", started.elapsed());
            return;
        }
    };
    let active_tenants = tenants
        .iter()
        .filter_map(|tenant| {
            tenant
                .namespace()
                .ok()
                .map(|namespace| (namespace, tenant.name()))
        })
        .collect::<Vec<_>>();
    metrics::prune_tenant_storage(&active_tenants);

    stream::iter(tenants)
        .for_each_concurrent(MAX_CONCURRENT_TENANT_POLLS, |tenant| {
            let client = client.clone();
            let cluster_domain = cluster_domain.to_string();
            async move {
                poll_tenant(client, tenant, &cluster_domain).await;
            }
        })
        .await;
}

async fn poll_tenant(client: Client, tenant: Tenant, cluster_domain: &str) {
    let started = Instant::now();
    let tenant_name = tenant.name();
    let namespace = match tenant.namespace() {
        Ok(namespace) => namespace,
        Err(error) => {
            warn!(tenant = %tenant_name, %error, "tenant storage monitor skipped tenant without namespace");
            metrics::record_tenant_monitor_poll("skipped", started.elapsed());
            return;
        }
    };

    if tenant.spec.creds_secret.is_none() {
        debug!(
            namespace = %namespace,
            tenant = %tenant_name,
            "tenant storage monitor skipped tenant without credsSecret"
        );
        metrics::record_tenant_monitor_skipped(&namespace, &tenant_name, started.elapsed());
        return;
    }

    match poll_tenant_storage(&client, &tenant, cluster_domain).await {
        Ok(storage) => {
            metrics::record_tenant_storage(&namespace, &tenant_name, storage);
            metrics::record_tenant_monitor_poll("success", started.elapsed());
        }
        Err(error) => {
            warn!(
                namespace = %namespace,
                tenant = %tenant_name,
                %error,
                "tenant storage monitor poll failed"
            );
            metrics::record_tenant_storage_error(&namespace, &tenant_name);
            metrics::record_tenant_monitor_poll("error", started.elapsed());
        }
    }
}

async fn list_all_tenants(client: Client) -> Result<Vec<Tenant>, kube::Error> {
    let tenants_api = Api::<Tenant>::all(client);
    let mut tenants = Vec::new();
    let mut continue_token = None;

    loop {
        let mut params = ListParams::default().limit(TENANT_LIST_PAGE_SIZE);
        if let Some(token) = continue_token.as_deref() {
            params = params.continue_token(token);
        }
        let page = tenants_api.list(&params).await?;
        tenants.extend(page.items);

        continue_token = page
            .metadata
            .continue_
            .filter(|token| !token.trim().is_empty());
        if continue_token.is_none() {
            return Ok(tenants);
        }
    }
}

async fn poll_tenant_storage(
    client: &Client,
    tenant: &Tenant,
    cluster_domain: &str,
) -> Result<TenantStorageMetrics, Box<dyn std::error::Error + Send + Sync>> {
    let credentials = RustfsAdminClient::load_tenant_credentials(client, tenant).await?;
    let rustfs_client = if tenant.spec.tls.as_ref().is_some_and(|tls| tls.is_enabled()) {
        RustfsAdminClient::from_tls_tenant_for_sts(client, tenant, credentials, cluster_domain)
            .await?
    } else {
        RustfsAdminClient::from_tenant(tenant, credentials)?
    };
    let info = rustfs_client.server_info().await?;

    Ok(storage_metrics_from_info(&info))
}

fn storage_metrics_from_info(info: &RustfsServerInfo) -> TenantStorageMetrics {
    let (online_drives, offline_drives, write_quorum_drives) = info
        .backend
        .as_ref()
        .map(|backend| {
            (
                backend.online_disks,
                backend.offline_disks,
                write_quorum_from_backend(backend),
            )
        })
        .unwrap_or_default();

    let mut healing_drives = 0u64;
    let mut raw_capacity_bytes = 0u64;
    let mut raw_used_bytes = 0u64;
    let mut pool_usage_bytes = 0u64;

    if let Some(pools) = &info.pools {
        for sets in pools.values() {
            for set in sets.values() {
                healing_drives = healing_drives.saturating_add(set.heal_disks);
                raw_capacity_bytes = raw_capacity_bytes.saturating_add(set.raw_capacity);
                raw_used_bytes = raw_used_bytes.saturating_add(set.raw_usage);
                pool_usage_bytes = pool_usage_bytes.saturating_add(set.usage);
            }
        }
    }

    let object_usage_bytes = info
        .usage
        .as_ref()
        .map(|usage| usage.size)
        .unwrap_or(pool_usage_bytes);

    TenantStorageMetrics {
        online_drives,
        offline_drives,
        healing_drives,
        raw_capacity_bytes,
        raw_used_bytes,
        object_usage_bytes,
        write_quorum_drives,
        healthy: online_drives > 0
            && offline_drives == 0
            && healing_drives == 0
            && (write_quorum_drives == 0 || online_drives >= write_quorum_drives),
    }
}

fn write_quorum_from_backend(backend: &crate::sts::rustfs_client::RustfsErasureBackend) -> u64 {
    let parity = backend.standard_sc_parity.unwrap_or_default();
    backend
        .total_sets
        .iter()
        .zip(backend.drives_per_set.iter())
        .map(|(sets, drives_per_set)| sets.saturating_mul(drives_per_set.saturating_sub(parity)))
        .sum()
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => {
                warn!(name, value, "invalid boolean env value, using default");
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sts::rustfs_client::{
        RustfsErasureBackend, RustfsErasureSetInfo, RustfsServerUsage,
    };
    use std::collections::BTreeMap;

    #[test]
    fn storage_metrics_capture_capacity_healing_and_quorum() {
        let mut sets = BTreeMap::new();
        sets.insert(
            "0".to_string(),
            RustfsErasureSetInfo {
                raw_usage: 100,
                raw_capacity: 400,
                usage: 70,
                heal_disks: 1,
                ..Default::default()
            },
        );
        let mut pools = BTreeMap::new();
        pools.insert("0".to_string(), sets);

        let info = RustfsServerInfo {
            usage: Some(RustfsServerUsage { size: 80 }),
            backend: Some(RustfsErasureBackend {
                online_disks: 3,
                offline_disks: 1,
                standard_sc_parity: Some(2),
                total_sets: vec![1],
                drives_per_set: vec![4],
            }),
            pools: Some(pools),
        };

        let metrics = storage_metrics_from_info(&info);

        assert_eq!(metrics.online_drives, 3);
        assert_eq!(metrics.offline_drives, 1);
        assert_eq!(metrics.healing_drives, 1);
        assert_eq!(metrics.raw_capacity_bytes, 400);
        assert_eq!(metrics.raw_used_bytes, 100);
        assert_eq!(metrics.object_usage_bytes, 80);
        assert_eq!(metrics.write_quorum_drives, 2);
        assert!(!metrics.healthy);
    }
}
