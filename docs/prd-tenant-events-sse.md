# PRD：Tenant Events 多资源聚合与 SSE 推送

**状态：** 草案  
**范围：** Console / Operator  
**更新：** 2026-03-29  

---

## 1. 背景与问题

Tenant 详情页 **Events** 仅列出 `involvedObject.name` 等于 Tenant 名的 Kubernetes `Event`，**看不到** Pod、StatefulSet、PVC 等子资源上的事件。详情页多为 **客户端路由** + 全量 `loadTenant()`（或等价数据加载），**不一定**触发浏览器整页刷新；但 **Events 子视图内无法单独增量刷新事件列表**，默认需 **重新进入详情** 或依赖 **全量 `loadTenant()`** 才能更新事件相关数据，排障效率低。

**与 `kubectl describe` 的关系：** 此处事件与 `kubectl describe` 输出中 **Events** 小节为 **同一数据源**——均为集群中的 `Event`（Phase 1 以 `core/v1` 为主，见 §3）。对 Tenant / Pod / StatefulSet / PVC 分别执行 `kubectl describe …` 时看到的事件行，与本页按 §4 合并后的条目 **语义一致**（同一 `involvedObject` 上的同一条 Event）。差异仅在于：Console **合并多资源**、**去重**、**统一排序**并可能 **截断条数**（如默认 200），与逐条 describe 的展示顺序、是否全量不一定逐行相同。

## 2. 目标

1. 在同一视图展示 **归属于该 Tenant** 的多资源事件（**Tenant CR、Pod、StatefulSet、PVC**）。
2. 通过 **SSE（Server-Sent Events）** 将 **合并后的事件快照** 推送到浏览器；**不**提供单独的 `GET .../events` HTTP 聚合接口（实现上可移除该路由及相关处理）。

**仅 SSE、移除 REST 的产品代价（强决策，评审必读）：**

- 去掉公开 `GET .../events` JSON 后，**脚本 / curl / 自动化**无法用单请求拉取合并后的列表（除非另加 **内部 / debug / 运维** 只读接口）。
- **集成测试**更依赖 **SSE 客户端** 或 **浏览器环境**，成本高于纯 REST 断言。

**可选变体：** 若不接受对全部调用方删除 REST：可对 **用户 UI** 关闭 `GET .../events`，**保留只读运维 API**（单独鉴权或网络策略）；与「完全删除」二选一，须在实现与评审中明确。代价与变体在 **§7** 展开。

## 3. Phase 1 非目标

- 不替代 K8s 审计日志或 RustFS 应用日志。
- 首版不强制迁移到 `events.k8s.io`；若集群以 `core/v1` `Event` 为主可继续沿用。
- 首版不引入 WebSocket（除非后续有强需求）。

## 4. 「归属于 Tenant」的判定

| 资源 | Phase 1 规则 |
|------|----------------|
| Tenant | `metadata.name == {tenant}`；事件侧须 **`involvedObject.name={tenant}` 且 `involvedObject.kind` 与 CRD 注册 Kind 一致（通常为 `Tenant`）**（见 §4.1）。 |
| Pod | 见 **§4.1**，与 Console **`GET .../pods`**（`list_pods`）同源。 |
| StatefulSet | 见 **§4.1**，与 Console **`GET .../pools`**（`list_pools`）所用 STS 同源。 |
| PersistentVolumeClaim | 见 **§4.1**；Console 无独立 PVC 列表 API，按与 Operator 一致的 **标签** 发现。 |

### 4.1 与 Pod / StatefulSet / PVC 发现对齐（固定约定）

合并事件所用的 **资源名白名单** 须与当前 Console 实现 **同一套 label 与命名规则**（同 namespace、同路径参数 `{tenant}`），避免 Events 与 Pods / Pools 页「各算各的」。

| 资源 | 与现有行为对齐方式 |
|------|---------------------|
| **Pod** | `Pod` 使用 **`ListParams` label：`rustfs.tenant=<tenant>`**。与 `src/console/handlers/pods.rs` 中 **`list_pods`** 一致。 |
| **StatefulSet** | `StatefulSet` 使用 **同一 label：`rustfs.tenant=<tenant>`**；STS 名称 **`{tenant}-{pool}`**（`pool` 来自 Tenant `spec.pools`），与 `src/console/handlers/pools.rs` 中 **`list_pools`** 一致。 |
| **PersistentVolumeClaim** | Operator 在 PVC 模板上注入 **`rustfs.tenant`**、**`rustfs.pool`** 等（见 `Tenant::pool_labels`，`src/types/v1alpha1/tenant/workloads.rs` 中 `volume_claim_templates`）。事件侧对 **`PersistentVolumeClaim`** 使用 **与 Pod 相同的租户标签 `rustfs.tenant=<tenant>`** 列出名集合，即与 Operator 创建的 PVC 一致。 |

