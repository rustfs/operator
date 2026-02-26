# RustFS Encapsulation in Kubernetes

This document explains in detail how the RustFS Kubernetes Operator encapsulates RustFS into Kubernetes and how it handles RustFS's dependency on system paths.

---

## 📋 Project Overview

### What Does This Project Do?

**RustFS Kubernetes Operator** is a Kubernetes Operator that:

1. **Automates RustFS Deployment**: Automatically creates and manages RustFS storage clusters through declarative configuration (CRD)
2. **Encapsulates Complexity**: Hides the complexity of Kubernetes resource creation (StatefulSet, Service, PVC, RBAC, etc.)
3. **Lifecycle Management**: Automatically handles creation, updates, scaling, and deletion of RustFS clusters
4. **Configuration Management**: Automatically generates environment variables and configurations required by RustFS

### Core Value

**Without Operator**, deploying RustFS requires manually creating:
- StatefulSet (managing Pods)
- PersistentVolumeClaim (storage volumes)
- Service (service discovery)
- RBAC (permissions)
- ConfigMap/Secret (configuration)
- Manually configuring `RUSTFS_VOLUMES` environment variable

**With Operator**, you only need:
```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: my-rustfs
spec:
  pools:
    - name: primary
      servers: 2
      persistence:
        volumesPerServer: 2
```

The Operator automatically creates all necessary resources!

---

## 🔍 RustFS Path Dependency Problem

### How Does RustFS Work?

RustFS is a distributed object storage system that requires:

1. **Local Storage Paths**: Each node needs to access local disk paths to store data
   - Example: `/data/rustfs0`, `/data/rustfs1`, `/data/rustfs2`, `/data/rustfs3`
   - These paths must exist and be writable

2. **Network Communication**: Nodes need to communicate over the network to coordinate data distribution
   - RustFS uses the `RUSTFS_VOLUMES` environment variable to discover other nodes
   - Format: `http://node1:9000/data/rustfs{0...N} http://node2:9000/data/rustfs{0...N} ...`

3. **Path Convention**: RustFS follows a specific path naming convention
   - Base path + `/rustfs{index}`
   - Example: `/data/rustfs0`, `/data/rustfs1`

### Problems with Traditional Deployment

Deploying RustFS on traditional servers:

```bash
# 1. Create directories
mkdir -p /data/rustfs{0..3}

# 2. Set permissions
chown -R rustfs:rustfs /data

# 3. Configure environment variables
export RUSTFS_VOLUMES="http://node1:9000/data/rustfs{0...3} http://node2:9000/data/rustfs{0...3}"

# 4. Start RustFS
rustfs server
```

**Problems**:
- ❌ Paths are hardcoded and inflexible
- ❌ Requires manual management of multiple nodes
- ❌ Difficult to use in container environments (container filesystems are ephemeral)
- ❌ Cannot leverage Kubernetes storage abstractions

---

## ✅ Kubernetes Solution

### Core Idea: Use PersistentVolume + VolumeMount

Kubernetes solves the path dependency problem through the following mechanisms:

1. **PersistentVolumeClaim (PVC)**: Abstracts storage without caring about the underlying implementation
2. **VolumeMount**: Mounts PVCs to specified paths in containers
3. **StatefulSet**: Ensures stable network identity and storage for Pods

### Implementation Principles

#### 1. Create PersistentVolumeClaim Templates

The Operator creates PVCs for each volume:

```rust
// Code location: src/types/v1alpha1/tenant/workloads.rs

fn volume_claim_templates(&self, pool: &Pool) -> Result<Vec<PersistentVolumeClaim>> {
    // Create PVC template for each volume
    // Example: vol-0, vol-1, vol-2, vol-3
    let templates: Vec<_> = (0..pool.persistence.volumes_per_server)
        .map(|i| PersistentVolumeClaim {
            metadata: ObjectMeta {
                name: Some(format!("vol-{}", i)),  // vol-0, vol-1, ...
                ..
            },
            spec: Some(PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                resources: Some(VolumeResourceRequirements {
                    requests: Some(resources),
                    ..
                }),
                ..
            }),
            ..
        })
        .collect();
}
```

**Generated PVCs**:
```yaml
# StatefulSet automatically creates these PVCs for each Pod
# Pod 0: dev-minimal-dev-pool-0-vol-0, dev-minimal-dev-pool-0-vol-1, ...
# Pod 1: dev-minimal-dev-pool-1-vol-0, dev-minimal-dev-pool-1-vol-1, ...
```

#### 2. Mount PVCs to Container Paths

