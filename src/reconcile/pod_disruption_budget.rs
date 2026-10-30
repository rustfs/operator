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

//! Per-pool PodDisruptionBudgets derived from the erasure layout RustFS reports.

use super::pool_lifecycle::{expected_pool_cmd_line, rustfs_admin_client, same_cmd_line};
use super::{Error, context_result, object_owned_by_tenant};
use crate::context::Context;
use crate::sts::rustfs_client::{RustfsPoolListItem, RustfsServerInfo};
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, Preconditions};
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use tracing::{debug, warn};

/// How long to wait before retrying when RustFS cannot report a usable layout yet.
pub(super) const POD_DISRUPTION_BUDGET_RETRY_INTERVAL: Duration = Duration::from_secs(60);

const DRIVE_STATE_OK: &str = "ok";

/// Reconciles one PodDisruptionBudget per pool.
///
/// Budgets of pools removed from the spec, and all budgets when
/// `spec.disablePodDisruptionBudget` is set, are deleted right away. Budgets are created or
/// updated only while every pool is settled (no rollout, decommission or unready Pods), because
/// the erasure layout is read from the running RustFS cluster. Returns a requeue interval when
/// the layout could not be read and the budgets may be stale.
pub(super) async fn reconcile_pod_disruption_budgets(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    workloads_settled: bool,
) -> Result<Option<Duration>, Error> {
    let existing = list_owned_budgets(ctx, tenant, namespace).await?;
    let disabled = tenant.spec.disable_pod_disruption_budget == Some(true);
    let desired_names = tenant
        .spec
        .pools
        .iter()
        .map(|pool| budget_name(tenant, pool))
        .collect::<HashSet<_>>();

    for budget in &existing {
        if disabled || !desired_names.contains(&budget.name_any()) {
            delete_budget(ctx, tenant, namespace, budget).await?;
        }
    }
    if disabled || !workloads_settled {
        return Ok(None);
    }

    let layouts = match read_pool_layouts(ctx, tenant).await {
        Ok(layouts) => layouts,
        Err(message) => {
            warn!(
                tenant = %tenant.name(),
                namespace = %namespace,
                message = %message,
                "cannot read RustFS erasure layout; keeping existing PodDisruptionBudgets"
            );
            return Ok(Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL));
        }
    };

    let mut requeue = None;
    for pool in &tenant.spec.pools {
        let name = budget_name(tenant, pool);
        let min_available = match pool_min_available(ctx, tenant, namespace, pool, &layouts) {
            Ok(min_available) => min_available,
            Err(message) => {
                warn!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    message = %message,
                    "cannot derive PodDisruptionBudget from RustFS erasure layout; keeping the existing one"
                );
                requeue = Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL);
                continue;
            }
        };

        match min_available {
            Some(min_available) => {
                let budget = new_pool_budget(tenant, namespace, pool, min_available);
                context_result(ctx.apply(&budget, namespace).await, ctx, tenant).await?;
            }
            None => {
                debug!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    "pool cannot lose a server without losing write quorum; no PodDisruptionBudget"
                );
                if let Some(budget) = existing.iter().find(|budget| budget.name_any() == name) {
                    delete_budget(ctx, tenant, namespace, budget).await?;
                }
            }
        }
    }

    Ok(requeue)
}

struct PoolLayouts {
    info: RustfsServerInfo,
    pools: Vec<RustfsPoolListItem>,
    parity: usize,
}

async fn read_pool_layouts(ctx: &Context, tenant: &Tenant) -> Result<PoolLayouts, String> {
    let client = rustfs_admin_client(ctx, tenant)
        .await
        .map_err(|error| format!("create RustFS admin client: {error}"))?;
    let info = client
        .server_info()
        .await
        .map_err(|error| format!("read RustFS server info: {error}"))?;
    let pools = client
        .list_pools()
        .await
        .map_err(|error| format!("list RustFS pools: {error}"))?;
    let parity = info
        .backend
        .as_ref()
        .and_then(|backend| backend.standard_sc_parity)
        .ok_or_else(|| "RustFS server info has no standard storage class parity".to_string())?;
    let parity = usize::try_from(parity)
        .map_err(|_| format!("RustFS standard storage class parity {parity} is out of range"))?;

    Ok(PoolLayouts {
        info,
        pools,
        parity,
    })
}

