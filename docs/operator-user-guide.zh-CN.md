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

# RustFS Operator 使用手册

本文档面向需要在 Kubernetes 上部署、配置和运维 RustFS Operator 的用户，作为 Operator 技术使用手册。

English version: [operator-user-guide.md](operator-user-guide.md)

## 1. 概述

RustFS Operator 用于在 Kubernetes 中管理 RustFS 对象存储集群。用户通过命名空间级别的 `Tenant` 自定义资源描述期望的 RustFS 集群，Operator 负责创建和维护运行 RustFS 所需的 Kubernetes 资源。

Operator 提供以下能力：

- `Tenant` CRD（`rustfs.com/v1alpha1`）：声明 RustFS pool、持久化、调度、凭据、TLS、日志、加密和初始化 provisioning。
- 控制器 reconciliation：维护 Tenant 相关的 RBAC、Service、StatefulSet、PVC 模板、状态条件和 Kubernetes Event。
- Helm Chart：位于 `deploy/rustfs-operator/`，用于安装 Operator。
- Operator Console API 和 UI：用于 Operator 管理场景。
- 可选 Operator STS：基于 Kubernetes 工作负载身份签发临时 RustFS 凭据。
- 可选 Tenant provisioning：自动创建 RustFS canned policy、普通用户和 bucket。
- Metrics、健康检查，以及可选 Prometheus Operator 集成。

注意区分以下服务：

| 组件 | 用途 | 默认端口 |
|------|------|----------|
| Tenant 内 RustFS S3 API | S3 兼容对象存储访问 | `9000` |
| RustFS Tenant Console | 单个 RustFS Tenant 的 Web Console | `9001` |
| Operator Console API/UI | Operator 管理 API 和 UI | `9090` |
| Operator STS | 临时凭据签发接口 | `4223` |
| Operator observability endpoint | `/metrics`、`/healthz`、`/readyz` | `8080` |

## 2. 架构模型

一个 `Tenant` 就是一个 RustFS 集群。一个 Tenant 可以包含一个或多个 pool，但同一个 Tenant 内的所有 pool 会组成一个统一的 RustFS 集群，不是冷热分层或性能分层。

如果你需要独立性能、独立生命周期、独立权限边界或独立管理边界，请创建多个 Tenant，而不是在一个 Tenant 内用多个 pool 模拟多个集群。

创建 Tenant 后，Operator 会创建并维护：

- Tenant RBAC 启用时的 ServiceAccount、Role、RoleBinding；
- headless Service：`{tenant}-hl`，用于 StatefulSet peer DNS；
- S3 Service：`{tenant}-io`，端口 `9000`；
- Tenant Console Service：`{tenant}-console`，端口 `9001`；
- 每个 pool 一个 StatefulSet；
- PVC 模板：`vol-0`、`vol-1` 等；
- 自动生成的 RustFS 环境变量，例如 `RUSTFS_VOLUMES`、`RUSTFS_ADDRESS`、`RUSTFS_CONSOLE_ADDRESS` 和 `RUSTFS_CONSOLE_ENABLE`。

## 3. 前置条件

- Kubernetes v1.30 或更高版本。
- 使用 Helm 安装时需要 Helm 3.0 或更高版本。
- 可满足 Tenant PVC 的 StorageClass。
- 已配置目标集群访问权限的 `kubectl`。
- 能够拉取配置的 Operator 镜像和 RustFS 镜像。
- 可选：启用 `ServiceMonitor` 或 `PrometheusRule` 时需要 Prometheus Operator。
- 可选：使用 cert-manager 管理 Tenant TLS 时需要 cert-manager。

## 4. 安装 Operator

使用仓库内 Helm Chart 安装：

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace
```

验证 Operator 和 Console Pod：

```bash
kubectl get pods -n rustfs-system
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=operator \
  -f
```

升级已有安装：

```bash
helm upgrade rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system
```

卸载：

```bash
helm uninstall rustfs-operator --namespace rustfs-system
```

## 5. Helm 配置

建议通过 values 文件管理安装配置：

```bash
helm upgrade --install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace \
  -f values.yaml
