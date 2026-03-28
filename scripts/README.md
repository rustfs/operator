# Scripts

Shell scripts for deploying, cleaning up, and checking RustFS Operator resources, grouped by purpose.

## Layout

```
scripts/
├── README.md           # This file
├── deploy/             # Deploy scripts
│   ├── deploy-rustfs.sh        # Kind single-node + simple Tenant
│   └── deploy-rustfs-4node.sh  # Kind 4-node + 4-node Tenant
├── cleanup/            # Cleanup scripts
│   ├── cleanup-rustfs.sh      # Undo resources created by deploy-rustfs.sh
│   └── cleanup-rustfs-4node.sh # Undo resources created by deploy-rustfs-4node.sh
├── check/              # Check scripts
│   └── check-rustfs.sh        # Cluster / Tenant status and access hints
└── test/               # Script self-check
    └── script-test.sh         # Shell syntax check for all scripts
```

## Usage

**Run from the repository root** (recommended). Scripts `cd` to the project root internally, so they also work if invoked from another working directory:

```bash
./scripts/deploy/deploy-rustfs.sh
./scripts/cleanup/cleanup-rustfs.sh
./scripts/check/check-rustfs.sh

# 4-node deploy and cleanup
./scripts/deploy/deploy-rustfs-4node.sh
./scripts/cleanup/cleanup-rustfs-4node.sh

# Validate shell syntax for all scripts
./scripts/test/script-test.sh
```

## Path conventions

- Scripts expect paths under the repo root: `deploy/`, `examples/`, `console-web/`, etc.
- Kind 4-node config: `deploy/kind/kind-rustfs-cluster.yaml`.
- Each script switches to the project root before running.

## Docker image builds (deploy scripts)

Deploy scripts use **`docker build` with layer caching by default** so repeated runs reuse `cargo-chef` and base layers (much faster than before, when `--no-cache` was always set).

- **`RUSTFS_DOCKER_NO_CACHE=true`** — force a full rebuild (equivalent to adding `--no-cache` to every `docker build` in the script). Use when you need a clean image, e.g. after changing base images or debugging cache issues.

From the repo root:

```bash
# Fast rebuilds (default): uses cache
./scripts/deploy/deploy-rustfs.sh
./scripts/deploy/deploy-rustfs-4node.sh

# One-off clean rebuild
RUSTFS_DOCKER_NO_CACHE=true ./scripts/deploy/deploy-rustfs-4node.sh
```

**Further speed-ups (optional):**

- **`docker buildx build --load`** — BuildKit builder; can pair with [cache backends](https://docs.docker.com/build/cache/backends/) (e.g. registry cache in CI). Local `docker build` already uses BuildKit when `DOCKER_BUILDKIT=1` (default on recent Docker Engine).
- **Avoid duplicate work** — `deploy-rustfs-4node.sh` may run `cargo build --release` on the host and the Dockerfile also compiles inside the image; the host step is not required for the image itself (only speeds local binaries). You can skip the host `cargo build` when you only need the container.
- **`.dockerignore`** — ensure large unrelated paths are ignored so `COPY . .` stays small (repo should already exclude `target/` where appropriate).

## Related docs

- Deployment: [deploy/README.md](../deploy/README.md)
- Tenant examples: [examples/README.md](../examples/README.md)
