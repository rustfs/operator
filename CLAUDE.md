# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Kubernetes operator for RustFS, written in Rust using the `kube-rs` library. The operator manages a custom resource `Tenant` (CRD) that provisions and manages RustFS storage clusters in Kubernetes.

**Current Status**: v0.1.0 (pre-release) - Early development, not yet production-ready
**Test Coverage**: 25 tests, all passing ✅

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

### Critical Architectural Understanding

**⚠️ IMPORTANT: RustFS Unified Cluster Architecture**

All pools within a single Tenant form **ONE unified RustFS erasure-coded cluster**:

1. **Unified Cluster**: Multiple pools do NOT create separate clusters; they create one combined cluster
2. **Uniform Data Distribution**: Erasure coding stripes data across ALL volumes in ALL pools equally
3. **No Storage Class Awareness**: RustFS does not intelligently place data based on storage performance
4. **Performance Limitation**: The entire cluster performs at the speed of the SLOWEST storage class
5. **External Tiering**: RustFS tiering uses lifecycle policies to external cloud storage (S3, Azure, GCS), NOT pool-based tiers

**Valid Multi-Pool Use Cases**:
- ✅ Cluster capacity expansion and gradual hardware migration
- ✅ Geographic distribution for compliance and disaster recovery
- ✅ Spot vs on-demand instance optimization (compute cost savings, not storage)
- ✅ Same storage class with different disk sizes
- ✅ Resource differentiation (CPU/memory) per pool
- ✅ Topology-aware distribution across failure domains

**Invalid Multi-Pool Use Cases**:
- ❌ Storage class mixing for performance tiering (NVMe for hot, HDD for cold)
- ❌ Automatic intelligent data placement based on access patterns

For separate RustFS clusters, create separate Tenants, NOT multiple pools.

See `docs/architecture-decisions.md` for detailed ADRs.

### Reconciliation Loop

The operator follows the standard Kubernetes controller pattern:
- **Entry Point**: `src/main.rs` - CLI with two subcommands: `crd` and `server`
- **Controller**: `src/lib.rs:run()` - Sets up the controller that watches `Tenant` resources and owned resources (ConfigMaps, Secrets, ServiceAccounts, Pods, StatefulSets)
- **Reconciliation Logic**: `src/reconcile.rs:reconcile_rustfs()` - Main reconciliation function that creates/updates Kubernetes resources for a Tenant
- **Error Handling**: `src/reconcile.rs:error_policy()` - Intelligent retry intervals based on error type:
  - Credential validation errors (user-fixable): 60-second requeue (reduces spam)
  - Transient API errors: 5-second requeue (fast recovery)
  - Other validation errors: 15-second requeue

### Custom Resource Definition (CRD)

- **Tenant CRD**: `src/types/v1alpha1/tenant.rs` - Defines the `Tenant` custom resource with spec and status
  - API Group: `rustfs.com/v1alpha1`
  - Primary spec fields: `pools`, `image`, `env`, `scheduler`, `configuration`, `image_pull_policy`, `pod_management_policy`
  - Each Tenant manages one or more Pools that form a unified cluster

- **Pool Spec**: `src/types/v1alpha1/pool.rs` - Defines a pool with `name`, `servers`, `persistence`, and `scheduling`
  - **Validation Rules**:
    - Pool name must not be empty
    - 2-server pools: must have at least 4 total volumes (`servers * volumesPerServer >= 4`)
    - 3-server pools: must have at least 6 total volumes
    - General: `servers * volumesPerServer >= 4`
  - **SchedulingConfig**: Per-pool scheduling (nodeSelector, affinity, tolerations, resources, topologySpreadConstraints, priorityClassName)
  - Uses `#[serde(flatten)]` to maintain flat YAML structure while grouping scheduling fields in code

- **Persistence Config**: `src/types/v1alpha1/persistence.rs`
  - `volumes_per_server`: Number of volumes per server (must be > 0)
  - `volume_claim_template`: **Required** field - must be specified
  - `path`: Optional custom volume mount path (default: `/data/rustfs{N}`)
  - `labels`, `annotations`: Optional metadata for PVCs

