# Changelog

All notable changes to the RustFS Kubernetes Operator will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Documentation

- Expanded root [`README.md`](README.md) with overview, quick start, development commands, CI vs `make pre-commit`, and documentation index.
- Aligned [`CLAUDE.md`](CLAUDE.md) and [`ROADMAP.md`](ROADMAP.md) with current code: Tenant status conditions and StatefulSet updates on the successful reconcile path are documented as implemented; remaining work (status on early errors, integration tests, rollout extras) is listed explicitly.
- Clarified the documentation map: [`CONTRIBUTING.md`](CONTRIBUTING.md) (quality gates and CI alignment), [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) (environment setup), [`docs/DEVELOPMENT-NOTES.md`](docs/DEVELOPMENT-NOTES.md) (historical notes, not normative).
- Updated [`examples/README.md`](examples/README.md): Tenant Services document S3 **9000** and RustFS Console **9001**; distinguished the Operator HTTP Console (default **9090**, `cargo run -- console`) from the Tenant `{tenant}-console` Service.
- Standardized [`README.md`](README.md), [`scripts/README.md`](scripts/README.md), and shell scripts under `scripts/` to English for consistency with architecture and development docs.
- Translated Rust doc and line comments in [`src/console/`](src/console/) to English (no behavior change).

### Fixed

- **`console-web` / `make pre-commit`**: `npm run lint` now runs `eslint .` (bare `eslint` only printed CLI help). Added `format` / `format:check` scripts; [`Makefile`](Makefile) `console-fmt` and `console-fmt-check` call them so Prettier resolves from `node_modules` after `npm install` in `console-web/`.

- **Tenant `Pool` CRD validation (CEL)**: Match the operator console API — require `servers × volumesPerServer >= 4` for every pool, and `>= 6` total volumes when `servers == 3` (fixes the previous 3-server rule using `< 4` in CEL). Regenerated [`deploy/rustfs-operator/crds/tenant-crd.yaml`](deploy/rustfs-operator/crds/tenant-crd.yaml) and [`tenant.yaml`](deploy/rustfs-operator/crds/tenant.yaml). Added [`validate_pool_total_volumes`](src/types/v1alpha1/pool.rs) as the shared Rust implementation used by [`src/console/handlers/pools.rs`](src/console/handlers/pools.rs).

- **Tenant name length**: [`validate_dns1035_label`](src/types/v1alpha1/tenant.rs) now caps `metadata.name` at **55** characters so derived names like `{name}-console` remain valid Kubernetes DNS labels (≤ 63).

### Changed

- **Deploy scripts** ([`scripts/deploy/deploy-rustfs.sh`](scripts/deploy/deploy-rustfs.sh), [`deploy-rustfs-4node.sh`](scripts/deploy/deploy-rustfs-4node.sh)): Docker builds use **layer cache by default** (`docker_build_cached`); set `RUSTFS_DOCKER_NO_CACHE=true` for a full rebuild. Documented in [`scripts/README.md`](scripts/README.md).
- **4-node deploy**: Help text moved to an early heredoc (avoids trailing `case`/parse issues); see script header.
- **4-node cleanup** ([`cleanup-rustfs-4node.sh`](scripts/cleanup/cleanup-rustfs-4node.sh)): Host storage dirs under `/tmp/rustfs-storage-*` may require `sudo rm -rf` after Kind (root-owned bind mounts).
- **Dockerfile** (operator and [`console-web/Dockerfile`](console-web/Dockerfile)): Build caching and reproducibility tweaks (cargo-chef pin, pnpm in frontend image as applicable).

### Added

- Cursor Agent skill [`.cursor/skills/rustfs-operator-contribute/SKILL.md`](.cursor/skills/rustfs-operator-contribute/SKILL.md) for `make pre-commit`, commit, push to fork `my`, and opening PRs to `rustfs/operator` with the project template.

#### **StatefulSet Reconciliation Improvements** (2025-12-03, Issue #43)

Implemented intelligent StatefulSet update detection and validation to improve reconciliation efficiency and safety:

