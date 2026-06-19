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

# RustFS 故障注入测试方案

本文档描述如何复用 RustFS Operator 测试基础设施，在真实 Kubernetes 测试集群中运行可执行、可诊断、可逐步增强的故障注入测试体系。故障测试不属于 Kind e2e suite。

核心原则：

- **Operator 负责测试环境编排**：创建 Tenant、准备本地 PV、暴露 RustFS S3 服务、等待状态、收集诊断现场。
- **故障注入器负责制造故障**：优先使用 Kubernetes-native 的 Chaos Mesh。
- **S3 workload 负责产生真实对象访问流量**：持续执行 `PUT`、`GET`、`HEAD`、`LIST` 等操作。
- **Jepsen-like checker 负责判断正确性**：它不制造故障，只基于操作历史和最终读取结果判断 RustFS 是否丢数据、读错数据或返回假成功。

也就是说，这套测试不是单纯验证 Operator 是否能拉起 StatefulSet，而是通过 Operator 部署出来的 RustFS 集群来验证 RustFS 在故障下的数据正确性。

## 边界澄清

这套故障测试的测试对象是 **Operator 编排出的 RustFS workload**，不是 Operator 控制面自身。

边界如下：

- Operator 只负责把 RustFS Tenant、Service、PVC、Secret 等测试环境编排出来。
- 故障注入作用于 RustFS Pod、RustFS 容器、RustFS 数据卷和 RustFS 服务路径。
- checker 判断的是 RustFS 对象读写正确性：已经确认成功写入的数据不能丢，成功读取不能返回错误内容。
- Operator 状态只作为恢复观察信号，例如故障解除后 Tenant 是否重新回到 Ready；它不是第一阶段 correctness verdict 的主体。
- 不在 Tenant Console 或生产 Operator Console 中提供 destructive fault test 入口。
- Chaos Mesh Dashboard 可以作为观察 Chaos 资源的外部工具，但 fault-test runner 的权威输出是 `history.jsonl`、`checker-report.json` 和 Kubernetes artifacts。

## 目标

故障注入测试需要回答这些问题：

1. RustFS 在 Pod、节点、网络、磁盘 I/O 故障下，已经成功写入的数据是否仍然存在。
2. RustFS 是否会在磁盘损坏或网络分区后，把错误对象内容以 `200 OK` 返回给客户端。
3. RustFS 在请求超时、连接中断、部分失败后，是否存在“客户端认为失败但服务端实际写入”的未知状态。
4. Operator 编排出的 Tenant 是否能在故障解除后回到 Ready，作为 RustFS workload 恢复观察信号。
5. 当测试失败时，fault-test runner 是否能留下足够的日志、事件、历史记录和 checker 报告用于定位。

最重要的判定不是“故障期间所有请求都成功”，而是：

```text
可以失败，但不能假成功。
可以超时，但不能返回错误数据。
故障恢复后，已经确认成功的数据必须一致。
```

## 非目标

第一阶段不做这些事：

- 不替代 RustFS 自身的单元测试、集成测试或存储引擎内部测试。
- 不直接引入完整 Clojure Jepsen 测试套件。
- 不在共享开发集群或生产集群上运行 destructive 测试；真实 Kubernetes 集群也必须使用专用测试 namespace、Tenant 和 StorageClass。
- 不把性能压测结果当成 correctness 结论。
- 不在第一版验证所有 S3 线性一致性细节。
- 不默认测试多 Tenant、跨集群、真实块设备故障。
- 不把故障测试放进 Tenant Console。
- 不在生产 Operator Console 提供运行 destructive 测试的入口。
- 不把 Operator 控制面重启、升级、Leader Election 等作为第一阶段核心验证对象。

第一阶段的目标是补齐当前最大缺口：**真实故障注入 + 对象内容正确性检查**。

## 可复用的测试基础设施

当前项目已经有适合故障测试的底层模块，不需要复制 kubectl、S3、history 和 checker 实现。但故障测试拥有独立配置、命令和安全边界，不属于 Kind e2e case inventory。

已有能力：

| 能力 | 当前位置 | 用途 |
| --- | --- | --- |
| destructive 入口 | `make fault-test` | 专门在真实 Kubernetes 测试集群运行破坏性故障测试。 |
| fault suite runners | `e2e/tests/faults.rs` | 真实集群 scenario-selected destructive runner，不属于 e2e case inventory。 |
| fault config/context guard | `e2e/src/framework/fault_config.rs` | 读取独立 fault-test 配置、绑定当前 context，并拒绝 Kind。 |
| Tenant/Secret 创建 | `e2e/src/framework/resources.rs` | 创建 fault-test namespace、凭据和真实集群 Tenant。 |
| S3 port-forward | `e2e/src/framework/port_forward.rs` | 将 Tenant S3 服务暴露到本地。 |
| artifact collector | `e2e/src/framework/artifacts.rs` | 测试失败后收集 Kubernetes 现场。 |

关键约定：

