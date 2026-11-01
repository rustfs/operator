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
use crate::context::{self, Context, is_kube_not_found};
use crate::sts::rustfs_client::{RustfsPoolListItem, RustfsServerInfo};
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::tenant::{RUSTFS_TENANT_LABEL, Tenant};
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::ResourceExt;
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams, PostParams, Preconditions};
use kube::runtime::events::EventType;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use tracing::{debug, warn};

/// How long to wait before re-reading the layout while it is unknown or drives are offline.
pub(super) const POD_DISRUPTION_BUDGET_RETRY_INTERVAL: Duration = Duration::from_secs(60);

const DRIVE_STATE_OK: &str = "ok";
const FIELD_MANAGER: &str = "rustfs-operator";

/// Reconciles one PodDisruptionBudget per pool.
///
/// Budgets of pools removed from the spec, and all budgets when
/// `spec.disablePodDisruptionBudget` is set, are deleted right away. Budgets are created or
/// updated only while every pool is settled (no rollout, decommission or unready Pods), because
/// the erasure layout is read from the running RustFS cluster. Returns a requeue interval while
/// the layout is unknown or drives are offline, so budgets follow drive recovery.
pub(super) async fn reconcile_pod_disruption_budgets(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    workloads_settled: bool,
) -> Result<Option<Duration>, Error> {
    reconcile_budgets_with_layouts(
        ctx,
        tenant,
        namespace,
        workloads_settled,
        read_pool_layouts(ctx, tenant),
    )
    .await
}

/// `read_layouts` is awaited only when budgets are enabled and every pool is settled.
async fn reconcile_budgets_with_layouts(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    workloads_settled: bool,
    read_layouts: impl Future<Output = Result<PoolLayouts, String>>,
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

    let layouts = read_layouts.await.inspect_err(|message| {
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            message = %message,
            "cannot read RustFS erasure layout; existing PodDisruptionBudgets block evictions"
        );
    });

    let mut requeue = None;
    for pool in &tenant.spec.pools {
        let name = budget_name(tenant, pool);
        let owned = existing.iter().find(|budget| budget.name_any() == name);
        let allowance = layouts
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|layouts| pool_allowance(ctx, tenant, namespace, pool, layouts));

        let min_available = match allowance {
            Ok(PoolAllowance::NoBudget) => {
                debug!(
                    tenant = %tenant.name(),
                    namespace = %namespace,
                    pool = %pool.name,
                    "pool cannot lose a server without losing write quorum; no PodDisruptionBudget"
                );
                if let Some(budget) = owned {
                    delete_budget(ctx, tenant, namespace, budget).await?;
                }
                continue;
            }
            Ok(PoolAllowance::Budget {
                min_available,
                degraded,
            }) => {
                if degraded {
                    requeue = Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL);
                }
                min_available
            }
            Err(message) => {
                requeue = Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL);
                if layouts.is_ok() {
                    warn!(
                        tenant = %tenant.name(),
                        namespace = %namespace,
                        pool = %pool.name,
                        message = %message,
                        "cannot derive PodDisruptionBudget from RustFS erasure layout; an existing one blocks evictions"
                    );
                }
                // The remaining fault tolerance is unknown, so an existing budget allows no
                // eviction until RustFS reports the pool again.
                if owned.is_none() {
                    continue;
                }
                pool.servers
            }
        };

        let budget = new_pool_budget(tenant, namespace, pool, min_available);
        ensure_budget(ctx, tenant, namespace, &budget).await?;
    }

    Ok(requeue)
}

struct PoolLayouts {
    info: RustfsServerInfo,
    pools: Vec<RustfsPoolListItem>,
    /// Standard parity of every pool; RustFS omits it when pools resolve different parities.
    parity: Option<usize>,
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
        .map(|parity| {
            usize::try_from(parity).map_err(|_| {
                format!("RustFS standard storage class parity {parity} is out of range")
            })
        })
        .transpose()?;