```

常用配置分组：

| 配置段 | 用途 |
|--------|------|
| `operator` | Operator Deployment 副本数、镜像、资源、探针、metrics、调度、leader election 和 Tenant monitor。 |
| `sts` | Operator STS 端点、Service 端口、TokenReview audience 和 TLS。 |
| `serviceAccount` / `rbac` | Operator ServiceAccount 和 RBAC 创建策略。 |
| `console` | Operator Console 后端/UI Deployment、Service、session cookie 密钥、Ingress、资源和可选独立前端。 |
| `namespace` | Chart 资源命名空间覆盖；默认使用 Helm release namespace。 |
| `commonLabels` / `commonAnnotations` | 添加到 Chart 管理资源上的统一 label 和 annotation。 |

生产风格 values 示例：

```yaml
operator:
  replicas: 2
  image:
    repository: registry.example.com/rustfs/operator
    tag: v0.1.0
  resources:
    requests:
      cpu: 200m
      memory: 256Mi
    limits:
      cpu: 1000m
      memory: 1Gi
  tenantMonitor:
    enabled: true
    intervalSeconds: 300
  serviceMonitor:
    enabled: true

console:
  enabled: true
  replicas: 2
  jwtSecret: "<stable-base64-or-random-secret>"
  ingress:
    enabled: true
    className: nginx
    hosts:
      - host: console.example.com

sts:
  enabled: true
  audience: sts.rustfs.com
  tls:
    enabled: true
    auto: true
```

配置说明：

- `operator.leaderElect` 可以不配置；当 `operator.replicas > 1` 时 Chart 会自动启用 leader election。
- 多副本 Console 部署需要保持 `console.jwtSecret` 稳定；不设置时 Chart 会生成或复用已有 Secret。
- 生产环境应使用 HTTPS 并保持 `CONSOLE_COOKIE_SECURE` 启用。仅本地 HTTP 调试时才关闭。
- `sts.tls.auto=true` 时，Operator 会在缺失时创建 `sts-tls` Secret。

## 6. 创建 Tenant

最小开发 Tenant 示例：

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: dev-minimal
  namespace: default
spec:
  image: rustfs/rustfs:latest
  pools:
    - name: dev-pool
      servers: 1
      persistence:
        volumesPerServer: 1
```

应用并检查：

```bash
kubectl apply -f tenant.yaml
kubectl get tenant dev-minimal
kubectl get pods,pvc,svc -l rustfs.tenant=dev-minimal
```

等待 Pod Ready：

```bash
kubectl wait --for=condition=ready pod \
  -l rustfs.tenant=dev-minimal \
  --timeout=300s
```

访问 Tenant S3 API：

```bash
kubectl port-forward svc/dev-minimal-io 9000:9000
```

访问 Tenant Console：

```bash
kubectl port-forward svc/dev-minimal-console 9001:9001
```

推荐从 `examples/` 目录选择示例开始：

| 示例 | 使用场景 |
|------|----------|
| `examples/minimal-dev-tenant.yaml` | 最小可用开发 Tenant。 |
| `examples/secret-credentials-tenant.yaml` | 基于 Secret 的管理员凭据。 |
| `examples/provisioning-tenant.yaml` | 初始化 policy、user 和 bucket。 |
| `examples/production-ha-tenant.yaml` | 高可用生产风格配置。 |
| `examples/multi-pool-tenant.yaml` | 一个统一 Tenant 集群内的多 pool 配置。 |
| `examples/custom-rbac-tenant.yaml` | 自定义 ServiceAccount 和 RBAC。 |

## 7. Tenant 配置说明

### 7.1 Tenant 命名

Tenant 名称必须兼容 DNS-1035，且长度不超过 55 个字符，因为 Operator 会派生 `{tenant}-console` 等 Service 名称。

建议使用小写名称，以字母开头，仅包含小写字母、数字和 `-`。

### 7.2 Pool 配置

`spec.pools` 是必填字段。每个 pool 会创建一个 StatefulSet。

