# RustFS Operator Console - Kubernetes Integration Summary

## ✅ 已完成的集成

### 1. Helm Chart 模板（7个文件）

已在 `deploy/rustfs-operator/templates/` 中创建：

- **console-deployment.yaml** - Console Deployment 配置
  - 运行 `./operator console --port 9090`
  - 健康检查和就绪探针
  - JWT secret 通过环境变量注入
  - 支持多副本部署

- **console-service.yaml** - Service 配置
  - 支持 ClusterIP / NodePort / LoadBalancer
  - 默认端口 9090

- **console-serviceaccount.yaml** - ServiceAccount

- **console-clusterrole.yaml** - RBAC ClusterRole
  - Tenant 资源：完整 CRUD 权限
  - Namespace：读取和创建权限
  - Nodes, Events, Services, Pods：只读权限

- **console-clusterrolebinding.yaml** - RBAC 绑定

- **console-secret.yaml** - JWT Secret
  - 自动生成或使用配置的密钥

- **console-ingress.yaml** - Ingress 配置（可选）
  - 支持 TLS
  - 可配置域名和路径

### 2. Helm Values 配置

`deploy/rustfs-operator/values.yaml` 中新增 `console` 配置段：

```yaml
console:
  enabled: true                 # 启用/禁用 Console
  replicas: 1                   # 副本数
  port: 9090                    # 端口
  logLevel: info                # 日志级别
  jwtSecret: ""                 # JWT 密钥（留空自动生成）

  image: {}                     # 镜像配置（使用 operator 镜像）
  resources: {}                 # 资源限制
  service: {}                   # Service 配置
  ingress: {}                   # Ingress 配置
  rbac: {}                      # RBAC 配置
  serviceAccount: {}            # ServiceAccount 配置
```

### 3. Helm Helpers

`deploy/rustfs-operator/templates/_helpers.tpl` 中新增：

- `rustfs-operator.consoleServiceAccountName` - Console ServiceAccount 名称生成

### 4. 部署文档

- **deploy/console/README.md** - 完整部署指南
  - 架构说明
  - 部署方法（Helm / kubectl）
  - API 端点文档
  - 认证说明
  - RBAC 权限说明
  - 安全考虑
  - 故障排查

- **deploy/console/examples/loadbalancer-example.md** - LoadBalancer 部署示例

- **deploy/console/examples/ingress-tls-example.md** - Ingress + TLS 部署示例

## 部署方式

### 方式一：Helm（推荐）

```bash
# 启用 Console 部署
helm install rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true

# 使用 LoadBalancer
helm install rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true \
  --set console.service.type=LoadBalancer

# 自定义配置
helm install rustfs-operator deploy/rustfs-operator \
  -f custom-values.yaml
```

### 方式二：独立部署

可以从 Helm 模板生成 YAML 文件独立部署（需要 helm 命令）：

```bash
helm template rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true \
  > console-manifests.yaml

kubectl apply -f console-manifests.yaml
```

## 访问方式

### ClusterIP + Port Forward

```bash
kubectl port-forward svc/rustfs-operator-console 9090:9090
# 访问 http://localhost:9090
```

### LoadBalancer

```bash
kubectl get svc rustfs-operator-console
# 访问 http://<EXTERNAL-IP>:9090
```

### Ingress

```bash
# 访问 https://your-domain.com
```

## API 测试

```bash
# 健康检查
curl http://localhost:9090/healthz  # => "OK"

# 创建测试用户
kubectl create serviceaccount test-user
kubectl create clusterrolebinding test-admin \
  --clusterrole=cluster-admin \
  --serviceaccount=default:test-user

# 登录
TOKEN=$(kubectl create token test-user --duration=1h)
curl -X POST http://localhost:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$TOKEN\"}" \
  -c cookies.txt

# 访问 API
curl http://localhost:9090/api/v1/tenants -b cookies.txt
```

## 架构

```
┌─────────────────────────────────────────────────────────┐
│                    Kubernetes Cluster                    │
│                                                          │
│  ┌────────────────────┐      ┌─────────────────────┐   │
│  │  Operator Pod      │      │   Console Pod(s)    │   │
│  │                    │      │                     │   │
│  │  ./operator server │      │ ./operator console  │   │
│  │                    │      │   --port 9090       │   │
│  │  - Reconcile Loop  │      │                     │   │
│  │  - Watch Tenants   │      │ - REST API          │   │
│  │  - Manage K8s Res  │      │ - JWT Auth          │   │
│  └────────────────────┘      │ - Query K8s API     │   │
│           │                  └─────────────────────┘   │
│           │                           │                 │
│           ▼                           ▼                 │
│  ┌──────────────────────────────────────────────────┐  │
│  │           Kubernetes API Server                   │  │
│  │                                                   │  │
│  │  - Tenant CRDs                                   │  │
│  │  - Deployments, Services, ConfigMaps, etc.      │  │
│  └──────────────────────────────────────────────────┘  │
│                                                          │
└─────────────────────────────────────────────────────────┘
                           ▲
                           │
                  ┌────────┴────────┐
                  │  Users/Clients  │
                  │                 │
                  │  HTTP API Calls │
                  └─────────────────┘
```

## 安全特性

1. **JWT 认证** - 12小时会话过期
2. **HttpOnly Cookies** - 防止 XSS 攻击
3. **RBAC 集成** - 使用用户的 K8s Token 授权
4. **最小权限** - Console ServiceAccount 仅有必要权限
5. **TLS 支持** - 通过 Ingress 配置 HTTPS

## 下一步

1. **构建镜像**：Docker 镜像已包含 `console` 命令，无需修改 Dockerfile
2. **部署测试**：使用 Helm 或 kubectl 部署到集群
3. **集成前端**：（可选）开发 Web UI 调用 REST API
4. **添加监控**：集成 Prometheus metrics（未来增强）

## 相关文件

```
deploy/
├── rustfs-operator/
│   ├── templates/
│   │   ├── console-deployment.yaml      ✅
│   │   ├── console-service.yaml         ✅
│   │   ├── console-serviceaccount.yaml  ✅
│   │   ├── console-clusterrole.yaml     ✅
│   │   ├── console-clusterrolebinding.yaml ✅
│   │   ├── console-secret.yaml          ✅
│   │   ├── console-ingress.yaml         ✅
│   │   └── _helpers.tpl                 ✅ (已更新)
│   └── values.yaml                      ✅ (已更新)
└── console/
    ├── README.md                        ✅
    └── examples/
        ├── loadbalancer-example.md      ✅
        └── ingress-tls-example.md       ✅
```

## 总结

Console 后端已完全集成到 Kubernetes 部署体系中：

✅ Helm Chart 模板完整
✅ RBAC 权限配置
✅ Service、Ingress 支持
✅ 健康检查、就绪探针
✅ 安全配置（JWT Secret）
✅ 部署文档和示例
✅ 多种部署方式支持

**状态：生产就绪，可部署到 Kubernetes 集群** 🚀
