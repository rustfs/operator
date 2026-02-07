# RustFS Object Storage Configuration and Usage Guide

This document explains in detail the meaning of RustFS configuration parameters and how to use RustFS as an object storage system.

---

## 📋 Configuration Parameters Explained

### Example Configuration

```yaml
pools:
  - name: dev-pool
    servers: 1              # Number of server nodes
    persistence:
      volumesPerServer: 4   # Number of storage volumes per server
```

### Parameter Meanings

#### `servers: 1`

**Meaning**: Number of server nodes in the RustFS cluster

- **Purpose**: Determines how many Pods to create (each Pod represents a RustFS server node)
- **Examples**:
  - `servers: 1` → Creates 1 Pod (single node, suitable for development)
  - `servers: 4` → Creates 4 Pods (4-node cluster, suitable for production)
  - `servers: 16` → Creates 16 Pods (large-scale cluster)

**Actual Effect**:
- Operator creates a StatefulSet with replicas = `servers`
- Each Pod runs a RustFS server instance
- Pod naming format: `{tenant-name}-{pool-name}-{0...servers-1}`

#### `volumesPerServer: 4`

**Meaning**: Number of storage volumes on each server node

- **Purpose**: Determines how many persistent storage volumes each Pod mounts
- **Examples**:
  - `volumesPerServer: 4` → Each Pod has 4 storage volumes
  - `volumesPerServer: 8` → Each Pod has 8 storage volumes

**Actual Effect**:
- Operator creates `volumesPerServer` PVCs for each Pod
- Each PVC is mounted to container paths: `/data/rustfs0`, `/data/rustfs1`, `/data/rustfs2`, `/data/rustfs3`
- PVC naming format: `{pod-name}-vol-0`, `{pod-name}-vol-1`, ...

#### Total Storage Volume Count

**Calculation Formula**: `Total volumes = servers × volumesPerServer`

**Total volumes for example configuration**:
```
servers: 1
volumesPerServer: 4
→ Total volumes = 1 × 4 = 4 storage volumes
```

**Minimum Requirement**: `servers × volumesPerServer >= 4`

This is RustFS's Erasure Coding requirement, which needs at least 4 storage volumes to function properly.

---

## 🏗️ Actually Created Resources

### What Does the Example Configuration Create?

```yaml
pools:
  - name: dev-pool
    servers: 1
    persistence:
      volumesPerServer: 4
```

#### 1. StatefulSet

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: dev-minimal-dev-pool
spec:
  replicas: 1  # servers: 1
  template:
    spec:
      containers:
      - name: rustfs
        image: rustfs/rustfs:latest
        env:
        - name: RUSTFS_VOLUMES
          value: "http://dev-minimal-dev-pool-{0...0}.dev-minimal-hl.default.svc.cluster.local:9000/data/rustfs{0...3}"
        volumeMounts:
        - name: vol-0
          mountPath: /data/rustfs0
        - name: vol-1
          mountPath: /data/rustfs1
        - name: vol-2
          mountPath: /data/rustfs2
        - name: vol-3
          mountPath: /data/rustfs3
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
  - metadata:
      name: vol-2
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 10Gi
  - metadata:
      name: vol-3
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 10Gi
```

#### 2. PersistentVolumeClaims (PVCs)

```
dev-minimal-dev-pool-0-vol-0  (10Gi)
dev-minimal-dev-pool-0-vol-1  (10Gi)
dev-minimal-dev-pool-0-vol-2  (10Gi)
dev-minimal-dev-pool-0-vol-3  (10Gi)
```

**Total Storage Capacity**: 4 × 10Gi = 40Gi (default 10Gi per volume)

#### 3. Pod

```
dev-minimal-dev-pool-0
```

Paths mounted inside the Pod:
- `/data/rustfs0` ← PVC `dev-minimal-dev-pool-0-vol-0`
- `/data/rustfs1` ← PVC `dev-minimal-dev-pool-0-vol-1`
- `/data/rustfs2` ← PVC `dev-minimal-dev-pool-0-vol-2`
- `/data/rustfs3` ← PVC `dev-minimal-dev-pool-0-vol-3`

---

## 💾 How RustFS Object Storage Works

### 1. Data Distribution Mechanism

RustFS uses **Erasure Coding** to distribute data:

```
User uploads object
    ↓
RustFS splits object into data shards
    ↓
Calculates parity shards
    ↓
Distributes data and parity shards across all storage volumes
    ↓