- RustFS Pod selector 可使用 `rustfs.tenant=<tenant-name>`。
- RustFS 容器名是 `rustfs`。
- RustFS 数据卷路径遵循 `/data/rustfs0`、`/data/rustfs1`。
- 常规场景要求真实集群提供动态 StorageClass；`dm-flakey` 只允许使用显式配置的专用静态 Local PV。

因此推荐方案是：

```text
复用当前测试基础设施
  + 独立 FaultTestConfig 与 Make 入口
  + 新增 Chaos Mesh 故障注入模块
  + 新增 S3 workload
  + 新增 operation history
  + 新增对象存储 checker
```

## 总体架构

```text
make fault-test -> e2e/tests/faults.rs
  |
  +-- 环境保护：destructive opt-in / current real Kubernetes context / required StorageClass
  +-- 环境准备：按 isolation reset 或复用 Tenant；DM 场景验证专用 PV 拓扑
  +-- S3 workload：持续读写对象
  +-- history recorder：记录每次操作的开始、结束、结果、hash
  +-- nemesis：通过 Chaos Mesh 对 RustFS workload 注入故障
  +-- checker：基于 history 和最终读回结果判断 RustFS 对象正确性
  +-- artifact collector：失败时收集诊断现场
```

建议新增模块：

```text
e2e/src/framework/chaos_mesh.rs
e2e/src/framework/fault_config.rs
e2e/src/framework/fault_scenarios.rs
e2e/src/framework/s3_workload.rs
e2e/src/framework/history.rs
e2e/src/framework/checker.rs
```

模块职责：

| 模块 | 职责 |
| --- | --- |
| `chaos_mesh` | 生成、apply、describe、delete Chaos Mesh 资源。 |
| `fault_scenarios` | 定义故障场景名称、默认参数、目标对象和执行顺序。 |
| `s3_workload` | 对 RustFS Tenant S3 endpoint 执行对象读写流量。 |
| `history` | 将每个 S3 操作记录成 JSON Lines。 |
| `checker` | 基于 history 和最终读回结果验证 RustFS 对象存储不变量。 |
| `faults.rs` | 编排完整测试流程，不承载底层实现细节。 |

## 为什么优先用 Chaos Mesh

当前场景是在 Kubernetes 中通过 Operator 部署 RustFS，因此故障注入也应该尽量 Kubernetes-native。

Chaos Mesh 适合第一阶段，原因：

- 可以通过 namespace 和 label 精准选择 RustFS Pod。
- 可以指定容器名，避免影响非目标 sidecar 或其他组件。
- 支持 `PodChaos`、`NetworkChaos`、`IOChaos`。
- `IOChaos` 能对指定挂载路径返回 `EIO`，适合模拟磁盘坏块或磁盘 I/O 错误。
- `IOChaos mistake` 能模拟读写返回错误字节，适合模拟 bit rot / 静默损坏。
- 以 CRD 形式管理故障，方便 fault-test runner apply/delete/describe/collect。

第一阶段建议只要求：

```text
Chaos Mesh 已安装
iochaos.chaos-mesh.org CRD 存在
podchaos.chaos-mesh.org CRD 存在
networkchaos.chaos-mesh.org CRD 存在
```

如果 CRD 不存在，测试应明确失败并给出提示，而不是静默跳过。

## 为什么不是直接上完整 Jepsen

完整 Jepsen 很强，但第一阶段不建议直接引入，原因：

- 当前项目 e2e 是 Rust-native，直接接入 Clojure Jepsen 成本高。
- 当前最大的缺口是“没有真实故障注入”和“没有对象内容正确性 checker”。
- 对象存储第一阶段最关键的不变量可以用更轻量的 checker 覆盖。
- 先把 `PUT/GET/hash` 这条基本正确性链路跑通，收益更高。

因此建议路线是：

```text
先做 Jepsen-like checker
后续再逐步增强为更完整的并发历史模型
```

Jepsen-like 的含义是：

- 有 workload。
- 有 nemesis。
- 有 operation history。
- 有明确 correctness model。
- 有自动 checker。

它不是简单 chaos smoke test。

## 安全模型

故障测试必须默认安全，只能面向当前真实 Kubernetes 测试集群，不能运行在 Kind、共享开发集群或生产集群。

必须保留并强化这些保护：

1. 必须设置 `RUSTFS_FAULT_TEST_DESTRUCTIVE=1`；`make fault-test` 会显式设置。
2. fault runner 使用当前 `kubectl config current-context`，并拒绝 `kind-*` context。
3. 必须显式提供 `RUSTFS_FAULT_TEST_STORAGE_CLASS`；除 `dm-flakey` 的专用静态 Local PV 外，目标 StorageClass 必须支持动态供给。
4. 目标 namespace 必须来自 fault-test 配置，默认 `rustfs-fault-test`；runner 创建 namespace 时必须写入 `app.kubernetes.io/managed-by=rustfs-operator-fault-test` label 和匹配 Tenant 的 `rustfs.com/fault-test-tenant` annotation。
5. 已存在 namespace 只有在上述所有权标记完全匹配时才允许 reset；runner 不得自动认领未标记 namespace。
6. 所有故障资源必须带唯一 run id label。
7. 每个 Chaos 资源必须有 RAII-style cleanup guard。
8. 正常结束和异常失败都必须 best-effort 删除故障资源。
9. `io-eio` 这类存储破坏/强干扰 case 必须在 case 前 reset Tenant/PVC/PV；后续 pod kill、network delay、短暂 disconnect 可以按场景复用 Tenant。
10. 默认故障持续时间要覆盖 workload 窗口，默认故障比例要小。
11. 测试失败时必须先收集 artifacts，再清理会影响诊断的信息。
12. destructive 场景保持 `#[ignore]`，只能通过显式 Make 目标执行。

