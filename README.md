# RustFS Kubernetes Operator

RustFS k8s operator（开发中，尚未可用于生产）。

## 项目结构概览

- **scripts/** — 部署/清理/检查脚本（见 [scripts/README.md](scripts/README.md)）
  - `scripts/deploy/` — 一键部署（Kind + Operator + Tenant）
  - `scripts/cleanup/` — 资源清理
  - `scripts/check/` — 集群与 Tenant 状态检查
- **deploy/** — K8s/Helm 部署清单与 Kind 配置
  - `deploy/rustfs-operator/` — Helm Chart
  - `deploy/k8s-dev/` — 开发用 K8s YAML
  - `deploy/kind/` — Kind 集群配置（如 4 节点）
- **examples/** — Tenant CR 示例
- **docs/** — 架构与开发文档