- **Diff Detection**: Added `statefulset_needs_update()` method to detect actual changes
  - Compares existing vs desired StatefulSet specs semantically
  - Avoids unnecessary API calls when no changes are needed
  - Checks: replicas, image, env vars, resources, scheduling, pod management policy, etc.

- **Immutable Field Validation**: Added `validate_statefulset_update()` method
  - Prevents modifications to immutable StatefulSet fields (selector, volumeClaimTemplates, serviceName)
  - Provides clear error messages for invalid updates (e.g., changing volumesPerServer)
  - Protects against API rejections during reconciliation

- **Enhanced Reconciliation Logic**: Refactored StatefulSet reconciliation loop
  - Checks if StatefulSet exists before attempting update
  - Validates update safety before applying changes
  - Only applies updates when actual changes are detected
  - Records Kubernetes events for update lifecycle (Created, UpdateStarted, UpdateValidationFailed)

- **Error Handling**: Extended error policy
  - Added 60-second requeue for immutable field modification errors (user-fixable)
  - Consistent error handling across credential and validation failures

- **New Error Types**: Added to `types::error::Error`
  - `InternalError` - For unexpected internal conditions
  - `ImmutableFieldModified` - For attempted modifications to immutable fields
  - `SerdeJson` - For JSON serialization errors during comparisons

- **Comprehensive Test Coverage**: Added 9 new unit tests (35 tests total)
  - Tests for diff detection (no changes, image, replicas, env vars, resources)
  - Tests for validation (selector, serviceName, volumesPerServer changes rejected)
  - Test for safe updates (image changes allowed)

**Benefits**:
- Reduces unnecessary API calls and reconciliation overhead
- Prevents reconciliation failures from invalid updates
- Provides better error messages for users
- Foundation for rollout monitoring (Phase 2)

### Changed

#### **Code Refactoring**: Credential Validation Simplification (2025-11-15)

- **Renamed**: `get_tenant_credentials()` → `validate_credential_secret()`
  - Function now only validates Secret structure (exists, has required keys)
  - No longer extracts or returns credential values
  - Removed environment variable fallback logic
  - Returns `Result<(), Error>` instead of `BTreeMap<String, String>`
  - **Added**: Minimum length validation (8 characters for both accesskey and secretkey)

- **Purpose**: Eliminate duplication between validation and runtime credential injection
  - Validation: Performed by `validate_credential_secret()` in reconciliation loop
  - Runtime: Handled by Kubernetes via `secretKeyRef` in StatefulSet environment variables

- **Benefits**:
  - Clearer separation of concerns
  - Credentials never loaded into operator memory (more secure)
  - Simpler code with single responsibility
  - Consistent behavior between validation and runtime
  - Better security with minimum length requirements

#### **BREAKING CHANGE**: Field Rename - `configuration` → `credsSecret` (2025-11-15)

- **Field Renamed**: `spec.configuration` → `spec.credsSecret`
  - **Rationale**: The name `configuration` was too generic and didn't clearly indicate its purpose (referencing a Secret containing RustFS credentials)
  - **New Name**: `credsSecret` follows Kubernetes naming conventions (similar to `imagePullSecrets`) and clearly indicates it references a Secret with credentials
  - **Migration Required**: Update your Tenant manifests to use `credsSecret` instead of `configuration`

**Before (v0.1.0):**
```yaml
spec:
  configuration:
    name: rustfs-credentials
```

**After (v0.2.0):**
```yaml
spec:
  credsSecret:
    name: rustfs-credentials
```

- **Impact**: All Tenant resources using `spec.configuration` must be updated
- **Migration**: Simple find-and-replace: `configuration:` → `credsSecret:`
- **Note**: This is acceptable at v0.1.0 (pre-release) stage before production adoption

### Added

#### Secret-Based Credential Management (2025-11-15)