当前使用的故障测试环境变量：

| 变量 | 默认值 | 作用 |
| --- | --- | --- |
| `RUSTFS_FAULT_TEST_STORAGE_CLASS` | required | 常规场景使用动态 StorageClass；`dm-flakey` 使用专用静态 Local PV StorageClass。 |
| `RUSTFS_FAULT_TEST_NAMESPACE` | `rustfs-fault-test` | 专用测试 namespace。 |
| `RUSTFS_FAULT_TEST_TENANT` | `fault-test-tenant` | 专用测试 Tenant。 |
| `RUSTFS_FAULT_TEST_SCENARIO` | `io-eio` | 选择故障场景。 |
| `RUSTFS_FAULT_TEST_DURATION_SECONDS` | `180` | 故障持续时间，默认覆盖串行小对象 workload。 |
| `RUSTFS_FAULT_TEST_PERCENT` | `20`；`disk-full` 为 `100` | 支持百分比注入的场景使用。 |
| `RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS` | `40` | 写入或校验对象数量。 |
| `RUSTFS_FAULT_TEST_REQUEST_TIMEOUT_SECONDS` | `3` | 单次 S3 请求超时时间。 |
| `RUSTFS_FAULT_TEST_REQUIRE_CLIENT_DISRUPTION` | `false` | 是否要求故障期间至少出现一次客户端可见失败/超时/unknown。 |
| `RUSTFS_FAULT_TEST_DM_NAME` | empty | `dm-flakey` 场景要切换的 device-mapper 设备名，必填。 |
| `RUSTFS_FAULT_TEST_DM_NODE` | empty | device-mapper 设备与目标 Local PV 所在 Kubernetes 节点，必填。 |
| `RUSTFS_FAULT_TEST_DM_MOUNT_PATH` | empty | 目标 PV 在节点上的 Local PV 挂载路径，必填。 |
| `RUSTFS_FAULT_TEST_DM_FAULT_TABLE` | empty | `dm-flakey` 场景注入故障时加载的 dmsetup table，必填。 |
| `RUSTFS_FAULT_TEST_DM_RECOVERY_TABLE` | current table | `dm-flakey` 场景恢复时加载的 dmsetup table；不填则使用注入前 table。 |
| `RUSTFS_FAULT_TEST_DM_HELPER_IMAGE` | `rancher/mirrored-library-busybox:1.37.0` | 目标节点 privileged helper Pod 镜像。 |
| `RUSTFS_FAULT_TEST_WARP_DURATION_SECONDS` | `60` | `warp-under-chaos` 场景中 Warp mixed workload 的运行时间。 |
| `RUSTFS_FAULT_TEST_CHAOS_NAMESPACE` | `chaos-mesh` | Chaos Mesh 资源所在 namespace。 |

## 操作历史模型

每个客户端可见的 S3 操作都应记录一条 JSON Lines。

示例：

```json
{
  "id": "op-000001",
  "scenario": "io-eio",
  "kind": "put",
  "bucket": "rustfs-fault-run123",
  "key": "fault-test/run-123/object-1",
  "value_sha256": "abc123",
  "size_bytes": 1048576,
  "started_at_ms": 1710000000000,
  "ended_at_ms": 1710000001234,
  "outcome": "ok",
  "http_status": 200,
  "error": null
}
```

`outcome` 建议只保留四类，语义必须清晰：

| outcome | 含义 | checker 处理 |
| --- | --- | --- |
| `ok` | 客户端收到明确成功响应。 | 作为强正确性输入。 |
| `failed` | 客户端收到明确失败响应。 | 不要求最终存在。 |
| `timeout` | 客户端超时，不知道服务端是否完成。 | 作为 unknown 处理。 |
| `unknown` | 连接中断、body 未读完、port-forward 中断等。 | 作为 unknown 处理。 |

第一版 checker 只对 `ok` 的 `PUT` 做强校验。

对于 `timeout` 和 `unknown` 的写入：

- 最终存在可以接受。
- 最终不存在也可以接受。
- 需要在 report 中单独列出，方便后续分析。

这样可以避免把网络中断导致的“未知成功”误判为 RustFS 数据错误。

## Checker 不变量

### 不变量 1：成功写入的数据不能丢

如果客户端收到了成功写入：

```text
PUT key value_hash=H -> ok
```

故障解除并等待 Tenant 恢复后，必须满足：

```text
GET key -> 200
sha256(body) == H
```

否则 hard fail。

### 不变量 2：成功读取不能返回错误内容

