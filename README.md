# RustFS Kubernetes Operator

RustFS Kubernetes operator (under development; not production-ready).

## Repository layout

- **scripts/** — Deploy, cleanup, and check scripts (see [scripts/README.md](scripts/README.md))
  - `scripts/deploy/` — One-shot deploy (Kind + Operator + Tenant)
  - `scripts/cleanup/` — Resource cleanup
  - `scripts/check/` — Cluster and Tenant status checks
- **deploy/** — Kubernetes / Helm manifests and Kind configs
  - `deploy/rustfs-operator/` — Helm chart
  - `deploy/k8s-dev/` — Development Kubernetes YAML
  - `deploy/kind/` — Kind cluster configs (e.g. 4-node)
- **examples/** — Sample Tenant CRs
- **docs/** — Architecture and development documentation