- **Secure Credentials via Kubernetes Secrets**: New `spec.credsSecret` field for referencing credentials Secret
  - **Recommended for production**: Store RustFS admin credentials in Kubernetes Secrets
  - **Secret Structure**: Must contain `accesskey` and `secretkey` keys
  - **Automatic Injection**: Credentials automatically injected as `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY` environment variables
  - **Validation**: Optional validation when Secret is configured
    - Secret must exist in the same namespace
    - Must have both `accesskey` and `secretkey` keys
    - Both keys must be valid UTF-8 strings
    - Both keys must be at least 8 characters long
  - **Priority**: Secret credentials take precedence over environment variables
  - **Backward Compatible**: Environment variable-based credentials still supported

- **Smart Error Retry Logic**:
  - Credential validation errors (user-fixable): 60-second retry interval (reduces log spam)
  - Transient API errors: 5-second retry (fast recovery)
  - Other validation errors: 15-second retry
  - Auto-recovery when Secret is fixed

- **New Example**: `examples/secret-credentials-tenant.yaml`
  - Complete working example with Secret + Tenant
  - Production security best practices
  - Troubleshooting guide
  - Error retry behavior documentation

- **Documentation Updates**:
  - Updated CLAUDE.md with credential management section
  - Updated ROADMAP.md (marked feature as completed ✅)
  - Enhanced examples/README.md with security guidance

#### Multi-Pool Scheduling Enhancements (2025-11-08)

- **Per-Pool Kubernetes Scheduling**: Added comprehensive scheduling configuration to Pool struct
  - `nodeSelector` - Target specific nodes by labels
  - `affinity` - Complex node/pod affinity rules
  - `tolerations` - Schedule on tainted nodes (e.g., spot instances)
  - `topologySpreadConstraints` - Distribute pods across failure domains
  - `resources` - CPU/memory requests and limits per pool
  - `priorityClassName` - Override tenant-level priority per pool

- **SchedulingConfig Struct**: Grouped scheduling fields for better code organization
  - Uses `#[serde(flatten)]` to maintain flat YAML structure
  - Follows industry-standard pattern (MongoDB, PostgreSQL operators)
  - 100% backward compatible

- **New Examples**:
  - `cluster-expansion-tenant.yaml` - Demonstrates capacity expansion and pool migration
  - `hardware-pools-tenant.yaml` - Shows heterogeneous disk sizes (same storage class)
  - `geographic-pools-tenant.yaml` - Multi-region deployment for compliance and DR
  - `spot-instance-tenant.yaml` - Cost optimization using spot instances

- **Documentation**:
  - `docs/multi-pool-use-cases.md` - Comprehensive multi-pool use case guide
  - `docs/architecture-decisions.md` - Critical architecture understanding
  - Updated `examples/README.md` with architecture warnings

- **Tests**: Added 5 new tests for scheduling field propagation (20 → 25 tests)

#### Required Environment Variables (2025-11-05)

- Operator now automatically sets required RustFS environment variables:
  - `RUSTFS_VOLUMES` - Multi-node volume configuration (already existed)
  - `RUSTFS_ADDRESS` - Server binding address (0.0.0.0:9000)
  - `RUSTFS_CONSOLE_ADDRESS` - Console binding address (0.0.0.0:9001)
  - `RUSTFS_CONSOLE_ENABLE` - Enable console UI (true)

### Fixed

#### Critical Port Corrections (2025-11-05)

- **Console Port**: Changed from 9090 to 9001 (correct RustFS default)
  - Fixed in `services.rs` and `workloads.rs`
  - Verified against RustFS source code constants

- **IO Service Port**: Changed from 90 to 9000 (S3 API standard)
  - Fixed in `services.rs`
  - Now matches S3-compatible service expectations

#### Volume Path Standardization (2025-11-05)

- **Volume Mount Paths**: Changed from `/data/{N}` to `/data/rustfs{N}`
  - Matches RustFS official Helm chart convention
  - Aligns with RustFS docker-compose examples
  - Verified against RustFS MNMD deployment guide

