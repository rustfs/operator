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

# RustFS Fault-Test Operations / RustFS 故障测试操作手册

本手册是 Agent 和开发人员使用 `e2e` package 故障测试工具的唯一操作入口。它说明执行步骤、步骤原因、安全边界、验收证据和清理方式。

This manual is the single operational entry point for agents and developers using the fault-test tooling in the `e2e` package. Fault-test commands, prerequisites, safety limits, evidence, and cleanup are intentionally kept here instead of duplicated in README files.

## 1. Purpose And Safety / 目的与安全边界

故障测试只允许在专用真实 Kubernetes 测试集群执行。测试会创建并删除专用 Tenant、PVC、Pod、Service、StatefulSet 和 Chaos resources。禁止把测试 namespace、Tenant、StorageClass 或 DM 路径指向现有业务资源。

Run fault tests only in a dedicated real Kubernetes test cluster. The suite creates and removes a dedicated Tenant, PVCs, Pods, Services, StatefulSets, and Chaos resources. Never point its namespace, Tenant, StorageClass, or DM path at application resources.

固定测试所有权：

```text
namespace:  rustfs-fault-test
tenant:     fault-test-tenant
manager:    app.kubernetes.io/managed-by=rustfs-operator-fault-test
annotation: rustfs.com/fault-test-tenant=fault-test-tenant
```

安全规则 / Safety rules:

- 当前 context 必须与 `RUSTFS_FAULT_TEST_EXPECTED_CONTEXT` 完全一致，并且不能是 `kind-*`。
- 四个 RustFS 测试 Pod 必须调度到至少四个 Ready 节点。
- 常规场景使用独立动态 StorageClass；`dm-flakey` 使用独立静态 Local PV StorageClass。
- Make 编排器会监控 Ready 节点数量；常规 Chaos 场景还会监控 Chaos Mesh 健康。其他 Tenant 不参与 preflight、health guard 或通过判定。
- `fault-cleanup` 只删除带正确所有权标记的 namespace 和 Chaos，不删除外部 StorageClass、PV 或主机设备。
- The current context must exactly match `RUSTFS_FAULT_TEST_EXPECTED_CONTEXT` and must not be `kind-*`.
- The four RustFS test Pods require at least four Ready schedulable nodes.
- Regular scenarios use a dedicated dynamic StorageClass; `dm-flakey` uses a dedicated static Local PV StorageClass.
- The Make runner monitors the Ready node count, and regular Chaos scenarios also monitor Chaos Mesh health. Other Tenants do not participate in preflight, health-guard, or pass/fail decisions.
- `fault-cleanup` removes only the owned namespace and managed Chaos. It never removes external StorageClasses, PVs, or host devices.

## 2. Workload Profile / 工作负载

每个场景使用 seed 确定性生成对象内容和尺寸顺序。未设置 `RUSTFS_FAULT_TEST_SEED` 时自动生成 seed；所有重放信息写入 `workload-plan.json` 和 `history.jsonl`。

Each scenario deterministically generates object content and size order from a seed. A seed is generated when `RUSTFS_FAULT_TEST_SEED` is unset. Replay information is recorded in `workload-plan.json` and `history.jsonl`.

| Size | Weight | Objects |
| --- | ---: | ---: |
| 4KiB | 85% | 34,000 |
| 16KiB | 10% | 4,000 |
| 8MiB | 4% | 1,600 |
| 16MiB | 1% | 400 |

```text
objects:           40,000
concurrency:       80
payload/scenario:  20,337,459,200 bytes (~18.94GiB)
PVCs:              4 × 100Gi
maximum fault TTL: 7,200 seconds
```

7,200 秒是故障资源的最大保护时间，不是固定等待时间。正常测试在 workload 完成后立即恢复故障。较长 TTL 防止 40,000 对象 workload 在完成前超过 Chaos duration。

