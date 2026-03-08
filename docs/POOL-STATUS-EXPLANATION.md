# Pool Status Structure Explanation

This document explains what the `Pool` structure in `src/types/v1alpha1/status/pool.rs` represents.

---

## 📋 Overview

The `Pool` struct in `src/types/v1alpha1/status/pool.rs` represents the **runtime status** of a storage pool in a RustFS Tenant. It is part of the Tenant's status field and tracks the actual state of the StatefulSet that manages the pool's Pods.

---

## 🔍 Key Concepts

### Two Different `Pool` Types

There are **two different** `Pool` structures in the codebase:

1. **`src/types/v1alpha1/pool.rs::Pool`** - **Spec (Desired State)**
   - User-defined configuration
   - Part of `TenantSpec`
   - Defines what the user wants (e.g., `servers: 4`, `volumesPerServer: 2`)

2. **`src/types/v1alpha1/status/pool.rs::Pool`** - **Status (Actual State)**
   - Runtime status information
   - Part of `TenantStatus`
   - Tracks what actually exists (e.g., `replicas: 4`, `ready_replicas: 3`)

### Relationship

```
Tenant CRD
├── spec.pools[]          ← User configuration (pool.rs::Pool)
│   └── name: "pool-0"
│       servers: 4
│       volumesPerServer: 2
│
└── status.pools[]        ← Runtime status (status/pool.rs::Pool)
    └── ss_name: "tenant-pool-0"
        state: "RolloutComplete"
        replicas: 4
        ready_replicas: 4
```

---

## 📊 Pool Status Structure

### Fields Explained

```rust
pub struct Pool {
    /// Name of the StatefulSet for this pool
    pub ss_name: String,
    
    /// Current state of the pool
    pub state: PoolState,
    
    /// Total number of non-terminated pods targeted by this pool's StatefulSet
    pub replicas: Option<i32>,
    
    /// Number of pods with Ready condition
    pub ready_replicas: Option<i32>,
    
    /// Number of pods with current revision
    pub current_replicas: Option<i32>,
    
    /// Number of pods with updated revision
    pub updated_replicas: Option<i32>,
    
    /// Current revision hash of the StatefulSet
    pub current_revision: Option<String>,
    
    /// Update revision hash of the StatefulSet (different from current during rollout)
    pub update_revision: Option<String>,
    
    /// Last time the pool status was updated
    pub last_update_time: Option<String>,
}
```

### Field Details

#### `ss_name: String`
- **Meaning**: The name of the StatefulSet that manages this pool
- **Format**: `{tenant-name}-{pool-name}`
- **Example**: `dev-minimal-dev-pool`
- **Purpose**: Used to identify and query the StatefulSet resource

#### `state: PoolState`
- **Meaning**: Current operational state of the pool
- **Possible Values**: See `PoolState` enum below
- **Purpose**: Quick status indicator for monitoring and debugging

#### `replicas: Option<i32>`
- **Meaning**: Total number of Pods that should exist (desired replicas)
- **Source**: `StatefulSet.status.replicas`
- **Example**: `4` means 4 Pods should exist
- **Purpose**: Track desired vs actual Pod count

#### `ready_replicas: Option<i32>`
- **Meaning**: Number of Pods that are Ready (passing readiness probe)
- **Source**: `StatefulSet.status.readyReplicas`
- **Example**: `3` means 3 out of 4 Pods are ready
- **Purpose**: Determine if pool is fully operational

#### `current_replicas: Option<i32>`
- **Meaning**: Number of Pods running the current (old) revision
- **Source**: `StatefulSet.status.currentReplicas`
- **Example**: During update, `2` means 2 Pods still on old version
- **Purpose**: Track rollout progress

#### `updated_replicas: Option<i32>`
- **Meaning**: Number of Pods running the updated (new) revision
- **Source**: `StatefulSet.status.updatedReplicas`
- **Example**: During update, `2` means 2 Pods on new version
- **Purpose**: Track rollout progress

#### `current_revision: Option<String>`
- **Meaning**: Revision hash of the current StatefulSet template
- **Source**: `StatefulSet.status.currentRevision`
- **Example**: `"tenant-pool-0-abc123"`
- **Purpose**: Identify which template version Pods are running

