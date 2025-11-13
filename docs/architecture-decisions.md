# Architecture Decisions

This document records key architectural decisions made in the design of the RustFS Kubernetes Operator.

---

## ADR-001: StatefulSet Per Pool

**Status**: Accepted

**Context**:
Each Tenant can have multiple Pools with different configurations. We needed to decide how to represent pools in Kubernetes.

**Options Considered**:

1. **Single StatefulSet with all pools**: One StatefulSet with complex pod indexing
2. **StatefulSet per pool**: Separate StatefulSet for each pool
3. **Deployment per pool**: Use Deployments instead of StatefulSets

**Decision**: Create one StatefulSet per pool.

**Rationale**:

- **Independent Scaling**: Each pool can be scaled independently
- **Different Configurations**: Each pool can have different resources, node selectors, etc.
- **Clear Ownership**: Pool-specific labels and selectors
- **StatefulSet Benefits**: Stable network identity, ordered/parallel deployment
- **Kubernetes Native**: Standard pattern for distributed stateful applications

**Implementation**:
- StatefulSet name: `{tenant-name}-{pool-name}`
- Pod naming: `{tenant-name}-{pool-name}-{index}`
- Shared headless service for DNS across all pools

**Tradeoffs**:
- More Kubernetes resources (one StatefulSet per pool)
- Slightly more complex reconciliation loop
- Better flexibility and independence per pool

---

## ADR-002: Unified RUSTFS_VOLUMES for All Pools

**Status**: Accepted

**Context**:
RustFS requires a RUSTFS_VOLUMES environment variable. With multiple pools, we needed to decide how to configure this.

**Options Considered**:

1. **Separate RUSTFS_VOLUMES per pool**: Each pool runs independent RustFS cluster
2. **Combined RUSTFS_VOLUMES**: All pools in one unified cluster
3. **Configurable**: Let users choose

**Decision**: Combine all pools into single RUSTFS_VOLUMES, forming one unified cluster.

**Rationale**:

- **RustFS Design**: RustFS is designed for single-cluster architecture
- **Erasure Coding**: Maximum redundancy across all volumes
- **Resource Efficiency**: Single cluster is more efficient than multiple
- **Simpler Operation**: One S3 endpoint, not multiple
- **Follows RustFS Patterns**: Official Helm chart uses same approach

**Implementation**:
```rust
fn rustfs_volumes_env_value(&self) -> Result<String> {
    let volume_specs: Vec<String> = self.spec.pools.iter()
        .map(|pool| { /* generate pool spec */ })
        .collect();
    Ok(volume_specs.join(" "))  // Space-separated
}
```

**Tradeoffs**:
- Cannot have pool-independent clusters within one Tenant
- All pools share performance characteristics
- Simpler user model (one cluster, not N clusters)

**Consequences**:
- Storage class mixing across pools degrades performance to slowest tier
- Multi-pool is for scheduling/placement, not storage isolation
- For separate clusters, users should create separate Tenants

---

## ADR-003: SchedulingConfig Struct with Flattened Serialization

**Status**: Accepted

**Context**:
Pools need Kubernetes scheduling fields (nodeSelector, affinity, tolerations, etc.). We needed to decide how to structure these in the Pool CRD.

**Options Considered**:

1. **Individual fields in Pool**: Each scheduling field as separate Pool field
2. **PodTemplateSpec**: Full Kubernetes PodTemplateSpec override
3. **SchedulingConfig with flatten**: Grouped struct, flat YAML

**Decision**: Use `SchedulingConfig` struct with `#[serde(flatten)]`.

**Rationale**:

**Why Not PodTemplateSpec**:
- No industry precedent (no operators use this pattern)
- Complex merging logic (how to merge containers, volumes, etc.)
- Users could set fields that break operator assumptions
- Hard to validate with CEL

**Why Not Individual Fields**:
- Code organization suffers
- Harder to maintain
- No clear grouping of related fields

**Why SchedulingConfig**:
- ✅ Industry standard (MongoDB, PostgreSQL operators use similar pattern)
- ✅ Better code organization
- ✅ Flat YAML structure (backward compatible)
- ✅ Type-safe with clear scope
- ✅ Can add methods/validation to SchedulingConfig
- ✅ Reusable if needed elsewhere

**Implementation**:
```rust
pub struct SchedulingConfig {
    pub node_selector: Option<BTreeMap<String, String>>,
    pub affinity: Option<corev1::Affinity>,
    // ... other fields
}

pub struct Pool {
    pub name: String,
    pub servers: i32,
    pub persistence: PersistenceConfig,

    #[serde(flatten)]  // Key: maintains flat YAML
    pub scheduling: SchedulingConfig,
}
```

