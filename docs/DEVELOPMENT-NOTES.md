# Development Notes

## Analysis Sessions

### Initial Bug Analysis (2025-11-05)

See [CHANGELOG.md](../CHANGELOG.md) for complete list of bugs found and fixed.

**Key Discovery**: Through comprehensive analysis of RustFS source code, found 5 critical bugs:
- Wrong ports (console: 9090, IO: 90)
- Missing environment variables
- Non-standard volume paths

**Methodology**: Analyzed RustFS repository at `~/git/rustfs` to verify correct implementation.

### Multi-Pool Enhancements (2025-11-08)

Added comprehensive Kubernetes scheduling capabilities to Pool struct.

**Design Decision**: Use `SchedulingConfig` struct with `#[serde(flatten)]`
- Better code organization
- Maintains flat YAML structure
- Follows industry patterns (MongoDB, PostgreSQL operators)

See [architecture-decisions.md](./architecture-decisions.md) for detailed rationale.

### RustFS Architecture Deep Dive (2025-11-08)

**Critical Finding**: All pools form ONE unified RustFS cluster, not independent storage tiers.

#### How RustFS Actually Works

From RustFS source code analysis (`~/git/rustfs`):

**1. Unified Cluster Architecture** (`crates/ecstore/src/pools.rs`):
- All pools combined into ONE `RUSTFS_VOLUMES` environment variable
- Single distributed hash ring across all volumes
- No pool independence

**2. Uniform Erasure Coding** (`crates/ecstore/src/erasure.rs`):
- Reed-Solomon erasure coding across ALL volumes
- Shards distributed uniformly (no preference for fast disks)
- Parity calculated for total drive count across all pools

**3. No Storage Class Awareness** (`crates/ecstore/src/config/storageclass.rs`):
- Storage class controls PARITY levels (EC:4, EC:2), NOT disk selection
- Does NOT control data placement or prefer certain disks
- No hot/warm/cold data awareness

**4. External Tiering Only** (`crates/ecstore/src/tier/tier.rs`):
- Tiering = transitioning to EXTERNAL cloud storage
- Types: `TierType::S3`, `TierType::Azure`, `TierType::GCS`
- NOT for internal disk class differentiation

#### Performance Implications of Storage Class Mixing

**Problem**: Mixing NVMe/SSD/HDD in one Tenant

**What Actually Happens**:
- Object is erasure-coded into shards
- Shards distributed across ALL volumes (NVMe + SSD + HDD)
- Write completes when ALL shards written (limited by slowest = HDD)
- Read requires fetching shards (limited by slowest = HDD)
- **Result**: Entire cluster performs at HDD speed, NVMe wasted

**Conclusion**: Do NOT mix storage classes for "performance tiers" - it doesn't work.

#### Valid Multi-Pool Purposes

✅ **What Works**:
- Cluster expansion (add pools for capacity)
- Geographic distribution (compliance/DR, not performance)
- Spot vs on-demand (compute cost, same storage class)
- Same class, different sizes (utilize mixed hardware)
- Resource differentiation (CPU/memory per pool)

❌ **What Doesn't Work**:
- NVMe for hot data, HDD for cold data
- Storage performance tiering via multi-pool
- Automatic intelligent data placement

**For Real Tiering**: Use RustFS lifecycle policies to external cloud storage (S3 Glacier, Azure Cool, GCS Nearline).

## Design Principles

### 1. Verify Against RustFS Source

All implementation decisions verified against official RustFS source code, not assumptions.

**Sources**:
- RustFS constants: `crates/config/src/constants/app.rs`
- RustFS config: `rustfs/src/config/mod.rs`
- RustFS Helm chart: `helm/rustfs/`

### 2. Follow Kubernetes Conventions

- Use recommended labels (`app.kubernetes.io/name`, etc.)
- Server-side apply for idempotency
- Owner references for garbage collection
- Industry-standard CRD patterns

### 3. Backward Compatibility

- All new fields are `Option<T>`
- Use `#[serde(flatten)]` to avoid breaking YAML structure
- Maintain existing behavior by default

### 4. User Experience First

- Clear, accurate examples
- Prominent warnings about gotchas
- Comprehensive documentation
- Prevent costly mistakes (storage class mixing)

## Testing Strategy

### Unit Tests

- Test resource structure creation
- Test field propagation (scheduling, RBAC, etc.)
- Test edge cases (None values, overrides)
- Currently: 25 tests, all passing

### Integration Tests (Future)

- Deploy actual Tenant
- Verify RustFS cluster formation
- Test multi-pool behavior
- Validate RUSTFS_VOLUMES expansion

## Code Organization

### Module Structure

```
src/
├── types/
│   └── v1alpha1/
│       ├── pool.rs (SchedulingConfig + Pool)
│       ├── persistence.rs
│       ├── tenant.rs
│       └── tenant/
│           ├── rbac.rs (RBAC factory methods)
│           ├── services.rs (Service factory methods)
│           └── workloads.rs (StatefulSet factory methods)
├── reconcile.rs (reconciliation logic)
└── context.rs (Kubernetes API wrapper)
```

### Pattern: Factory Methods

Each resource type has a factory method on Tenant:
- `new_role()`, `new_service_account()`, `new_role_binding()`
- `new_io_service()`, `new_console_service()`, `new_headless_service()`
- `new_statefulset(pool)`

This keeps logic organized and testable.

## Common Pitfalls to Avoid

### 1. Storage Class Mixing

❌ **Don't**: Create pools with different storage classes for "performance tiering"
```yaml
pools:
  - name: fast
    storageClassName: nvme  # ← Don't mix
  - name: slow
    storageClassName: hdd   # ← Performance tiers
```

✅ **Do**: Use same storage class, different sizes
```yaml
pools:
  - name: large
    storageClassName: ssd  # ← Same class
    storage: 10Ti
  - name: small
    storageClassName: ssd  # ← Same class
    storage: 2Ti
```

### 2. Assuming Pool Independence

❌ **Don't**: Think pools are independent clusters

✅ **Do**: Understand all pools form ONE unified cluster via RUSTFS_VOLUMES

### 3. Missing Required Fields

Always set in operator (users don't need to):
- RUSTFS_VOLUMES (generated)
- RUSTFS_ADDRESS (auto-set)
- RUSTFS_CONSOLE_ADDRESS (auto-set)
- RUSTFS_CONSOLE_ENABLE (auto-set)

## Future Enhancements

### Planned

- Status field population
- Configuration secret mounting
- Image pull policy application
- Health probes
- Per-pool status tracking

### Under Consideration

- Dynamic pool addition API
- Pool decommissioning automation
- Pool-specific service endpoints
- Advanced topology awareness

## References

- [Multi-Pool Use Cases](./multi-pool-use-cases.md)
- [Architecture Decisions](./architecture-decisions.md)
- [CHANGELOG](../CHANGELOG.md)

---

**Last Updated**: 2025-11-08

[[Index|← Back to Index]]
