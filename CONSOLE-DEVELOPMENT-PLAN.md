# RustFS Operator Console 开发方案

**版本**: v1.0
**日期**: 2025-01-29
**状态**: 方案设计阶段

---

## 目录

1. [方案概述](#方案概述)
2. [需求分析](#需求分析)
3. [技术架构设计](#技术架构设计)
4. [实施路线图](#实施路线图)
5. [详细设计](#详细设计)
6. [开发计划](#开发计划)

---

## 方案概述

### 项目目标

为 RustFS Operator 开发一个 Web 管理控制台，提供图形化界面管理 RustFS Tenant 资源，参考 MinIO Operator Console 的设计理念，结合 RustFS 的特性进行定制开发。

### 核心价值

1. **降低使用门槛**: 通过 GUI 简化 RustFS Tenant 的创建和管理
2. **可视化监控**: 实时展示集群状态、存储使用量、Pod 健康状态
3. **运维效率**: 快速诊断问题、查看日志、管理资源
4. **用户体验**: 提供友好的交互界面，减少 YAML 配置错误

### 设计原则

- ✅ **云原生**: 无数据库设计，直接查询 Kubernetes API
- ✅ **轻量级**: 单容器部署，与 Operator 共用镜像
- ✅ **安全优先**: JWT 认证，RBAC 授权，HttpOnly Cookie
- ✅ **类型安全**: Rust 后端 + TypeScript 前端
- ✅ **声明式**: 通过 CRD 管理，保持 GitOps 友好

---

## 需求分析

### 现有 Operator 能力盘点

根据代码分析，RustFS Operator (v0.1.0) 已具备以下能力：

#### ✅ 已实现

| 功能模块 | 实现状态 | 代码位置 |
|---------|---------|---------|
| **Tenant CRD 定义** | ✅ 完整 | `src/types/v1alpha1/tenant.rs` |
| **Pool 管理** | ✅ 多 Pool 支持 | `src/types/v1alpha1/pool.rs` |
| **RBAC 资源** | ✅ Role/SA/RoleBinding | `src/types/v1alpha1/tenant/rbac.rs` |
| **Service 管理** | ✅ IO/Console/Headless | `src/types/v1alpha1/tenant/services.rs` |
| **StatefulSet 创建** | ✅ 每个 Pool 一个 SS | `src/types/v1alpha1/tenant/workloads.rs` |
| **凭证管理** | ✅ Secret + 环境变量 | `src/context.rs:validate_credential_secret()` |
| **日志配置** | ✅ Stdout/EmptyDir/Persistent | `src/types/v1alpha1/logging.rs` |
| **调度策略** | ✅ NodeSelector/Affinity/Tolerations | `src/types/v1alpha1/pool.rs:SchedulingConfig` |
| **事件记录** | ✅ Kubernetes Events | `src/context.rs:record()` |

#### ❌ 待实现 (Console 需要)

| 功能模块 | 优先级 | 说明 |
|---------|-------|------|
| **REST API** | 🔴 高 | 当前无 HTTP API,仅有 Reconcile 逻辑 |
| **认证授权** | 🔴 高 | 需要 JWT + K8s RBAC 集成 |
| **状态查询 API** | 🔴 高 | 查询 Tenant/Pod/PVC/Event |
| **资源计算 API** | 🟡 中 | 节点资源、Erasure Coding 计算 |
| **日志查询 API** | 🟡 中 | Pod 日志流式传输 |
| **前端界面** | 🔴 高 | React SPA |

### 功能需求清单

#### 核心功能 (MVP - v1.0)

**1. Tenant 生命周期管理**
- ✅ 创建 Tenant (多步骤向导)
- ✅ 查看 Tenant 列表
- ✅ 查看 Tenant 详情
- ✅ 删除 Tenant
- ⚠️ 更新 Tenant (v1.1)

**2. Pool 管理**
- ✅ 查看 Pool 列表和状态
- ✅ Pool 资源配置 (Servers、Volumes、Storage)
- ⚠️ 添加 Pool (v1.1)
- ⚠️ Pool 扩缩容 (v1.2)

**3. 资源监控**
- ✅ Pod 列表和状态
- ✅ PVC 列表和使用量
- ✅ Event 事件查看
- ✅ 集群资源统计

**4. 运维功能**
- ✅ Pod 日志查看
- ✅ Pod Describe
- ✅ Pod 删除/重启
- ⚠️ YAML 导入/导出 (v1.1)

**5. 认证与权限**
- ✅ JWT Token 登录
- ✅ Session 管理
- ⚠️ OAuth2/OIDC (v1.2)

#### 扩展功能 (v1.1+)

**6. 高级配置**
- 凭证管理 (Secret 创建/更新)
- 日志配置 (Stdout/EmptyDir/Persistent)
- 调度策略 (NodeSelector/Affinity)
- 镜像和版本管理

**7. 监控与告警** (v1.2)
- Prometheus 集成
- Grafana Dashboard 链接
- 健康检查状态

**8. 多租户与安全** (v1.3)
- Namespace 隔离
- RBAC 细粒度权限
- 审计日志

---

## 技术架构设计

### 整体架构

```
┌─────────────────────────────────────────────────────────┐
│                   浏览器 (用户)                          │
└──────────────────────┬──────────────────────────────────┘
                       │ HTTPS
                       ↓
┌─────────────────────────────────────────────────────────┐
│          Kubernetes Ingress / LoadBalancer              │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│              Console Service (ClusterIP)                │
│                 Port: 9090 (HTTP)                       │
│                 Port: 9443 (HTTPS)                      │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│          Console Pod (rustfs-operator 容器)              │
│  ┌─────────────────────────────────────────────────┐    │
│  │  Rust HTTP Server                               │    │
│  │  - Axum Web Framework                           │    │
│  │  - JWT 认证                                      │    │
│  │  - REST API (/api/v1/*)                         │    │
│  │  - 静态文件服务 (前端 SPA)                       │    │
│  └─────────────┬───────────────────────────────────┘    │
│                │ kube-rs client-go                       │
│                ↓                                         │
└────────────────────────────────────────────────────────┬┘
                                                          │
                                                          ↓
┌─────────────────────────────────────────────────────────┐
│             Kubernetes API Server                       │
│  ┌───────────────────────────────────────────────────┐  │
│  │               etcd (数据存储)                      │  │
│  │  • Tenant CRD                                     │  │
│  │  • Pod, Service, PVC, Secret                     │  │
│  │  • StatefulSet, Event                            │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### 技术栈选型

#### 后端 (Rust)

**核心框架**:
```toml
[dependencies]
# HTTP 框架 - 选择 Axum (性能优异 + 类型安全)
axum = { version = "0.7", features = ["ws", "multipart"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "compression-gzip", "trace"] }

# JSON 序列化 (已有)
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# JWT 认证
jsonwebtoken = "9.3"

# Kubernetes 客户端 (已有)
kube = { version = "2.0", features = ["runtime", "derive", "client", "rustls-tls"] }
k8s-openapi = { version = "0.26", features = ["v1_30"] }

# 异步运行时 (已有)
tokio = { version = "1.49", features = ["rt-multi-thread", "macros", "fs", "io-util"] }

# 日志和追踪 (已有)
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# 错误处理 (已有)
snafu = { version = "0.8", features = ["futures"] }
```

**为什么选择 Axum**:
- ✅ 与 tokio 生态完美集成
- ✅ 类型安全的路由和中间件
- ✅ 性能优异 (基于 hyper)
- ✅ 社区活跃,文档完善
- ✅ 支持 WebSocket (日志流式传输)

**替代方案对比**:
| 框架 | 优势 | 劣势 | 选择 |
|------|------|------|------|
| **Axum** | 类型安全、性能好、tokio 集成 | 生态相对年轻 | ✅ **推荐** |
| Actix-web | 成熟、性能最佳 | 类型复杂、actix 运行时 | ❌ |
| Rocket | 易用、宏强大 | 性能一般、async 支持晚 | ❌ |
| Warp | 函数式、灵活 | 学习曲线陡、错误难调试 | ❌ |

#### 前端 (TypeScript + React)

**技术栈** (参考 MinIO Operator Console):
```json
{
  "核心框架": "React 18",
  "语言": "TypeScript 5",
  "状态管理": "@reduxjs/toolkit",
  "路由": "react-router-dom 6",
  "UI 组件库": "shadcn/ui (Tailwind CSS + Radix UI)",
  "HTTP 客户端": "axios",
  "图表": "recharts",
  "构建工具": "Vite",
  "代码规范": "ESLint + Prettier"
}
```

**UI 组件库选择 - shadcn/ui**:
- ✅ 现代化设计 (基于 Tailwind CSS)
- ✅ 可复制代码,非 npm 依赖
- ✅ 高度可定制
- ✅ Radix UI 无障碍支持
- ✅ TypeScript 友好

**为什么不用 MinIO Design System (mds)**:
- ❌ 依赖 MinIO 特定设计
- ❌ 社区支持有限
- ❌ 定制难度大

### API 设计 (RESTful)

#### API 基础路径
```
/api/v1/*  - Console REST API
/         - 前端 SPA (index.html)
```

#### API 端点列表 (MVP)

**认证与会话**
```
POST   /api/v1/login           - JWT 登录
POST   /api/v1/logout          - 登出
GET    /api/v1/session         - 检查会话
```

**Tenant 管理**
```
GET    /api/v1/tenants                         - 列出所有 Tenants
POST   /api/v1/tenants                         - 创建 Tenant
GET    /api/v1/namespaces/{ns}/tenants         - 按命名空间列出
GET    /api/v1/namespaces/{ns}/tenants/{name}  - 获取详情
DELETE /api/v1/namespaces/{ns}/tenants/{name}  - 删除 Tenant
```

**Pool 管理**
```
GET    /api/v1/namespaces/{ns}/tenants/{name}/pools  - Pool 列表
```

**Pod 管理**
```
GET    /api/v1/namespaces/{ns}/tenants/{name}/pods          - Pod 列表
GET    /api/v1/namespaces/{ns}/tenants/{name}/pods/{pod}    - Pod 日志
GET    /api/v1/namespaces/{ns}/tenants/{name}/pods/{pod}/describe - Describe
DELETE /api/v1/namespaces/{ns}/tenants/{name}/pods/{pod}    - 删除 Pod
```

**PVC 管理**
```
GET    /api/v1/namespaces/{ns}/tenants/{name}/pvcs  - PVC 列表
```

**事件管理**
```
GET    /api/v1/namespaces/{ns}/tenants/{name}/events  - Event 列表
```

**集群资源**
```
GET    /api/v1/cluster/nodes           - 节点列表
GET    /api/v1/cluster/resources       - 可分配资源
GET    /api/v1/namespaces              - Namespace 列表
POST   /api/v1/namespaces              - 创建 Namespace
```

**健康检查**
```
GET    /healthz                        - 健康检查
GET    /readyz                         - 就绪检查
```

### 数据流设计

**无数据库架构** (与 MinIO Operator Console 一致):

```
前端请求
  ↓
Axum HTTP Handler
  ↓
kube::Client (已有 Context)
  ↓
Kubernetes API Server
  ↓
etcd (Tenant CRD, Pod, PVC, etc.)
```

**优势**:
- ✅ 无需维护数据库
- ✅ 数据始终最新 (实时查询)
- ✅ 简化部署和运维
- ✅ GitOps 友好

### 认证授权设计

#### JWT Token 认证流程

```
┌─────────────────────────────────────────────────────────┐
│  1. 用户获取 K8s ServiceAccount Token                    │
│     kubectl create token console-sa -n rustfs-operator   │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  2. 前端提交 Token 到 /api/v1/login                      │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  3. 后端验证 Token (调用 K8s API 测试权限)               │
│     kube::Client::new_with_token(token)                 │
│     client.list::<Tenant>().limit(1)  // 测试权限        │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  4. 生成 Console Session Token (JWT)                    │
│     Claims { k8s_token, exp: now + 12h }                │
│     签名: HMAC-SHA256(secret)                           │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  5. 设置 HttpOnly Cookie                                │
│     Set-Cookie: session=<jwt>; HttpOnly; Secure         │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  6. 后续请求携带 Cookie                                  │
│     Cookie: session=<jwt>                               │
└──────────────────┬──────────────────────────────────────┘
                   │
                   ↓
┌─────────────────────────────────────────────────────────┐
│  7. 中间件验证 JWT,提取 K8s Token                        │
│     使用 K8s Token 创建 Client,查询资源                  │
└─────────────────────────────────────────────────────────┘
```

#### RBAC 设计

**Console ServiceAccount**:
```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: console-sa
  namespace: rustfs-operator
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: rustfs-console-role
rules:
  # Tenant CRD 完整权限
  - apiGroups: ["rustfs.com"]
    resources: ["tenants"]
    verbs: ["get", "list", "watch", "create", "update", "delete"]
  - apiGroups: ["rustfs.com"]
    resources: ["tenants/status"]
    verbs: ["get", "update"]

  # 查看 K8s 资源
  - apiGroups: [""]
    resources: ["pods", "pods/log", "services", "persistentvolumeclaims", "events", "secrets", "configmaps"]
    verbs: ["get", "list", "watch"]

  # 删除 Pod (重启)
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["delete"]

  # 查看节点信息
  - apiGroups: [""]
    resources: ["nodes", "namespaces"]
    verbs: ["get", "list"]

  # 创建 Namespace
  - apiGroups: [""]
    resources: ["namespaces"]
    verbs: ["create"]

  # 查看 StatefulSet
  - apiGroups: ["apps"]
    resources: ["statefulsets"]
    verbs: ["get", "list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: rustfs-console-binding
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: rustfs-console-role
subjects:
  - kind: ServiceAccount
    name: console-sa
    namespace: rustfs-operator
```

---

## 实施路线图

### 阶段划分

#### 第一阶段: 后端 API 开发 (4-6 周)

**Week 1-2: 基础架构**
- [ ] Axum 项目初始化
- [ ] JWT 认证中间件
- [ ] 错误处理和日志
- [ ] 健康检查端点
- [ ] 基础测试框架

**Week 3-4: 核心 API**
- [ ] Tenant CRUD API
- [ ] Pool 查询 API
- [ ] Pod 管理 API
- [ ] PVC 查询 API
- [ ] Event 查询 API

**Week 5-6: 高级功能**
- [ ] 集群资源查询
- [ ] Pod 日志流式传输 (WebSocket)
- [ ] Session 管理
- [ ] API 文档生成 (OpenAPI)

#### 第二阶段: 前端开发 (6-8 周)

**Week 1-2: 项目搭建**
- [ ] Vite + React + TypeScript 初始化
- [ ] shadcn/ui 组件集成
- [ ] 路由和布局
- [ ] API 客户端生成
- [ ] 状态管理 (Redux Toolkit)

**Week 3-4: 核心页面**
- [ ] 登录页面
- [ ] Tenant 列表页面
- [ ] Tenant 创建向导
- [ ] Tenant 详情页面

**Week 5-6: 管理功能**
- [ ] Pod 管理页面
- [ ] PVC 管理页面
- [ ] Event 查看页面
- [ ] 日志查看器

**Week 7-8: 优化与测试**
- [ ] 响应式设计
- [ ] 错误处理优化
- [ ] 前端单元测试
- [ ] E2E 测试 (Playwright)

#### 第三阶段: 集成与部署 (2-3 周)

**Week 1: 集成测试**
- [ ] 前后端集成
- [ ] Kind/k3s 集群测试
- [ ] 性能测试
- [ ] 安全审计

**Week 2: 部署准备**
- [ ] Docker 镜像构建
- [ ] Helm Chart 开发
- [ ] 部署文档
- [ ] 用户手册

**Week 3: 发布准备**
- [ ] Release Notes
- [ ] 示例和教程
- [ ] CI/CD 配置
- [ ] v1.0 发布

#### 第四阶段: 迭代优化 (持续)

**v1.1 (1-2 月)**
- [ ] Tenant 更新功能
- [ ] Pool 添加功能
- [ ] YAML 导入/导出
- [ ] 凭证管理界面
- [ ] 日志配置界面

**v1.2 (3-4 月)**
- [ ] Pool 扩缩容
- [ ] Prometheus 集成
- [ ] OAuth2/OIDC 认证
- [ ] 多语言支持 (i18n)

**v1.3 (5-6 月)**
- [ ] 审计日志
- [ ] RBAC 细粒度权限
- [ ] Grafana 集成
- [ ] 告警配置

---

## 详细设计

### 后端项目结构

```
operator/
├── src/
│   ├── main.rs                    # 入口 (CLI 新增 console 子命令)
│   ├── lib.rs                     # 库入口
│   ├── reconcile.rs               # Operator reconcile 逻辑 (已有)
│   ├── context.rs                 # K8s Client Context (已有)
│   │
│   ├── console/                   # 🆕 Console 模块
│   │   ├── mod.rs                # Console 模块入口
│   │   ├── server.rs             # Axum HTTP Server
│   │   ├── routes/               # 路由模块
│   │   │   ├── mod.rs
│   │   │   ├── auth.rs           # 认证路由
│   │   │   ├── tenants.rs        # Tenant API
│   │   │   ├── pools.rs          # Pool API
│   │   │   ├── pods.rs           # Pod API
│   │   │   ├── pvcs.rs           # PVC API
│   │   │   ├── events.rs         # Event API
│   │   │   └── cluster.rs        # 集群资源 API
│   │   ├── handlers/             # 业务逻辑
│   │   │   ├── mod.rs
│   │   │   ├── tenant_handlers.rs
│   │   │   ├── pod_handlers.rs
│   │   │   └── ...
│   │   ├── middleware/           # 中间件
│   │   │   ├── auth.rs           # JWT 认证
│   │   │   ├── cors.rs           # CORS
│   │   │   └── logger.rs         # 请求日志
│   │   ├── models/               # API 数据模型
│   │   │   ├── mod.rs
│   │   │   ├── auth.rs           # LoginRequest, SessionResponse
│   │   │   ├── tenant.rs         # TenantListItem, CreateTenantRequest
│   │   │   └── ...
│   │   ├── services/             # 业务服务层
│   │   │   ├── tenant_service.rs
│   │   │   ├── k8s_service.rs    # K8s API 封装
│   │   │   └── ...
│   │   └── utils/                # 工具函数
│   │       ├── jwt.rs            # JWT 生成/验证
│   │       └── response.rs       # 统一响应格式
│   │
│   └── types/                    # CRD 类型 (已有)
│       └── v1alpha1/
│           ├── tenant.rs
│           ├── pool.rs
│           └── ...
│
├── console-ui/                   # 🆕 前端项目 (独立目录)
│   ├── src/
│   │   ├── main.tsx
│   │   ├── App.tsx
│   │   ├── api/                  # API 客户端
│   │   ├── components/           # UI 组件
│   │   ├── pages/                # 页面
│   │   ├── store/                # Redux Store
│   │   └── utils/
│   ├── public/
│   ├── index.html
│   ├── package.json
│   ├── vite.config.ts
│   └── tsconfig.json
│
├── Cargo.toml                    # 新增 console 依赖
├── Dockerfile                    # 修改: 多阶段构建 (前端 + 后端)
└── deploy/
    └── rustfs-operator/
        ├── console-deployment.yaml  # 🆕 Console Deployment
        └── console-service.yaml     # 🆕 Console Service
```

### 关键代码示例

#### 1. main.rs 新增 console 子命令

```rust
// src/main.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rustfs-operator")]
#[command(about = "RustFS Kubernetes Operator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate CRD YAML
    Crd {
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Run the operator controller
    Server,
    /// Run the console UI server  🆕
    Console {
        #[arg(long, default_value = "9090")]
        port: u16,
        #[arg(long)]
        tls_cert: Option<String>,
        #[arg(long)]
        tls_key: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Crd { file } => {
            // 已有逻辑
        }
        Commands::Server => {
            // 已有逻辑
        }
        Commands::Console { port, tls_cert, tls_key } => {
            // 🆕 启动 Console Server
            console::server::run(port, tls_cert, tls_key).await?;
        }
    }

    Ok(())
}
```

#### 2. Console HTTP Server (Axum)

```rust
// src/console/server.rs
use axum::{
    Router,
    routing::{get, post, delete},
    middleware,
};
use tower_http::{
    cors::CorsLayer,
    compression::CompressionLayer,
    trace::TraceLayer,
};
use std::net::SocketAddr;

pub async fn run(port: u16, tls_cert: Option<String>, tls_key: Option<String>) -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    // 构建路由
    let app = Router::new()
        // 健康检查
        .route("/healthz", get(health_check))
        .route("/readyz", get(ready_check))

        // API 路由
        .nest("/api/v1", api_routes())

        // 静态文件服务 (前端 SPA)
        .fallback_service(serve_static_files())

        // 中间件
        .layer(middleware::from_fn(auth_middleware))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    // 监听地址
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Console server listening on {}", addr);

    // 启动服务器
    if let (Some(cert), Some(key)) = (tls_cert, tls_key) {
        // HTTPS
        let config = rustls_config(cert, key)?;
        axum_server::bind_rustls(addr, config)
            .serve(app.into_make_service())
            .await?;
    } else {
        // HTTP
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await?;
    }

    Ok(())
}

fn api_routes() -> Router {
    Router::new()
        // 认证
        .route("/login", post(routes::auth::login))
        .route("/logout", post(routes::auth::logout))
        .route("/session", get(routes::auth::session_check))

        // Tenant
        .route("/tenants", get(routes::tenants::list_all))
        .route("/tenants", post(routes::tenants::create))
        .route("/namespaces/:ns/tenants", get(routes::tenants::list_by_ns))
        .route("/namespaces/:ns/tenants/:name", get(routes::tenants::get_details))
        .route("/namespaces/:ns/tenants/:name", delete(routes::tenants::delete))

        // Pod
        .route("/namespaces/:ns/tenants/:name/pods", get(routes::pods::list))
        .route("/namespaces/:ns/tenants/:name/pods/:pod", get(routes::pods::get_logs))
        .route("/namespaces/:ns/tenants/:name/pods/:pod", delete(routes::pods::delete))

        // ... 更多路由
}
```

#### 3. JWT 认证中间件

```rust
// src/console/middleware/auth.rs
use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, Validation, DecodingKey};

pub async fn auth_middleware(
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // 跳过登录等公开路径
    if req.uri().path().starts_with("/api/v1/login") || req.uri().path() == "/healthz" {
        return Ok(next.run(req).await);
    }

    // 从 Cookie 中提取 JWT
    let cookies = req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = parse_session_cookie(cookies)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // 验证 JWT
    let claims = decode::<Claims>(
        &token,
        &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?
    .claims;

    // 将 K8s Token 注入请求扩展
    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}

#[derive(Deserialize, Serialize)]
pub struct Claims {
    pub k8s_token: String,
    pub exp: usize,
}
```

#### 4. Tenant 创建 API

```rust
// src/console/handlers/tenant_handlers.rs
use axum::{
    extract::{Extension, Json},
    http::StatusCode,
};
use crate::console::models::tenant::{CreateTenantRequest, CreateTenantResponse};
use crate::context::Context;
use crate::types::v1alpha1::tenant::Tenant;

pub async fn create_tenant(
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateTenantRequest>,
) -> Result<Json<CreateTenantResponse>, StatusCode> {
    // 使用 K8s Token 创建 Client
    let client = kube::Client::try_from_token(&claims.k8s_token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let ctx = Context::new(client);

    // 构造 Tenant CRD
    let tenant = Tenant {
        metadata: ObjectMeta {
            name: Some(req.name.clone()),
            namespace: Some(req.namespace.clone()),
            ..Default::default()
        },
        spec: TenantSpec {
            pools: req.pools.into_iter().map(|p| p.into()).collect(),
            image: req.image,
            creds_secret: req.creds_secret.map(|name| LocalObjectReference { name }),
            ..Default::default()
        },
        status: None,
    };

    // 创建 Tenant
    let created = ctx.create(&tenant, &req.namespace).await
        .map_err(|e| {
            tracing::error!("Failed to create tenant: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(CreateTenantResponse {
        name: created.name_any(),
        namespace: created.namespace().unwrap_or_default(),
        created_at: created.metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }))
}
```

### 前端关键组件

#### 1. API 客户端

```typescript
// console-ui/src/api/client.ts
import axios, { AxiosInstance } from 'axios';

class ApiClient {
  private client: AxiosInstance;

  constructor() {
    this.client = axios.create({
      baseURL: '/api/v1',
      withCredentials: true, // 发送 Cookie
      headers: {
        'Content-Type': 'application/json',
      },
    });

    // 响应拦截器 - 处理 401
    this.client.interceptors.response.use(
      (response) => response,
      (error) => {
        if (error.response?.status === 401) {
          window.location.href = '/login';
        }
        return Promise.reject(error);
      }
    );
  }

  // Tenant API
  async listTenants() {
    const { data } = await this.client.get('/tenants');
    return data;
  }

  async createTenant(request: CreateTenantRequest) {
    const { data } = await this.client.post('/tenants', request);
    return data;
  }

  async getTenantDetails(namespace: string, name: string) {
    const { data } = await this.client.get(`/namespaces/${namespace}/tenants/${name}`);
    return data;
  }

  // ... 更多方法
}

export const api = new ApiClient();
```

#### 2. Tenant 列表页面

```tsx
// console-ui/src/pages/Tenants/TenantList.tsx
import { useEffect, useState } from 'react';
import { api } from '@/api/client';
import { Button } from '@/components/ui/button';
import { Table } from '@/components/ui/table';

export function TenantList() {
  const [tenants, setTenants] = useState([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    loadTenants();
  }, []);

  const loadTenants = async () => {
    try {
      const data = await api.listTenants();
      setTenants(data.tenants);
    } catch (error) {
      console.error('Failed to load tenants:', error);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="p-6">
      <div className="flex justify-between items-center mb-6">
        <h1 className="text-2xl font-bold">Tenants</h1>
        <Button onClick={() => navigate('/tenants/create')}>
          Create Tenant
        </Button>
      </div>

      {loading ? (
        <div>Loading...</div>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Namespace</TableHead>
              <TableHead>Pools</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
              <TableHead>Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {tenants.map((tenant) => (
              <TableRow key={tenant.name}>
                <TableCell>{tenant.name}</TableCell>
                <TableCell>{tenant.namespace}</TableCell>
                <TableCell>{tenant.poolCount}</TableCell>
                <TableCell>
                  <Badge variant={tenant.status === 'Ready' ? 'success' : 'warning'}>
                    {tenant.status}
                  </Badge>
                </TableCell>
                <TableCell>{new Date(tenant.createdAt).toLocaleString()}</TableCell>
                <TableCell>
                  <Button variant="ghost" size="sm" onClick={() => navigate(`/tenants/${tenant.namespace}/${tenant.name}`)}>
                    Details
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  );
}
```

---

## 开发计划

### 人力资源

**推荐配置**:
- **后端开发** (Rust): 1-2 人
- **前端开发** (TypeScript/React): 1-2 人
- **全栈开发** (可替代上述): 2 人
- **UI/UX 设计** (兼职): 0.5 人
- **测试工程师** (兼职): 0.5 人

**技能要求**:
- Rust: 熟悉 async/await、tokio、kube-rs
- TypeScript: 熟悉 React、Redux、TypeScript
- Kubernetes: 理解 CRD、RBAC、Controller 模式
- DevOps: Docker、Helm、CI/CD

### 里程碑

| 里程碑 | 时间 | 交付物 |
|--------|------|--------|
| **M1: 后端 API MVP** | Week 6 | 核心 API 完成,可通过 curl 测试 |
| **M2: 前端 MVP** | Week 14 | 基本 UI 完成,可创建/查看 Tenant |
| **M3: Alpha 版本** | Week 16 | 前后端集成,可在 Kind 集群测试 |
| **M4: Beta 版本** | Week 18 | 功能完善,性能优化,文档完备 |
| **M5: v1.0 发布** | Week 20 | 生产可用,发布到 GitHub Release |

### 风险评估

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|----------|
| **Axum 学习曲线** | 中 | 中 | 提前 PoC,参考官方示例 |
| **K8s API 复杂度** | 高 | 低 | 复用 Context 模块,借鉴 kube-rs 示例 |
| **前端状态管理** | 中 | 中 | 使用 Redux Toolkit 简化 |
| **WebSocket 实现** | 中 | 低 | Axum 内置支持,参考文档 |
| **性能瓶颈** | 中 | 低 | 早期性能测试,优化热点路径 |
| **安全漏洞** | 高 | 中 | 代码审查、依赖扫描、渗透测试 |

---

## 附录

### A. 参考资料

**MinIO Operator Console**:
- 源码: `~/my/minio-operator`
- 架构文档: `OPERATOR-CONSOLE-ARCHITECTURE.md`
- API 分析: `CONSOLE-API-ANALYSIS.md`

**Axum 文档**:
- 官方文档: https://docs.rs/axum
- GitHub: https://github.com/tokio-rs/axum
- 示例: https://github.com/tokio-rs/axum/tree/main/examples

**kube-rs 文档**:
- 官方文档: https://docs.rs/kube
- Controller Guide: https://kube.rs/controllers/intro/

**shadcn/ui**:
- 官网: https://ui.shadcn.com
- GitHub: https://github.com/shadcn-ui/ui

### B. 开发环境准备

**后端开发环境**:
```bash
# Rust 工具链 (已有)
rustc --version  # 应该是 Rust 1.91+

# 安装开发工具
cargo install cargo-watch  # 自动重新编译
cargo install cargo-nextest  # 更好的测试运行器

# 运行 Console (开发模式)
cargo watch -x 'run -- console --port 9090'
```

**前端开发环境**:
```bash
# Node.js (推荐 v20 LTS)
node --version  # v20.x

# 创建前端项目
cd operator
npm create vite@latest console-ui -- --template react-ts

# 安装依赖
cd console-ui
npm install

# 开发服务器 (代理到后端)
npm run dev  # http://localhost:5173
```

**Kubernetes 集群**:
```bash
# Kind (推荐用于本地开发)
kind create cluster --name rustfs-dev

# 部署 CRD
kubectl apply -f deploy/rustfs-operator/crds/

# 部署 Console
kubectl apply -f deploy/rustfs-operator/console-deployment.yaml
```

### C. 测试策略

**单元测试**:
- 后端: `cargo test` (所有 handlers、services)
- 前端: `npm test` (组件、工具函数)

**集成测试**:
- API 测试: Postman/Insomnia 集合
- E2E 测试: Playwright

**性能测试**:
- 并发测试: Apache Bench / wrk
- 内存分析: heaptrack / valgrind

---

## 总结

本方案为 RustFS Operator 设计了一个完整的 Web Console 开发计划,主要特点:

✅ **技术选型合理**: Axum (后端) + React (前端),与现有技术栈契合
✅ **架构清晰**: 参考 MinIO Operator Console,无数据库设计
✅ **分阶段实施**: 4 个阶段,20 周完成 MVP
✅ **风险可控**: 识别主要风险并提供缓解措施
✅ **可扩展性**: 预留 v1.1-v1.3 迭代计划

**下一步行动**:
1. 评审本方案,确定技术选型
2. 搭建 PoC (Proof of Concept) 验证可行性
3. 开始第一阶段开发 (后端 API)
4. 定期 Review 进度,调整计划

---

**文档版本**: v1.0
**最后更新**: 2025-01-29
**作者**: Claude Code