#### `update_revision: Option<String>`
- **Meaning**: Revision hash of the updated StatefulSet template (during rollout)
- **Source**: `StatefulSet.status.updateRevision`
- **Example**: `"tenant-pool-0-def456"`
- **Purpose**: Identify which template version is being rolled out

#### `last_update_time: Option<String>`
- **Meaning**: Timestamp when this status was last updated
- **Format**: RFC3339 timestamp
- **Example**: `"2025-01-15T10:30:00Z"`
- **Purpose**: Track when status was last refreshed

---

## 🎯 PoolState Enum

The `PoolState` enum represents the operational state of a pool:

```rust
pub enum PoolState {
    Created,           // PoolCreated - StatefulSet exists
    NotCreated,       // PoolNotCreated - StatefulSet doesn't exist or has 0 replicas
    Initialized,      // PoolInitialized - Pool is initialized but not all replicas ready
    Updating,         // PoolUpdating - Rollout in progress
    RolloutComplete,  // PoolRolloutComplete - All replicas ready and updated
    RolloutFailed,    // PoolRolloutFailed - Rollout failed
    Degraded,         // PoolDegraded - Some replicas not ready
}
```

### State Determination Logic

The state is determined based on StatefulSet status:

```rust
if desired == 0 {
    PoolState::NotCreated
} else if ready == desired && updated == desired {
    PoolState::RolloutComplete  // All good!
} else if updated < desired || current < desired {
    PoolState::Updating  // Rollout in progress
} else if ready < desired {
    PoolState::Degraded  // Some Pods not ready
} else {
    PoolState::Initialized  // Initialized but not fully ready
}
```

---

## 🔄 How It's Used

### 1. Status Collection

During reconciliation, the operator:

1. **Queries StatefulSets** for each pool in `spec.pools`
2. **Extracts status** from each StatefulSet
3. **Builds Pool status** using `build_pool_status()` method
4. **Aggregates** all pool statuses into `TenantStatus.pools[]`

### 2. Status Update Flow

```
Reconciliation Loop
    ↓
For each pool in spec.pools:
    ↓
Get StatefulSet: {tenant-name}-{pool-name}
    ↓
Extract StatefulSet.status
    ↓
Build Pool status object
    ↓
Add to TenantStatus.pools[]
    ↓
Update Tenant.status
```

### 3. Example Status Output

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: dev-minimal
status:
  currentState: "Ready"
  availableReplicas: 4
  pools:
  - ssName: "dev-minimal-dev-pool"
    state: "PoolRolloutComplete"
    replicas: 4
    readyReplicas: 4
    currentReplicas: 4
    updatedReplicas: 4
    currentRevision: "dev-minimal-dev-pool-abc123"
    updateRevision: "dev-minimal-dev-pool-abc123"
    lastUpdateTime: "2025-01-15T10:30:00Z"
```

---

## 💡 Use Cases

### 1. Monitoring Pool Health

```bash
# Check pool status
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[*].state}'

# Check ready replicas
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[*].readyReplicas}'
```

### 2. Detecting Rollout Progress

```bash
# Check if pool is updating
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[?(@.state=="PoolUpdating")]}'

# Compare current vs updated replicas
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[*].currentReplicas}'
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[*].updatedReplicas}'
```

### 3. Debugging Issues

```bash
# Check if pool is degraded
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[?(@.state=="PoolDegraded")]}'

# View full pool status
kubectl get tenant dev-minimal -o jsonpath='{.status.pools[*]}' | jq
```

---

## 🔗 Related Code

- **Status Collection**: `src/types/v1alpha1/tenant.rs::build_pool_status()`
- **Status Aggregation**: `src/reconcile.rs` (reconciliation loop)
- **Status Definition**: `src/types/v1alpha1/status.rs::Status`
- **Pool Spec**: `src/types/v1alpha1/pool.rs::Pool`

---

## Summary

**`status/pool.rs::Pool`** represents:
- ✅ **Runtime status** of a storage pool
- ✅ **StatefulSet status** information
- ✅ **Pod replica counts** and readiness
- ✅ **Rollout progress** during updates
- ✅ **Operational state** (Ready, Updating, Degraded, etc.)

**Key Distinction**:
- `spec.pools[]` = What you want (configuration)
- `status.pools[]` = What actually exists (runtime status)

This separation allows the operator to track the difference between desired and actual state, enabling proper reconciliation and status reporting.