The 7,200-second duration is a maximum fault-resource safety window, not a fixed wait. Successful runs recover immediately after the workload. The larger window prevents the 40,000-object workload from outliving Chaos.

Tenant `Ready` 之后、注入故障之前，以及故障恢复之后，测试都会等待四个 RustFS Pod 连续 60 秒保持 `Running/Ready`，且 Pod UID 和容器重启数不变。这个稳定窗口避免把启动期 DNS 或 Pod 重启抖动误判为故障注入结果。

After Tenant `Ready`, both before injection and after recovery, the test requires all four RustFS Pods to remain `Running/Ready` for 60 seconds with unchanged Pod UIDs and container restart counts. This stability window prevents startup DNS or restart churn from being misclassified as a fault-injection result.

## 3. Package Commands / Package 命令

所有公共入口都位于 `e2e/Makefile`。从仓库根目录执行：

All public entry points are in `e2e/Makefile`. Run them from the repository root:

```bash
make -C e2e help
make -C e2e fault-check
make -C e2e fault-preflight SCENARIO=io-eio
make -C e2e fault-run SCENARIO=io-eio
make -C e2e fault-run-regular
make -C e2e fault-run-dm
make -C e2e fault-cleanup
```

| Target | Behavior / 行为 |
| --- | --- |
| `fault-check` | 单 job Rust fmt/test/clippy 和 Bash 语法检查；不访问集群。 / Single-job Rust fmt, tests, clippy, and Bash syntax; no cluster mutation. |
| `fault-preflight` | 校验 context、CRD、StorageClass、Chaos、节点和 namespace 所有权。 / Validates context, CRDs, storage, Chaos, nodes, and namespace ownership. |
| `fault-run` | 运行一个场景，持续健康守护并验收 artifacts。 / Runs one guarded scenario and validates artifacts. |
| `fault-run-regular` | 串行运行六个常规场景，首败停止。 / Runs six regular scenarios serially and stops on first failure. |
| `fault-run-dm` | 使用预先准备的静态 PV 和 DM 设备运行 `dm-flakey`。 / Runs `dm-flakey` with pre-provisioned static PVs and DM storage. |
| `fault-cleanup` | 安全删除 owned namespace 和 managed Chaos。 / Safely removes the owned namespace and managed Chaos. |

`fault-run*` 会先用单 job、最低主机优先级预编译精确的 `faults` 测试二进制。故障窗口直接运行该二进制，不再次调用 Cargo。预编译不计入故障窗口；预编译前后都会重新执行 preflight。

Before creating a fault Tenant, every `fault-run*` target prebuilds the exact `faults` binary with one job and the lowest host priority. The fault window executes that binary directly without invoking Cargo again. Compilation is outside the fault window, and the runner reruns preflight before and after prebuild.

### 3.1 Recommended Flow / 推荐执行顺序

1. 运行 `make -C e2e fault-check`，先确认本地代码、脚本和普通测试可用。 / Run `make -C e2e fault-check` first to validate code, scripts, and non-live tests.
2. 准备真实测试集群、专用 StorageClass、Chaos Mesh 和固定 digest 的 RustFS image。 / Prepare the real test cluster, dedicated StorageClass, Chaos Mesh, and a pinned RustFS image digest.
3. 导出 `RUSTFS_FAULT_TEST_EXPECTED_CONTEXT`、`RUSTFS_FAULT_TEST_STORAGE_CLASS` 和 `RUSTFS_FAULT_TEST_SERVER_IMAGE`。 / Export the required context, StorageClass, and image variables.
4. 先执行 `make -C e2e fault-preflight SCENARIO=io-eio`，再单独跑 `io-eio`。 / Run `io-eio` preflight first, then run `io-eio` alone.
5. `io-eio` 通过后再执行 `make -C e2e fault-run-regular`。 / After `io-eio` passes, run the remaining regular scenarios with `fault-run-regular`.
6. 只有准备好静态 Local PV 和 Device Mapper 后，才执行 `make -C e2e fault-run-dm`。 / Run `fault-run-dm` only after static Local PVs and Device Mapper are ready.
7. 结束后先收集 artifacts，再执行 `make -C e2e fault-cleanup`。 / Collect artifacts before running `fault-cleanup`.