Data redundantly stored, can recover even if some volumes fail
```

**Example** (with 4 volumes):
- Object is split into 2 data shards + 2 parity shards
- Each shard is stored on a different volume
- Even if 2 volumes fail, data can still be recovered from the remaining 2 volumes

### 2. Role of Storage Volumes

Each storage volume (`/data/rustfs0`, `/data/rustfs1`, ...):
- **Stores data shards**: Part of the object's data
- **Stores metadata**: Object metadata, indexes, etc.
- **Participates in erasure coding**: Works with other volumes to provide data redundancy

### 3. Why Are At Least 4 Volumes Required?

RustFS's erasure coding algorithm requires:
- **Minimum data shards**: At least 2 data shards
- **Minimum parity shards**: At least 2 parity shards
- **Total**: At least 4 shards → At least 4 storage volumes

**Configuration Examples**:
- ✅ `servers: 1, volumesPerServer: 4` → 4 volumes (minimum configuration)
- ✅ `servers: 2, volumesPerServer: 2` → 4 volumes (minimum configuration)
- ✅ `servers: 4, volumesPerServer: 1` → 4 volumes (minimum configuration)
- ❌ `servers: 1, volumesPerServer: 2` → 2 volumes (insufficient, won't work)
- ❌ `servers: 2, volumesPerServer: 1` → 2 volumes (insufficient, won't work)

---

## 🚀 How to Use RustFS Object Storage

### 1. Deploy RustFS Cluster

```bash
# Apply configuration
kubectl apply -f examples/minimal-dev-tenant.yaml

# Wait for Pods to be ready
kubectl wait --for=condition=ready pod -l rustfs.tenant=dev-minimal --timeout=300s

# Check status
kubectl get tenant dev-minimal
kubectl get pods -l rustfs.tenant=dev-minimal
```

### 2. Access S3 API

RustFS provides S3-compatible object storage API. The Service type created by the Operator is `ClusterIP`, which means:

- **Inside cluster**: Can directly use Service DNS names to access (**no port-forward needed**)
- **Outside cluster**: Requires port forwarding, Ingress, or LoadBalancer

#### Method 1: Cluster-Internal Access (Recommended for Production)

**Use Case**: Applications running inside Kubernetes cluster accessing RustFS

**Service DNS Name Format**:
- S3 API: `http://rustfs.{namespace}.svc.cluster.local:9000`
- Console UI: `http://{tenant-name}-console.{namespace}.svc.cluster.local:9001`

**Example** (in Pod or cluster-internal application):

```bash
# Use Service DNS name (no port-forward needed)
# S3 API endpoint
http://rustfs.default.svc.cluster.local:9000

# Console UI endpoint
http://dev-minimal-console.default.svc.cluster.local:9001
```

**Using MinIO Client (Cluster-Internal)**:
```bash
# Execute in Pod inside cluster
mc alias set rustfs http://rustfs.default.svc.cluster.local:9000 rustfsadmin rustfsadmin
mc mb rustfs/my-bucket
mc cp file.txt rustfs/my-bucket/
```

**Using AWS CLI (Cluster-Internal)**:
```bash
# Execute in Pod inside cluster
aws --endpoint-url http://rustfs.default.svc.cluster.local:9000 s3 ls
aws --endpoint-url http://rustfs.default.svc.cluster.local:9000 s3 mb s3://my-bucket
```

**Using Python SDK (Cluster-Internal)**:
```python
import boto3
from botocore.client import Config

# Use Service DNS (no port-forward needed)
s3 = boto3.client(
    's3',
    endpoint_url='http://rustfs.default.svc.cluster.local:9000',  # Cluster-internal DNS
    aws_access_key_id='rustfsadmin',
    aws_secret_access_key='rustfsadmin',
    config=Config(signature_version='s3v4'),
    region_name='us-east-1'
)
```

#### Method 2: Port Forwarding (Local Development/Testing)

**Use Case**: Accessing RustFS in cluster from local machine (development, testing, debugging)

⚠️ **Note**: This method requires keeping the `kubectl port-forward` command running

```bash
# Terminal 1: Forward S3 API port (9000)
kubectl port-forward svc/rustfs 9000:9000

# Terminal 2: Use localhost to access (requires port forwarding)
mc alias set devlocal http://localhost:9000 rustfsadmin rustfsadmin
mc mb devlocal/my-bucket
mc cp file.txt devlocal/my-bucket/
```

**Using MinIO Client (Requires port-forward)**:
```bash
# Must execute port forwarding first
kubectl port-forward svc/rustfs 9000:9000

# Then use localhost
mc alias set devlocal http://localhost:9000 rustfsadmin rustfsadmin
mc mb devlocal/my-bucket
mc cp /path/to/file.txt devlocal/my-bucket/
mc ls devlocal/my-bucket
```