    Ok(PoolLayouts {
        info,
        pools,
        parity,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolAllowance {
    /// The pool cannot lose a server even with every drive online.
    NoBudget,
    /// `degraded` is set while some drives of the pool are offline.
    Budget { min_available: i32, degraded: bool },
}

fn pool_allowance(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    pool: &Pool,
    layouts: &PoolLayouts,
) -> Result<PoolAllowance, String> {
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

    if tolerated_server_losses(&sets, layouts.parity, DriveScope::All) == 0 {
        return Ok(PoolAllowance::NoBudget);
    }
    // Pods with offline drives stay Ready, so offline drives reduce the allowance directly; once
    // they consume it, `minAvailable` equals the server count and no eviction is allowed.
    let losses = tolerated_server_losses(&sets, layouts.parity, DriveScope::Online);
    let losses = i32::try_from(losses).unwrap_or(i32::MAX);
    let degraded = sets
        .values()
        .flat_map(BTreeMap::values)
        .any(|drives| drives.online < drives.total);

    Ok(PoolAllowance::Budget {
        min_available: pool.servers.saturating_sub(losses).max(1),
        degraded,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ServerDrives {
    total: usize,
    online: usize,
}

/// Drives per server for every erasure set of a pool, keyed by set index.
type ErasureSets = BTreeMap<i32, BTreeMap<String, ServerDrives>>;

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
        let set_index = drive
            .set_index
            .filter(|index| *index >= 0)
            .ok_or_else(|| format!("drive '{}' has no erasure set index", drive.endpoint))?;
        let host = endpoint_host(&drive.endpoint)
            .ok_or_else(|| format!("drive endpoint '{}' has no host", drive.endpoint))?;

        let server = sets
            .entry(set_index)
            .or_default()
            .entry(host.to_string())
            .or_default();
        server.total += 1;
        server.online += usize::from(drive.state == DRIVE_STATE_OK);
        drives += 1;
    }

    if drives != expected_drives {
        return Err(format!(
            "RustFS reports {drives} drives for pool {pool_id}, the spec declares {expected_drives}"
        ));
    }
    Ok(sets)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveScope {
    All,
    Online,
}

/// Number of servers of a pool that may be down at once while every erasure set keeps write
/// quorum for the STANDARD storage class.
///
/// `reported_parity` is the parity RustFS reports for all pools; without it, every set uses the
/// automatic parity for its size, which is how RustFS resolves pools with different parities.
fn tolerated_server_losses(
    sets: &ErasureSets,
    reported_parity: Option<usize>,
    scope: DriveScope,
) -> usize {
    sets.values()
        .map(|servers| {
            let drives = servers.values().map(|server| server.total).sum::<usize>();
            let available = match scope {
                DriveScope::All => drives,
                DriveScope::Online => servers.values().map(|server| server.online).sum(),
            };
            let parity = reported_parity.unwrap_or_else(|| default_parity_count(drives));
            let data = drives.saturating_sub(parity);
            let write_quorum = data + usize::from(data == parity);
            let drives_per_server = servers
                .values()
                .map(|server| server.total)
                .max()
                .unwrap_or(1)
                .max(1);
            available.saturating_sub(write_quorum) / drives_per_server
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

/// Creates the budget, or updates it when the Tenant owns it. A budget with the same name that
/// the Tenant does not own is left unchanged, including one created concurrently.
async fn ensure_budget(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    budget: &PodDisruptionBudget,
) -> Result<(), Error> {
    let api: Api<PodDisruptionBudget> = Api::namespaced(ctx.client.clone(), namespace);
    let name = budget.name_any();

    let current = match ctx.get::<PodDisruptionBudget>(&name, namespace).await {
        Ok(current) => current,
        Err(error) if is_kube_not_found(&error) => {
            let params = PostParams {
                field_manager: Some(FIELD_MANAGER.to_string()),
                ..Default::default()
            };
            match api.create(&params, budget).await {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(response)) if response.code == 409 => {
                    context_result(ctx.get(&name, namespace).await, ctx, tenant).await?
                }
                Err(source) => {
                    return context_result(Err(context::Error::Kube { source }), ctx, tenant).await;
                }
            }
        }
        Err(error) => return context_result(Err(error), ctx, tenant).await,
    };

    if !object_owned_by_tenant(&current.metadata, tenant) {
        warn!(
            tenant = %tenant.name(),
            namespace = %namespace,
            budget = %name,
            "PodDisruptionBudget is not owned by the Tenant; leaving it unchanged"
        );
        let _ = ctx
            .record(
                tenant,
                EventType::Warning,
                "PodDisruptionBudgetConflict",
                &format!(
                    "PodDisruptionBudget '{name}' exists and is not owned by this Tenant; it is left unchanged"
                ),
            )
            .await;
        return Ok(());
    }

    let params = PatchParams::apply(FIELD_MANAGER).force();
    let result = api
        .patch(&name, &params, &Patch::Apply(budget))
        .await
        .map_err(|source| context::Error::Kube { source });
    context_result(result, ctx, tenant).await.map(drop)
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
        Err(error) if is_kube_not_found(&error) => Ok(()),
        result => context_result(result, ctx, tenant).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sts::rustfs_client::{RustfsDriveInfo, RustfsServerProperties};
    use http::{Method, Request, Response, StatusCode};
    use http_body_util::BodyExt;
    use kube::{Client, client::Body};
    use serde_json::{Value, json};
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};
    use tower::service_fn;

    /// Drives of one pool; RustFS expands `pod-{0...N}/rustfs{0...V}` with the server varying
    /// fastest and cuts the list into consecutive sets.
    fn pool_servers(
        pool: i32,
        servers: usize,
        volumes: usize,
        set_size: usize,
    ) -> Vec<RustfsServerProperties> {
        let mut properties = (0..servers)
            .map(|server| RustfsServerProperties {
                endpoint: format!("t-p{pool}-{server}.t-hl.ns.svc:9000"),
                drives: Vec::new(),
            })
            .collect::<Vec<_>>();
        for volume in 0..volumes {
            for server in 0..servers {
                let index = volume * servers + server;
                properties[server].drives.push(RustfsDriveInfo {
                    endpoint: format!(
                        "http://t-p{pool}-{server}.t-hl.ns.svc:9000/data/rustfs{volume}"
                    ),
                    state: DRIVE_STATE_OK.to_string(),
                    pool_index: Some(pool),
                    set_index: Some(i32::try_from(index / set_size).expect("set index")),
                });
            }
        }
        properties
    }

    fn info(servers: usize, volumes: usize, set_size: usize) -> RustfsServerInfo {
        RustfsServerInfo {
            servers: pool_servers(0, servers, volumes, set_size),
            ..Default::default()
        }
    }

    fn sets(servers: usize, volumes: usize, set_size: usize) -> ErasureSets {
        pool_erasure_sets(&info(servers, volumes, set_size), 0, servers * volumes).expect("layout")
    }

    fn losses(servers: usize, volumes: usize, set_size: usize, parity: usize) -> usize {
        tolerated_server_losses(
            &sets(servers, volumes, set_size),
            Some(parity),
            DriveScope::All,
        )
    }

    fn take_drive_offline(sets: &mut ErasureSets) {
        let server = sets
            .values_mut()
            .flat_map(BTreeMap::values_mut)
            .next()
            .expect("a server");
        server.online -= 1;
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
    fn missing_parity_uses_the_automatic_parity_of_each_set() {
        // RustFS omits the scalar parity when pools resolve different automatic parities.
        let auto = |servers, set_size| {
            tolerated_server_losses(&sets(servers, 1, set_size), None, DriveScope::All)
        };
        assert_eq!(auto(4, 4), 1);
        assert_eq!(auto(8, 8), 3);
    }

    #[test]
    fn offline_drives_reduce_the_allowance() {
        // 8 x 2 in one set of 16 with EC:4: write quorum 12, two servers may go with all
        // drives online, one server once a drive is offline.
        let mut layout = sets(8, 2, 16);
        take_drive_offline(&mut layout);
        assert_eq!(
            tolerated_server_losses(&layout, Some(4), DriveScope::All),
            2
        );
        assert_eq!(
            tolerated_server_losses(&layout, Some(4), DriveScope::Online),
            1
        );

        let mut layout = sets(3, 1, 3);
        take_drive_offline(&mut layout);
        assert_eq!(
            tolerated_server_losses(&layout, Some(1), DriveScope::Online),
            0
        );
    }

    #[test]
    fn layout_counts_offline_drives_and_rejects_missing_ones() {
        let mut layout = info(3, 1, 3);
        assert!(pool_erasure_sets(&layout, 0, 4).is_err());
        layout.servers[1].drives[0].state = "offline".to_string();
        let sets = pool_erasure_sets(&layout, 0, 3).expect("offline drives are counted");
        let online = sets
            .values()
            .flat_map(BTreeMap::values)
            .map(|server| server.online)
            .sum::<usize>();
        assert_eq!(online, 2);
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

    const BUDGETS_PATH: &str = "/apis/policy/v1/namespaces/default/poddisruptionbudgets";

    #[derive(Debug)]
    struct KubeCall {
        method: Method,
        path: String,
        body: Option<Value>,
    }

    type KubeCalls = Arc<Mutex<Vec<KubeCall>>>;

    fn tenant_with_pools(servers: &[i32]) -> Tenant {
        let mut tenant = crate::tests::create_test_tenant(None, None);
        let mut template = tenant.spec.pools[0].clone();
        template.persistence.volumes_per_server = 1;
        tenant.spec.pools = servers
            .iter()
            .enumerate()
            .map(|(index, servers)| Pool {
                name: format!("pool-{index}"),
                servers: *servers,
                ..template.clone()
            })
            .collect();
        tenant
    }

    fn existing_budget(tenant: &Tenant, pool: &str, owned: bool) -> PodDisruptionBudget {
        PodDisruptionBudget {
            metadata: metav1::ObjectMeta {
                name: Some(format!("{}-{pool}", tenant.name())),
                namespace: Some("default".to_string()),
                uid: Some(format!("{pool}-budget-uid")),
                labels: Some(BTreeMap::from([(
                    RUSTFS_TENANT_LABEL.to_string(),
                    tenant.name(),
                )])),
                owner_references: owned.then(|| vec![tenant.new_owner_ref()]),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn status(code: u16, reason: &str) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Failure",
            "reason": reason,
            "code": code
        })
    }

    /// Kubernetes API with `existing` budgets. `created_concurrently` appears under its name
    /// when the Operator tries to create it, so the create returns 409.
    fn mock_kube(
        existing: Vec<PodDisruptionBudget>,
        created_concurrently: Option<PodDisruptionBudget>,
    ) -> (Context, KubeCalls) {
        let calls = KubeCalls::default();
        let stored = Arc::new(Mutex::new(existing));
        let service = service_fn({
            let calls = Arc::clone(&calls);
            move |request: Request<Body>| {
                let calls = Arc::clone(&calls);
                let stored = Arc::clone(&stored);
                let created_concurrently = created_concurrently.clone();
                async move {
                    let method = request.method().clone();
                    let path = request.uri().path().to_string();
                    let bytes = request
                        .into_body()
                        .collect()
                        .await
                        .expect("request body should be readable")
                        .to_bytes();
                    let body = (!bytes.is_empty()).then(|| {
                        serde_json::from_slice::<Value>(&bytes).expect("request body is JSON")
                    });
                    let name = path
                        .strip_prefix(BUDGETS_PATH)
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(str::to_string);
                    let find = |name: &str| {
                        stored
                            .lock()
                            .expect("budgets lock")
                            .iter()
                            .find(|budget| budget.name_any() == name)
                            .cloned()
                    };
                    let (code, response) = match (&method, name.as_deref()) {
                        (&Method::GET, None) if path == BUDGETS_PATH => (
                            200,
                            json!({
                                "apiVersion": "policy/v1",
                                "kind": "PodDisruptionBudgetList",
                                "metadata": {},
                                "items": *stored.lock().expect("budgets lock")
                            }),
                        ),
                        (&Method::GET, Some(name)) => match find(name) {
                            Some(budget) => (200, json!(budget)),
                            None => (404, status(404, "NotFound")),
                        },
                        (&Method::POST, None) if path == BUDGETS_PATH => match created_concurrently
                        {
                            Some(budget) => {
                                stored.lock().expect("budgets lock").push(budget);
                                (409, status(409, "AlreadyExists"))
                            }
                            None => (200, body.clone().expect("create carries the budget")),
                        },
                        (&Method::PATCH, Some(_)) => {
                            (200, body.clone().expect("apply carries the budget"))
                        }
                        (&Method::DELETE, Some(_)) => (
                            200,
                            json!({"apiVersion": "v1", "kind": "Status", "status": "Success"}),
                        ),
                        (&Method::POST, _) if path.ends_with("/events") => (200, json!({})),
                        _ => panic!("unexpected Kubernetes request: {method} {path}"),
                    };
                    calls
                        .lock()
                        .expect("calls lock")
                        .push(KubeCall { method, path, body });
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::from_u16(code).expect("status code"))
                            .body(Body::from(
                                serde_json::to_vec(&response).expect("response serializes"),
                            ))
                            .expect("response builds"),
                    )
                }
            }
        });
        (Context::new(Client::new(service, "default")), calls)
    }

    fn layouts_for(ctx: &Context, tenant: &Tenant, parity: Option<usize>) -> PoolLayouts {
        let mut servers = Vec::new();
        let mut pools = Vec::new();
        for (index, pool) in tenant.spec.pools.iter().enumerate() {
            let count = usize::try_from(pool.servers).expect("servers");
            servers.extend(pool_servers(
                i32::try_from(index).expect("pool index"),
                count,
                1,
                count,
            ));
            let cmd_line = expected_pool_cmd_line(tenant, "default", pool, ctx.cluster_domain())
                .expect("pool cmdline");
            pools.push(
                serde_json::from_value(json!({
                    "id": index,
                    "cmdline": cmd_line,
                    "lastUpdate": "",
                    "status": "active"
                }))
                .expect("pool list item"),
            );
        }
        PoolLayouts {
            info: RustfsServerInfo {
                servers,
                ..Default::default()
            },
            pools,
            parity,
        }
    }

    async fn layouts_never_read() -> Result<PoolLayouts, String> {
        panic!("the erasure layout must not be read")
    }

    fn requests(calls: &KubeCalls) -> Vec<(Method, String)> {
        calls
            .lock()
            .expect("calls lock")
            .iter()
            .map(|call| (call.method.clone(), call.path.clone()))
            .collect()
    }

    fn written_min_available(calls: &KubeCalls, method: Method) -> Vec<(String, IntOrString)> {
        calls
            .lock()
            .expect("calls lock")
            .iter()
            .filter(|call| call.method == method && call.path.starts_with(BUDGETS_PATH))
            .map(|call| {
                let budget: PodDisruptionBudget =
                    serde_json::from_value(call.body.clone().expect("budget")).expect("budget");
                let min_available = budget
                    .spec
                    .and_then(|spec| spec.min_available)
                    .expect("minAvailable");
                (budget.metadata.name.expect("name"), min_available)
            })
            .collect()
    }

    fn budget_path(name: &str) -> String {
        format!("{BUDGETS_PATH}/{name}")
    }

    #[tokio::test]
    async fn settled_pool_gets_a_budget_from_its_layout() {
        let tenant = tenant_with_pools(&[4]);
        let (ctx, calls) = mock_kube(Vec::new(), None);
        let layouts = layouts_for(&ctx, &tenant, Some(2));

        let requeue =
            reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
                .await
                .expect("reconcile");

        assert_eq!(requeue, None);
        assert_eq!(
            requests(&calls),
            vec![
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::GET, budget_path("test-tenant-pool-0")),
                (Method::POST, BUDGETS_PATH.to_string()),
            ]
        );
        let calls = calls.lock().expect("calls lock");
        let budget: PodDisruptionBudget =
            serde_json::from_value(calls[2].body.clone().expect("budget")).expect("budget");
        let spec = budget.spec.expect("spec");
        assert_eq!(spec.min_available, Some(IntOrString::Int(3)));
        assert_eq!(
            spec.selector.and_then(|selector| selector.match_labels),
            Some(tenant.pool_selector_labels(&tenant.spec.pools[0]))
        );
        assert!(object_owned_by_tenant(&budget.metadata, &tenant));
    }

    #[tokio::test]
    async fn owned_budget_is_updated_with_server_side_apply() {
        let tenant = tenant_with_pools(&[4]);
        let (ctx, calls) = mock_kube(vec![existing_budget(&tenant, "pool-0", true)], None);
        let layouts = layouts_for(&ctx, &tenant, Some(2));

        reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
            .await
            .expect("reconcile");

        assert_eq!(
            written_min_available(&calls, Method::PATCH),
            vec![("test-tenant-pool-0".to_string(), IntOrString::Int(3))]
        );
    }

    #[tokio::test]
    async fn heterogeneous_pool_parity_from_the_admin_response() {
        let tenant = tenant_with_pools(&[4, 8]);
        let (ctx, calls) = mock_kube(Vec::new(), None);
        let mut layouts = layouts_for(&ctx, &tenant, None);
        // RustFS 1.0.0 reports no scalar standard parity when pools resolve different ones.
        let response = json!({
            "backend": {
                "backendType": "Erasure",
                "onlineDisks": 12,
                "offlineDisks": 0,
                "standardSCParity": null,
                "rrSCParity": 1,
                "totalSets": [1, 1],
                "totalDrivesPerSet": [4, 8]
            },
            "servers": []
        });
        let parsed: RustfsServerInfo = serde_json::from_value(response).expect("admin info parses");
        layouts.parity = parsed
            .backend
            .and_then(|backend| backend.standard_sc_parity)
            .map(|parity| usize::try_from(parity).expect("parity"));

        reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
            .await
            .expect("reconcile");

        // 4 drives resolve EC:2 (write quorum 3), 8 drives EC:4 (write quorum 5).
        assert_eq!(
            written_min_available(&calls, Method::POST),
            vec![
                ("test-tenant-pool-0".to_string(), IntOrString::Int(3)),
                ("test-tenant-pool-1".to_string(), IntOrString::Int(5)),
            ]
        );
    }

    #[tokio::test]
    async fn pool_without_spare_servers_loses_its_budget() {
        let tenant = tenant_with_pools(&[2]);
        let (ctx, calls) = mock_kube(vec![existing_budget(&tenant, "pool-0", true)], None);
        let layouts = layouts_for(&ctx, &tenant, Some(1));

        reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
            .await
            .expect("reconcile");

        assert_eq!(
            requests(&calls),
            vec![
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::DELETE, budget_path("test-tenant-pool-0")),
            ]
        );
    }

    #[tokio::test]
    async fn disabling_deletes_only_budgets_owned_by_the_tenant() {
        let mut tenant = tenant_with_pools(&[4, 4]);
        tenant.spec.disable_pod_disruption_budget = Some(true);
        let (ctx, calls) = mock_kube(
            vec![
                existing_budget(&tenant, "pool-0", true),
                existing_budget(&tenant, "pool-1", false),
            ],
            None,
        );

        let requeue =
            reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, layouts_never_read())
                .await
                .expect("reconcile");

        assert_eq!(requeue, None);
        assert_eq!(
            requests(&calls),
            vec![
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::DELETE, budget_path("test-tenant-pool-0")),
            ]
        );
        let calls = calls.lock().expect("calls lock");
        assert_eq!(
            calls[1]
                .body
                .as_ref()
                .map(|body| &body["preconditions"]["uid"]),
            Some(&json!("pool-0-budget-uid"))
        );
    }

    #[tokio::test]
    async fn removed_pool_budget_is_deleted_while_pools_are_not_settled() {
        let tenant = tenant_with_pools(&[4]);
        let (ctx, calls) = mock_kube(
            vec![
                existing_budget(&tenant, "pool-0", true),
                existing_budget(&tenant, "pool-1", true),
            ],
            None,
        );

        let requeue =
            reconcile_budgets_with_layouts(&ctx, &tenant, "default", false, layouts_never_read())
                .await
                .expect("reconcile");

        assert_eq!(requeue, None);
        assert_eq!(
            requests(&calls),
            vec![
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::DELETE, budget_path("test-tenant-pool-1")),
            ]
        );
    }

    #[tokio::test]
    async fn unreadable_layout_blocks_evictions_through_existing_budgets() {
        let tenant = tenant_with_pools(&[4, 4]);
        let (ctx, calls) = mock_kube(vec![existing_budget(&tenant, "pool-0", true)], None);

        let requeue = reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async {
            Err("RustFS is not reachable".to_string())
        })
        .await
        .expect("reconcile");

        assert_eq!(requeue, Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL));
        assert_eq!(
            written_min_available(&calls, Method::PATCH),
            vec![("test-tenant-pool-0".to_string(), IntOrString::Int(4))]
        );
        assert!(written_min_available(&calls, Method::POST).is_empty());
    }

    #[tokio::test]
    async fn offline_drive_that_consumes_the_allowance_blocks_evictions() {
        // 4 drives with EC:2: write quorum 3, so one offline drive leaves no spare server.
        let tenant = tenant_with_pools(&[4]);
        let (ctx, calls) = mock_kube(vec![existing_budget(&tenant, "pool-0", true)], None);
        let mut layouts = layouts_for(&ctx, &tenant, Some(2));
        layouts.info.servers[2].drives[0].state = "offline".to_string();

        let requeue =
            reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
                .await
                .expect("reconcile");

        assert_eq!(requeue, Some(POD_DISRUPTION_BUDGET_RETRY_INTERVAL));
        assert_eq!(
            written_min_available(&calls, Method::PATCH),
            vec![("test-tenant-pool-0".to_string(), IntOrString::Int(4))]
        );
    }

    #[tokio::test]
    async fn same_name_budget_without_tenant_owner_is_left_unchanged() {
        let tenant = tenant_with_pools(&[4]);
        let mut foreign = existing_budget(&tenant, "pool-0", false);
        foreign.spec =
            Some(new_pool_budget(&tenant, "default", &tenant.spec.pools[0], 3).spec).flatten();
        let (ctx, calls) = mock_kube(vec![foreign], None);
        let layouts = layouts_for(&ctx, &tenant, Some(2));

        reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
            .await
            .expect("reconcile");

        let requests = requests(&calls);
        assert_eq!(
            requests[..2],
            [
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::GET, budget_path("test-tenant-pool-0")),
            ]
        );
        assert!(
            requests[2..]
                .iter()
                .all(|(method, path)| *method == Method::POST && path.ends_with("/events")),
            "only a warning event may follow: {requests:?}"
        );
    }

    #[tokio::test]
    async fn budget_created_concurrently_by_someone_else_is_left_unchanged() {
        let tenant = tenant_with_pools(&[4]);
        let (ctx, calls) = mock_kube(Vec::new(), Some(existing_budget(&tenant, "pool-0", false)));
        let layouts = layouts_for(&ctx, &tenant, Some(2));

        reconcile_budgets_with_layouts(&ctx, &tenant, "default", true, async { Ok(layouts) })
            .await
            .expect("reconcile");

        let requests = requests(&calls);
        assert_eq!(
            requests[..4],
            [
                (Method::GET, BUDGETS_PATH.to_string()),
                (Method::GET, budget_path("test-tenant-pool-0")),
                (Method::POST, BUDGETS_PATH.to_string()),
                (Method::GET, budget_path("test-tenant-pool-0")),
            ]
        );
        assert!(
            requests.iter().all(|(method, _)| *method != Method::PATCH),
            "a budget the Tenant does not own must not be patched: {requests:?}"
        );
    }
}
