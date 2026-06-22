<!--
Copyright 2025 RustFS Team

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# RustFS Fault-Test Operations

This guide is the operational entry point for running RustFS workload fault
tests from the `e2e` package.

## Scope

Fault tests run only on a dedicated real Kubernetes or K3s cluster. They are
not Kind tests and are not designed for shared application clusters. The runner
creates and deletes its own namespace, Tenant, PVCs, Pods, Services, StatefulSet,
and Chaos Mesh resources.

The fault-test runner owns only the resources it creates in the fault namespace.
Pre-existing non-fault Tenants are treated as health guardrails: preflight
requires them to be Ready, and each scenario stops if one becomes non-Ready. The
runner never modifies, restarts, scales, or cleans up those Tenants. Do not point
the fault namespace, Tenant, StorageClass, or device-mapper path at shared or
production resources.

Default owned resources:

```text
namespace: rustfs-fault-test
tenant:    fault-test-tenant
manager:   app.kubernetes.io/managed-by=rustfs-operator-fault-test
```

## Commands

Run all commands from the repository root.

```bash
make -C e2e fault-check
make -C e2e fault-list
make -C e2e fault-preflight SCENARIO=io-eio
make -C e2e fault-run SCENARIO=io-eio
make -C e2e fault-run-dm
make -C e2e fault-cleanup
```

`fault-check` is local only. It runs Bash syntax, Rust fmt, tests, and clippy.

`fault-run` prebuilds the ignored `faults` test binary before the fault window,
then runs that binary directly. The runner reruns preflight before and after the
build.

## Required Environment

Only these variables are required for non-static fault scenarios:

```bash
export RUSTFS_FAULT_TEST_STORAGE_CLASS=<dedicated-dynamic-storage-class>
export RUSTFS_FAULT_TEST_SERVER_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
```

`RUSTFS_FAULT_TEST_SERVER_IMAGE` must be explicit. Prefer a pinned digest so a
failed run can be reproduced.

`RUSTFS_FAULT_TEST_EXPECTED_CONTEXT` is optional. When set, both the shell
runner and Rust test entrypoint require the current context to match it exactly.
When unset, the current non-Kind context is used and pinned for the run.
When `KUBECONFIG` is unset on a K3s host, the runner inherits
`/etc/rancher/k3s/k3s.yaml` if it is readable so the Rust Kubernetes client uses
the same cluster as `kubectl`.

```bash
export RUSTFS_FAULT_TEST_EXPECTED_CONTEXT=<dedicated-k8s-or-k3s-context>
```

## Common Overrides

Defaults are centralized in `e2e/src/fault/config.rs`. The shell
runner passes the same values into the Rust test and validates artifacts against
the selected values. Shell preflight mirrors the Rust entrypoint for numeric
ranges, booleans, and scenario-specific percent overrides.

Fault-test orchestration lives under `e2e/src/fault/`: runtime configuration,
scenario catalog, plan expansion, fault backends, fixture ownership checks, S3
workload history, and the Rust runner. Shared Kubernetes wrappers, kubectl
command helpers, artifact collection, port-forwarding, and generic Tenant
resource cleanup remain under `e2e/src/framework/`.

| Variable | Default | Use |
| --- | --- | --- |
| `RUSTFS_FAULT_TEST_NAMESPACE` | `rustfs-fault-test` | Fault namespace. |
| `RUSTFS_FAULT_TEST_TENANT` | `fault-test-tenant` | Tenant name. |
| `RUSTFS_FAULT_TEST_CHAOS_NAMESPACE` | `chaos-mesh` | Chaos Mesh namespace. |
| `RUSTFS_FAULT_TEST_USE_CLUSTER_IP` | `false` | Set to `1` when the runner can reach Service ClusterIPs. |
| `RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS` | `40000` | Total object count; must be at least 12. |
| `RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY` | `80` | S3 workload concurrency; must be 1 through object count. |
| `RUSTFS_FAULT_TEST_DURATION_SECONDS` | `7200` | Maximum fault TTL. Successful runs recover earlier. |
| `RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS` | `30` | Per S3 request timeout. |
| `RUSTFS_FAULT_TEST_TIMEOUT_SECONDS` | `300` | Kubernetes wait timeout. |
| `RUSTFS_FAULT_TEST_SEED` | generated | Reuse a workload plan. |
| `RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` | `false` | Force at least one client-visible failed/timeout/unknown S3 operation even when the catalog marks disruption optional. |
| `RUSTFS_FAULT_TEST_BUILD_JOBS` | `1` | Cargo prebuild job count. |
| `RUSTFS_FAULT_TEST_RUN_ROOT` | timestamped target dir | Artifact root. |