关键字段：

| 字段 | 用途 |
|------|------|
| `name` | Pool 名称，用于 label、StatefulSet 名称和 peer DNS。同一个 Tenant 内必须唯一。 |
| `servers` | 该 pool 的 RustFS Pod 数量。必须大于 `0`。创建后不可变。 |
| `persistence.volumesPerServer` | 每个 server 挂载的 PVC 数量。必须大于 `0`。创建后不可变。 |
| `persistence.volumeClaimTemplate` | 每个数据卷的 PVC spec，可设置容量、access mode 和 StorageClass。 |
| `persistence.path` | 数据卷挂载基础路径。默认 `/data`，最终路径为 `{path}/rustfs0`、`{path}/rustfs1` 等。 |
| `nodeSelector`、`affinity`、`tolerations`、`topologySpreadConstraints` | Pool 级调度控制。 |
| `resources` | Pool 容器资源 request 和 limit。 |
| `priorityClassName` | Pool 级 PriorityClass 覆盖。 |

Operator admission 检查：

- `servers` 和 `persistence.volumesPerServer` 必须大于 `0`。
- Pool 名称必须唯一。
- Pool peer DNS label 必须满足 Kubernetes DNS label 长度限制。
- 已存在 pool 的 `servers` 和 `volumesPerServer` 不能原地修改。

Operator 不校验 RustFS 存储布局、erasure set 大小或 storage class parity 是否被支持。这些检查由 Tenant workload 启动后的 RustFS 自行完成。

示例：

```yaml
spec:
  pools:
    - name: pool-0
      servers: 4
      persistence:
        volumesPerServer: 4
        volumeClaimTemplate:
          accessModes: ["ReadWriteOnce"]
          resources:
            requests:
              storage: 100Gi
          storageClassName: fast-ssd
      resources:
        requests:
          cpu: "2"
          memory: 8Gi
        limits:
          cpu: "4"
          memory: 16Gi
```

### 7.3 凭据配置

生产环境建议使用 `spec.credsSecret`。Secret 必须与 Tenant 在同一 namespace，并包含 UTF-8 编码的 `accesskey` 和 `secretkey` 两个 key，两个值长度都至少为 8 个字符。

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-admin-creds
  namespace: storage
type: Opaque
stringData:
  accesskey: "replace-with-access-key"
  secretkey: "replace-with-secret-key"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  credsSecret:
    name: rustfs-admin-creds
  pools:
    - name: pool-0
      servers: 2
      persistence:
        volumesPerServer: 2
