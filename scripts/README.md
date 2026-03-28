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

## Related docs

- Deployment: [deploy/README.md](../deploy/README.md)
- Tenant examples: [examples/README.md](../examples/README.md)
