# Scripts 脚本目录

本目录包含 RustFS Operator 的部署、清理与检查脚本，按用途归类。

## 目录结构

```
scripts/
├── README.md           # 本说明
├── deploy/             # 部署脚本
│   ├── deploy-rustfs.sh        # Kind 单节点 + 简单 Tenant 一键部署
│   └── deploy-rustfs-4node.sh  # Kind 4 节点 + 4 节点 Tenant 部署
├── cleanup/            # 清理脚本
│   ├── cleanup-rustfs.sh      # 清理 deploy-rustfs.sh 创建的资源
│   └── cleanup-rustfs-4node.sh # 清理 deploy-rustfs-4node.sh 创建的资源
├── check/              # 检查脚本
│   └── check-rustfs.sh        # 集群/Tenant 状态与访问信息
└── test/               # 脚本自检
    └── script-test.sh         # 校验各脚本语法
```

## 使用方式

**建议在项目根目录执行**（脚本内部会自动 `cd` 到项目根，因此从任意目录执行也可）：

```bash
# 从项目根执行
./scripts/deploy/deploy-rustfs.sh
./scripts/cleanup/cleanup-rustfs.sh
./scripts/check/check-rustfs.sh

# 4 节点部署与清理
./scripts/deploy/deploy-rustfs-4node.sh
./scripts/cleanup/cleanup-rustfs-4node.sh

# 校验所有脚本语法
./scripts/test/script-test.sh
```

## 依赖的路径约定

- 脚本依赖项目根目录下的 `deploy/`、`examples/`、`console-web/` 等路径。
- Kind 4 节点配置：`deploy/kind/kind-rustfs-cluster.yaml`。
- 脚本会先 `cd` 到项目根再执行，因此可从任意当前目录调用。

## 相关文档

- 部署说明：[deploy/README.md](../deploy/README.md)
- Tenant 示例：[examples/README.md](../examples/README.md)