```

凭据优先级：

1. `spec.credsSecret`。
2. `spec.env` 中显式配置 `RUSTFS_ACCESS_KEY` 和 `RUSTFS_SECRET_KEY`。
3. RustFS 内置默认值。默认值仅适合开发测试。

### 7.4 工作负载配置

常用 Tenant 级字段：

| 字段 | 用途 |
|------|------|
| `image` | RustFS server 镜像。未配置时使用 Operator fallback。 |
| `imagePullSecret` | 镜像拉取 Secret。 |
| `imagePullPolicy` | RustFS 镜像拉取策略。 |
| `scheduler` | 自定义 scheduler 名称。 |
| `env` | 额外 RustFS 容器环境变量。不要覆盖 Operator 自动管理的变量。 |
| `serviceAccountName` | RustFS Pod 使用的自定义 ServiceAccount。 |
| `createServiceAccountRbac` | 是否由 Operator 为 Tenant ServiceAccount 创建 Role/RoleBinding。 |
| `priorityClassName` | Tenant 级 PriorityClass。 |
| `lifecycle` | Kubernetes 容器 lifecycle hook。 |
| `podManagementPolicy` | StatefulSet pod management policy。 |
| `podDeletionPolicyWhenNodeIsDown` | 节点 NotReady/Unknown 时的 Pod 删除策略。 |
| `securityContext` | RustFS Pod 的 Pod SecurityContext 覆盖。 |

Operator 会自动管理以下环境变量：

- `RUSTFS_VOLUMES`
- `RUSTFS_ADDRESS`
- `RUSTFS_CONSOLE_ADDRESS`
- `RUSTFS_CONSOLE_ENABLE`
- `RUSTFS_KMS_*` 变量；请改用 `spec.encryption` 配置
- 启用 TLS 时的 RustFS TLS 相关变量

对于单 pool 的单节点单盘 Tenant，`RUSTFS_VOLUMES` 会渲染为本地数据路径，例如 `/data/rustfs0`。多 pool Tenant 和其他布局仍会通过 Tenant headless Service 渲染 peer DNS URL，并由 RustFS 在运行时校验。

`podDeletionPolicyWhenNodeIsDown` 支持以下值：

- `DoNothing`：不自动删除 Pod。
- `Delete`：发起普通 Pod 删除。
- `ForceDelete`：使用 `gracePeriodSeconds=0` 强制删除 Pod。
- `DeleteStatefulSetPod`：Longhorn 兼容模式，强制删除 down node 上卡住的 StatefulSet Pod。
- `DeleteDeploymentPod`：Longhorn 兼容模式，强制删除 down node 上卡住的 Deployment Pod。
- `DeleteBothStatefulSetAndDeploymentPod`：Longhorn 兼容模式，同时处理 StatefulSet 和 Deployment Pod。

强制删除可能影响数据一致性。只有当存储后端和运维流程明确支持该故障处理方式时才应启用。强制删除要求 Node 对象已删除，或 Node 带有对目标 Pod 生效且未被该 Pod tolerate 的 `node.kubernetes.io/out-of-service` taint，确保 volume detach fencing 是显式的。

### 7.5 TLS

Tenant TLS 通过 `spec.tls` 配置。

关键字段：

| 字段 | 用途 |
|------|------|
| `mode` | 当前可用配置为 `disabled` 或 `certManager`。`external` 是保留模式，目前会阻塞 reconcile。 |
| `mountPath` | TLS 挂载路径。默认 `/var/run/rustfs/tls`。 |
| `rotationStrategy` | 当前支持 `Rollout`。`HotReload` 会被 CRD 接受，但目前会阻塞 reconcile。 |
| `enableInternodeHttps` | RustFS 节点间通信是否使用 HTTPS。 |
| `requireSanMatch` | 是否要求证书 SAN 匹配生成的 DNS 名称。默认 `true`。 |
| `certManager` | 兼容旧版本的单证书配置。`certificates` 为空且 `mode: certManager` 时必须设置 `secretName`。 |
| `certificates` | 多个服务端证书配置，会被渲染到 RustFS TLS 目录供 SNI 使用。必须且只能有一个条目设置 `default: true`，非默认条目必须设置 `hosts`。 |
| `caTrust` | RustFS 进程级信任配置。它控制 `ca.crt`、`client_ca.crt`、`RUSTFS_TRUST_SYSTEM_CA` 和服务端 mTLS，不会按 SNI host 分别生效。 |

cert-manager 证书示例：

```yaml
spec:
  tls:
    mode: certManager
    rotationStrategy: Rollout
    enableInternodeHttps: true
    certManager:
      manageCertificate: true
      secretName: rustfs-a-server-tls
      issuerRef:
        group: cert-manager.io
        kind: Issuer
        name: rustfs-issuer
      includeGeneratedDnsNames: true
```

当 `manageCertificate: true` 时，`issuerRef` 也是必填项。Operator 会创建或更新 cert-manager `Certificate`，等待引用的 Secret 就绪，校验 `tls.crt` 和 `tls.key`，并在未配置其它 CA trust source 时使用 `ca.crt`。

公有域名和内部域名使用不同证书时：

```yaml
spec:
  tls:
    mode: certManager
    rotationStrategy: Rollout
    enableInternodeHttps: true
    caTrust:
      source: CertificateSecretCa
    certificates:
      - name: internal
        default: true
        hosts:
          - rustfs.internal.example.local
        certManager:
          manageCertificate: true
          secretName: rustfs-internal-tls
          issuerRef:
            group: cert-manager.io
            kind: Issuer
            name: private-ca
          includeGeneratedDnsNames: true
      - name: public
        hosts:
          - s3.example.com
        certManager:
          manageCertificate: true
          secretName: rustfs-public-tls
          issuerRef:
            group: cert-manager.io
            kind: ClusterIssuer
            name: letsencrypt-prod
          includeGeneratedDnsNames: false