### RustFS-Specific Constants and Standards

**Service Ports** (verified against RustFS source code):
- **IO Service (S3 API)**: Port `9000` (not 90)
- **Console UI**: Port `9001` (not 9090)

**Volume Paths** (matches RustFS Helm chart and docker-compose):
- Mount path pattern: `/data/rustfs{0...N}` (not `/data/{N}`)
- Uses 3-dot ellipsis notation for RustFS expansion

**Required Environment Variables** (automatically set by operator):
- `RUSTFS_VOLUMES` - Combined volumes from all pools (space-separated)
- `RUSTFS_ADDRESS` - Server binding address (0.0.0.0:9000)
- `RUSTFS_CONSOLE_ADDRESS` - Console binding address (0.0.0.0:9001)
- `RUSTFS_CONSOLE_ENABLE` - Enable console UI (true)

**Credentials** (optional - from Secrets or environment variables):
- **Recommended**: Use a Secret referenced via `spec.credsSecret.name` (see `examples/secret-credentials-tenant.yaml`)
- **Alternative**: Provide via environment variables in `spec.env` (e.g., `RUSTFS_ACCESS_KEY`, `RUSTFS_SECRET_KEY`)
- **If neither provided**: RustFS will use built-in defaults (`rustfsadmin` / `rustfsadmin`) - acceptable for development, change for production
- Secret must contain: `accesskey` and `secretkey` keys (both required, valid UTF-8, minimum 8 characters)
- Priority: Secret credentials > Environment variables > RustFS defaults
- Validation: Only performed when Secret is configured
  - Secret exists in same namespace
  - Has both required keys
  - Keys are valid UTF-8
  - Keys are at least 8 characters long

### Context and API Wrapper

- **Context**: `src/context.rs` - Wraps the Kubernetes client and provides helper methods for CRUD operations
  - `apply()` - Server-side apply for declarative resource management
  - `get()`, `create()`, `delete()`, `list()` - Standard CRUD operations
  - `update_status()` - Updates Tenant status with retry logic for conflicts
  - `record()` - Publishes Kubernetes events for reconciliation actions
  - `validate_credential_secret()` - Validates credential Secret structure (when configured)
    - ✅ Validates Secret exists and has required keys (`accesskey`, `secretkey`)
    - ✅ Validates keys contain valid UTF-8 data
    - ✅ Validates minimum 8 characters for both keys
    - Does NOT extract credential values (for security)
    - Actual credential injection handled by Kubernetes via `secretKeyRef`
    - Returns comprehensive error messages for debugging

### Resource Creation

The `Tenant` type in `src/types/v1alpha1/tenant.rs` has factory methods for creating Kubernetes resources:
- **RBAC**: `new_role()`, `new_service_account()`, `new_role_binding()`
- **Services**: `new_io_service()`, `new_console_service()`, `new_headless_service()`
- **Workloads**: `new_statefulset()` - Creates one StatefulSet per pool
- **Helper Methods**: Extracted to `src/types/v1alpha1/tenant/helper.rs` for better organization
- All created resources include proper owner references for garbage collection

### Status Management

- **Status Types**: `src/types/v1alpha1/status/` - Status structures including state, pool status, and certificate status
- The status is updated via the Kubernetes status subresource
- **TODO at `reconcile.rs:92`**: Implement comprehensive status condition updates on errors (Ready, Progressing, Degraded)

### Utilities

- **TLS Utilities**: `src/utils/tls.rs` - X.509 certificate and private key validation
  - Supports RSA, ECDSA (P-256), and Ed25519 key types
  - Supports PKCS#1, PKCS#8, and SEC1 formats
  - Validates that private keys match certificate public keys

### Test Infrastructure

- **Test Module**: `src/tests.rs` - Centralized test helpers
  - `create_test_tenant()` - Helper function for consistent test tenant creation
  - Used across test suites for better maintainability