The Operator creates VolumeMounts to mount PVCs to paths expected by RustFS:

```rust
// Code location: src/types/v1alpha1/tenant/workloads.rs

let base_path = pool.persistence.path.as_deref().unwrap_or("/data");
let mut volume_mounts: Vec<VolumeMount> = (0..pool.persistence.volumes_per_server)
    .map(|i| VolumeMount {
        name: format!("vol-{}", i),  // Corresponds to PVC name
        mount_path: format!("{}/rustfs{}", base_path, i),  // /data/rustfs0, /data/rustfs1, ...
        ..
    })
    .collect();
```

**Result**:
- PVC `vol-0` → mounted to `/data/rustfs0`
- PVC `vol-1` → mounted to `/data/rustfs1`
- PVC `vol-2` → mounted to `/data/rustfs2`
- PVC `vol-3` → mounted to `/data/rustfs3`

#### 3. Automatically Generate RUSTFS_VOLUMES Environment Variable

The Operator automatically generates `RUSTFS_VOLUMES` to tell RustFS how to find other nodes:

```rust
// Code location: src/types/v1alpha1/tenant/workloads.rs

fn rustfs_volumes_env_value(&self) -> Result<String> {
    // Generated format:
    // http://{tenant}-{pool}-{0...servers-1}.{service}.{namespace}.svc.cluster.local:9000{path}/rustfs{0...volumes-1}
    
    format!(
        "http://{tenant}-{pool}-{{0...{}}}.{service}.{namespace}.svc.cluster.local:9000{}/rustfs{{0...{}}}",
        servers - 1,
        base_path,  // /data
        volumes_per_server - 1
    )
}
```

**Example Output** (2 servers, 2 volumes each):
```
http://dev-minimal-dev-pool-{0...1}.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs{0...1}
```

**Expanded**:
```
http://dev-minimal-dev-pool-0.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs0
http://dev-minimal-dev-pool-0.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs1
http://dev-minimal-dev-pool-1.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs0
http://dev-minimal-dev-pool-1.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs1
```

---

## 🏗️ Complete Architecture Diagram

```
User creates Tenant CRD
        ↓
Operator reconciliation loop
        ↓
┌─────────────────────────────────────────┐
│  1. Create RBAC Resources               │
│     - Role                              │
│     - ServiceAccount                    │
│     - RoleBinding                       │
└─────────────────────────────────────────┘
        ↓
┌─────────────────────────────────────────┐
│  2. Create Services                    │
│     - IO Service (port 9000)            │
│     - Console Service (port 9001)       │
│     - Headless Service (DNS)            │
└─────────────────────────────────────────┘
        ↓
┌─────────────────────────────────────────┐
│  3. Create StatefulSet for each Pool   │
│     ├─ Pod Template                     │
│     │  ├─ Container: rustfs/rustfs     │
│     │  ├─ VolumeMounts:                 │
│     │  │  ├─ vol-0 → /data/rustfs0     │
│     │  │  ├─ vol-1 → /data/rustfs1     │
│     │  │  ├─ vol-2 → /data/rustfs2     │
│     │  │  └─ vol-3 → /data/rustfs3     │
│     │  └─ Env:                         │
│     │     └─ RUSTFS_VOLUMES=...        │
│     └─ VolumeClaimTemplates:            │
│        ├─ vol-0 (10Gi)                 │
│        ├─ vol-1 (10Gi)                 │
│        ├─ vol-2 (10Gi)                │
│        └─ vol-3 (10Gi)                │
└─────────────────────────────────────────┘
        ↓
Kubernetes creates resources
        ↓
┌─────────────────────────────────────────┐
│  StatefulSet Controller creates Pods   │
│  ├─ Pod: dev-minimal-dev-pool-0        │
│  │  ├─ PVC: dev-minimal-dev-pool-0-vol-0
│  │  ├─ PVC: dev-minimal-dev-pool-0-vol-1
│  │  ├─ PVC: dev-minimal-dev-pool-0-vol-2
│  │  └─ PVC: dev-minimal-dev-pool-0-vol-3
│  └─ Pod: dev-minimal-dev-pool-1        │
│     ├─ PVC: dev-minimal-dev-pool-1-vol-0
│     ├─ PVC: dev-minimal-dev-pool-1-vol-1
│     ├─ PVC: dev-minimal-dev-pool-1-vol-2
│     └─ PVC: dev-minimal-dev-pool-1-vol-3
└─────────────────────────────────────────┘
        ↓
Storage Provider (StorageClass) creates PV
        ↓
Pod starts, RustFS accesses mounted paths
```

---

## 🔄 Data Persistence Flow