```

默认条目会被投影到 `mountPath` 根目录下的 `rustfs_cert.pem` 和 `rustfs_key.pem`，供 RustFS 作为 fallback 证书和节点间 HTTPS 证书使用。每个 `hosts` 值会被投影为 RustFS SNI 子目录，例如 `s3.example.com/rustfs_cert.pem` 和 `s3.example.com/rustfs_key.pem`。
配置了 `certificates` 时，进程级 trust 应放在顶层 `caTrust` 或 `default: true` 证书条目的 `caTrust` 中。旧的 `certManager.caTrust` 只对单证书写法生效。

### 7.6 日志配置

Tenant 日志通过 `spec.logging` 配置。

模式：

| 模式 | 用途 |
|------|------|
| `stdout` | 默认且推荐。日志由 Kubernetes 从 stdout/stderr 采集。 |
| `emptyDir` | 临时本地日志，适合调试；Pod 重启后丢失。 |
| `persistent` | 使用 PVC 持久化日志。仅应使用独立于 RustFS 的外部存储。 |

不要把 RustFS 启动日志存储到 RustFS 自己里面。这会产生循环依赖：服务启动前对象存储接口尚不可用。

示例：

```yaml
spec:
  logging:
    mode: stdout
```

### 7.7 加密 / KMS

Tenant 加密通过 `spec.encryption` 配置。

支持的 backend：

| Backend | 用途 |
|---------|------|
| `local` | 文件型本地 KMS key 目录和本地 master key。目录必须是绝对路径，必须位于 RustFS 数据 PVC mount 下的子目录，且整个 Tenant 所有 pool 的 server 总数必须为 1。 |
| `vault` | HashiCorp Vault endpoint，需要包含 `vault-token` 的 Secret。 |

Local KMS 不使用 `kmsSecret`；即使设置也会被忽略。请通过 `spec.encryption.local.masterKeySecretRef` 配置本地 master key，它会映射到 RustFS 的 `RUSTFS_KMS_LOCAL_MASTER_KEY`。默认情况下，Local KMS 会把 key 文件存放在第一块数据 PVC mount 下，例如 `persistence.path` 为 `/data` 时使用 `/data/rustfs0/.kms-keys`。多 server Tenant 应使用 Vault KMS。`allowInsecureDevDefaults: true` 会映射到 `RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS=true`，只能用于开发环境，因为 RustFS 可能用 plaintext-dev-only 方式保存 Local KMS key material。

升级提示：旧版本 Operator 的 Local KMS 默认目录是 `/data/kms-keys`，该路径不在数据 PVC 下。Operator 不会自动迁移 key 文件。对于仍使用旧隐式默认值，或显式设置了旧路径的已有 Local KMS Tenant，Operator 会阻断 reconcile 或 StatefulSet 滚动更新；请先把旧目录中的 key 文件和 `.master-key.salt` 复制到新的 PVC 持久化目录，继续使用同一个本地 master key Secret，然后显式设置 `spec.encryption.local.keyDirectory` 为该子目录。

Local 示例：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-local-kms
  namespace: storage
type: Opaque
stringData:
  local-master-key: "replace-with-a-strong-local-kms-master-key"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-local
  namespace: storage
spec:
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
  encryption:
    enabled: true
    backend: local
    local:
      keyDirectory: /data/rustfs0/.kms-keys
      masterKeySecretRef:
        name: rustfs-local-kms
        key: local-master-key
    defaultKeyId: tenant-default
```

Vault 示例：

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-kms
  namespace: storage