任何一次 `GET` 只要返回 `200 OK`，并且该 key 有已知成功写入值，则：

```text
sha256(body) == expected_hash
```

如果 `GET` 返回 `200` 但 hash 不一致，这是最高优先级失败。

这比“请求是否成功”更重要，因为对象存储最危险的问题不是失败，而是**成功返回错误数据**。

### 不变量 3：明确失败的写入不要求存在

如果 `PUT` 返回明确失败：

```text
PUT key -> failed
```

那么最终这个 key 存在或不存在，都不作为第一版 hard fail。

### 不变量 4：未知结果单独记录

如果 `PUT` 是：

```text
timeout
unknown
```

则 checker 记录它最终是否 materialized，但不作为第一版 hard fail。

### 不变量 5：恢复后的 LIST 先作为 warning

故障解除并等待 Tenant Ready 后：

```text
LIST prefix
```

理论上应包含所有成功 `PUT` 且未成功 `DELETE` 的 key。

第一版可以将 LIST 缺失作为 warning，而不是 hard fail。等 RustFS 对 LIST 一致性的目标语义确认后，再升级为 hard fail。

## S3 workload 设计

第一阶段建议使用 Rust 代码实现 S3 workload，而不是依赖外部 `aws` 或 `mc` CLI。

原因：

- 操作历史更容易结构化记录。
- 请求 timeout、transport error、body error 更容易准确分类。
- 对象 hash 和操作结果可以在同一进程中关联。
- CI 和本地依赖更少。
- 后续可以扩展为并发 workload 和 checker replay。

建议在 `e2e/Cargo.toml` 后续增加：

```text
aws-sdk-s3
aws-config
aws-credential-types
sha2
rand
hex
```

第一版 workload 操作：

```text
CreateBucket
PutObject
GetObject
HeadObject
ListObjectsV2
```

第一版建议使用唯一 key，不要并发覆盖同一个 key。

key 格式：

```text
fault-test/<run-id>/small/<uuid>
fault-test/<run-id>/medium/<uuid>
fault-test/<run-id>/large/<uuid>
```

对象大小建议：

| 类型 | 大小 |
| --- | --- |
| small | 4 KiB |
| medium | 64 KiB |
| large | 1 MiB |
| xlarge | 8 MiB |

第一版不建议默认使用太大对象，避免故障测试运行过慢。

## 初始故障场景优先级

| 优先级 | 场景 | 后端 | 目的 |
| --- | --- | --- | --- |
| P0 | `io-eio` | Chaos Mesh `IOChaos` | 模拟单个 RustFS 数据卷读写返回 `EIO`。 |
| P0 | `pod-kill-one` | Chaos Mesh `PodChaos` | 模拟一个 RustFS Pod 死亡和 StatefulSet 恢复。 |
| P1 | `network-partition-one` | Chaos Mesh `NetworkChaos` | 模拟一个 RustFS Pod 与集群网络分区。 |
| P1 | `io-read-mistake` | Chaos Mesh `IOChaos` | 模拟读路径返回错误字节，即静默坏块。 |
| P1 | `disk-full` | Chaos Mesh `IOChaos` errno 28 | 在不消耗节点磁盘的情况下验证 ENOSPC 行为。 |
| P2 | `direct-volume-corruption` | 存储后端专用测试环境 | 模拟已经落盘的数据被破坏。 |
| P2 | `node-restart` | 集群节点运维接口 | 模拟节点重启。 |
| P3 | `dm-flakey` | device mapper / loop device | 更接近真实块设备故障。 |
| P3 | `warp-under-chaos` | MinIO Warp + chaos | 故障期间性能退化分析。 |

`operator-restart` 可以作为独立 Operator 控制面韧性测试，但不放入本方案第一阶段的 RustFS workload fault matrix，避免混淆测试对象。

## P0 场景：磁盘 EIO

这是建议最先实现的场景。

它能直接验证 RustFS 在真实集群 CSI 数据卷发生读写错误时，是否会丢失已提交对象。

目标：

```text
让某一个 RustFS Pod 的某一块数据卷，在部分 READ/WRITE 调用上返回 EIO。
```

Chaos Mesh `IOChaos` 示例：

```yaml
apiVersion: chaos-mesh.org/v1alpha1
kind: IOChaos
metadata:
  name: rustfs-fault-io-eio
  namespace: chaos-mesh
  labels:
    rustfs-fault-test/run-id: "<run-id>"
spec:
  action: fault
  mode: one
  selector:
    namespaces:
      - rustfs-fault-test
    labelSelectors:
      rustfs.tenant: fault-test-tenant
  containerNames:
    - rustfs
  volumePath: /data/rustfs0
  path: /data/rustfs0/**/*
  methods:
    - READ
    - WRITE
  errno: 5
  percent: 20
  duration: "60s"
```

关键点：

- `volumePath` 是 RustFS 容器内的 CSI 数据卷挂载路径。
- `errno: 5` 对应 Linux `EIO`。
- `mode: one` 表示只选择一个匹配 Pod，避免第一版故障面过大。
- `percent: 20` 表示只影响部分 I/O 调用，避免全量不可用。

