# RustFS Kubernetes Operator

A Kubernetes operator for [RustFS](https://rustfs.com/) object storage, written in Rust with [kube-rs](https://github.com/kube-rs/kube). It reconciles a **`Tenant` custom resource** (`rustfs.com/v1alpha1`), validates referenced credential and KMS Secrets, and applies RBAC, Services, and StatefulSets so RustFS runs as an erasure-coded cluster inside your cluster.

**Status:** v0.1.0 pre-release — under active development.

## Features

- **Tenant CRD** — Declare pools, persistence, scheduling, credentials (Secret or env), TLS, and more; see [`examples/`](examples/).
- **Controller** — Reconciliation loop with status conditions (`Ready` / `Progressing` / `Degraded`), events, and safe StatefulSet update checks.
- **Operator HTTP console** — Optional management API (`cargo run -- console`, default port **9090**) used by [`console-web/`](console-web/) (Next.js UI).
- **Tooling** — CRD YAML generation, Docker multi-stage images, and a Rust-native Kind e2e harness under [`e2e/`](e2e/).

RustFS **S3 API** and **RustFS Console UI** inside a Tenant are exposed on **9000** and **9001** respectively; the operator’s own HTTP API is separate (typically **9090**).

## Architecture

![RustFS Operator Architecture](assets/rustfs-operator-architecture.png)

## Requirements

- **Rust** — Toolchain from [`rust-toolchain.toml`](rust-toolchain.toml) (stable; edition 2024).
- **Kubernetes** — Target API **v1.30** (see `Cargo.toml` / `k8s-openapi` features); a reachable cluster for `server` mode.
- **console-web** (optional) — **Node.js ≥ 20** and `pnpm install` in `console-web/` if you run frontend lint/format or UI dev.

## Quick start

**Local CLI**

```bash
# Clone and build
git clone https://github.com/rustfs/operator.git
cd operator
cargo build --release

# Emit Tenant CRD YAML (stdout or file)
cargo run -- crd
cargo run -- crd -f tenant-crd.yaml

# Run the controller (needs kubeconfig / in-cluster config)
cargo run -- server

# Run the operator HTTP console API (default :9090)
cargo run -- console

# For local HTTP-only browser testing with console-web on :3000
CONSOLE_COOKIE_SECURE=false CORS_ALLOWED_ORIGINS=http://localhost:3000,http://127.0.0.1:3000 cargo run -- console

# Or choose a custom Console API port
cargo run -- console --port 19090
```

**Docker image**

```bash
docker build -t rustfs/operator:dev .
docker run --rm rustfs/operator:dev -h
```

**Kind e2e**

Use the Make targets in [Development](#development). They drive the Rust-native e2e harness in [`e2e/`](e2e/) and use a dedicated Kind cluster named `rustfs-e2e`.

## Development

From the repo root:

| Command | Purpose |
|--------|---------|
| `make pre-commit` | Full local gate: Rust `fmt` / `clippy` / `test` + `console-web` ESLint, build, and Prettier (run after `pnpm install` in `console-web/`). |
| `make fmt` / `make clippy` / `make test` | Individual Rust checks. |
| `make console-lint` / `make console-fmt-check` | Frontend only. |
| `make e2e-check` | Validate the e2e harness without creating a live cluster. |
| `make e2e-live-create` | Build e2e images, recreate the dedicated Kind cluster, install cert-manager, and load images. |
| `make e2e-live-run` | Deploy the dev control plane and run all non-destructive live suites. |
| `make e2e-live-faults` | Run destructive live fault suites with `RUSTFS_E2E_DESTRUCTIVE=1`. |
| `make e2e-live-update` | Rebuild images, reload them into Kind, and roll out control-plane deployments. |
| `make e2e-live-delete` | Delete the dedicated Kind cluster and its local storage. |

CI (`.github/workflows/ci.yml`) runs Rust tests (including `nextest`), `cargo fmt --check`, `clippy`, the Rust-native e2e harness checks, and `console-web` lint/build/format checks. Use **`make pre-commit`** before opening a PR so local validation stays aligned.

Contribution workflow, commit style, and PR expectations: [`CONTRIBUTING.md`](CONTRIBUTING.md).

### Run a local controller against e2e

`cargo run -- server` uses the current kubeconfig context. To debug controller or reconcile changes against the dedicated e2e Kind cluster, point kubectl at `kind-rustfs-e2e` and stop the in-cluster operator first so only one controller reconciles the test resources:

```bash
make e2e-live-create
make e2e-live-run

kubectl --context kind-rustfs-e2e -n rustfs-system scale deploy/rustfs-operator --replicas=0
kubectl config use-context kind-rustfs-e2e

RUST_LOG=info cargo run -- server
```

Restore the in-cluster operator when you are done:

```bash
kubectl --context kind-rustfs-e2e -n rustfs-system scale deploy/rustfs-operator --replicas=1
kubectl --context kind-rustfs-e2e -n rustfs-system rollout status deploy/rustfs-operator
```

## Live e2e access

After `make e2e-live-create` and `make e2e-live-run`, the live environment uses:

- Kubernetes context: `kind-rustfs-e2e`
- Operator namespace: `rustfs-system`
- Tenant namespace: `rustfs-e2e-smoke`
- Tenant name: `e2e-tenant`

Useful checks:

```bash
kubectl --context kind-rustfs-e2e -n rustfs-system get pods,svc
kubectl --context kind-rustfs-e2e -n rustfs-e2e-smoke get tenant,pods,svc,pvc
```

Port-forward the operator Console API:

```bash
kubectl --context kind-rustfs-e2e -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090
curl http://127.0.0.1:19090/healthz
```

Port-forward the operator Console Web UI:

```bash
kubectl --context kind-rustfs-e2e -n rustfs-system port-forward svc/rustfs-operator-console-frontend 18080:80
```

Get a login token for the e2e Console:

```bash
TOKEN=$(kubectl --context kind-rustfs-e2e -n rustfs-system create token rustfs-operator-console --duration=24h)
printf '%s\n' "$TOKEN"
```

Open `http://127.0.0.1:18080` and paste the token into the login form. The dev/e2e Console deployment sets `CONSOLE_COOKIE_SECURE=false` for HTTP port-forwarding. The frontend proxies `/api/v1` to the Console API inside the cluster, so the Web UI only needs the frontend port-forward above.

Port-forward the e2e Tenant S3 API and Tenant Console:

```bash
kubectl --context kind-rustfs-e2e -n rustfs-e2e-smoke port-forward svc/e2e-tenant-io 19000:9000
kubectl --context kind-rustfs-e2e -n rustfs-e2e-smoke port-forward svc/e2e-tenant-console 19001:9001
```

Then use `http://127.0.0.1:19000` for the Tenant S3 API and `http://127.0.0.1:19001` for the Tenant Console.

## Repository layout

- **src/** — Operator controller, reconciler, CRD types, and Console API server.
- **console-web/** — Operator management UI built with Next.js.
- **deploy/** — Kubernetes deployment assets.
  - `deploy/rustfs-operator/` — Helm chart, templates, values, and packaged CRDs.
  - `deploy/k8s-dev/` — Development manifests used by the dev/e2e deployment flows.
  - `deploy/kind/` — Kind cluster configuration for local development.
- **e2e/** — Rust-native Kind e2e harness, live test suites, and dedicated manifests.
- **examples/** — Sample `Tenant` custom resources and usage notes.
- **docs/** — Design notes, GA planning material, and supporting images.
- **assets/** — README and documentation images.

## Documentation

| Doc | Content |
|-----|---------|
| [CONTRIBUTING.md](CONTRIBUTING.md) | Quality gates, `make pre-commit`, PR rules. |
| [examples/README.md](examples/README.md) | Tenant manifests and usage notes. |
| [deploy/README.md](deploy/README.md) | Helm and Kubernetes deployment entry point. |
| [deploy/rustfs-operator/README.md](deploy/rustfs-operator/README.md) | Helm chart values and examples. |
| [console-web/README.md](console-web/README.md) | Operator console frontend development and deployment. |

## License

Licensed under the **Apache License 2.0** — see [LICENSE](LICENSE).