type: Opaque
stringData:
  vault-token: "replace-with-vault-token"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  pools:
    - name: pool-0
      servers: 2
      persistence:
        volumesPerServer: 2
  encryption:
    enabled: true
    backend: vault
    vault:
      endpoint: https://vault.example.com:8200
    kmsSecret:
      name: rustfs-kms
    defaultKeyId: tenant-default
```

### 7.8 初始化 Provisioning

Operator 可以在 Tenant workload Ready 后自动创建 RustFS policy、user 和 bucket。需要配置：

- `spec.credsSecret`：RustFS 管理员凭据。
- `spec.policies`：从 ConfigMap 读取 policy document。
- `spec.users`：普通用户。每个 user 必须至少直接绑定一个 policy。
- `spec.buckets`：bucket，可选择开启 object lock。

ConfigMap 和 user Secret 必须位于 Tenant namespace。若这些资源不是通过 Operator Console 创建，建议添加 label：`rustfs.tenant=<tenant-name>`，这样资源变化可以触发 owning Tenant reconcile。

每个 `spec.users[]` 条目都会读取一个与 user 名同名的 Secret。Secret 必须包含 `accesskey` 和 `secretkey`，或者 MinIO 兼容 key：`CONSOLE_ACCESS_KEY` 和 `CONSOLE_SECRET_KEY`。如果两种 key 同时存在，值必须一致。user access key 至少 8 个字符，且不能包含空白、`=` 或 `,`；user secret key 至少 8 个字符。

更新 user Secret 的 `secretkey` 会轮换对应 RustFS user 的凭据。首次成功 provisioning 后，`accesskey` 不可变；如需变更，请新建 user 条目和 Secret，迁移客户端后再移除旧条目。

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-policy
  namespace: storage
  labels:
    rustfs.tenant: rustfs-a
data:
  policy.json: |
    {
      "Version": "2012-10-17",
      "Statement": [
        {
          "Effect": "Allow",
          "Action": ["s3:ListBucket", "s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
          "Resource": ["arn:aws:s3:::app-data", "arn:aws:s3:::app-data/*"]
        }
      ]
    }
---
apiVersion: v1
kind: Secret
metadata:
  name: app-user
  namespace: storage
  labels:
    rustfs.tenant: rustfs-a
type: Opaque
stringData:
  accesskey: appuser01
  secretkey: appuser01secret
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  credsSecret:
    name: rustfs-admin-creds
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 4
  policies:
    - name: app-readwrite
      document:
        configMapKeyRef:
          name: app-policy
          key: policy.json
  users:
    - name: app-user
      policies:
        - app-readwrite
  buckets:
    - name: app-data
      objectLock: true
```

删除行为是保守的：从 Tenant spec 移除已 provisioning 的资源时，实际 RustFS 资源会保留。

### 7.9 Pool 生命周期

`spec.poolLifecycle` 用于显式 pool 生命周期请求。当前 PVC retention policy 为 `Retain`。

Decommission 请求示例：

```yaml
spec:
  poolLifecycle:
    pvcRetentionPolicy: Retain
    decommissionRequests:
      - poolName: pool-old
        requestId: decommission-pool-old-20250623
        action: Start
        reason: "capacity migrated to pool-new"
```

Pool 生命周期操作需要谨慎执行。操作前应确认备份，并验证 RustFS 层面的 decommission 行为。

## 8. Operator Console

Helm Chart 默认启用 Operator Console：`console.enabled=true`。

推荐同源部署：

```yaml
console:
  enabled: true
  ingress:
    enabled: true
    className: nginx
    hosts:
      - host: console.example.com
```

统一 Operator 镜像会通过同一个 Console Service 提供 `/` 和 `/api/v1`。该模式不需要后端 CORS 配置。

Console 登录需要 Kubernetes ServiceAccount bearer token。Chart 管理的 Console ServiceAccount 可以这样生成短期 token：

```bash
kubectl -n rustfs-system create token rustfs-operator-console --duration=24h
```

将 token 粘贴到 Console 登录页。Console 会把验证后的 token 存入加密 session cookie。

本地 port-forward 调试：