预期行为：

- 故障期间 S3 请求可以失败、超时或返回 5xx。
- RustFS 不能把错误数据作为成功响应返回。
- 已经成功 `PUT` 的对象，在故障解除后必须 hash 一致。
- Tenant 可以短暂 Degraded，但最终应回到 Ready。
- Chaos 资源必须被删除。

## P1 场景：静默坏块 / bit rot

EIO 是显式错误，比较容易处理；更危险的是静默损坏。

静默坏块的模拟方式：

```text
磁盘读操作看起来成功，但返回的字节是错的。
```

Chaos Mesh `IOChaos mistake` 示例：

```yaml
apiVersion: chaos-mesh.org/v1alpha1
kind: IOChaos
metadata:
  name: rustfs-fault-io-read-mistake
  namespace: chaos-mesh
spec:
  action: mistake
  mode: one
  selector:
    namespaces:
      - rustfs-fault-test
    labelSelectors:
      rustfs.tenant: fault-test-tenant
  containerNames:
    - rustfs
  volumePath: /data/rustfs0
  path: /data/rustfs0/**/*
  methods:
    - READ
  mistake:
    filling: random
    maxOccurrences: 1
    maxLength: 4096
  percent: 5
  duration: "60s"
```

预期行为：

- RustFS 可以返回错误。
- RustFS 可以从健康 shard 修复或读取。
- RustFS 不能返回 `200 OK` 且 body hash 错误。

这个场景是对象存储非常关键的测试，因为它验证的是“不要静默返回坏数据”。

## P2 场景：存储后端级数据破坏

真实集群不能假设能够直接访问宿主机或 CSI 后端文件。该场景必须在专用存储测试环境中，通过存储后端提供的故障工具、快照克隆或块设备测试接口实现。

这个场景比 `IOChaos mistake` 更接近真实“落盘数据已经损坏”，但也更危险：

- 可能破坏 RustFS 元数据。
- 可能导致恢复语义更复杂。
- 需要更明确的预期结果。
- 适合作为 P2，不适合作为第一版。

## 测试流程

当前 runner 使用如下流程：

```text
1. 读取 FaultTestConfig
2. 检查 RUSTFS_FAULT_TEST_DESTRUCTIVE=1
3. 读取当前 kube context 并拒绝 kind-* context
4. 检查 RUSTFS_FAULT_TEST_STORAGE_CLASS 已配置
5. 根据 RUSTFS_FAULT_TEST_SCENARIO 解析 FaultScenarioSpec
6. 按场景检查 Chaos Mesh CRD 或专用 host-side 工具配置
7. 检查 fault-test namespace 不存在，或所有权标记与配置完全匹配
8. 根据 `FaultIsolation` reset 或复用专用 fault-test Tenant/PVC
9. namespace 不存在时由 runner 使用 create 创建带所有权标记的 fault-test namespace；不得通过 apply 认领竞态中出现的同名 namespace
10. 创建真实集群 fault-test Tenant
11. 等待 Tenant Ready
12. 启动 Tenant S3 port-forward，等待 S3 endpoint 可用
13. 创建 run-scoped bucket
14. prefill 一批对象，记录 key、size、sha256；prefill 必须成功
15. apply 当前 scenario 的 Chaos Mesh 资源或 host-side fault
16. 对持续型 Chaos 等待 active
17. 故障期间执行 PUT/GET mixed workload，并输出 workload-summary.json
18. 如果要求 client-visible disruption，则确认 workload 观察到了失败、超时或 unknown
19. 确认持续型 Chaos 没有早于 workload 结束恢复
20. 删除 Chaos 或通过目标节点 helper Pod 恢复 dmsetup table
21. 等待 Tenant 再次 Ready
22. 对所有成功 PUT 对象做最终 GET + sha256 校验
23. 执行 prefix LIST 并记录 warning
24. 写入 checker-report.json 和 fault-evidence.json
25. 失败时收集 Kubernetes artifacts、故障状态和故障资源 describe/yaml
```

伪代码：