/// `Ok(None)` means the pool cannot tolerate losing any server.
fn pool_min_available(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    layouts: &PoolLayouts,
) -> Result<Option<i32>, String> {
    let cmd_line = expected_pool_cmd_line(tenant, namespace, pool, ctx.cluster_domain())?;
    let pool_id = layouts
        .pools
        .iter()
        .find(|item| same_cmd_line(&item.cmd_line, &cmd_line))
        .map(|item| item.id)
        .ok_or_else(|| format!("RustFS pool list has no pool with cmdline '{cmd_line}'"))?;
    let expected_drives = usize::try_from(pool.servers)
        .ok()
        .zip(usize::try_from(pool.persistence.volumes_per_server).ok())
        .map(|(servers, volumes)| servers.saturating_mul(volumes))
        .ok_or_else(|| "pool has invalid servers or volumesPerServer".to_string())?;
    let sets = pool_erasure_sets(&layouts.info, pool_id, expected_drives)?;

    let losses = tolerated_server_losses(&sets, layouts.parity);
    if losses == 0 {
        return Ok(None);
    }
    let losses = i32::try_from(losses).unwrap_or(i32::MAX);
    Ok(Some(pool.servers.saturating_sub(losses).max(1)))
}

/// Drive count per server for every erasure set of a pool, keyed by set index.
type ErasureSets = BTreeMap<i32, BTreeMap<String, usize>>;

fn pool_erasure_sets(
    info: &RustfsServerInfo,
    pool_id: usize,
    expected_drives: usize,
) -> Result<ErasureSets, String> {
    let mut sets = ErasureSets::new();
    let mut drives = 0usize;

    for drive in info.servers.iter().flat_map(|server| server.drives.iter()) {
        if drive
            .pool_index
            .and_then(|index| usize::try_from(index).ok())
            != Some(pool_id)
        {
            continue;
        }
        if drive.state != DRIVE_STATE_OK {
            return Err(format!(
                "drive '{}' is in state '{}'",
                drive.endpoint, drive.state
            ));
        }
        let set_index = drive
            .set_index
            .filter(|index| *index >= 0)
            .ok_or_else(|| format!("drive '{}' has no erasure set index", drive.endpoint))?;
        let host = endpoint_host(&drive.endpoint)
            .ok_or_else(|| format!("drive endpoint '{}' has no host", drive.endpoint))?;

        *sets
            .entry(set_index)
            .or_default()
            .entry(host.to_string())
            .or_default() += 1;
        drives += 1;
    }

    if drives != expected_drives {
        return Err(format!(
            "RustFS reports {drives} drives for pool {pool_id}, the spec declares {expected_drives}"
        ));
    }
    Ok(sets)
}

/// Number of servers of a pool that may be down at once while every erasure set keeps write
/// quorum.
///
/// RustFS reports one standard parity for the whole cluster, while automatic parity is
/// resolved per pool from its set size, so the smaller of the two is used.
fn tolerated_server_losses(sets: &ErasureSets, reported_parity: usize) -> usize {
    sets.values()
        .map(|servers| {
            let drives = servers.values().sum::<usize>();
            let parity = reported_parity.min(default_parity_count(drives));
            let data = drives.saturating_sub(parity);
            let write_quorum = data + usize::from(data == parity);
            let spare_drives = drives.saturating_sub(write_quorum);
            let drives_per_server = servers.values().copied().max().unwrap_or(1).max(1);
            spare_drives / drives_per_server
        })
        .min()
        .unwrap_or(0)
}

/// Mirrors RustFS `default_parity_count` for automatic standard parity.
fn default_parity_count(drives: usize) -> usize {
    match drives {
        0 | 1 => 0,
        2 | 3 => 1,
        4 | 5 => 2,
        6 | 7 => 3,
        _ => 4,
    }
}

fn endpoint_host(endpoint: &str) -> Option<&str> {
    let rest = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or_default();
    (!host.is_empty()).then_some(host)
}

fn budget_name(tenant: &Tenant, pool: &Pool) -> String {
    format!("{}-{}", tenant.name(), pool.name)
}