```bash
kubectl -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090
```

浏览器打开 `http://127.0.0.1:19090`。

## 9. Operator STS

Operator STS 允许 Kubernetes workload 使用 projected ServiceAccount token 换取临时 RustFS 凭据，权限由 `PolicyBinding` 控制。

STS 路由：

```text
POST /sts/{tenantNamespace}/{tenantName}
```

在目标 Tenant namespace 创建 `PolicyBinding`：

```yaml
apiVersion: sts.rustfs.com/v1alpha1
kind: PolicyBinding
metadata:
  name: reports-readonly
  namespace: storage
spec:
  application:
    namespace: reports
    serviceaccount: reports-api
  policies:
    - readonly
```

workload ServiceAccount token 的 audience 必须匹配 `sts.audience`，默认是 `sts.rustfs.com`。

```yaml
volumes:
  - name: rustfs-sts-token
    projected:
      sources:
        - serviceAccountToken:
            path: token
            audience: sts.rustfs.com
            expirationSeconds: 3600
```

workload 内调用 STS：

```bash
TOKEN="$(cat /var/run/secrets/rustfs-sts/token)"

curl -sS -X POST \
  --cacert /var/run/secrets/rustfs-sts-ca/ca.crt \
  "https://rustfs-operator-sts.rustfs-system.svc:4223/sts/storage/rustfs-a" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  --data-urlencode "Version=2011-06-15" \
  --data-urlencode "Action=AssumeRoleWithWebIdentity" \
  --data-urlencode "WebIdentityToken=${TOKEN}" \
  --data-urlencode "DurationSeconds=3600"
```

当前 STS 约束：

- STS 只为启用 TLS 的 Tenant 签发凭据。
- Operator STS 使用显式 Tenant 路由，路径中同时包含 namespace 和 Tenant name。
- `PolicyBinding` 至少需要引用一个 policy。
- 匹配到的 `PolicyBinding` 引用的每个 policy 都必须能解析为有效 RustFS policy 文档。
- 调用方传入的 `Policy` 请求参数会被拒绝；签发凭据只使用匹配的 `PolicyBinding` policies。
- 如果 Tenant 要求 Operator STS 调用 Tenant 时使用 client certificate，目前会被 Operator STS 拒绝。

## 10. 监控和状态

查看 Tenant 状态：

```bash
kubectl get tenant -A
kubectl describe tenant -n <namespace> <tenant>
```

`status.currentState` 常见值：

- `Ready`
- `Reconciling`
- `Blocked`
- `Degraded`
- `NotReady`
- `Unknown`

重要 condition：

- `Ready`
- `Reconciling`
- `Degraded`
- `SpecValid`
- `CredentialsReady`
- `KmsReady`
- `TlsReady`
- `PoolsReady`
- `WorkloadsReady`
- `ProvisioningReady`

查看 Chart 管理的 observability endpoint：

```bash
kubectl -n rustfs-system port-forward svc/rustfs-operator-metrics 18080:8080
curl http://127.0.0.1:18080/healthz
curl http://127.0.0.1:18080/readyz
curl http://127.0.0.1:18080/metrics
```

启用 Prometheus Operator 集成：

```yaml
operator:
  serviceMonitor:
    enabled: true
  prometheusRule:
    enabled: true
```

## 11. 运维操作

### 修改 RustFS 镜像

```yaml
spec:
  image: rustfs/rustfs:v1.0.0
```

Operator 会 reconcile StatefulSet，并通过 Tenant condition 和 pool status 报告 rollout 状态。

### 修改存储容量

PVC 扩容取决于 StorageClass 和 Kubernetes 环境。不要原地修改不可变的 pool 形态字段（`servers` 和 `volumesPerServer`）。需要扩容时，可按需新增 pool，并结合 RustFS decommission 和迁移流程操作。

### 重启 Tenant Pod

使用 Kubernetes 原生命令：

```bash
kubectl rollout restart statefulset -n <namespace> -l rustfs.tenant=<tenant>
kubectl rollout status statefulset -n <namespace> -l rustfs.tenant=<tenant>
```