```rust
#[tokio::test]
#[ignore = "destructive fault scenario; run through `make fault-test`"]
async fn fault_io_eio_preserves_committed_objects() -> Result<()> {
    let config = FaultTestConfig::from_env()?;

    config.require_destructive_enabled()?;
    chaos_mesh::require_iochaos_crd(&config.cluster)?;

    let result = async {
        resources::reset_fault_tenant_resources(&config.cluster)?;
        resources::apply_fault_tenant_resources(&config.cluster)?;

        let client = kube_client::default_client().await?;
        let tenants = kube_client::tenant_api(client.clone(), &config.cluster.test_namespace);
        wait::wait_for_tenant_ready(
            tenants,
            &config.cluster.tenant_name,
            config.cluster.timeout,
        )
        .await?;

        let mut port_forward = PortForwardSpec::start_tenant_io(&config.cluster)?;
        let s3 = s3_workload::Client::from_tenant_port_forward(
            &config.cluster,
            &mut port_forward,
        )
        .await?;

        let mut history = history::Recorder::new("io-eio")?;
        s3.create_bucket().await?;
        s3.prefill_objects(&mut history).await?;

        let chaos = chaos_mesh::IoChaos::eio_on_rustfs_volume(
            &config.cluster,
            "/data/rustfs0",
            20,
            Duration::from_secs(60),
        );

        let guard = chaos.apply()?;
        s3.run_mixed_workload(&mut history).await?;
        drop(guard);

        wait::wait_for_tenant_ready(
            kube_client::tenant_api(client, &config.cluster.test_namespace),
            &config.cluster.tenant_name,
            config.cluster.timeout,
        )
        .await?;

        let report = checker::check_s3_history(&s3, &history).await?;
        report.require_success()?;

        Ok(())
    }
    .await;

    if result.is_err() {
        ArtifactCollector::new(&config.artifacts_dir)
            .collect_kubernetes_snapshot("fault_io_eio_preserves_committed_objects", &config)?;
    }

    result
}
## Chaos Mesh 模块设计

`chaos_mesh.rs` 当前提供这些能力：

```rust
pub fn require_iochaos_crd(config: &ClusterTestConfig) -> Result<()>;
pub fn require_podchaos_crd(config: &ClusterTestConfig) -> Result<()>;
pub fn require_networkchaos_crd(config: &ClusterTestConfig) -> Result<()>;
pub fn cleanup_managed_iochaos(config: &ClusterTestConfig, namespace: &str) -> Result<()>;
pub fn cleanup_managed_podchaos(config: &ClusterTestConfig, namespace: &str) -> Result<()>;
pub fn cleanup_managed_networkchaos(config: &ClusterTestConfig, namespace: &str) -> Result<()>;
pub fn apply_iochaos(config: &ClusterTestConfig, spec: &IoChaosSpec) -> Result<ChaosGuard>;
pub fn apply_podchaos(config: &ClusterTestConfig, spec: &PodChaosSpec) -> Result<ChaosGuard>;
pub fn apply_networkchaos(config: &ClusterTestConfig, spec: &NetworkChaosSpec) -> Result<ChaosGuard>;

pub enum IoChaosAction {
    Fault { errno: u8 },
    Mistake {
        filling: String,
        max_occurrences: u8,
        max_length: usize,
    },
}
pub struct IoChaosSpec {
    pub name: String,
    pub namespace: String,
    pub run_id: String,
    pub scenario: String,
    pub target_namespace: String,
    pub tenant_name: String,
    pub container_name: String,
    pub volume_path: String,
    pub methods: Vec<String>,
    pub action: IoChaosAction,
    pub percent: u8,
    pub duration: Duration,
}
```

实现要求：

- 所有 `kubectl` 命令必须通过现有 `framework::kubectl` 和 `framework::command` 边界。
- apply 前检查 CRD 是否存在。
- apply 后保存 manifest；失败时可以 `kubectl describe/get yaml` 保存到 artifacts。
- `ChaosGuard::delete()` 必须明确返回结果；`Drop` 只做 best-effort cleanup，不应 panic。
- 每个资源都带 `rustfs-fault-test/run-id` label。
- 每个资源都带 `rustfs-fault-test/scenario` label。
- 每个资源都带 `app.kubernetes.io/managed-by=rustfs-operator-fault-test` label，便于按 suite 清理残留。
- 允许按 label 清理上一次异常残留。

## S3 workload 模块设计

`s3_workload.rs` 当前提供：

```rust
pub struct S3WorkloadClient {
    bucket: String,
    request_timeout: Duration,
}

pub struct ObjectSpec {
    pub key: String,
    pub size_bytes: usize,
    pub sha256: String,
}