fn new_pool_budget(
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    min_available: i32,
) -> PodDisruptionBudget {
    PodDisruptionBudget {
        metadata: metav1::ObjectMeta {
            name: Some(budget_name(tenant, pool)),
            namespace: Some(namespace.to_string()),
            owner_references: Some(vec![tenant.new_owner_ref()]),
            labels: Some(tenant.pool_labels(pool)),
            ..Default::default()
        },
        spec: Some(PodDisruptionBudgetSpec {
            min_available: Some(IntOrString::Int(min_available)),
            selector: Some(metav1::LabelSelector {
                match_labels: Some(tenant.pool_selector_labels(pool)),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn list_owned_budgets(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
) -> Result<Vec<PodDisruptionBudget>, Error> {
    let params = ListParams::default().labels(&format!("{RUSTFS_TENANT_LABEL}={}", tenant.name()));
    let budgets = context_result(
        ctx.list_with_params::<PodDisruptionBudget>(namespace, &params)
            .await,
        ctx,
        tenant,
    )
    .await?;

    Ok(budgets
        .items
        .into_iter()
        .filter(|budget| object_owned_by_tenant(&budget.metadata, tenant))
        .collect())
}

async fn delete_budget(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    budget: &PodDisruptionBudget,
) -> Result<(), Error> {
    let params = DeleteParams {
        preconditions: Some(Preconditions {
            uid: budget.metadata.uid.clone(),
            resource_version: None,
        }),
        ..Default::default()
    };
    match ctx
        .delete_with_params::<PodDisruptionBudget>(&budget.name_any(), namespace, &params)
        .await
    {
        Err(error) if crate::context::is_kube_not_found(&error) => Ok(()),
        result => context_result(result, ctx, tenant).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sts::rustfs_client::{RustfsDriveInfo, RustfsServerProperties};

    fn info(servers: usize, volumes: usize, set_size: usize) -> RustfsServerInfo {
        // RustFS expands `pod-{0...N}/rustfs{0...V}` with the server varying fastest.
        let endpoints = (0..volumes)
            .flat_map(|volume| {
                (0..servers).map(move |server| {
                    (
                        server,
                        format!("http://t-p-{server}.t-hl.ns.svc:9000/data/rustfs{volume}"),
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut properties = (0..servers)
            .map(|server| RustfsServerProperties {
                endpoint: format!("t-p-{server}.t-hl.ns.svc:9000"),
                drives: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (index, (server, endpoint)) in endpoints.into_iter().enumerate() {
            properties[server].drives.push(RustfsDriveInfo {
                endpoint,
                state: DRIVE_STATE_OK.to_string(),
                pool_index: Some(0),
                set_index: Some(i32::try_from(index / set_size).unwrap()),
            });
        }
        RustfsServerInfo {
            servers: properties,
            ..Default::default()
        }
    }

    fn losses(servers: usize, volumes: usize, set_size: usize, parity: usize) -> usize {
        let sets = pool_erasure_sets(&info(servers, volumes, set_size), 0, servers * volumes)
            .expect("layout");
        tolerated_server_losses(&sets, parity)
    }

    #[test]
    fn three_servers_with_one_drive_tolerate_one_server() {
        assert_eq!(losses(3, 1, 3, 1), 1);
    }

    #[test]
    fn equal_data_and_parity_needs_an_extra_drive() {
        // 4 drives with EC:2 keep write quorum only with 3 drives online.
        assert_eq!(losses(4, 1, 4, 2), 1);
        assert_eq!(losses(2, 1, 2, 1), 0);
    }

    #[test]
    fn drives_per_server_in_a_set_limit_server_losses() {
        // 8 servers x 2 drives, one set of 16, EC:4: each server holds two drives of the set.
        assert_eq!(losses(8, 2, 16, 4), 2);
        // 3 servers x 4 drives, one set of 12, EC:4: each server holds four drives.
        assert_eq!(losses(3, 4, 12, 4), 1);
        assert_eq!(losses(3, 4, 12, 2), 0);
    }

    #[test]
    fn single_server_pools_tolerate_nothing() {
        assert_eq!(losses(1, 4, 4, 2), 0);
        assert_eq!(losses(1, 1, 1, 0), 0);
    }

    #[test]
    fn reported_parity_is_capped_by_automatic_parity_of_the_pool() {
        // Pool 0 reports EC:4; a pool of 3 drives resolves automatic parity to 1.
        assert_eq!(losses(3, 1, 3, 4), 1);
    }

    #[test]
    fn layout_rejects_missing_or_unhealthy_drives() {
        let mut layout = info(3, 1, 3);
        assert!(pool_erasure_sets(&layout, 0, 4).is_err());
        layout.servers[1].drives[0].state = "offline".to_string();
        assert!(pool_erasure_sets(&layout, 0, 3).is_err());
    }

    #[test]
    fn layout_ignores_other_pools() {
        let mut layout = info(3, 1, 3);
        layout.servers[0].drives[0].pool_index = Some(1);
        let error = pool_erasure_sets(&layout, 0, 3).expect_err("one drive belongs to pool 1");
        assert!(error.contains("reports 2 drives"));
    }

    #[test]
    fn endpoint_host_keeps_port_and_drops_path() {
        assert_eq!(
            endpoint_host("https://t-p-0.t-hl.ns.svc:9000/data/rustfs0"),
            Some("t-p-0.t-hl.ns.svc:9000")
        );
        assert_eq!(endpoint_host("/data/rustfs0"), None);
    }
}