For a small rehearsal run:

```bash
export RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS=64
export RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY=8
make -C e2e fault-run SCENARIO=io-eio
```

`RUSTFS_FAULT_TEST_PERCENT` applies only when the Rust scenario catalog marks
the scenario as percent-based. Fixed-target scenarios reject a percent override.
Run `make -C e2e fault-list` or `cargo run --manifest-path e2e/Cargo.toml --bin
rustfs-e2e -- fault-catalog-json` to inspect the current catalog.

## Cluster Preparation

Check the current context first:

```bash
kubectl config current-context
kubectl get nodes
kubectl get crd tenants.rustfs.com
kubectl get storageclass
```

Requirements:

- The current context must be a real Kubernetes or K3s cluster, not `kind-*`.
- At least four schedulable Ready nodes are required for the current default
  Tenant shape.
- Non-static scenarios need a dedicated dynamic StorageClass.
- `dm-flakey` needs a dedicated static Local PV StorageClass and explicit
  device-mapper variables.
- Other Tenants in the cluster are intentionally ignored by preflight, health
  guard, and pass/fail checks.

For K3s with local-path storage, verify the actual backing filesystem has
enough free space. PVC capacity alone may not enforce real disk quota.

```bash
kubectl -n kube-system get configmap local-path-config -o yaml
df -h <actual-provisioner-path>
```

Chaos Mesh is required for Chaos Mesh-backed scenarios. The validated version is v2.8.3:

```bash
helm repo add chaos-mesh https://charts.chaos-mesh.org
helm repo update
helm upgrade --install chaos-mesh chaos-mesh/chaos-mesh \
  -n chaos-mesh --create-namespace --version 2.8.3 \
  --set chaosDaemon.runtime=containerd \
  --set chaosDaemon.socketPath=/run/k3s/containerd/containerd.sock \
  --set dashboard.create=false \
  --wait --timeout 10m

kubectl -n chaos-mesh get deployment,daemonset
kubectl get crd iochaos.chaos-mesh.org podchaos.chaos-mesh.org networkchaos.chaos-mesh.org stresschaos.chaos-mesh.org
```

Use the actual runtime socket for non-K3s clusters.

## Recommended Run Flow

1. Run the local gate:

   ```bash
   make -C e2e fault-check
   ```

2. Export the required runtime values:

   ```bash
   export RUSTFS_FAULT_TEST_STORAGE_CLASS=<dedicated-dynamic-storage-class>
   export RUSTFS_FAULT_TEST_SERVER_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
   export RUSTFS_FAULT_TEST_USE_CLUSTER_IP=1
   ```

3. Run preflight and the first P0 scenario:

   ```bash
   make -C e2e fault-preflight SCENARIO=io-eio
   make -C e2e fault-run SCENARIO=io-eio
   ```

4. List the catalog and run each selected scenario explicitly:

   ```bash
   make -C e2e fault-list
   make -C e2e fault-run SCENARIO=network-delay
   ```

5. Collect artifacts, then clean owned resources:

   ```bash
   make -C e2e fault-cleanup
   ```

## Scenario Catalog

The Rust catalog in `e2e/src/fault/scenarios.rs` is the only maintained scenario
source of truth. The shell runner and this guide query that catalog instead of
duplicating scenario names, percent rules, CRDs, tools, or impact policy.

```bash
make -C e2e fault-list
cargo run --manifest-path e2e/Cargo.toml --bin rustfs-e2e -- fault-catalog-json
```

Each run names exactly one scenario with `SCENARIO=<name>`. SRE-owned scheduling
or automation should live outside this repository and call the same explicit
command for the desired scenario. Tool requirements, such as `warp` for
`warp-under-chaos`, are read from the Rust catalog during preflight.

## Artifacts And Pass Criteria

Artifacts are written under:

```text
${RUSTFS_FAULT_TEST_RUN_ROOT:-e2e/target/fault-tests/<timestamp>}/<scenario>/
```