impl S3WorkloadClient {
    pub async fn new(...) -> Result<Self>;
    pub async fn create_bucket(&self, recorder: &mut Recorder) -> Result<OperationOutcome>;
    pub async fn put_object(&self, object: &ObjectSpec, recorder: &mut Recorder) -> Result<OperationOutcome>;
    pub async fn get_object_result(&self, key: &str, recorder: &mut Recorder) -> Result<GetObjectResult>;
    pub async fn head_object(&self, key: &str, recorder: &mut Recorder) -> Result<OperationOutcome>;
    pub async fn list_prefix(&self, prefix: &str, recorder: &mut Recorder) -> Result<Option<Vec<String>>>;
}
```

注意点：

- 每个请求必须有明确 timeout。
- 不要在 workload 层做无限 retry。
- 如果要 retry，必须记录每次尝试，而不是只记录最终结果。
- body 读取失败不能记为 `failed`，应记为 `unknown`。
- `PUT` 返回成功后才进入 committed set。

## Checker report 设计

最终 report 建议保存为 JSON：

```json
{
  "scenario": "io-eio",
  "run_id": "run-123",
  "committed_puts": 200,
  "missing_committed_objects": [],
  "hash_mismatches": [],
  "successful_corrupted_reads": [],
  "unknown_writes_materialized": [],
  "list_warnings": [],
  "tenant_recovered": true,
  "passed": true
}
```

hard fail 条件：

1. 成功 `PUT` 的对象最终 `GET` 不到。
2. 成功 `PUT` 的对象最终 `GET` hash 不一致。
3. 任意成功 `GET` 返回的 body hash 与预期不一致。
4. 故障解除后 Tenant 在 timeout 内没有回到 Ready。
5. Chaos 资源删除失败并仍然残留。
6. RustFS Pod 进入不可恢复 CrashLoopBackOff。

允许出现：

1. 故障期间 S3 请求失败。
2. 故障期间 S3 请求 timeout。
3. 故障期间 port-forward 连接中断。
4. Tenant 短暂 Degraded。
5. unknown write 最终存在或不存在。
6. 故障期间 LIST 不完整。

## artifacts 设计

每次 fault run 至少应该保存：

```text
history.jsonl
checker-report.json
chaos-manifest.yaml
chaos-describe.txt
chaos-describe-<failure-stage>.txt
chaos-<failure-stage>.yaml
events.yaml
pv-paths.txt
rustfs-pods-current.log
rustfs-pods-previous.log
tenant-describe.txt
pods-describe.txt
```

其中最关键的是：

- `history.jsonl`：复盘客户端看到的世界。
- `checker-report.json`：复盘 correctness verdict。
- `chaos-describe-<failure-stage>.txt` / `chaos-<failure-stage>.yaml`：在故障资源被清理前保留 Chaos Mesh 现场。
- `rustfs-pods-current.log`：定位 RustFS 如何处理故障。
- `events.yaml`：定位 Kubernetes 层是否出现调度、挂载、重启问题。
- `pv-paths.txt`：定位具体 PVC/PV、StorageClass 和节点映射。

## Makefile 入口

使用独立入口：

```bash
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> make fault-test
```

该入口使用当前 `kubectl` context，拒绝 Kind，并使用 `RUSTFS_FAULT_TEST_STORAGE_CLASS` 指向的真实集群测试存储。

`e2e/tests/faults.rs` 只有一个 ignored dispatcher。运行时通过 `RUSTFS_FAULT_TEST_SCENARIO` 从 7 个 catalog 场景中选择并执行一个，因此测试结果不会把未选中的场景计为通过。故障测试只面向真实 Kubernetes 测试集群，不保留 Kind 后端；Kind e2e 生命周期测试是独立部分。

示例：

```bash
# 默认场景：io-eio；make fault-test 会注入 RUSTFS_FAULT_TEST_DESTRUCTIVE=1
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> make fault-test