**Using AWS CLI (Requires port-forward)**:
```bash
# Must execute port forwarding first
kubectl port-forward svc/rustfs 9000:9000

# Then use localhost
export AWS_ACCESS_KEY_ID=rustfsadmin
export AWS_SECRET_ACCESS_KEY=rustfsadmin
aws --endpoint-url http://localhost:9000 s3 ls
aws --endpoint-url http://localhost:9000 s3 mb s3://my-bucket
aws --endpoint-url http://localhost:9000 s3 cp file.txt s3://my-bucket/
```

**Using Python SDK (Requires port-forward)**:
```python
import boto3
from botocore.client import Config

# Must execute first: kubectl port-forward svc/rustfs 9000:9000
s3 = boto3.client(
    's3',
    endpoint_url='http://localhost:9000',  # Requires port forwarding
    aws_access_key_id='rustfsadmin',
    aws_secret_access_key='rustfsadmin',
    config=Config(signature_version='s3v4'),
    region_name='us-east-1'
)
```

#### Method 3: Using Ingress (Recommended for Production)

**Use Case**: Production environment, requires HTTPS and domain name access

Create Ingress resource:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: rustfs-ingress
  namespace: default
spec:
  rules:
  - host: rustfs.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: rustfs
            port:
              number: 9000
```

Then access using domain name:
```bash
mc alias set production https://rustfs.example.com rustfsadmin rustfsadmin
```

#### Method 4: Using LoadBalancer (Cloud Environments)

**Use Case**: Cloud environments (AWS, GCP, Azure), requires external IP

Modify Service type (requires manual modification or Helm values):

```yaml
# Note: Operator creates ClusterIP by default, need to manually change to LoadBalancer
apiVersion: v1
kind: Service
metadata:
  name: rustfs
spec:
  type: LoadBalancer  # Change to LoadBalancer
  ports:
  - port: 9000
```

Then access using external IP:
```bash
# Get external IP
kubectl get svc rustfs

# Use external IP
mc alias set production http://<EXTERNAL-IP>:9000 rustfsadmin rustfsadmin
```

---

### Access Method Comparison

| Access Method | Requires port-forward? | Use Case | Endpoint Example |
|--------------|----------------------|----------|------------------|
| **Cluster-Internal** | ❌ **No** | Cluster-internal applications | `http://rustfs.default.svc.cluster.local:9000` |
| **Port Forwarding** | ✅ **Yes** | Local development/testing | `http://localhost:9000` |
| **Ingress** | ❌ No | Production environment (HTTPS) | `https://rustfs.example.com` |
| **LoadBalancer** | ❌ No | Cloud environments | `http://<EXTERNAL-IP>:9000` |

---

### 3. Access Web Console

#### Cluster-Internal Access (No port-forward needed)

```bash
# In Pod inside cluster
curl http://dev-minimal-console.default.svc.cluster.local:9001
```

#### Port Forwarding Access (Requires port-forward)

```bash
# Forward console port (9001)
kubectl port-forward svc/dev-minimal-console 9001:9001

# Open in browser
open http://localhost:9001
```

**Default Credentials**:
- Username: `rustfsadmin`
- Password: `rustfsadmin`

⚠️ **Must change default credentials in production!**

---

## 📊 Configuration Examples Comparison

### Development Environment (Minimal Configuration)

```yaml
pools:
  - name: dev-pool
    servers: 1              # 1 node
    persistence:
      volumesPerServer: 4   # 4 volumes per node
```

**Result**:
- 1 Pod
- 4 PVCs (10Gi each)
- Total storage: 40Gi
- **Use Case**: Local development, testing, learning

### Production Environment (High Availability)

```yaml
pools:
  - name: production
    servers: 8              # 8 nodes
    persistence:
      volumesPerServer: 4   # 4 volumes per node
      volumeClaimTemplate:
        resources:
          requests:
            storage: 100Gi  # 100Gi per volume
```

**Result**:
- 8 Pods (distributed across multiple nodes)
- 32 PVCs (100Gi each)
- Total storage: 3.2Ti
- **Use Case**: Production environment, high availability, large capacity

### Multi-Pool Configuration (Scaling Storage)

```yaml
pools:
  - name: pool-0
    servers: 4
    persistence:
      volumesPerServer: 4   # 16 volumes
      
  - name: pool-1
    servers: 4
    persistence:
      volumesPerServer: 4   # 16 volumes
```