## 4. Cluster Preparation / 集群准备

### 4.1 Required Tools / 必需工具

```bash
rustc --version
cargo --version
kubectl version --client
jq --version
make -C e2e fault-check
```

`warp` v1.3.1 仅用于 `warp-under-chaos`。运行机必须能访问 Kubernetes API；如果设置 ClusterIP 直连，还必须能访问 Service ClusterIP。

`warp` v1.3.1 is required only for `warp-under-chaos`. The runner must reach the Kubernetes API and, when ClusterIP mode is enabled, Service ClusterIPs.

### 4.2 Kubernetes And Storage / Kubernetes 与存储

```bash
kubectl config current-context
kubectl get nodes
kubectl get crd tenants.rustfs.com
kubectl get storageclass
kubectl get tenant -A
```

常规场景要求动态 StorageClass。每个承载测试 PVC 的节点应在实际 provisioner 路径上至少有 120Gi 可用空间。hostPath/local-path 的 PVC capacity 通常不执行真实配额，必须检查后端文件系统，而不能只看 `kubectl get pvc`。

Regular scenarios require a dynamic StorageClass. Every node that can host a test PVC should have at least 120Gi available on the actual provisioner filesystem. hostPath/local-path capacity is commonly not enforced, so inspect the backing filesystem instead of trusting only `kubectl get pvc`.

```bash
kubectl -n kube-system get configmap local-path-config -o yaml
kubectl get pv -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.spec.hostPath.path}{"\n"}{end}'
df -h <actual-provisioner-path>
```

如果 K3s 默认 `/var/lib/rancher/k3s/storage` 位于小系统盘，应创建独立 provisioner/StorageClass，把 fault-test PVC 放到 `/data/rustfs/rustfs-fault-local-path` 等专用数据盘目录。不得修改现有业务 PVC 或默认 provisioner。

If K3s stores its default local-path data on a small system disk, create an independent provisioner and StorageClass backed by a dedicated data-disk path such as `/data/rustfs/rustfs-fault-local-path`. Do not modify existing application PVCs or the default provisioner.

### 4.3 Chaos Mesh / Chaos Mesh

已验证版本为 Chaos Mesh v2.8.3：

The validated version is Chaos Mesh v2.8.3:

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
kubectl get crd iochaos.chaos-mesh.org podchaos.chaos-mesh.org networkchaos.chaos-mesh.org
```

非 K3s 集群必须使用实际 container runtime socket。

Non-K3s clusters must use their actual container runtime socket.

## 5. Regular Scenarios / 常规场景

先固定 context、动态 StorageClass 和 RustFS image digest。测试机位于集群节点或 Pod 内时使用 ClusterIP，避免 80 并发经过 `kubectl port-forward`。

Pin the context, dynamic StorageClass, and RustFS image digest. Use ClusterIP when the runner is on a cluster node or in a Pod so 80 concurrent requests do not traverse `kubectl port-forward`.

```bash
export RUSTFS_FAULT_TEST_EXPECTED_CONTEXT=default
export RUSTFS_FAULT_TEST_STORAGE_CLASS=<dedicated-dynamic-storage-class>
export RUSTFS_FAULT_TEST_SERVER_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
export RUSTFS_FAULT_TEST_USE_CLUSTER_IP=1
export RUSTFS_FAULT_TEST_RUN_ROOT="$PWD/e2e/target/fault-tests/$(date -u +%Y%m%dT%H%M%SZ)"