Key files:

```text
run-metadata.json
workload-plan.json
history.jsonl
workload-summary.json
recommit-report.json
checker-pre-recommit-report.json
checker-report.json
fault-evidence.json
chaos-manifest.yaml
fault-status-*.json
nodes-*.txt
pods-*.txt
pvcs-*.txt
pvs-*.txt
events-*.txt
*.log
```

A successful run must show:

- `fault-evidence.json`: `injected`, `active_during_workload`, and `recovered`
  are `true`.
- `checker-pre-recommit-report.json` and `checker-report.json`: `passed` is
  `true`; expected live objects are GET+sha256 verified; missing objects, hash
  mismatches, successful corrupted reads, unexpected visible deleted objects,
  and LIST warnings are empty.
- `recommit-report.json`: every previously unconfirmed write was recommitted
  and GET verified after recovery.
- `workload-plan.json`: object count, concurrency, and payload distribution are
  internally consistent with the selected environment values.

If a scenario fails, inspect `failure-summary.json`,
`runner-failure-summary.json`, `test.log`, `fault-status-*.json`, and the
RustFS Pod logs first.

## Cleanup

`fault-cleanup` removes managed Chaos resources and the owned fault namespace.
It does not remove external StorageClasses, PVs, provisioners, host paths, loop
devices, or device-mapper devices.

```bash
make -C e2e fault-cleanup
```

Manual checks:

```bash
kubectl -n "${RUSTFS_FAULT_TEST_CHAOS_NAMESPACE:-chaos-mesh}" \
  get iochaos,podchaos,networkchaos,stresschaos \
  -l app.kubernetes.io/managed-by=rustfs-operator-fault-test

kubectl get namespace "${RUSTFS_FAULT_TEST_NAMESPACE:-rustfs-fault-test}"
```

## dm-flakey

`dm-flakey` is an explicit scenario that needs a dedicated static Local PV setup
and privileged helper access on the fault namespace.

There is no Make target that installs this environment. Prepare the host storage
and Kubernetes Local PVs first, then use `fault-preflight` to verify them.

### dm-flakey Host Storage

Prefer real dedicated block devices. The loop-file commands below are for lab
clusters only. Run them on the Kubernetes nodes that will host the four static
Local PVs.

On the node that will receive the device-mapper fault:

```bash
export LAB=/data/rustfs/rustfs-fault-lab
export DM_NAME=rustfs-fault-dm

sudo mkdir -p "$LAB/volume"
sudo truncate -s 120G "$LAB/disk.img"
export BACKING="$(sudo losetup --find --show "$LAB/disk.img")"
export SECTORS="$(sudo blockdev --getsz "$BACKING")"
sudo dmsetup create "$DM_NAME" --table "0 $SECTORS linear $BACKING 0"
sudo mkfs.ext4 -F "/dev/mapper/$DM_NAME"
sudo mount "/dev/mapper/$DM_NAME" "$LAB/volume"
sudo chmod 0777 "$LAB/volume"

sudo dmsetup table "$DM_NAME"
findmnt -n -o SOURCE --target "$LAB/volume"
```

On each of the other three nodes:

```bash
export LAB=/data/rustfs/rustfs-fault-lab

sudo mkdir -p "$LAB/volume"
sudo truncate -s 120G "$LAB/disk.img"
export BACKING="$(sudo losetup --find --show "$LAB/disk.img")"
sudo mkfs.ext4 -F "$BACKING"
sudo mount "$BACKING" "$LAB/volume"
sudo chmod 0777 "$LAB/volume"
findmnt -n -o SOURCE --target "$LAB/volume"
```

### dm-flakey Kubernetes Storage

Create one `kubernetes.io/no-provisioner` StorageClass and exactly four `100Gi`
Local PVs for the fault StorageClass. Each PV must point at the host path created
above and must use node affinity for its real node name.

```bash
export DM_STORAGE_CLASS=rustfs-fault-dm
export DM_MOUNT_PATH=/data/rustfs/rustfs-fault-lab/volume

kubectl apply -f - <<EOF
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: ${DM_STORAGE_CLASS}
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Retain
EOF
```

Repeat this PV manifest for each of the four worker nodes, changing
`<pv-name>` and `<node-name>` each time:

```bash
kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolume
metadata:
  name: <pv-name>
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
spec:
  capacity:
    storage: 100Gi
  volumeMode: Filesystem
  accessModes:
    - ReadWriteOnce
  persistentVolumeReclaimPolicy: Retain
  storageClassName: ${DM_STORAGE_CLASS}
  local:
    path: ${DM_MOUNT_PATH}
  nodeAffinity:
    required:
      nodeSelectorTerms:
        - matchExpressions:
            - key: kubernetes.io/hostname
              operator: In
              values:
                - <node-name>
EOF
```

Pre-create or update the fault namespace so the helper pod can run privileged
and the runner can prove ownership:

```bash
export RUSTFS_FAULT_TEST_NAMESPACE="${RUSTFS_FAULT_TEST_NAMESPACE:-rustfs-fault-test}"
export RUSTFS_FAULT_TEST_TENANT="${RUSTFS_FAULT_TEST_TENANT:-fault-test-tenant}"

kubectl create namespace "$RUSTFS_FAULT_TEST_NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -
kubectl label namespace "$RUSTFS_FAULT_TEST_NAMESPACE" \
  app.kubernetes.io/managed-by=rustfs-operator-fault-test \
  pod-security.kubernetes.io/enforce=privileged \
  --overwrite
kubectl annotate namespace "$RUSTFS_FAULT_TEST_NAMESPACE" \
  "rustfs.com/fault-test-tenant=$RUSTFS_FAULT_TEST_TENANT" \
  --overwrite
```

Verify the storage setup before running the scenario:

```bash
kubectl get storageclass "$DM_STORAGE_CLASS"
kubectl get pv -o wide | grep "$DM_STORAGE_CLASS"
kubectl get namespace "$RUSTFS_FAULT_TEST_NAMESPACE" --show-labels
```

The `dm-flakey` preflight requires exactly four `Available` or `Bound` `100Gi`
PVs in the selected static StorageClass.

### dm-flakey Run

Required variables on the machine that runs the e2e command:

```bash
export RUSTFS_FAULT_TEST_SERVER_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
export RUSTFS_FAULT_TEST_STORAGE_CLASS=rustfs-fault-dm
export RUSTFS_FAULT_TEST_DM_NAME=rustfs-fault-dm
export RUSTFS_FAULT_TEST_DM_NODE=<dm-node-name>
export RUSTFS_FAULT_TEST_DM_MOUNT_PATH=/data/rustfs/rustfs-fault-lab/volume
export RUSTFS_FAULT_TEST_DM_FAULT_TABLE='0 <sectors> flakey <backing-device> 0 1 15'
```

Use the `SECTORS` and `BACKING` values from the DM node host-storage setup for
`<sectors>` and `<backing-device>`.

Optional:

```bash
export RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE='<dmsetup recovery table>'
export RUSTFS_FAULT_TEST_DM_HELPER_IMAGE='rancher/mirrored-library-busybox:1.37.0'
```

Run:

```bash
make -C e2e fault-preflight SCENARIO=dm-flakey
make -C e2e fault-run-dm
```

The Rust test reads the original `dmsetup table` as the recovery table when
`RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE` is unset. On normal failure paths it
restores that table, but operators must still verify host storage manually after
the run.

### dm-flakey Cleanup

`fault-cleanup` removes the owned Kubernetes namespace and managed Chaos
resources only. It does not remove the static StorageClass, PVs, loop devices,
mounts, or device-mapper device.

```bash
make -C e2e fault-cleanup
kubectl delete pv -l app.kubernetes.io/managed-by=rustfs-operator-fault-test
kubectl delete storageclass rustfs-fault-dm
```

On the DM node:

```bash
sudo umount /data/rustfs/rustfs-fault-lab/volume
sudo dmsetup remove rustfs-fault-dm
sudo losetup -j /data/rustfs/rustfs-fault-lab/disk.img
sudo losetup -d <loop-device>
sudo rm -rf /data/rustfs/rustfs-fault-lab
```

On the other three nodes:

```bash
sudo umount /data/rustfs/rustfs-fault-lab/volume
sudo losetup -j /data/rustfs/rustfs-fault-lab/disk.img
sudo losetup -d <loop-device>
sudo rm -rf /data/rustfs/rustfs-fault-lab
```