**实现要求：** Events 合并逻辑应 **复用或抽取**与 `list_pods` / `list_pools` **相同的 label 字符串与 STS 命名公式**，禁止另写一套查询；变更 Pod/Pool 发现时，Events 须同步修改或共用模块。

**Tenant CR 自身：** `involvedObject.name={tenant}` 且 `involvedObject.kind` 与 CRD 注册 Kind 一致（通常为 `Tenant`）。**现状缺口：** `src/console/handlers/events.rs` 仅按 `involvedObject.name` 过滤，**未**约束 kind；本需求实现 **须补齐** kind 条件（field selector 若支持则联合 `involvedObject.kind`；否则 list/watch 后 **等价后滤**），避免同 namespace **同名不同 kind** 资源事件误混入。

**实现原则：** 与 **`list_pods` / `list_pools` 及 PVC 标签约定**（§4.1）一致。

**范围边界（必须）：** SSE 路径中的 `{tenant}` 即当前详情页 Tenant；**仅**合并、展示 **该 Tenant** 下按上表判定的资源相关事件。**禁止**混入同 namespace 内其他 Tenant 的 Pod/STS/PVC 等事件；服务端以「当前 tenant 的发现集合」为白名单过滤，前端 **只渲染本页 tenant** 的数据，切换 Tenant 或离开页面须 **丢弃** 旧列表 state，避免串数据。

## 5. 功能需求

### 5.1 SSE：`GET /api/v1/namespaces/{ns}/tenants/{tenant}/events/stream`

**不提供**单独的 `GET .../tenants/{tenant}/events` HTTP 聚合接口；合并后的事件列表 **仅**通过本 SSE 端点以 **JSON 快照** 下发（实现可删除既有 events REST 路由与 handler）。移除 REST 的 **代价** 与 **可选变体** 见 **§7**。

- **租户范围：** 快照中每条事件必须属于 **路径参数 `{tenant}`** 对应之发现集合（见 §4）；不得包含其他 Tenant 资源的事件。
- **合并**来源：**Tenant CR：** `involvedObject.name={tenant}` **且** `involvedObject.kind` 为 Tenant（或 CRD 等价 Kind；field selector 若不支持联合 kind 则见 §4.1 **后滤**）；**另**合并 **该 tenant 范围内**每个 **Pod 名、StatefulSet 名、PVC 名** 对应的、**kind 匹配** 的 `involvedObject` 事件（服务端多次 list 再合并，或等价实现）。
- **去重：** 优先 `metadata.uid`；否则用 `(kind, name, reason, firstTimestamp, message)` 弱去重。
- **排序：** 按 `lastTimestamp` / `eventTime` 降序；**默认每帧快照最多 200 条**（常量可配置，需写入 API 说明）。
- **错误：** 建立连接前或 Watch 无法启动等 **关键失败** 时返回 **明确 HTTP 错误**；不得在成功 `200`/建立流后长期以「空快照」掩盖失败（与现有 Console 错误策略一致）。
- **鉴权：** 与现有 Console（JWT + 用户 K8s token）一致。
- **Content-Type：** `text/event-stream`。
- **行为：** 在 namespace 内 Watch `Event`（或等价），服务端仅按 **当前路径 `{tenant}`** 对应的 involvedObject 集合过滤后再推送；**不得**将无关 Tenant 的事件推入该连接。
- **负载：** 每次事件推送 **完整快照** JSON：`{ "events": [ ... ] }`，字段约定写入 API 说明，同样 200 条上限。
- **首包：** 连接建立后 **必须**尽快发送至少一条 **snapshot**，作为首屏数据源（无独立 REST 兜底）。
- **断线：** 客户端 `EventSource` 退避重连；服务端用 `resourceVersion` 等在合理范围内恢复 Watch。

### 5.2 前端（console-web）

- 进入 Events 标签：**建立 SSE**，以 **首包及后续快照** 更新 state（无单独 HTTP 拉取 events）。
- **鉴权与 `EventSource`：** 当前 Session 为 **Cookie**（与现有 Console 一致）时，须 **同站 / 可携带凭证**（如 `credentials: 'include'` / `withCredentials: true`），并与 **CORS** 策略一致。**若将来**改为 **Authorization 头**：原生 `EventSource` **无法设置自定义 Header**；备选为继续依赖 **Cookie**、或 **query token**（泄露风险须单独评估），在设计与评审中明确。
- SSE 失败：**非阻塞** toast，保留上次数据，提供 **重试** 或 **手动刷新**。
- 表格列语义不变：**类型**、**对象** 的展示与筛选枚举见 **§5.3**；**对象** 列展示为 `Kind/Name`（`Name` 为资源名，非枚举）。
- **仅当前 Tenant：** 列表与筛选结果 **不得**包含其他 Tenant 的事件；`tenant` 路由参数变化或卸载页面前 **清空** events state，避免残留。
- **筛选（客户端）：** 在 **当前 tenant 已加载** 的合并列表上支持按 **类型**、**对象（Kind）** 与 **时间**（基于 `lastTimestamp` / `eventTime` 的范围或相对区间）过滤展示；对象侧可按 **Kind 多选** + **名称关键字**（匹配 `involvedObject.name`）组合；**不**要求 SSE 负载或 URL 增加服务端筛选参数（Phase 1）。

