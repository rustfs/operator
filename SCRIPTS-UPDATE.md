# 脚本更新总结

## ✅ 已更新的脚本

### 1. deploy-rustfs.sh

**新增功能：**
- ✅ 添加 `start_console()` 函数 - 启动 Console 进程
- ✅ Console 进程后台运行，输出到 `console.log`
- ✅ Console PID 保存到 `console.pid`
- ✅ 更新访问信息，包含 Console API 端点说明
- ✅ 显示 Console 和 Operator 的日志路径

**启动流程：**
```bash
./deploy-rustfs.sh
```

**启动内容：**
1. 部署 CRD
2. 创建命名空间
3. 构建 Operator
4. 启动 Operator (`./operator server`)
5. **启动 Console (`./operator console --port 9090`)** ← 新增
6. 部署 Tenant

**Console 访问：**
- 本地 API: `http://localhost:9090`
- 健康检查: `curl http://localhost:9090/healthz`
- 日志文件: `console.log`
- PID 文件: `console.pid`

### 2. cleanup-rustfs.sh

**新增功能：**
- ✅ 添加 `stop_console()` 函数 - 停止 Console 进程
- ✅ 清理 `console.log` 和 `console.pid`
- ✅ 验证 Console 进程已停止

**清理顺序：**
1. 删除 Tenant
2. **停止 Console** ← 新增
3. 停止 Operator
4. 删除 Namespace
5. 删除 CRD
6. 清理本地文件

**验证检查：**
- ✓ Tenant 清理
- ✓ Namespace 清理
- ✓ CRD 清理
- ✓ Operator 停止
- **✓ Console 停止** ← 新增

### 3. check-rustfs.sh

**新增功能：**
- ✅ 检查 Console 本地进程是否运行
- ✅ 显示 Console API 访问信息
- ✅ 显示如何创建 K8s token 和登录

**Console 状态检查：**
```bash
./check-rustfs.sh
```

**输出信息：**
```
✅ Operator Console (local):
  Running at: http://localhost:9090
  Health check: curl http://localhost:9090/healthz
  API docs: deploy/console/README.md

  Create K8s token: kubectl create token default --duration=24h
  Login: POST http://localhost:9090/api/v1/login
```

## 使用场景

### 开发测试流程

```bash
# 1. 完整部署（Operator + Console + Tenant）
./deploy-rustfs.sh

# 2. 检查状态（包含 Console 状态）
./check-rustfs.sh

# 3. 测试 Console API
curl http://localhost:9090/healthz

# 创建测试 token
TOKEN=$(kubectl create token default --duration=24h)

# 登录 Console
curl -X POST http://localhost:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$TOKEN\"}" \
  -c cookies.txt

# 查询 Tenants
curl http://localhost:9090/api/v1/tenants -b cookies.txt

# 4. 查看日志
tail -f operator.log   # Operator 日志
tail -f console.log    # Console 日志

# 5. 清理所有资源
./cleanup-rustfs.sh
```

### 仅启动 Console

```bash
# 如果只需要 Console（CRD 已部署）
cargo run --release -- console --port 9090 > console.log 2>&1 &
echo $! > console.pid

# 停止 Console
kill $(cat console.pid)
rm console.pid
```

## 文件结构

```
.
├── deploy-rustfs.sh       ✅ 已更新（支持 Console）
├── cleanup-rustfs.sh      ✅ 已更新（清理 Console）
├── check-rustfs.sh        ✅ 已更新（检查 Console）
├── operator.log           # Operator 日志
├── operator.pid           # Operator 进程 ID
├── console.log            # Console 日志（新增）
├── console.pid            # Console 进程 ID（新增）
└── deploy/
    └── console/
        ├── README.md                    # Console 部署文档
        ├── KUBERNETES-INTEGRATION.md    # K8s 集成说明
        └── examples/
            ├── loadbalancer-example.md
            └── ingress-tls-example.md
```

## 进程管理

### 查看进程状态

```bash
# 查看 Operator 进程
pgrep -f "target/release/operator.*server"
ps aux | grep "[t]arget/release/operator.*server"

# 查看 Console 进程
pgrep -f "target/release/operator.*console"
ps aux | grep "[t]arget/release/operator.*console"
```

### 手动停止

```bash
# 停止 Operator
pkill -f "target/release/operator.*server"

# 停止 Console
pkill -f "target/release/operator.*console"
```

## 与 Kubernetes 部署的区别

### 本地部署（脚本）

- **Operator**: 本地进程，监控 K8s 集群
- **Console**: 本地进程，端口 9090
- **适用场景**: 开发、测试、调试

### Kubernetes 部署（Helm）

- **Operator**: Deployment，运行在集群内
- **Console**: Deployment，Service，可选 Ingress
- **适用场景**: 生产环境

**部署 Console 到 K8s：**
```bash
helm install rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true
```

参考文档: `deploy/console/README.md`

## 总结

三个脚本已全部更新，完整支持 Console：

✅ **deploy-rustfs.sh** - 自动启动 Console 进程
✅ **cleanup-rustfs.sh** - 自动停止和清理 Console
✅ **check-rustfs.sh** - 检查 Console 状态并显示访问信息

**一键部署测试环境，包含完整的 Operator + Console 功能！**