## Code Structure Notes

- Uses `kube-rs` with specific git revisions for `k8s-openapi` and `kube` crates
- Kubernetes version target: v1.30
- Error handling uses the `snafu` crate for structured error types
- All files include Apache 2.0 license headers
- Uses `strum::Display` for enum-to-string conversions (`ImagePullPolicy`, `PodManagementPolicy`, `PoolState`, `State`)

## Important Dependencies

- **kube** and **k8s-openapi**: Pinned to specific git revisions (not crates.io versions)
  - TODO: Evaluate migration to crates.io versions
- Uses Rust edition 2024
- Build script (`build.rs`) generates build metadata using the `built` crate

## Known Issues and TODOs

### High Priority
- [x] ~~**Secret-based credential management**~~ ✅ **COMPLETED** (2025-11-15, Issue #41)
- [ ] **Status condition management** (`src/reconcile.rs:92`, Issue #42)
- [ ] **StatefulSet reconciliation** (`reconcile.rs`) - Creation works, updates need refinement (Issue #43)
- [ ] **Integration tests** - Only unit tests currently exist

### Medium Priority
- [ ] Status subresource update retry logic improvements
- [ ] TLS certificate rotation automation
- [ ] Configuration validation enhancements (storage class existence, node selector validity)

## Documentation Structure

- **CHANGELOG.md** - All notable changes following Keep a Changelog format
- **ROADMAP.md** - Development roadmap organized by focus areas (Core Stability, Advanced Features, Enterprise Features, Production Ready)
- **docs/architecture-decisions.md** - ADRs documenting key architectural decisions
- **docs/multi-pool-use-cases.md** - Comprehensive guide for multi-pool scenarios
- **docs/DEVELOPMENT-NOTES.md** - Development workflow and contribution guidelines

## Examples

Located in `examples/` directory (moved from `deploy/rustfs-operator/examples/`):

**Production Examples**:
- `production-ha-tenant.yaml` - Production HA with topology spread constraints
- `cluster-expansion-tenant.yaml` - Capacity expansion and hardware migration
- `geographic-pools-tenant.yaml` - Multi-region deployment

**Development Examples**:
- `simple-tenant.yaml` - Simple single-pool tenant with documentation
- `minimal-dev-tenant.yaml` - Minimal development configuration
- `multi-pool-tenant.yaml` - Basic multi-pool example

**Advanced Scenarios**:
- `spot-instance-tenant.yaml` - Cost optimization using spot instances
- `hardware-pools-tenant.yaml` - Heterogeneous disk sizes (same storage class)
- `custom-rbac-tenant.yaml` - Custom RBAC configuration

All examples include:
- Inline documentation explaining configuration choices
- Architectural warnings about RustFS unified cluster behavior
- kubectl verification commands

See `examples/README.md` for comprehensive usage guide.

## Development Priorities (from ROADMAP.md)

### Core Stability (Highest Priority)
- Secret-based credential management
- Status condition management (Ready, Progressing, Degraded)
- StatefulSet update and rollout management
- Improved error handling and observability
- Integration test suite

### Advanced Features
- Tenant lifecycle management with finalizers
- Pool lifecycle management (add/remove/scale)
- TLS/certificate automation (cert-manager integration)
- Monitoring and alerting (Prometheus, Grafana)

### Enterprise Features
- Multi-tenancy enhancements
- Security hardening (Pod Security Standards)
- Compliance and audit logging
- Advanced networking and storage enhancements

### Production Ready (Long-term Goals)
- 95%+ test coverage
- Complete API documentation
- Ecosystem integration (OperatorHub, Helm, OLM)
- Community and support channels

## Verification Standards

All RustFS-specific constants and behaviors should be verified against:
- RustFS source code (`~/git/rustfs`)
- RustFS Helm chart (`helm/rustfs/`)
- RustFS docker-compose examples
- RustFS MNMD deployment guide
- RustFS configuration constants

**Do not invent or assume RustFS features** - always verify against official sources.