make -C e2e fault-preflight SCENARIO=io-eio
make -C e2e fault-run SCENARIO=io-eio
```

场景顺序 / Scenario order:

```text
io-eio
pod-kill-one
network-partition-one
io-read-mistake
disk-full
warp-under-chaos
```

完整运行：

Run all regular scenarios:

```bash
make -C e2e fault-run-regular
```

分阶段验证时，可以先运行 `io-eio`，再通过 `RUSTFS_FAULT_TEST_SCENARIOS` 指定剩余场景：

For staged validation, run `io-eio` first and then select the remaining scenarios with `RUSTFS_FAULT_TEST_SCENARIOS`:

```bash
export RUSTFS_FAULT_TEST_SCENARIOS='pod-kill-one network-partition-one io-read-mistake disk-full warp-under-chaos'
make -C e2e fault-run-regular
unset RUSTFS_FAULT_TEST_SCENARIOS
```

测试可能持续数小时。不要并行运行场景。每个场景完成后编排脚本会校验 seed、尺寸分布、故障状态、40,000 committed PUT 和 checker verdict。

The suite can run for several hours. Do not run scenarios in parallel. After every scenario, the runner validates the seed, size distribution, fault state, 40,000 committed PUTs, and checker verdict.

## 6. dm-flakey / dm-flakey

`dm-flakey` 不需要重装 Kubernetes、Operator、Chaos Mesh 或 Rust。它只需要把 fault Tenant 的存储切换为四个专用静态 Local PV，其中一个 PV 由 Device Mapper 提供。

`dm-flakey` does not require reinstalling Kubernetes, the Operator, Chaos Mesh, or Rust. It only switches the fault Tenant to four dedicated static Local PVs, one backed by Device Mapper.

### 6.1 Host Storage / 主机存储

真实专用块设备优先。loop 文件仅适用于实验室。每个 backing 至少 120Gi，并且路径必须只服务 fault-test。

Prefer dedicated block devices. Loop files are for lab use only. Each backing device must be at least 120Gi and serve only fault-test.

DM 节点示例 / DM-node example:

```bash
export LAB=/data/rustfs/rustfs-fault-lab
export DM_NAME=rustfs-fault-dm
sudo mkdir -p "$LAB/volume"
sudo truncate -s 120G "$LAB/disk.img"
export BACKING=$(sudo losetup --find --show "$LAB/disk.img")
export SECTORS=$(sudo blockdev --getsz "$BACKING")
sudo dmsetup create "$DM_NAME" --table "0 $SECTORS linear $BACKING 0"
sudo mkfs.ext4 -F "/dev/mapper/$DM_NAME"
sudo mount "/dev/mapper/$DM_NAME" "$LAB/volume"
sudo chmod 0777 "$LAB/volume"
```

其他三个节点 / Other three nodes:

```bash
export LAB=/data/rustfs/rustfs-fault-lab
sudo mkdir -p "$LAB/volume"
sudo truncate -s 120G "$LAB/disk.img"
export BACKING=$(sudo losetup --find --show "$LAB/disk.img")
sudo mkfs.ext4 -F "$BACKING"
sudo mount "$BACKING" "$LAB/volume"
sudo chmod 0777 "$LAB/volume"
```

### 6.2 Static StorageClass And PVs / 静态 StorageClass 与 PV

创建 `kubernetes.io/no-provisioner` StorageClass，并为四个节点各创建一个 `100Gi` Local PV。每个 PV 的 node affinity 必须匹配实际节点；`local.path` 必须是 `/data/rustfs/rustfs-fault-lab/volume`。

Create a `kubernetes.io/no-provisioner` StorageClass and one `100Gi` Local PV per node. Each PV must use the matching node affinity and `/data/rustfs/rustfs-fault-lab/volume` as `local.path`.

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: rustfs-fault-dm
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Retain
---
apiVersion: v1
kind: PersistentVolume
metadata:
  name: rustfs-fault-dm-<node>
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
spec:
  capacity:
    storage: 100Gi
  volumeMode: Filesystem
  accessModes: [ReadWriteOnce]
  persistentVolumeReclaimPolicy: Retain
  storageClassName: rustfs-fault-dm
  local:
    path: /data/rustfs/rustfs-fault-lab/volume
  nodeAffinity:
    required:
      nodeSelectorTerms:
        - matchExpressions:
            - key: kubernetes.io/hostname
              operator: In
              values: [<node>]
```