**YAML Structure** (unchanged from individual fields):
```yaml
pools:
  - name: my-pool
    servers: 4
    nodeSelector: {...}    # Still flat
    affinity: {...}        # Still flat
    resources: {...}       # Still flat
```

**Tradeoffs**:
- Code access is `pool.scheduling.field` vs `pool.field` (one extra level)
- Better organization worth the extra level

---

## ADR-004: Server-Side Apply for Resource Management

**Status**: Accepted

**Context**:
Resources created by the operator need to be managed declaratively and idempotently.

**Options Considered**:

1. **Create/Update pattern**: Check if exists, create or update
2. **Server-side apply**: Kubernetes server-side apply
3. **Client-side apply**: kubectl-style apply

**Decision**: Use server-side apply.

**Rationale**:

- **Idempotent**: Safe to call repeatedly
- **Field Ownership**: Operator owns specific fields, other managers can own others
- **Conflict Resolution**: Kubernetes handles conflicts
- **Declarative**: Matches Kubernetes philosophy
- **Official Pattern**: Recommended by Kubernetes sig-api-machinery

**Implementation**:
```rust
pub async fn apply<T>(&self, resource: &T, namespace: &str) -> Result<T> {
    let api: Api<T> = Api::namespaced(self.client.clone(), namespace);
    api.patch(
        &resource.name_any(),
        &PatchParams::apply("rustfs-operator"),  // Field manager
        &Patch::Apply(resource),
    ).await
}
```

**Field Manager**: `"rustfs-operator"`

**Tradeoffs**:
- Requires understanding of field ownership
- More sophisticated than simple create/update
- Correct pattern for operators

---

## ADR-005: Owner References for Garbage Collection

**Status**: Accepted

**Context**:
When a Tenant is deleted, all created resources (StatefulSets, Services, RBAC) should be deleted automatically.

**Options Considered**:

1. **Manual Cleanup**: Finalizers with manual deletion logic
2. **Owner References**: Kubernetes automatic garbage collection
3. **Hybrid**: Owner references + finalizers for external resources

**Decision**: Use owner references for automatic garbage collection.

**Rationale**:

- **Kubernetes Native**: Built-in garbage collection
- **Automatic**: No manual cleanup code needed
- **Reliable**: Kubernetes guarantees cleanup
- **Standard Pattern**: Used by most operators
- **No External Resources**: We only create Kubernetes resources (no external systems to clean)

**Implementation**:
```rust
pub fn new_owner_ref(&self) -> metav1::OwnerReference {
    metav1::OwnerReference {
        api_version: Self::api_version(&()).to_string(),
        kind: Self::kind(&()).to_string(),
        name: self.name(),
        uid: self.meta().uid.clone().unwrap_or_default(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}
```

All created resources include `owner_references: Some(vec![self.new_owner_ref()])`.

**Tradeoffs**:
- No control over deletion order (Kubernetes decides)
- Fine for our use case (no external dependencies)

**Future**: If we add external resources (cloud storage, DNS), add finalizers.

---

## ADR-006: Pool-Level Priority Class Override

**Status**: Accepted

**Context**:
Both Tenant and Pool can specify priority class. We needed to decide precedence.

**Options Considered**:

1. **Tenant-only**: Pool cannot override
2. **Pool-only**: Ignore tenant-level
3. **Pool overrides tenant**: Pool takes precedence if set

**Decision**: Pool-level priority class overrides tenant-level.

**Rationale**:

- **Flexibility**: Different pools can have different priorities
- **Use Case**: Critical pool on high priority, elastic pool on standard
- **Fallback**: Use tenant-level if pool-level not set
- **Principle**: More specific wins (pool more specific than tenant)

**Implementation**:
```rust
priority_class_name: pool.scheduling.priority_class_name.clone()
    .or_else(|| self.spec.priority_class_name.clone()),
```

**Example**:
```yaml
spec:
  priorityClassName: standard  # Tenant default

  pools:
    - name: critical
      priorityClassName: high  # Override
    - name: normal
      # Uses tenant default (standard)
```

---

## ADR-007: Shared Services Across All Pools

**Status**: Accepted

**Context**:
Should each pool have its own services or share services?

**Options Considered**:

1. **Shared Services**: One set of services for all pools
2. **Per-Pool Services**: Separate services per pool
3. **Hybrid**: Shared API, separate console per pool

**Decision**: Shared services across all pools.

**Rationale**:

- **Unified Cluster**: All pools form one RustFS cluster
- **Single S3 Endpoint**: Users access one S3 API, not multiple
- **Simpler**: Fewer resources, easier management
- **RustFS Design**: RustFS expects to be accessed as single cluster

