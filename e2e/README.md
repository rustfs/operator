# RustFS Operator E2E Harness

This crate is the Rust-native integration-test harness for release-grade validation of the RustFS Operator and its Console API.

The harness is intentionally separated from the main operator crate so heavy e2e dependencies do not slow the default `make pre-commit` path. It is driven through the reduced live entrypoints `e2e-live-create`, `e2e-live-run`, `e2e-live-update`, and `e2e-live-delete`.

## Architecture

The harness is split into four top-level domains:

- `manifests/`: e2e-owned static manifests such as the dedicated Kind config.
- `framework/`: reusable infrastructure primitives.
- `cases/`: release test-case inventory grouped by product boundary.
- `tests/`: executable suite entrypoints; live tests are ignored by default and run only through explicit Make targets.

```text
e2e/
  Cargo.toml
  manifests/
    kind-rustfs-e2e.yaml  dedicated 1 control-plane + 3 worker Kind cluster
  src/
    lib.rs
    bin/rustfs-e2e.rs  small CLI for plan/doctor/kind/image/storage commands
    framework/
      config.rs          environment and CI knobs
      command.rs         safe subprocess wrapper for kind/docker/kubectl
      kind.rs            Kind cluster lifecycle and host mount preparation
      kubectl.rs         kubectl command construction boundary
      live.rs            live-run guardrails and context safety
      tools.rs           local tool doctor checks
      kube_client.rs     kube-rs client boundary
      console_client.rs  reqwest Console API boundary
      wait.rs            timeout/polling helpers and Tenant Ready wait
      artifacts.rs       failure artifact collection boundary
      port_forward.rs    kubectl port-forward boundary
      images.rs          operator/console/rustfs image set boundary
      resources.rs       namespace/Secret/Tenant apply boundary
      storage.rs         local StorageClass/PV preparation boundary
      assertions.rs      Kubernetes and Tenant status assertions
      tenant_factory.rs  reusable Tenant manifests for e2e
    cases/
      smoke.rs           install and health checks
      operator.rs        reconcile/status/conditions/events checks
      console.rs         Console API/auth/topology/events checks
      faults.rs          deterministic failure/recovery checks
  tests/
    smoke.rs             ignored live smoke entrypoints
    operator.rs          ignored live Operator assertions
    console.rs           ignored live Console API assertions
    faults.rs            destructive-fault guard entrypoint
```

## Boundary rules

1. `framework::command` is the only layer that should execute host commands directly.
2. `framework::kubectl` is the shell/Kubernetes YAML boundary and must always pin `--context`.
3. `framework::kube_client` is the typed Kubernetes API boundary.
4. `framework::console_client` is the HTTP boundary for Console API tests.
5. `framework::storage` owns e2e local PV setup; `framework::resources` owns e2e namespace/Secret/Tenant setup.
6. `framework::live` owns live-run opt-in and dedicated-context checks.
7. `cases/*` should describe behavior and call framework helpers; avoid shell details there.
8. Destructive tests must use dedicated e2e namespaces and must never run against an arbitrary current context.

## Safety defaults

Default configuration targets a dedicated Kind cluster:

```text
cluster:          rustfs-e2e
context:          kind-rustfs-e2e
operator ns:      rustfs-system
test namespace:   rustfs-e2e-smoke
tenant name:      e2e-tenant
console URL:      http://127.0.0.1:19090
storage class:    local-storage
PV count:         12
kind config:      e2e/manifests/kind-rustfs-e2e.yaml
```

Live tests are `#[ignore]` and require explicit opt-in:

```bash
RUSTFS_E2E_LIVE=1 make e2e-smoke-live
```

Fault tests also require:

```bash
RUSTFS_E2E_DESTRUCTIVE=1
```

The harness refuses to run live tests unless the active Kubernetes context matches the configured dedicated Kind context.

## Usage (保留 4 个常用入口)

- `make e2e-live-create`：
  创建 live 环境（默认会 build e2e 镜像，删除旧 `kind-rustfs-e2e`，清理 dedicated storage，再 create + load 镜像）。
- `make e2e-live-run`：
  在现有 live 环境执行全部 live 用例（smoke/operator/console）。
- `make e2e-live-update`：
  只重建发生变化的镜像（默认增量），然后把镜像更新到 live（load + rollout）。
  前提：控制面组件已部署（`make e2e-live-run` 之后或手工 `e2e-deploy-dev`）。
  如需强制重建可加 `E2E_FORCE_REBUILD=1`。
- `make e2e-live-delete`：
  删除 live 集群，并清理 dedicated storage 目录 `/tmp/rustfs-e2e-storage-{1,2,3}`。

推荐工作流：

```bash
# 首次创建环境
make e2e-live-create

# 先跑一遍用例（会部署控制面并创建 tenant）
make e2e-live-run

# 修改代码后，更新镜像并重启 deployment
make e2e-live-update

# 更新后再跑用例
make e2e-live-run

# 用完后清理
make e2e-live-delete
```

`make e2e-faults-live` 仍作为显式破坏性场景保留（需 `RUSTFS_E2E_DESTRUCTIVE=1`）。