验证四个 PV 为 `Available`：

Verify all four PVs are `Available`:

```bash
kubectl get storageclass rustfs-fault-dm
kubectl get pv -l app.kubernetes.io/managed-by=rustfs-operator-fault-test -o wide
```

helper Pod 需要 privileged Pod Security。复用常规场景创建的 namespace 时补充 label；如果 namespace 不存在，则预创建带完整所有权的 namespace：

The helper Pod requires privileged Pod Security. Label the namespace left by regular scenarios, or pre-create an owned namespace when it does not exist:

```bash
if kubectl get namespace rustfs-fault-test >/dev/null 2>&1; then
  kubectl label namespace rustfs-fault-test \
    pod-security.kubernetes.io/enforce=privileged --overwrite
else
  kubectl create namespace rustfs-fault-test
  kubectl label namespace rustfs-fault-test \
    app.kubernetes.io/managed-by=rustfs-operator-fault-test \
    pod-security.kubernetes.io/enforce=privileged
  kubectl annotate namespace rustfs-fault-test \
    rustfs.com/fault-test-tenant=fault-test-tenant
fi
```

### 6.3 Run / 执行

```bash
export RUSTFS_FAULT_TEST_STORAGE_CLASS=rustfs-fault-dm
export RUSTFS_FAULT_TEST_DM_NAME=rustfs-fault-dm
export RUSTFS_FAULT_TEST_DM_NODE=<dm-node-name>
export RUSTFS_FAULT_TEST_DM_MOUNT_PATH=/data/rustfs/rustfs-fault-lab/volume
export RUSTFS_FAULT_TEST_DM_FAULT_TABLE="0 $SECTORS flakey $BACKING 0 1 15"

make -C e2e fault-preflight SCENARIO=dm-flakey
make -C e2e fault-run-dm
```

## 7. Evidence And Acceptance / 证据与验收

每个场景目录至少包含：

Each scenario directory contains at least:

```text
test.log
health-watch.log
run-metadata.json
workload-plan.json
history.jsonl
workload-summary.json
recommit-report.json
checker-report.json
fault-evidence.json
failure-summary.json / runner-failure-summary.json (failure only)
nodes-before.txt / nodes-after.txt
tenants-before.txt / tenants-after.txt
pods-before.txt / pods-after.txt
Chaos or DM snapshots
```

通过条件 / Pass criteria:

- 测试退出码为 0。
- `run-metadata.json` 记录 scenario、run id、context、StorageClass、RustFS image 和 workload 参数。
- `fault-evidence.json` 的 `injected`、`active_during_workload`、`recovered` 都为 `true`。
- `workload-plan.json` 精确记录 40,000 对象、80 并发和四档尺寸分布。
- `recommit-report.json` 的 `attempted == committed` 且 `failed == 0`。
- `checker-report.json` 的 `committed_puts=40000`，并且 missing、hash mismatch、successful corrupted read、LIST warning 均为空。
- fault Tenant 恢复 Ready；Ready 节点数量不下降；常规 Chaos 场景中 Chaos Mesh 保持健康。
- The test exits with zero.
- `run-metadata.json` records the scenario, run id, context, StorageClass, RustFS image, and workload parameters.
- `fault-evidence.json` reports `injected`, `active_during_workload`, and `recovered` as `true`.
- `workload-plan.json` reports exactly 40,000 objects, concurrency 80, and the four size classes.
- `recommit-report.json` reports `attempted == committed` and `failed == 0`.
- `checker-report.json` reports `committed_puts=40000` with no missing object, hash mismatch, successful corrupted read, or LIST warning.
- The fault Tenant recovers Ready, the Ready node count does not drop, and Chaos Mesh remains healthy during regular Chaos scenarios.