### Data Persists Across Pod Restarts

1. **StatefulSet Guarantees**:
   - Stable Pod names: `dev-minimal-dev-pool-0`
   - Stable PVC names: `dev-minimal-dev-pool-0-vol-0`
   - Even if Pod restarts, PVCs remain unchanged

2. **Storage Persistence**:
   ```
   Pod deleted → PVC retained → Pod recreated → PVC remounted → Data restored
   ```

3. **Path Consistency**:
   - PVCs are always mounted to the same paths (`/data/rustfs0`)
   - RustFS doesn't need to know what the underlying storage is (local disk, network storage, cloud storage)

---

## 💡 Key Design Decisions

### 1. Why Use StatefulSet?

- ✅ **Stable Network Identity**: Pods have stable DNS names for `RUSTFS_VOLUMES`
- ✅ **Ordered Deployment**: Can control Pod startup order
- ✅ **Stable Storage**: Each Pod has independent PVCs, data persists when Pod is recreated

### 2. Why Use VolumeClaimTemplates?

- ✅ **Automation**: No need to manually create PVCs
- ✅ **Dynamic Creation**: StatefulSet automatically creates PVCs for each Pod
- ✅ **Naming Convention**: PVC names are associated with Pod names

### 3. Why Are Paths `/data/rustfs{0...N}`?

- ✅ **RustFS Convention**: Follows RustFS path naming conventions
- ✅ **Configurable**: Users can customize the base path via `persistence.path`
- ✅ **Clear**: Path names clearly indicate the volume's purpose

---

## 📝 Practical Example

### User Configuration

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: my-rustfs
spec:
  pools:
    - name: primary
      servers: 2
      persistence:
        volumesPerServer: 2
        path: /data  # Optional, defaults to /data
```

### Resources Generated by Operator

#### StatefulSet

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: my-rustfs-primary
spec:
  replicas: 2
  serviceName: my-rustfs-hl
  template:
    spec:
      containers:
      - name: rustfs
        image: rustfs/rustfs:latest
        env:
        - name: RUSTFS_VOLUMES
          value: "http://my-rustfs-primary-{0...1}.my-rustfs-hl.default.svc.cluster.local:9000/data/rustfs{0...1}"
        volumeMounts:
        - name: vol-0
          mountPath: /data/rustfs0
        - name: vol-1
          mountPath: /data/rustfs1
  volumeClaimTemplates:
  - metadata:
      name: vol-0
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 10Gi
  - metadata:
      name: vol-1
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 10Gi
```

#### Actually Created Pods and PVCs

**Pod 0**:
- Pod name: `my-rustfs-primary-0`
- PVC: `my-rustfs-primary-0-vol-0` → mounted to `/data/rustfs0`
- PVC: `my-rustfs-primary-0-vol-1` → mounted to `/data/rustfs1`

**Pod 1**:
- Pod name: `my-rustfs-primary-1`
- PVC: `my-rustfs-primary-1-vol-0` → mounted to `/data/rustfs0`
- PVC: `my-rustfs-primary-1-vol-1` → mounted to `/data/rustfs1`

---

## 🎯 Summary

### Solution to RustFS Path Dependency

| Problem | Traditional Approach | Kubernetes Approach |
|---------|---------------------|---------------------|
| **Path Management** | Manually create directories | VolumeMount automatically mounts |
| **Storage Abstraction** | Direct use of local disk | PVC abstraction, supports multiple storage backends |
| **Data Persistence** | Depends on physical disk | PVC ensures data persistence |
| **Multi-node Coordination** | Manually configure IPs | Headless Service + DNS |
| **Configuration Management** | Manually set environment variables | Operator automatically generates |

### Core Advantages

1. **Declarative Configuration**: Users only declare "what they want", Operator handles "how to do it"
2. **Storage Abstraction**: Doesn't care if the underlying storage is local disk, NFS, cloud storage, or others
3. **Automation**: Automatically creates, configures, and manages all resources
4. **Portability**: Same configuration can run on any Kubernetes cluster
5. **Scalability**: Easily add nodes and scale storage

---

## 🔗 Related Documentation

- [Architecture Decisions](./architecture-decisions.md)
- [Development Notes](./DEVELOPMENT-NOTES.md)
- [Usage Examples](../examples/README.md)

---

**Key Understanding**: RustFS does depend on system paths, but Kubernetes uses the VolumeMount mechanism to "disguise" persistent storage as filesystem paths, making RustFS think it's accessing local disk when it's actually accessing Kubernetes-managed persistent storage. This is the core idea of containerized storage systems!