# 运行其他场景
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=pod-kill-one make fault-test
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=network-partition-one make fault-test
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=io-read-mistake make fault-test
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=disk-full make fault-test
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=dm-flakey make fault-test
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=warp-under-chaos make fault-test
```

普通开发检查仍然使用：

```bash
make e2e-check
make pre-commit
```

不要把 destructive 场景混进普通 `make e2e-live-run`。

## 当前可交付范围

当前 fault suite 实现 7 个真实集群 runner：

```text
fault_io_eio_preserves_committed_objects
fault_pod_kill_one_preserves_committed_objects
fault_network_partition_one_preserves_committed_objects
fault_io_read_mistake_rejects_corrupt_reads
fault_disk_full_preserves_committed_objects
fault_dm_flakey_preserves_committed_objects
fault_warp_under_chaos_reports_performance_separately
```

这些 runner 共享同一条 correctness 验证链路：

1. destructive/current real Kubernetes context guard。
2. 按场景检查 Chaos Mesh CRD 或专用 host-side 工具配置。
3. 启动前按 `app.kubernetes.io/managed-by=rustfs-operator-fault-test` 清理上次异常残留的 Chaos 资源。
4. reset 前验证 namespace 所有权标记；未标记或 Tenant 不匹配时 fail closed。
5. Fresh/Dedicated 场景 reset Tenant/PVC；Pod Kill 和网络分区可复用已验证所有权的 Tenant。
6. Tenant 创建和 Ready 等待。
7. S3 bucket 创建。
8. S3 prefill 对象并记录 hash；prefill 阶段必须明确成功，避免空用例通过。
9. apply 对应故障：Chaos Mesh `IOChaos` / `PodChaos` / `NetworkChaos`，或目标节点 helper Pod 执行 dm-flakey、Warp under chaos。
10. 对持续型 Chaos 资源等待进入 active，再开始故障 workload。
11. 故障期间持续读写并输出 `workload-summary.json`。
12. 对持续型故障确认 workload 没有跑出故障窗口。
13. 故障 workload 失败、故障证据不足或 Chaos 删除失败时，先保存 describe/yaml 或 host fault 输出，再触发 cleanup。
14. 删除 Chaos 资源，或恢复 dmsetup table 并删除 helper Pod。
15. Tenant 恢复 Ready 等待。
16. 所有成功 `PUT` 对象最终 `GET + sha256` 校验。
17. 恢复后执行 `LIST prefix`，缺失项先作为 warning。
17. AWS SDK error 按 service failure / timeout / dispatch-response unknown 分类写入 history。
18. history、workload summary、fault evidence 和 checker report 输出。
19. 失败时 artifacts 收集。

这个版本已经能证明系统从“占位骨架”升级为“真实故障注入 + 数据正确性校验”。

## 后续增强计划

当前 catalog 包含 7 个 real-cluster scenario，由一个 dispatcher 精确选择执行。后续工作重点是提高故障强度、判定模型和长稳覆盖。

### Phase 1：runner hardening

- 在测试环境逐个验证 7 个 executable scenario 的前置条件、故障注入、清理和 artifacts 输出。
- 为 PodChaos、NetworkChaos、IOChaos mistake 补充更细的 CRD status 断言。
- 保持 `fault-evidence.json` 的后端状态结构稳定，便于 CI artifact 聚合和历史对比。
- 保持每个 scenario 独立选择执行，避免多个故障在同一次测试中相互污染。

验收：

- `make e2e-check` 通过。
- `RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> RUSTFS_FAULT_TEST_SCENARIO=<scenario> make fault-test` 可在当前真实 Kubernetes 测试集群逐个运行 scenario，并拒绝 Kind。
- 如果 committed object 丢失，测试失败。
- 如果 successful GET 返回错误字节，测试失败。
- 如果 workload 跑出 IOChaos active 窗口，测试失败。
- fault runner 不进入 Kind e2e case inventory；其边界是 `rustfs-workload/fault-injection`。
- 每个 scenario 都能在失败时留下足够定位信息。
- 每个 scenario 结束后能清理自己创建的 Chaos 资源、helper Pod 或恢复 dmsetup table。

### Phase 2：一致性模型增强

- 引入 same-key overwrite、delete、multipart、prefix/list 等更接近 Jepsen register/set 模型的 workload。
- 将 operation history 扩展成可回放的事件日志，明确 invoke/ok/fail/info。
- 在 checker 中区分 linearizable、eventual recovery、data corruption、availability degradation。

验收：

- 成功写入的对象不得丢失。
- 成功读取不得返回错误字节。
- List 缺失、陈旧读、超时、服务错误分别记录，不能混成同一种 failure。

### Phase 3：长稳和性能

- 增加长时间 soak runner。
- 增加随机但可复现的故障调度。
- 将 Warp 结果固定为性能/压力信号，不作为 correctness verdict。

注意：

- 性能结果和 correctness verdict 必须分离。
- 压测失败不等于数据错误。
- 数据错误永远是 hard fail。

### Phase 4：块设备级故障实验室

- 研究 `dm-flakey`、`dm-error`、loop device-backed PV。
- 只在 Linux runner 或专用环境启用。
- 不进入默认 fault-test 流程。
- 现有 dm-flakey runner 通过 `RUSTFS_FAULT_TEST_DM_*` 显式接入专用设备映射。
- 后续可以在专用 Linux runner 上扩展 `dm-error`、loop device-backed PV 和更细粒度的 I/O 延迟/丢写模型。
- 这些场景只进入明确标记的专用环境，不进入默认 fault-test 流程。

这个方向更接近真实磁盘坏块，但环境成本明显更高，必须保持强隔离。

## 与其他测试框架的关系

| 框架或工具 | 当前项目定位 |
| --- | --- |
| 共享测试基础设施 | Operator 编排、Tenant 生命周期、artifacts 收集。 |
| Chaos Mesh | Kubernetes-native nemesis，负责制造故障。 |
| Jepsen-like checker | 判断对象存储 correctness，不制造故障。 |
| MinIO Mint | 后续用于 S3 API 兼容性，不作为故障 checker。 |
| MinIO Warp | 用于故障期间性能压测，不作为 correctness verdict。 |
| COSBench | 后续用于大规模对象存储压测。 |
| Ceph s3-tests | 后续用于 S3 行为兼容性参考。 |
| Ceph Teuthology | 借鉴大规模编排思想，当前不直接引入。 |
| Ozone fault injection | 借鉴 FUSE/agent 精细磁盘故障思想，作为后续增强。 |

当前最优组合：

```text
RustFS real-cluster fault-test runner
  + Chaos Mesh
  + Rust-native S3 workload
  + Jepsen-like object checker
```

## 实现注意事项

- 所有外部调用必须有 timeout。
- workload 不要无限 retry。
- retry 必须记录每次尝试。
- 不要把 transport unknown 错误归类为 definite failed。
- 不要把 performance degradation 误判为 correctness failure。
- 故障资源必须总是 best-effort cleanup。
- artifacts 中不要记录密钥明文。
- 第一版避免覆盖同一个 key，降低 checker 复杂度。
- 后续再逐步加入 same-key overwrite、delete、multipart、LIST consistency。

## 参考资料

- [Chaos Mesh IOChaos](https://chaos-mesh.org/docs/simulate-io-chaos-on-kubernetes/)
- [Chaos Mesh Documentation](https://chaos-mesh.org/docs/)
- [Jepsen](https://jepsen.io/)
- [MinIO Warp](https://docs.min.io/warp/)
- [COSBench](https://github.com/intel-cloud/cosbench)
- [Ceph s3-tests](https://github.com/ceph/s3-tests)