客户端没有看到错误并不表示故障无效。故障是否生效由 Chaos/DM 后端证据判断；客户端 disruption 单独记录。

No client-visible error does not mean the fault was inactive. Chaos/DM backend evidence proves injection; client disruption is reported separately.

## 8. Cleanup And Recovery / 清理与恢复

先运行安全清理：

Start with owned-resource cleanup:

```bash
make -C e2e fault-cleanup
```

然后由运维删除本次创建的外部 StorageClass、静态 PV、独立 provisioner 和主机设备。DM 实验室清理示例：

Operators must then remove the external StorageClass, static PVs, independent provisioner, and host devices created for the run. Lab DM cleanup example:

```bash
sudo umount /data/rustfs/rustfs-fault-lab/volume
sudo dmsetup remove rustfs-fault-dm  # DM node only
sudo losetup -d <loop-device>
sudo rm -rf /data/rustfs/rustfs-fault-lab
kubectl delete pv -l app.kubernetes.io/managed-by=rustfs-operator-fault-test
kubectl delete storageclass rustfs-fault-dm
```

最终确认 / Final checks:

```bash
kubectl get nodes
kubectl get tenant -A
kubectl -n chaos-mesh get deployment,daemonset
kubectl get iochaos,podchaos,networkchaos -A
kubectl get namespace rustfs-fault-test
```

## 9. Runtime Variables / 运行参数

| Variable | Default | Purpose / 用途 |
| --- | --- | --- |
| `RUSTFS_FAULT_TEST_EXPECTED_CONTEXT` | required | 防止在错误 context 执行。 / Prevents execution against the wrong context. |
| `RUSTFS_FAULT_TEST_STORAGE_CLASS` | required | 常规动态 SC 或 DM 静态 SC。 / Dynamic regular SC or static DM SC. |
| `RUSTFS_FAULT_TEST_SERVER_IMAGE` | required by Make | 建议固定 digest。 / Pin an image digest. |
| `RUSTFS_FAULT_TEST_RUN_ROOT` | timestamp directory | 整次运行的 artifacts 根目录。 / Artifact root for the run. |
| `RUSTFS_FAULT_TEST_SCENARIOS` | six regular scenarios | `fault-run-regular` 的空格分隔场景列表。 / Space-separated regular scenario list. |
| `RUSTFS_FAULT_TEST_SEED` | generated | 固定后可重放相同对象。 / Replays the same objects when set. |
| `RUSTFS_FAULT_TEST_USE_CLUSTER_IP` | `false` | 集群节点/Pod 内建议设为 `1`。 / Set to `1` on a node or in-cluster runner. |
| `RUSTFS_FAULT_TEST_BUILD_JOBS` | `1` | 预编译并行度；小型控制面保持为 1。 / Prebuild parallelism; keep at 1 on small control planes. |
| `RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS` | `40000` | Make runner 强制验收该值。 / Required object count. |
| `RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY` | `80` | Make runner 强制验收该值。 / Required concurrency. |
| `RUSTFS_FAULT_TEST_DURATION_SECONDS` | `7200` | 最大故障 TTL。 / Maximum fault TTL. |
| `RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS` | `30` | 单次 S3 请求超时。 / Per-request S3 timeout. |
| `RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` | `false` | 是否要求客户端可见错误。 / Whether client-visible disruption is mandatory. |
| `RUSTFS_FAULT_TEST_CHAOS_NAMESPACE` | `chaos-mesh` | Chaos resource namespace。 |
| `RUSTFS_FAULT_TEST_DM_*` | unset | `dm-flakey` 专用映射参数。 / DM mapping parameters. |