### 5.3 类型与对象枚举（Phase 1）

与 `core/v1` `Event` 及本页合并范围对齐；供 **表格列展示** 与 **筛选器** 使用。

| 维度 | 枚举（固定） | 对应 K8s 字段 | 说明 |
|------|----------------|-----------------|------|
| **类型** | `Normal`，`Warning` | `Event.type` | **无标准 `Error` 类型：** Kubernetes `Event.type` 仅约定 `Normal` / `Warning`；失败、不可调度等「错误语义」事件在 API 中一般为 **`Warning`**，而非单独 `Error`。与 [Event v1](https://kubernetes.io/docs/reference/kubernetes-api/cluster-resources/event-v1/) 一致；筛选器仅这两项。若 API 返回空或非上述字符串（含个别组件自定义值），**类型**列 **原样显示**，该项 **不参与**「类型」枚举筛选（或归入「其他」选项，实现二选一并在 UI 文案中写清）。 |
| **对象（Kind）** | `Tenant`，`Pod`，`StatefulSet`，`PersistentVolumeClaim` | `involvedObject.kind` | Phase 1 与 §4 资源范围一致。筛选为 **Kind 多选**；`involvedObject.name` 用 **字符串** 展示与 **可选关键字** 过滤，不设枚举。 |

**实现提示：** 前端可用 TypeScript 字面量联合或常量数组表达上述枚举，避免魔法字符串分散。

## 6. 非功能需求

| 维度 | 要求 |
|------|------|
| RBAC | 用户需能 `list`/`watch` `events`，并能 `list` 用于发现 Pod/STS/PVC 的资源。 |
| 性能 | 合并列表有上限；连接断开必须释放 Watch；避免每 Tab 无界协程。 |
| 多副本 | 若无会话粘滞，需文档说明 **SSE 须 sticky** 或 Phase 1 仅单副本；避免 Watch 落在错误实例上长期悬挂。 |
| 网关 / 代理 | 常见 **Nginx / Ingress** 默认 **读超时（如 60s）** 会切断长时间无响应字节的 SSE，表现为 **静默断流**、客户端 **频繁重连**。**上线 checklist：** 调大 `proxy_read_timeout`（或 Envoy 等 **等价超时**），与 **多副本 sticky** 并列；具体数值由运维与是否采用服务端注释/心跳等策略共同决定。 |
| 安全 | SSE 快照 DTO 不包含 Secret 内容；**租户隔离**：流与 UI 仅暴露当前 `{tenant}` 范围内事件。 |

## 7. 发布策略

1. **直接交付 SSE** 为事件唯一通道；**删除**（或不实现）`GET .../tenants/{tenant}/events` 聚合 HTTP 接口，避免双路径维护。
2. **产品代价（与 §2 一致）：** 移除公开 JSON 后，**脚本 / curl / 自动化**无法用单请求拉取合并后的 events（除非另加 **内部 / debug / 运维** 接口）；**集成测试**更依赖 **SSE 客户端** 或 **浏览器环境**。
3. **可选变体：** 若团队不接受对全部调用方删除 REST：可对 **用户 UI** 关闭 `GET .../events`，**保留只读运维 API**（单独鉴权或网络策略）；与「完全删除」二选一并在文档中写明。
4. 无需「先 REST、后开 SSE」或 **SSE 默认关闭** 的阶段性开关；以 SSE 首包 snapshot 满足首屏与更新。

## 8. 验收标准

1. 人为制造 Pod 级 **Warning** 事件（如不可调度），**约 15s 内** 表格出现对应行，**Object** 为 `Pod/...`，无需整页刷新。  
2. 无 events REST 时，仅靠 SSE **首包与后续快照** 可得到 **合并、排序、截断** 后的一致列表。  
3. RBAC 不足或连接失败时返回 **明确错误**（或 SSE 合理失败语义），不出现「空表误导」。  
4. 关闭标签页后服务端 **停止** 对应 Watch/SSE（开发环境可通过日志验证）。  
5. 同 namespace 存在 **多个 Tenant** 时，在 Tenant A 详情 Events 中 **不出现** Tenant B 的 Pod/STS/PVC 等事件（服务端与前端均需满足）。  
6. 合并所用 Pod / StatefulSet / PVC 名集合与 **§4.1** 及对应 handler 行为一致（代码审查或单测可对照 `rustfs.tenant` 与 `{tenant}-{pool}` 规则）。  
7. **Tenant CR 事件**仅包含 **`involvedObject.kind=Tenant`（或 CRD 等价 Kind）且 `involvedObject.name={tenant}`**；**不得**因同名不同 kind 混入其他资源事件；可验证 **field selector 含 kind** 或文档化的 **等价后滤**（§4.1）。

---

*一页 PRD 结束。*