**Result**:
- 8 Pods (2 StatefulSets)
- 32 PVCs
- **All pools form a unified cluster**
- **Use Case**: Need to scale storage capacity

---

## 🔍 Data Storage Flow

### Write Data Flow

```
1. Client uploads object to S3 API (port 9000)
   ↓
2. RustFS receives object
   ↓
3. RustFS uses erasure coding algorithm:
   - Splits object into data shards
   - Calculates parity shards
   ↓
4. Distributes shards across multiple storage volumes:
   - /data/rustfs0 ← Data shard 1
   - /data/rustfs1 ← Data shard 2
   - /data/rustfs2 ← Parity shard 1
   - /data/rustfs3 ← Parity shard 2
   ↓
5. Data persisted to PVC (underlying storage)
```

### Read Data Flow

```
1. Client requests object
   ↓
2. RustFS locates object shard positions
   ↓
3. Reads shards from multiple storage volumes:
   - /data/rustfs0 → Data shard 1
   - /data/rustfs1 → Data shard 2
   - /data/rustfs2 → Parity shard 1 (if needed)
   ↓
4. Uses erasure coding algorithm to reconstruct complete object
   ↓
5. Returns object to client
```

### Failure Recovery Flow

```
Scenario: /data/rustfs0 volume failure
   ↓
1. RustFS detects volume unavailable
   ↓
2. Reads data and parity shards from other volumes:
   - /data/rustfs1 → Data shard 2
   - /data/rustfs2 → Parity shard 1
   - /data/rustfs3 → Parity shard 2
   ↓
3. Uses erasure coding algorithm to reconstruct lost data shard
   ↓
4. When volume recovers, automatically rebuilds data
```

---

## 📈 Capacity Planning

### Storage Capacity Calculation

**Formula**: `Total capacity = servers × volumesPerServer × single volume capacity`

**Example**:
```yaml
servers: 4
volumesPerServer: 4
volumeClaimTemplate:
  resources:
    requests:
      storage: 100Gi
```

**Calculation**:
- Total volumes: 4 × 4 = 16 volumes
- Total capacity: 16 × 100Gi = 1.6Ti

### Usable Capacity

Due to erasure coding redundancy, **usable capacity < total capacity**:

- **EC:2** (2 data shards + 2 parity shards): Usable capacity = Total capacity × 50%
- **EC:4** (4 data shards + 4 parity shards): Usable capacity = Total capacity × 50%

**Example**:
- Total capacity: 1.6Ti
- Usable capacity: Approximately 800Gi (50%)

### Performance Considerations

- **More volumes**: Better parallel I/O, higher throughput
- **More nodes**: Better load distribution, higher availability
- **Storage type**: SSD > HDD (performance)

---

## 🎯 Use Cases

### 1. Application Data Storage

```yaml
# Use RustFS as object storage backend in application configuration
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  S3_ENDPOINT: "http://rustfs.default.svc.cluster.local:9000"
  S3_BUCKET: "app-data"
  S3_ACCESS_KEY: "rustfsadmin"
  S3_SECRET_KEY: "rustfsadmin"
```

### 2. Backup Storage

```yaml
# Velero backup to RustFS
apiVersion: velero.io/v1
kind: BackupStorageLocation
metadata:
  name: rustfs-backup
spec:
  provider: aws
  objectStorage:
    bucket: velero-backups
    prefix: backups
  config:
    region: us-east-1
    s3ForcePathStyle: "true"
    s3Url: "http://rustfs.default.svc.cluster.local:9000"
```

### 3. CI/CD Build Artifact Storage

```yaml
# GitLab CI configuration
build:
  script:
    - aws s3 cp build.tar.gz s3://artifacts/myapp/ --endpoint-url http://rustfs:9000
```

---

## 🔗 Related Documentation

- [RustFS Kubernetes Integration](./RUSTFS-K8S-INTEGRATION.md)
- [Development Environment Setup](./DEVELOPMENT.md)
- [Usage Examples](../examples/README.md)

---

## Summary

**Configuration Meanings**:
- `servers: 1` → 1 RustFS server node (Pod)
- `volumesPerServer: 4` → 4 storage volumes per node
- **Total volumes** = 1 × 4 = 4 volumes (meets minimum requirement)

**Object Storage Usage**:
1. RustFS provides S3-compatible API (port 9000)
2. Data is distributed across all storage volumes via erasure coding
3. Supports standard S3 clients and SDKs
4. Provides Web console (port 9001) for management

**Key Understanding**:
- Storage volumes are RustFS's physical storage units
- Multiple volumes provide data redundancy and performance
- At least 4 volumes are required for normal operation (erasure coding requirement)
