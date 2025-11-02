# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Kubernetes operator for RustFS, written in Rust using the `kube-rs` library. The operator manages a custom resource `Tenant` (CRD) that provisions and manages RustFS storage clusters in Kubernetes. The project is currently in early development and not yet production-ready.

## Build and Development Commands

### Building
```bash
cargo build              # Debug build
cargo build --release    # Release build
```

### Testing
```bash
cargo test               # Run all tests
cargo test -- --ignored  # Run ignored tests (includes TLS tests)
```

### Running the Operator
```bash
# Generate CRD YAML to stdout
cargo run -- crd

# Generate CRD YAML to file
cargo run -- crd -f tenant-crd.yaml

# Run the controller (requires Kubernetes cluster access)
cargo run -- server
```

### Docker
```bash
# Build the Docker image
docker build -t operator .
```

Note: The Dockerfile uses a multi-stage build with Rust 1.91-alpine.

## Architecture Overview

### Reconciliation Loop
The operator follows the standard Kubernetes controller pattern:
- **Entry Point**: `src/main.rs` - CLI with two subcommands: `crd` and `server`
- **Controller**: `src/lib.rs:run()` - Sets up the controller that watches `Tenant` resources and owned resources (ConfigMaps, Secrets, ServiceAccounts, Pods, StatefulSets)
- **Reconciliation Logic**: `src/reconcile.rs:reconcile_rustfs()` - Main reconciliation function that creates/updates Kubernetes resources for a Tenant
- **Error Handling**: `src/reconcile.rs:error_policy()` - Returns a 5-second requeue on errors

### Custom Resource Definition (CRD)
- **Tenant CRD**: `src/types/v1alpha1/tenant.rs` - Defines the `Tenant` custom resource with spec and status
  - API Group: `rustfs.com/v1alpha1`
  - Primary spec fields: `pools`, `image`, `env`, `scheduler`, `configuration`
  - Each Tenant manages one or more Pools
- **Pool Spec**: `src/types/v1alpha1/pool.rs` - Defines a pool with `name`, `servers`, and `volumes_per_server`
  - Validation: `servers * volumesPerServer >= 4`

### Context and API Wrapper
- **Context**: `src/context.rs` - Wraps the Kubernetes client and provides helper methods for CRUD operations
  - `apply()` - Server-side apply for declarative resource management
  - `get()`, `create()`, `delete()`, `list()` - Standard CRUD operations
  - `update_status()` - Updates Tenant status with retry logic for conflicts
  - `record()` - Publishes Kubernetes events for reconciliation actions

### Resource Creation
The `Tenant` type in `src/types/v1alpha1/tenant.rs` has factory methods for creating Kubernetes resources:
- `new_role()`, `new_service_account()`, `new_role_binding()` - RBAC resources
- `new_io_service()`, `new_console_service()`, `new_headless_service()` - Service resources
- `new_statefulset()` - StatefulSet for pool management (not yet implemented)
- All created resources include proper owner references for garbage collection

### Status Management
- **Status Types**: `src/types/v1alpha1/status/` - Status structures including state, pool status, and certificate status
- The status is updated via the Kubernetes status subresource

### Utilities
- **TLS Utilities**: `src/utils/tls.rs` - X.509 certificate and private key validation
  - Supports RSA, ECDSA (P-256), and Ed25519 key types
  - Validates that private keys match certificate public keys

## Code Structure Notes

- Uses `kube-rs` with specific git revisions for `k8s-openapi` and `kube` crates
- Kubernetes version target: v1.30
- Error handling uses the `snafu` crate for structured error types
- The reconciliation loop currently creates RBAC resources but StatefulSet creation is incomplete (see TODO comment in `reconcile.rs:33-37`)
- All files include Apache 2.0 license headers

## Important Dependencies

- `kube` and `k8s-openapi`: Pinned to specific git revisions (not crates.io versions)
- Uses Rust edition 2024
- Build script (`build.rs`) generates build metadata using the `built` crate