### 轮换管理员凭据

更新引用的 Secret，然后重启 Tenant StatefulSet，让 Pod 读取新 Secret：

```bash
kubectl create secret generic rustfs-admin-creds \
  -n <namespace> \
  --from-literal=accesskey=<new-access-key> \
  --from-literal=secretkey=<new-secret-key> \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl rollout restart statefulset -n <namespace> -l rustfs.tenant=<tenant>
```

## 12. 故障排查

### Tenant 处于 Blocked

```bash
kubectl describe tenant -n <namespace> <tenant>
kubectl get events -n <namespace> --sort-by=.lastTimestamp
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=operator
```

常见 blocked reason：

| Reason | 检查项 |
|--------|--------|
| `InvalidTenantName` | Tenant 名称长度和 DNS-1035 格式。 |
| `InvalidPoolSpec` | Pool 数量、总卷数、pool 名称和不可变字段。 |
| `CredentialSecretNotFound` | Secret 是否存在于 Tenant namespace。 |
| `CredentialSecretMissingKey` | Secret 是否包含 `accesskey` 和 `secretkey`。 |
| `CredentialSecretTooShort` | 两个凭据值是否都至少 8 个字符。 |
| `KmsSecretNotFound` / `KmsSecretMissingKey` | KMS Secret 是否存在，并包含必要 key，例如 Vault 的 `vault-token` 或 Local KMS 的 `masterKeySecretRef.key`。 |
| `CertManagerCrdMissing` / `CertManagerIssuerNotFound` | cert-manager 是否安装，issuer 是否存在。 |
| `StatefulSetUpdateValidationFailed` | 是否修改了不可变 StatefulSet 字段或 pool 形态字段。 |
| `ProvisioningFailed` | 检查 `status.provisioning`、policy ConfigMap、user Secret 和 RustFS 管理员凭据。 |

### Pod 没有 Ready

```bash
kubectl get pods -n <namespace> -l rustfs.tenant=<tenant>
kubectl describe pod -n <namespace> -l rustfs.tenant=<tenant>
kubectl logs -n <namespace> -l rustfs.tenant=<tenant>
```

重点检查 PVC 绑定、StorageClass、镜像拉取、node selector、toleration 和资源 request。

### S3 API 不可访问

检查 Tenant S3 Service 和 endpoints：

```bash
kubectl get svc,endpoints -n <namespace> <tenant>-io
kubectl port-forward -n <namespace> svc/<tenant>-io 9000:9000
```

### Console 登录失败

Operator Console 登录失败时，检查 ServiceAccount token 和 Console 日志：

```bash
kubectl -n rustfs-system create token rustfs-operator-console --duration=24h
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=console
```

RustFS Tenant Console 登录失败时，应使用 `spec.credsSecret` 或 RustFS 环境变量中配置的 Tenant 管理员凭据。

## 13. 最佳实践

- 生产环境使用 `spec.credsSecret` 或外部 Secret 管理系统。
- 开启 Kubernetes Secret at-rest encryption。
- 同一个 Tenant 内尽量使用同一性能等级的 StorageClass，除非你明确理解 RustFS 布局影响。
- 不要把一个 Tenant 内的多个 pool 当作冷热分层。
- 独立集群、独立管理边界或独立性能隔离应使用多个 Tenant。
- 生产环境 Operator Console 使用 HTTPS。
- 多副本 Console 部署保持 `console.jwtSecret` 稳定。
- 仅在安装 Prometheus Operator 后启用 `ServiceMonitor` 和 `PrometheusRule`。
- Tenant YAML 可以进入版本控制，但不要提交明文 Secret 值。
- 优先查看 `status.conditions`，再进一步排查 StatefulSet 和 Pod。

## 14. 相关文档

- [项目 README](../README.md)
- [部署入口文档](../deploy/README.md)
- [Helm Chart README](../deploy/rustfs-operator/README.md)
- [Tenant 示例](../examples/README.md)
- [Console 前端 README](../console-web/README.md)