**Implementation**:
- One IO service (port 9000) for all pools
- One Console service (port 9001) for all pools
- One headless service for StatefulSet DNS

**Service Selectors**: `rustfs.tenant={name}` (matches all pools)

**Tradeoffs**:
- Cannot have pool-specific endpoints
- All pools accessed via same service
- Simpler for users (one endpoint to remember)

---

## ADR-008: Automatic Environment Variable Management

**Status**: Accepted

**Context**:
RustFS requires specific environment variables. Should users set them or operator?

**Decision**: Operator automatically sets required RustFS environment variables.

**Rationale**:

- **User Experience**: Users don't need to know RustFS internals
- **Correctness**: Operator ensures correct configuration
- **Consistency**: Same environment across all deployments
- **Override**: Users can still override if needed (their vars applied after)

**Automatically Set**:
- `RUSTFS_VOLUMES` - Generated from pools
- `RUSTFS_ADDRESS` - 0.0.0.0:9000
- `RUSTFS_CONSOLE_ADDRESS` - 0.0.0.0:9001
- `RUSTFS_CONSOLE_ENABLE` - true

**Implementation**:
```rust
let mut env_vars = Vec::new();
env_vars.push(/* RUSTFS_VOLUMES */);
env_vars.push(/* RUSTFS_ADDRESS */);
env_vars.push(/* RUSTFS_CONSOLE_ADDRESS */);
env_vars.push(/* RUSTFS_CONSOLE_ENABLE */);

// User vars can override
for user_env in &self.spec.env {
    env_vars.retain(|e| e.name != user_env.name);
    env_vars.push(user_env.clone());
}
```

**Tradeoffs**:
- Less user control over these specific vars
- Better user experience (works out of box)
- Advanced users can still override

---

## ADR-009: Label Strategy

**Status**: Accepted

**Context**:
Resources need labels for selection, grouping, and management.

**Decision**: Use minimal selectors, comprehensive labels.

**Rationale**:

**Selectors** (stable, minimal):
```yaml
# Tenant selector
rustfs.tenant: {tenant-name}

# Pool selector
rustfs.tenant: {tenant-name}
rustfs.pool: {pool-name}
```

**Labels** (comprehensive, can change):
```yaml
app.kubernetes.io/name: rustfs
app.kubernetes.io/instance: {tenant-name}
app.kubernetes.io/managed-by: rustfs-operator
app.kubernetes.io/component: storage  # Pool resources
rustfs.tenant: {tenant-name}
rustfs.pool: {pool-name}  # Pool resources
```

**Why Minimal Selectors**:
- Selectors are immutable in StatefulSet
- Cannot be changed without recreating resource
- Minimal selectors provide stability

**Why Comprehensive Labels**:
- Labels can be added/changed
- Useful for grouping, monitoring, policies
- Follow Kubernetes recommended labels

---

## ADR-010: RBAC Conditional Creation

**Status**: Accepted

**Context**:
Users may want to use custom ServiceAccounts. How should RBAC be handled?

**Decision**: Conditional RBAC creation based on configuration.

**Logic**:
```rust
let custom_sa = spec.service_account_name.is_some();
let create_rbac = spec.create_service_account_rbac.unwrap_or(false);

if !custom_sa || create_rbac {
    // Create Role
    if !custom_sa {
        // Create ServiceAccount + RoleBinding
    } else {
        // Create RoleBinding only (bind custom SA)
    }
}
```

**Scenarios**:
1. No custom SA → Create SA, Role, RoleBinding
2. Custom SA + `createServiceAccountRbac=true` → Create Role, RoleBinding
3. Custom SA + `createServiceAccountRbac=false` → Skip all RBAC

**Rationale**:
- **Flexibility**: Support both managed and custom SA
- **Cloud Integration**: Allow workload identity (AWS IAM, GCP, Azure)
- **Security**: Users can provide their own RBAC with additional permissions
- **Default Simplicity**: Works out of box without custom SA

---

## Future Architectural Decisions

### Under Consideration

1. **Finalizers for External Resources**: If we add external integrations
2. **Status Subresource Population**: When to update status, conflict handling
3. **Per-Pool Status Tracking**: Whether to track pool health separately
4. **Dynamic Pool Addition**: API for adding pools without recreation

---

## Related Documents

- [Multi-Pool Use Cases](./multi-pool-use-cases.md) - Valid multi-pool patterns
- [DEVELOPMENT-NOTES.md](./DEVELOPMENT-NOTES.md) - Implementation details and discoveries

---

**Format**: Loosely based on [Architecture Decision Records (ADR)](https://adr.github.io/)
**Last Updated**: 2025-11-08