- **RUSTFS_VOLUMES Format**: Updated path from `/data/{0...N}` to `/data/rustfs{0...N}`
  - Consistent with RustFS ecosystem standards
  - Uses 3-dot ellipsis notation for RustFS expansion

#### Architecture Corrections (2025-11-08)

- **Storage Class Mixing**: Corrected examples that incorrectly mixed storage classes
  - Updated `hardware-pools-tenant.yaml` to use same storage class with different sizes
  - Fixed `spot-instance-tenant.yaml` to use uniform storage class
  - Added warnings to `geographic-pools-tenant.yaml` about unified cluster behavior

- **Architectural Clarifications**:
  - All pools form ONE unified RustFS erasure-coded cluster
  - Data is striped uniformly across ALL volumes regardless of storage class
  - Mixing NVMe/SSD/HDD results in HDD-level performance for entire cluster
  - RustFS has no intelligent storage class-based data placement

#### Examples Bug Fixes (2025-11-05)

- Fixed `multi-pool-tenant.yaml` syntax error (missing `persistence:` nesting)
- Moved examples from `deploy/rustfs-operator/examples/` to `examples/` at project root
- Created comprehensive `examples/README.md` with usage guide

### Changed

#### Example Improvements (2025-11-05 to 2025-11-08)

- **simple-tenant.yaml**: Added documentation for all scheduling fields
- **production-ha-tenant.yaml**: Added topology spread constraints and resource requirements
- **minimal-dev-tenant.yaml**: Corrected port references and added verification commands
- **custom-rbac-tenant.yaml**: Clarified RBAC patterns

### Removed

- **tiered-storage-tenant.yaml** (2025-11-05): Removed example with fabricated RustFS features
  - Contained non-existent environment variables
  - Made false claims about automatic storage tiering
  - Replaced with architecturally sound examples

### Documentation

#### Architecture Understanding (2025-11-08)

Key architectural facts now documented:

1. **Unified Cluster Architecture**: All pools in a Tenant form ONE erasure-coded cluster
2. **Uniform Data Distribution**: Erasure coding stripes data across ALL volumes equally
3. **No Storage Class Awareness**: RustFS does not prefer fast disks over slow disks
4. **Performance Limitation**: Cluster performs at speed of SLOWEST storage class
5. **External Tiering**: RustFS tiering uses lifecycle policies to external cloud storage (S3, Azure, GCS)

#### Valid Multi-Pool Use Cases

Documented valid uses:
- ✅ Cluster capacity expansion and hardware migration
- ✅ Geographic distribution for compliance and disaster recovery
- ✅ Spot vs on-demand instance optimization (compute cost savings)
- ✅ Same storage class with different disk sizes
- ✅ Resource differentiation (CPU/memory) per pool
- ✅ Topology-aware distribution across failure domains

Invalid uses clarified:
- ❌ Storage class mixing for performance tiering (NVMe for hot, HDD for cold)
- ❌ Automatic intelligent data placement based on access patterns

---

## [0.1.0] - 2025-11-05

### Initial State

- Basic Tenant CRD with pool support
- RBAC resource creation (Role, ServiceAccount, RoleBinding)
- Service creation (IO, Console, Headless)
- StatefulSet creation per pool
- Volume claim template generation
- RUSTFS_VOLUMES automatic configuration

### Known Issues in 0.1.0 (Before Fixes)

- Incorrect console port (9090 instead of 9001)
- Incorrect IO service port (90 instead of 9000)
- Missing required RustFS environment variables
- Non-standard volume mount paths
- Limited multi-pool scheduling capabilities
- Misleading examples with fabricated features

---

## Verification

All changes verified against:
- RustFS source code (`~/git/rustfs`)
- RustFS Helm chart (`helm/rustfs/`)
- RustFS docker-compose examples
- RustFS MNMD deployment guide
- RustFS configuration constants

## Testing

- **Test Count**: 25 tests
- **Status**: All passing ✅
- **Build**: Successful ✅
- **Backward Compatibility**: 100% maintained ✅

---

**Branch**: `feature/pool-scheduling-enhancements`
**Status**: Ready for merge
