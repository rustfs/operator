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

# RustFS Fault Injection Operations Manual / RustFS 故障注入测试操作手册

- [中文操作手册](#中文操作手册)
- [English Operations Manual](#english-operations-manual)

## 中文操作手册

### 1. 目的与范围

本手册用于在专用的真实 Kubernetes 测试集群中运行 RustFS 故障注入测试。测试对象是由 RustFS Operator 创建的测试 Tenant，不是现有业务 Tenant，也不是生产 Operator 控制面。

每次执行 `make fault-test` 只运行 `RUSTFS_FAULT_TEST_SCENARIO` 选择的一个场景，并只报告一个真实的 destructive test。七个场景必须串行执行。

测试分为两类：

1. 六个 Kubernetes-native 场景，使用 Chaos Mesh 和动态 StorageClass。
2. 一个 `dm-flakey` 场景，使用专用静态 Local PV、Linux Device Mapper 和 privileged helper Pod。

执行 `dm-flakey` 前不需要重装 Kubernetes、RustFS Operator、Chaos Mesh 或 Rust 工具链；只需要把 fault-test Tenant 的存储 fixture 切换为专用静态 Local PV。

### 2. 安全要求

必须满足以下要求：

- 只能在专用测试集群执行，禁止在生产或共享开发集群执行。
- 当前 context 不能以 `kind-` 开头。
- 不得把 `RUSTFS_FAULT_TEST_NAMESPACE` 或 `RUSTFS_FAULT_TEST_TENANT` 指向现有业务资源。
- 常规场景必须使用支持动态供给的 StorageClass。
- `dm-flakey` 只能使用专用的 `kubernetes.io/no-provisioner` StorageClass 和专用块设备或 loop 文件。
- DM Local PV 路径不得复用现有 RustFS 数据目录。
- 所有场景必须串行运行；默认本地 port-forward 端口为 `19000`。
- 失败时先保存 artifacts，再清理故障资源和测试 namespace。

测试 runner 默认创建：

```text
namespace: rustfs-fault-test
tenant:    fault-test-tenant
```

如果 namespace 已存在，必须同时具备：

```text
app.kubernetes.io/managed-by=rustfs-operator-fault-test
rustfs.com/fault-test-tenant=fault-test-tenant
```

runner 不会自动认领未标记的 namespace，也不会删除不属于它的 namespace。

### 3. 场景目录

| 场景 | 后端 | 隔离方式 | 主要验证 |
| --- | --- | --- | --- |
| `io-eio` | Chaos Mesh IOChaos | 新 Tenant/PVC | 一个数据卷发生 EIO 后，已提交对象不丢失、不损坏。 |
| `pod-kill-one` | Chaos Mesh PodChaos | 可复用 Ready Tenant | 删除一个 RustFS Pod 后，替代 Pod 出现且对象保持正确。 |
| `network-partition-one` | Chaos Mesh NetworkChaos | 可复用 Ready Tenant | 一个 Pod 与同 Tenant peers 分区后，恢复时对象保持正确。 |
| `io-read-mistake` | Chaos Mesh IOChaos | 新 Tenant/PVC | 读路径被篡改时，成功 GET 不能返回错误内容。 |
| `disk-full` | Chaos Mesh IOChaos | 新 Tenant/PVC | 写操作返回 ENOSPC 后，已提交对象保持正确。 |
| `warp-under-chaos` | Warp + IOChaos | 新 Tenant/PVC | 记录故障下性能，正确性仍由 history/checker 判断。 |
| `dm-flakey` | Linux Device Mapper | 专用静态 Local PV | 底层块设备间歇性 EIO 后，恢复时对象保持正确。 |

默认 workload 写入或确认 4000 个对象，并使用 50 并发。尺寸计划先按固定比例生成，再由 seed 确定性打乱：4KiB 85%（3400 个）、16KiB 10%（400 个）、8MiB 4%（160 个）、16MiB 1%（40 个）。每个场景的逻辑 payload 为 2,033,745,920 bytes，约 1.89GiB。

对象内容由同一个 seed 和对象索引通过 `splitmix64-v1` 确定性生成。`workload-plan.json` 记录 seed、生成器版本、并发、尺寸分布和总 payload；`history.jsonl` 记录每个 key 的 size、SHA-256 和结果。设置 `RUSTFS_FAULT_TEST_SEED=<u64>` 可以重放相同尺寸顺序和对象内容。

客户端没有看到错误不代表故障未生效；权威故障证据来自 Chaos 状态或 DM table/status，以及 `fault-evidence.json`。

`RUSTFS_FAULT_TEST_PERCENT=20` 表示 Chaos Mesh 对匹配 I/O 操作的注入概率，不表示预先固定选择 20% 的对象。

### 4. 测试机要求

运行测试的主机需要：

- `kubectl`
- Rust stable 和 Cargo，支持 Rust edition 2024
- GNU Make
- 可访问 Kubernetes API 的 kubeconfig
- `warp` v1.3.1，仅 `warp-under-chaos` 需要
- 足够空间保存 `target/fault-tests` artifacts

建议测试账户在专用测试集群使用 cluster-admin。最小权限至少需要：

- 读取 CRD、Node 和 StorageClass
- 创建、读取、更新和删除 namespace、Secret、Pod、Service、PVC、StatefulSet 和 Tenant
- 在 Chaos Mesh namespace 管理 IOChaos、PodChaos 和 NetworkChaos
- 读取 Pod 日志、events，并执行 `kubectl exec`
- `dm-flakey` 允许创建 privileged、`hostPID`、`hostPath: /` 的 helper Pod

代码检查：

```bash
rustc --version
cargo --version
kubectl version --client
make e2e-check
```

### 5. Kubernetes 和 RustFS 前置检查

切换并记录目标 context：

```bash
kubectl config use-context <real-test-cluster>
kubectl config current-context
kubectl get nodes
```

确认 RustFS Operator、Tenant CRD 和 StorageClass：

```bash
kubectl get crd tenants.rustfs.com
kubectl -n rustfs-system get deployment
kubectl get storageclass
```

常规场景需要至少四个可调度节点和四个 `80Gi` RWO PVC。fault Tenant 使用 required pod anti-affinity，把四个 RustFS Pod 分散到不同的 `kubernetes.io/hostname`。StorageClass 必须支持动态供给，不能是 `kubernetes.io/no-provisioner`。每个承载 fault-test PVC 的节点应至少有 100Gi 可用空间；执行前必须按实际 StorageClass 拓扑核对容量。

不能只看 PVC 显示的 capacity。hostPath/local-path provisioner 通常不执行容量配额，必须检查它的实际 node path 和对应文件系统：

```bash
kubectl -n kube-system get configmap local-path-config -o yaml
kubectl get pv -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.spec.hostPath.path}{"\n"}{end}'
df -h <provisioner-node-path>
```

K3s 默认 `/var/lib/rancher/k3s/storage` 经常位于较小的系统盘。若该文件系统不足 100Gi，不得用于本测试；应部署专用的动态 provisioner/StorageClass，把新 fault-test PVC 放到 `/data/rustfs/rustfs-fault-local-path` 之类的独立数据盘目录。不要修改或迁移现有业务 PVC。

建议固定已验证的 RustFS image digest，避免 `latest` 漂移：

```bash
export RUSTFS_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
```

### 6. 安装和验证 Chaos Mesh

以下示例使用已验证的 Chaos Mesh v2.8.3：

```bash
helm repo add chaos-mesh https://charts.chaos-mesh.org
helm repo update

helm upgrade --install chaos-mesh chaos-mesh/chaos-mesh \
  -n chaos-mesh --create-namespace \
  --version 2.8.3 \
  --set chaosDaemon.runtime=containerd \
  --set chaosDaemon.socketPath=/run/containerd/containerd.sock \
  --set dashboard.create=false \
  --wait --timeout 10m
```

K3s 使用：

```text
/run/k3s/containerd/containerd.sock
```

其他发行版必须根据实际容器运行时修改 `chaosDaemon.runtime` 和 `chaosDaemon.socketPath`。

验证：

```bash
kubectl -n chaos-mesh get deployment,daemonset
kubectl get crd \
  iochaos.chaos-mesh.org \
  podchaos.chaos-mesh.org \
  networkchaos.chaos-mesh.org
```

要求 controller-manager 全部 Ready，chaos-daemon 在所有目标节点 Ready。

### 7. 运行普通测试

先设置公共参数：

```bash
export RUSTFS_FAULT_TEST_STORAGE_CLASS=<dynamic-storage-class>
export RUSTFS_FAULT_TEST_SERVER_IMAGE="$RUSTFS_IMAGE"
export RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE=rustfs-system
export RUSTFS_FAULT_TEST_NAMESPACE=rustfs-fault-test
export RUSTFS_FAULT_TEST_TENANT=fault-test-tenant
export RUSTFS_FAULT_TEST_CHAOS_NAMESPACE=chaos-mesh
export RUN_ROOT="target/fault-tests/$(date -u +%Y%m%dT%H%M%SZ)"
```

运行一个场景：

```bash
RUSTFS_FAULT_TEST_SCENARIO=io-eio \
RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/io-eio" \
make fault-test
```

`make fault-test` 会在内部设置 `RUSTFS_FAULT_TEST_DESTRUCTIVE=1`。不要直接绕过 Make 入口运行 destructive test。

测试期间持续观察节点、现有业务 Tenant 和 fault-test Tenant。任一非目标资源变为非 Ready 时，应立即删除当前 managed Chaos resource、停止后续场景并收集现场。

按推荐顺序运行六个普通场景，并在首个失败后停止：

```bash
for scenario in \
  io-eio \
  pod-kill-one \
  network-partition-one \
  io-read-mistake \
  disk-full \
  warp-under-chaos
do
  RUSTFS_FAULT_TEST_SCENARIO="$scenario" \
  RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/$scenario" \
  make fault-test || break
done
```

`warp-under-chaos` 执行前验证：

```bash
warp --version
```

Warp 性能数据不参与 correctness verdict。

### 8. `dm-flakey` 专用操作

#### 8.1 不需要重装集群

如果前六个场景已经执行，只需：

1. 保留 Kubernetes、Operator、Chaos Mesh 和 Rust 工具链。
2. 停止其他 fault-test 进程。
3. 为四个测试 Pod 准备四个专用静态 Local PV。
4. 其中一个 PV 必须由 Device Mapper 设备提供。
5. 使用新的静态 StorageClass 运行 `dm-flakey`。

runner 会 reset fault-test Tenant/PVC，但不会创建主机块设备、静态 PV 或 StorageClass。

#### 8.2 允许 privileged helper

如果 fault-test namespace 已存在：

```bash
kubectl label namespace rustfs-fault-test \
  pod-security.kubernetes.io/enforce=privileged \
  --overwrite
```

如果要在第一次运行前预创建 namespace：

```bash
kubectl create namespace rustfs-fault-test
kubectl label namespace rustfs-fault-test \
  app.kubernetes.io/managed-by=rustfs-operator-fault-test \
  pod-security.kubernetes.io/enforce=privileged
kubectl annotate namespace rustfs-fault-test \
  rustfs.com/fault-test-tenant=fault-test-tenant
```

#### 8.3 准备四个专用卷

推荐使用四个真实专用测试块设备。loop 文件仅适用于实验室环境。每个 backing filesystem 建议至少 `90Gi`，静态 PV capacity 固定为 `80Gi`。

目标 DM 节点的实验室 loop 示例；使用真实专用块设备时跳过 `truncate` 和 `losetup`：

```bash
export LAB=/data/rustfs/rustfs-fault-lab
export DM_NAME=rustfs-fault-dm

mkdir -p "$LAB/volume"
truncate -s 90G "$LAB/disk.img"
BACKING=$(losetup --find --show "$LAB/disk.img")
SECTORS=$(blockdev --getsz "$BACKING")
dmsetup create "$DM_NAME" --table "0 $SECTORS linear $BACKING 0"
mkfs.ext4 -F "/dev/mapper/$DM_NAME"
mount "/dev/mapper/$DM_NAME" "$LAB/volume"
```

其他三个节点把各自专用块设备直接格式化并挂载到同一路径：

```bash
mkdir -p /data/rustfs/rustfs-fault-lab/volume
mkfs.ext4 -F <dedicated-block-device>
mount <dedicated-block-device> /data/rustfs/rustfs-fault-lab/volume
```

不得格式化或挂载现有 RustFS 数据盘。

#### 8.4 创建静态 StorageClass 和 Local PV

StorageClass：

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: rustfs-fault-dm
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Retain
```

为四个节点分别创建一个 PV。每个 PV 使用唯一名称和对应 node affinity：

```yaml
apiVersion: v1
kind: PersistentVolume
metadata:
  name: rustfs-fault-dm-<node-name>
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
spec:
  capacity:
    storage: 80Gi
  volumeMode: Filesystem
  accessModes:
    - ReadWriteOnce
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
              values:
                - <node-name>
```

验证四个 PV 均为 `Available`：

```bash
kubectl get storageclass rustfs-fault-dm
kubectl get pv -l app.kubernetes.io/managed-by=rustfs-operator-fault-test -o wide
```

#### 8.5 运行 `dm-flakey`

目标节点名必须是 Kubernetes `metadata.name`，挂载路径必须与目标 PV 的 `spec.local.path` 完全一致。

先在目标节点执行 `blockdev --getsz <target-node-backing-device>`，再把结果设置为测试机上的 `SECTORS`。

```bash
export DM_NODE=<kubernetes-node-name>
export DM_MOUNT_PATH=/data/rustfs/rustfs-fault-lab/volume
export BACKING_DEVICE=<target-node-backing-device>
export SECTORS=<value-from-blockdev-getsz-on-target-node>

RUSTFS_FAULT_TEST_SCENARIO=dm-flakey \
RUSTFS_FAULT_TEST_STORAGE_CLASS=rustfs-fault-dm \
RUSTFS_FAULT_TEST_SERVER_IMAGE="$RUSTFS_IMAGE" \
RUSTFS_FAULT_TEST_DM_NAME=rustfs-fault-dm \
RUSTFS_FAULT_TEST_DM_NODE="$DM_NODE" \
RUSTFS_FAULT_TEST_DM_MOUNT_PATH="$DM_MOUNT_PATH" \
RUSTFS_FAULT_TEST_DM_FAULT_TABLE="0 $SECTORS flakey $BACKING_DEVICE 0 1 15" \
RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/dm-flakey" \
make fault-test
```

该 table 表示底层设备正常 1 秒、故障 15 秒并循环。helper 会验证 Pod、PVC、PV、节点、Local PV 路径和 Device Mapper mount source 的关系，然后加载 fault table。恢复时使用注入前的 linear table。

#### 8.6 DM 紧急恢复

如果测试进程异常退出且设备仍为 flakey，立即在目标节点执行：

```bash
dmsetup suspend --noflush rustfs-fault-dm
dmsetup load rustfs-fault-dm \
  --table "0 $SECTORS linear $BACKING_DEVICE 0"
dmsetup resume --noudevsync rustfs-fault-dm
dmsetup table rustfs-fault-dm
```

确认 table 已恢复为 `linear` 后再删除测试 Pod、PVC 或卸载文件系统。

### 9. 验收标准

每个场景必须满足：

- `make fault-test` 退出码为 0。
- `fault-evidence.json` 中 `injected=true`、`active_during_workload=true`、`recovered=true`。
- `checker-report.json` 中 `committed_puts=4000`。
- `missing_committed_objects` 为空。
- `hash_mismatches` 为空。
- `successful_corrupted_reads` 为空。
- `list_warnings` 为空。
- fault-test Tenant 恢复 Ready。

`RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` 默认是 `false`。因此客户端没有失败或超时可以接受，只要故障后端明确证明故障已选中并注入。

主要 artifacts：

```text
history.jsonl
workload-plan.json
workload-summary.json
checker-report.json
fault-evidence.json
chaos-manifest.yaml
dm-flakey-active.json
Kubernetes logs/events/snapshots
```

### 10. 清理

先确认 namespace 所有权：

```bash
kubectl get namespace rustfs-fault-test --show-labels
kubectl get namespace rustfs-fault-test \
  -o jsonpath='{.metadata.annotations.rustfs\.com/fault-test-tenant}{"\n"}'
```

清理测试资源：

```bash
kubectl delete namespace rustfs-fault-test --wait=true
kubectl delete iochaos,podchaos,networkchaos \
  -n chaos-mesh \
  -l app.kubernetes.io/managed-by=rustfs-operator-fault-test \
  --ignore-not-found
```

动态 PV 是否自动删除取决于 StorageClass reclaim policy。`Retain` PV 必须由运维手动删除并清理后端数据。

DM 场景额外清理：

1. 删除 fault-test namespace，等待 Pod/PVC 消失。
2. 删除四个静态 PV 和 `rustfs-fault-dm` StorageClass。
3. 在目标节点确认 DM table 为 `linear`。
4. 卸载四个实验卷。
5. 删除 DM mapping。
6. detach loop 设备并删除专用实验目录。

示例：

```bash
umount /data/rustfs/rustfs-fault-lab/volume
dmsetup remove rustfs-fault-dm
losetup -d <loop-device>   # 仅 loop 实验环境
rm -rf /data/rustfs/rustfs-fault-lab
```

最后确认：

```bash
kubectl get nodes
kubectl -n rustfs-system get deployment
kubectl -n chaos-mesh get deployment,daemonset
kubectl get pv
kubectl get iochaos,podchaos,networkchaos -A
```

### 11. 常用环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `RUSTFS_FAULT_TEST_STORAGE_CLASS` | 必填 | 常规动态 StorageClass 或 DM 专用静态 StorageClass。 |
| `RUSTFS_FAULT_TEST_DESTRUCTIVE` | 由 Make 设置 | destructive opt-in，不应手动绕过 Make 入口。 |
| `RUSTFS_FAULT_TEST_SCENARIO` | `io-eio` | 选择七个场景之一。 |
| `RUSTFS_FAULT_TEST_NAMESPACE` | `rustfs-fault-test` | 专用测试 namespace。 |
| `RUSTFS_FAULT_TEST_TENANT` | `fault-test-tenant` | 专用测试 Tenant。 |
| `RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE` | `rustfs-system` | Operator namespace。 |
| `RUSTFS_FAULT_TEST_SERVER_IMAGE` | `rustfs/rustfs:latest` | 建议设置为已验证 digest。 |
| `RUSTFS_FAULT_TEST_ARTIFACTS` | `target/fault-tests/artifacts` | 当前场景 artifacts 目录。 |
| `RUSTFS_FAULT_TEST_TIMEOUT_SECONDS` | `300` | Kubernetes/Tenant 等待超时。 |
| `RUSTFS_FAULT_TEST_DURATION_SECONDS` | `900` | Chaos 故障持续时间。 |
| `RUSTFS_FAULT_TEST_PERCENT` | `20`；`disk-full` 为 `100` | 支持百分比的故障注入比例。 |
| `RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS` | `4000` | workload 对象数量。 |
| `RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY` | `50` | prefill、故障 workload、恢复重写和 checker 的最大并发。 |
| `RUSTFS_FAULT_TEST_SEED` | 随机生成 | 可选 u64 seed；设置后可重放尺寸顺序和对象内容。 |
| `RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS` | `30` | 单个 S3 操作超时。 |
| `RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` | `false` | 是否强制要求客户端看到故障。 |
| `RUSTFS_FAULT_TEST_CHAOS_NAMESPACE` | `chaos-mesh` | Chaos Mesh resource namespace。 |
| `RUSTFS_FAULT_TEST_WARP_DURATION_SECONDS` | `60` | Warp mixed workload 时间。 |
| `RUSTFS_FAULT_TEST_DM_NAME` | 无 | DM mapping 名称，DM 场景必填。 |
| `RUSTFS_FAULT_TEST_DM_NODE` | 无 | DM 目标 Kubernetes 节点，DM 场景必填。 |
| `RUSTFS_FAULT_TEST_DM_MOUNT_PATH` | 无 | DM Local PV 路径，DM 场景必填。 |
| `RUSTFS_FAULT_TEST_DM_FAULT_TABLE` | 无 | 注入时的 dmsetup table，DM 场景必填。 |
| `RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE` | 注入前 table | 可选恢复 table。 |
| `RUSTFS_FAULT_TEST_DM_HELPER_IMAGE` | `rancher/mirrored-library-busybox:1.37.0` | privileged helper image。 |

## English Operations Manual

### 1. Purpose and scope

This manual describes how to run RustFS fault-injection tests in a dedicated, real Kubernetes test cluster. The target is the test Tenant created by the RustFS Operator, not an existing application Tenant or the production Operator control plane.

Each `make fault-test` invocation runs exactly one destructive test selected by `RUSTFS_FAULT_TEST_SCENARIO`. Run all seven scenarios serially.

The suite has two operational groups:

1. Six Kubernetes-native scenarios using Chaos Mesh and a dynamic StorageClass.
2. One `dm-flakey` scenario using dedicated static Local PVs, Linux Device Mapper, and a privileged helper Pod.

Running `dm-flakey` does not require reinstalling Kubernetes, the RustFS Operator, Chaos Mesh, or the Rust toolchain. Only the fault-test Tenant storage fixture must be replaced with dedicated static Local PVs.

### 2. Safety requirements

- Run only in a dedicated test cluster; never use a production or shared development cluster.
- The current context must not start with `kind-`.
- Never point the configured namespace or Tenant at existing application resources.
- Use a dynamically provisioned StorageClass for regular scenarios.
- Use a dedicated `kubernetes.io/no-provisioner` StorageClass and dedicated devices or loop files for `dm-flakey`.
- Never reuse an existing RustFS data directory for a DM Local PV.
- Run scenarios serially because the default namespace, Tenant, and local port `19000` are shared.
- On failure, preserve artifacts before removing the fault and test resources.

The default test resources are:

```text
namespace: rustfs-fault-test
tenant:    fault-test-tenant
```

An existing namespace must contain both ownership markers:

```text
app.kubernetes.io/managed-by=rustfs-operator-fault-test
rustfs.com/fault-test-tenant=fault-test-tenant
```

The runner never claims an unmarked namespace.

### 3. Scenario catalog

| Scenario | Backend | Isolation | Main validation |
| --- | --- | --- | --- |
| `io-eio` | Chaos Mesh IOChaos | Fresh Tenant/PVC | Committed objects survive EIO on one data volume. |
| `pod-kill-one` | Chaos Mesh PodChaos | Reusable Ready Tenant | A killed Pod is replaced without losing committed objects. |
| `network-partition-one` | Chaos Mesh NetworkChaos | Reusable Ready Tenant | Objects remain correct after one Pod is partitioned from its peers. |
| `io-read-mistake` | Chaos Mesh IOChaos | Fresh Tenant/PVC | A successful GET never returns altered bytes. |
| `disk-full` | Chaos Mesh IOChaos | Fresh Tenant/PVC | Committed objects survive injected ENOSPC write failures. |
| `warp-under-chaos` | Warp + IOChaos | Fresh Tenant/PVC | Performance is reported separately from correctness. |
| `dm-flakey` | Linux Device Mapper | Dedicated static Local PV | Objects remain correct after intermittent block-device EIO. |

The default workload commits or reconciles 4000 objects with concurrency 50. The size plan is generated with fixed weights and then deterministically shuffled by the seed: 4KiB 85% (3400 objects), 16KiB 10% (400), 8MiB 4% (160), and 16MiB 1% (40). The logical payload per scenario is 2,033,745,920 bytes, approximately 1.89GiB.

Object content is deterministically generated from the same seed and object index by `splitmix64-v1`. `workload-plan.json` records the seed, generator version, concurrency, size distribution, and total payload. `history.jsonl` records each key's size, SHA-256, and outcome. Set `RUSTFS_FAULT_TEST_SEED=<u64>` to replay the same size order and object content.

A lack of client-visible errors does not mean that injection failed. Backend state and `fault-evidence.json` are the authoritative fault evidence.

`RUSTFS_FAULT_TEST_PERCENT=20` is an injection probability for matching I/O operations, not a fixed selection of 20 percent of the objects.

### 4. Runner requirements

The runner host needs:

- `kubectl`
- Rust stable and Cargo with Rust edition 2024 support
- GNU Make
- A kubeconfig that can reach the target Kubernetes API
- `warp` v1.3.1 for `warp-under-chaos`
- Sufficient space for `target/fault-tests` artifacts

Cluster-admin is recommended in a dedicated test cluster. At minimum, the account needs CRUD access to the fault-test Kubernetes resources and Chaos CRs, Pod logs/events/exec access, and permission to create the privileged DM helper Pod.

Validate the code and tools:

```bash
rustc --version
cargo --version
kubectl version --client
make e2e-check
```

### 5. Kubernetes and RustFS preflight

```bash
kubectl config use-context <real-test-cluster>
kubectl config current-context
kubectl get nodes
kubectl get crd tenants.rustfs.com
kubectl -n rustfs-system get deployment
kubectl get storageclass
```

Regular scenarios require four schedulable nodes and four `80Gi` RWO PVCs. The fault Tenant uses required Pod anti-affinity to spread the four RustFS Pods across distinct `kubernetes.io/hostname` values. The selected StorageClass must support dynamic provisioning and must not use `kubernetes.io/no-provisioner`. Each node that hosts a fault-test PVC should have at least 100Gi available; verify capacity against the actual StorageClass topology before running.

Do not trust the capacity displayed on a PVC alone. hostPath/local-path provisioners commonly do not enforce capacity. Inspect the actual node path and its backing filesystem:

```bash
kubectl -n kube-system get configmap local-path-config -o yaml
kubectl get pv -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.spec.hostPath.path}{"\n"}{end}'
df -h <provisioner-node-path>
```

The K3s default `/var/lib/rancher/k3s/storage` is often on a smaller system disk. If that filesystem has less than 100Gi available, do not use it for this suite. Deploy a dedicated dynamic provisioner/StorageClass that places new fault-test PVCs under an isolated data-disk path such as `/data/rustfs/rustfs-fault-local-path`. Do not modify or migrate existing application PVCs.

Pin a validated RustFS image digest instead of using `latest`:

```bash
export RUSTFS_IMAGE='docker.io/rustfs/rustfs@sha256:<digest>'
```

### 6. Install and validate Chaos Mesh

The following example uses the validated Chaos Mesh v2.8.3 release:

```bash
helm repo add chaos-mesh https://charts.chaos-mesh.org
helm repo update

helm upgrade --install chaos-mesh chaos-mesh/chaos-mesh \
  -n chaos-mesh --create-namespace \
  --version 2.8.3 \
  --set chaosDaemon.runtime=containerd \
  --set chaosDaemon.socketPath=/run/containerd/containerd.sock \
  --set dashboard.create=false \
  --wait --timeout 10m
```

K3s uses `/run/k3s/containerd/containerd.sock`. Adjust the runtime and socket path for other distributions.

```bash
kubectl -n chaos-mesh get deployment,daemonset
kubectl get crd \
  iochaos.chaos-mesh.org \
  podchaos.chaos-mesh.org \
  networkchaos.chaos-mesh.org
```

All controller-manager replicas and all target-node chaos-daemon Pods must be Ready.

### 7. Run the regular scenarios

Set common parameters:

```bash
export RUSTFS_FAULT_TEST_STORAGE_CLASS=<dynamic-storage-class>
export RUSTFS_FAULT_TEST_SERVER_IMAGE="$RUSTFS_IMAGE"
export RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE=rustfs-system
export RUSTFS_FAULT_TEST_NAMESPACE=rustfs-fault-test
export RUSTFS_FAULT_TEST_TENANT=fault-test-tenant
export RUSTFS_FAULT_TEST_CHAOS_NAMESPACE=chaos-mesh
export RUN_ROOT="target/fault-tests/$(date -u +%Y%m%dT%H%M%SZ)"
```

Run one scenario:

```bash
RUSTFS_FAULT_TEST_SCENARIO=io-eio \
RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/io-eio" \
make fault-test
```

`make fault-test` sets `RUSTFS_FAULT_TEST_DESTRUCTIVE=1` internally. Do not bypass the Make entry point to invoke the destructive test directly.

Continuously monitor nodes, any existing application Tenant, and the fault-test Tenant. If a non-target resource becomes non-Ready, remove the current managed Chaos resource, stop subsequent scenarios, and collect evidence.

Run all six regular scenarios in the recommended order and stop after the first failure:

```bash
for scenario in \
  io-eio \
  pod-kill-one \
  network-partition-one \
  io-read-mistake \
  disk-full \
  warp-under-chaos
do
  RUSTFS_FAULT_TEST_SCENARIO="$scenario" \
  RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/$scenario" \
  make fault-test || break
done
```

Run `warp --version` before `warp-under-chaos`. Warp output is performance evidence and does not determine the correctness verdict.

### 8. Dedicated `dm-flakey` procedure

#### 8.1 No cluster reinstall is required

After running the six regular scenarios, keep Kubernetes, the Operator, Chaos Mesh, and the Rust toolchain. Stop other fault-test processes, prepare four dedicated static Local PVs, put one PV behind Device Mapper, and run the scenario with the static StorageClass.

The runner resets the fault-test Tenant and PVCs, but it does not create host block devices, static PVs, or the StorageClass.

#### 8.2 Allow the privileged helper

For an existing namespace:

```bash
kubectl label namespace rustfs-fault-test \
  pod-security.kubernetes.io/enforce=privileged \
  --overwrite
```

To pre-create the namespace before the first run:

```bash
kubectl create namespace rustfs-fault-test
kubectl label namespace rustfs-fault-test \
  app.kubernetes.io/managed-by=rustfs-operator-fault-test \
  pod-security.kubernetes.io/enforce=privileged
kubectl annotate namespace rustfs-fault-test \
  rustfs.com/fault-test-tenant=fault-test-tenant
```

#### 8.3 Prepare four dedicated volumes

Prefer four dedicated test block devices. Loop files are acceptable only in a lab. Each backing filesystem should be at least `90Gi`, while static PV capacity is fixed at `80Gi`.

Lab loop example on the target DM node; skip `truncate` and `losetup` when using a real dedicated block device:

```bash
export LAB=/data/rustfs/rustfs-fault-lab
export DM_NAME=rustfs-fault-dm

mkdir -p "$LAB/volume"
truncate -s 90G "$LAB/disk.img"
BACKING=$(losetup --find --show "$LAB/disk.img")
SECTORS=$(blockdev --getsz "$BACKING")
dmsetup create "$DM_NAME" --table "0 $SECTORS linear $BACKING 0"
mkfs.ext4 -F "/dev/mapper/$DM_NAME"
mount "/dev/mapper/$DM_NAME" "$LAB/volume"
```

On each of the other three nodes, format and mount its dedicated device directly at `/data/rustfs/rustfs-fault-lab/volume`. Never format an existing RustFS data device.

#### 8.4 Create the static StorageClass and Local PVs

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: rustfs-fault-dm
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
reclaimPolicy: Retain
```

Create four copies of this PV template, with a unique name and the corresponding node affinity:

```yaml
apiVersion: v1
kind: PersistentVolume
metadata:
  name: rustfs-fault-dm-<node-name>
  labels:
    app.kubernetes.io/managed-by: rustfs-operator-fault-test
spec:
  capacity:
    storage: 80Gi
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
              values: [<node-name>]
```

Verify that all four PVs are `Available` before running the test.

#### 8.5 Run `dm-flakey`

The configured node must match Kubernetes `metadata.name`, and the mount path must exactly match the target PV `spec.local.path`.

Run `blockdev --getsz <target-node-backing-device>` on the target node first, then set that value as `SECTORS` on the runner host.

```bash
export DM_NODE=<kubernetes-node-name>
export DM_MOUNT_PATH=/data/rustfs/rustfs-fault-lab/volume
export BACKING_DEVICE=<target-node-backing-device>
export SECTORS=<value-from-blockdev-getsz-on-target-node>

RUSTFS_FAULT_TEST_SCENARIO=dm-flakey \
RUSTFS_FAULT_TEST_STORAGE_CLASS=rustfs-fault-dm \
RUSTFS_FAULT_TEST_SERVER_IMAGE="$RUSTFS_IMAGE" \
RUSTFS_FAULT_TEST_DM_NAME=rustfs-fault-dm \
RUSTFS_FAULT_TEST_DM_NODE="$DM_NODE" \
RUSTFS_FAULT_TEST_DM_MOUNT_PATH="$DM_MOUNT_PATH" \
RUSTFS_FAULT_TEST_DM_FAULT_TABLE="0 $SECTORS flakey $BACKING_DEVICE 0 1 15" \
RUSTFS_FAULT_TEST_ARTIFACTS="$RUN_ROOT/dm-flakey" \
make fault-test
```

The fault table alternates between one second up and fifteen seconds down. The helper verifies the Pod-to-PVC-to-PV-to-node-to-mount relationship before loading the table, and restores the original linear table afterward.

#### 8.6 Emergency DM recovery

If the test process exits while the target is still flakey, restore it immediately on the target node:

```bash
dmsetup suspend --noflush rustfs-fault-dm
dmsetup load rustfs-fault-dm \
  --table "0 $SECTORS linear $BACKING_DEVICE 0"
dmsetup resume --noudevsync rustfs-fault-dm
dmsetup table rustfs-fault-dm
```

Confirm that the table is `linear` before deleting Pods/PVCs or unmounting the filesystem.

### 9. Acceptance criteria

For every scenario:

- `make fault-test` exits with status 0.
- `fault-evidence.json` reports `injected=true`, `active_during_workload=true`, and `recovered=true`.
- `checker-report.json` reports `committed_puts=4000`.
- `missing_committed_objects`, `hash_mismatches`, `successful_corrupted_reads`, and `list_warnings` are empty.
- The fault-test Tenant returns to Ready.

`RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` defaults to `false`. No client-visible failure is acceptable when the backend evidence proves that the fault was selected and injected.

Key artifacts are `workload-plan.json`, `history.jsonl`, `workload-summary.json`, `checker-report.json`, `fault-evidence.json`, Chaos manifests/status, DM snapshots, and Kubernetes logs/events.

### 10. Cleanup

Verify namespace ownership before deletion, then remove the test namespace and any managed Chaos resources:

```bash
kubectl get namespace rustfs-fault-test --show-labels
kubectl delete namespace rustfs-fault-test --wait=true
kubectl delete iochaos,podchaos,networkchaos \
  -n chaos-mesh \
  -l app.kubernetes.io/managed-by=rustfs-operator-fault-test \
  --ignore-not-found
```

Dynamic PV deletion depends on the StorageClass reclaim policy. Retained PVs and backend data require manual cleanup.

For `dm-flakey`, delete the namespace first, then the four static PVs and StorageClass. Confirm a linear DM table, unmount all four lab filesystems, remove the DM mapping, detach any loop devices, and delete only the dedicated lab directory.

```bash
umount /data/rustfs/rustfs-fault-lab/volume
dmsetup remove rustfs-fault-dm
losetup -d <loop-device>   # lab loop setup only
rm -rf /data/rustfs/rustfs-fault-lab
```

Finally verify nodes, the Operator, Chaos Mesh, PVs, and remaining Chaos resources.

### 11. Environment variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUSTFS_FAULT_TEST_STORAGE_CLASS` | required | Dynamic class for regular scenarios or dedicated static class for DM. |
| `RUSTFS_FAULT_TEST_DESTRUCTIVE` | set by Make | Destructive opt-in; do not bypass the Make entry point. |
| `RUSTFS_FAULT_TEST_SCENARIO` | `io-eio` | Selects one of the seven scenarios. |
| `RUSTFS_FAULT_TEST_NAMESPACE` | `rustfs-fault-test` | Dedicated test namespace. |
| `RUSTFS_FAULT_TEST_TENANT` | `fault-test-tenant` | Dedicated test Tenant. |
| `RUSTFS_FAULT_TEST_OPERATOR_NAMESPACE` | `rustfs-system` | Operator namespace. |
| `RUSTFS_FAULT_TEST_SERVER_IMAGE` | `rustfs/rustfs:latest` | Pin a validated digest in real runs. |
| `RUSTFS_FAULT_TEST_ARTIFACTS` | `target/fault-tests/artifacts` | Current scenario artifact directory. |
| `RUSTFS_FAULT_TEST_TIMEOUT_SECONDS` | `300` | Kubernetes/Tenant wait timeout. |
| `RUSTFS_FAULT_TEST_DURATION_SECONDS` | `900` | Chaos duration. |
| `RUSTFS_FAULT_TEST_PERCENT` | `20`; `100` for `disk-full` | Injection percentage where supported. |
| `RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS` | `4000` | Workload object count. |
| `RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY` | `50` | Maximum concurrency for prefill, fault workload, recovery writes, and checker reads. |
| `RUSTFS_FAULT_TEST_SEED` | generated randomly | Optional u64 seed for replaying the size order and object content. |
| `RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS` | `30` | S3 operation timeout. |
| `RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` | `false` | Require client-visible disruption when enabled. |
| `RUSTFS_FAULT_TEST_CHAOS_NAMESPACE` | `chaos-mesh` | Namespace for Chaos resources. |
| `RUSTFS_FAULT_TEST_WARP_DURATION_SECONDS` | `60` | Warp mixed workload duration. |
| `RUSTFS_FAULT_TEST_DM_NAME` | unset | DM mapping name; required for DM. |
| `RUSTFS_FAULT_TEST_DM_NODE` | unset | Target Kubernetes node; required for DM. |
| `RUSTFS_FAULT_TEST_DM_MOUNT_PATH` | unset | Target Local PV path; required for DM. |
| `RUSTFS_FAULT_TEST_DM_FAULT_TABLE` | unset | Fault dmsetup table; required for DM. |
| `RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE` | original table | Optional explicit recovery table. |
| `RUSTFS_FAULT_TEST_DM_HELPER_IMAGE` | `rancher/mirrored-library-busybox:1.37.0` | Privileged helper image. |
