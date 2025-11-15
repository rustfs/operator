# RustFS Operator Examples

This directory contains example Tenant configurations for the RustFS Kubernetes Operator, covering various use cases from development to production.

## Quick Start Guide

**Choose the right example for your needs:**

| Example | Use Case | Complexity | Storage | Best For |
|---------|----------|------------|---------|----------|
| [minimal-dev-tenant.yaml](./minimal-dev-tenant.yaml) | Development/Learning | ⭐ Simple | 40Gi | **Start here** if new |
| [simple-tenant.yaml](./simple-tenant.yaml) | Documentation Reference | ⭐⭐ Moderate | Configurable | Learning all options |
| [secret-credentials-tenant.yaml](./secret-credentials-tenant.yaml) | Secret-based Credentials | ⭐ Simple | Configurable | **Production credential security** |
| [multi-pool-tenant.yaml](./multi-pool-tenant.yaml) | Multiple Pools | ⭐⭐ Moderate | ~160Gi | Multi-pool setups |
| [production-ha-tenant.yaml](./production-ha-tenant.yaml) | Production HA | ⭐⭐⭐ Advanced | 6.4TB | HA with zone distribution |
| [cluster-expansion-tenant.yaml](./cluster-expansion-tenant.yaml) | Capacity Expansion | ⭐⭐⭐ Advanced | 384TB | Growing cluster capacity |
| [hardware-pools-tenant.yaml](./hardware-pools-tenant.yaml) | Mixed Disk Sizes | ⭐⭐⭐ Advanced | 352TB | Same class, different sizes |
| [geographic-pools-tenant.yaml](./geographic-pools-tenant.yaml) | Multi-Region | ⭐⭐⭐⭐ Expert | 480TB | Compliance & geo-distribution |
| [spot-instance-tenant.yaml](./spot-instance-tenant.yaml) | Cost Optimization | ⭐⭐⭐⭐ Expert | 288TB | 70-90% cost savings |
| [custom-rbac-tenant.yaml](./custom-rbac-tenant.yaml) | Security/RBAC | ⭐⭐⭐ Advanced | Configurable | Custom security needs |

**Recommended learning path:**
1. Start with **minimal-dev-tenant.yaml** to see it work
2. Read **simple-tenant.yaml** to understand all options
3. Explore other examples based on your use case

**Important Notes:**
- RustFS S3 API runs on port **9000**
- RustFS Console UI runs on port **9001**
- **Credentials**: Use Secrets for production (see `secret-credentials-tenant.yaml`)
- Default dev credentials: `rustfsadmin` / `rustfsadmin` ⚠️ **Change for production!**
- Operator automatically sets: `RUSTFS_VOLUMES`, `RUSTFS_ADDRESS`, `RUSTFS_CONSOLE_ADDRESS`, `RUSTFS_CONSOLE_ENABLE`

**⚠️ Critical Architecture Understanding:**
- **All pools form ONE unified cluster** - Data is erasure-coded across ALL volumes
- **Use same storage class across all pools** - Mixing NVMe/SSD/HDD results in HDD-level performance for everything
- **Valid multi-pool uses**: Geographic distribution, capacity scaling, compute differentiation (spot vs on-demand)
- **Invalid uses**: Storage performance tiering (NVMe for hot, HDD for cold) - this doesn't work!

## Available Examples

### 1. [minimal-dev-tenant.yaml](./minimal-dev-tenant.yaml) 🚀 **Start Here**

**Smallest valid configuration** for learning, testing, and local development.

**Configuration:**
- 1 server × 4 volumes = 4 total volumes (minimum valid)
- 40Gi total storage (4 × 10Gi default)
- Debug logging enabled

**Use case:** Local development, learning the operator, quick testing.

**Deployment:**
```bash
kubectl apply -f examples/minimal-dev-tenant.yaml
```

---

### 2. [simple-tenant.yaml](./simple-tenant.yaml) 📚 **Detailed Reference**

Single-pool tenant with **comprehensive comments** explaining all available options.

**Features demonstrated:**
- Basic pool configuration with validation
- Persistence settings with volume claim templates
- Custom labels and annotations for PVCs
- Environment variable configuration
- Automatic RUSTFS_VOLUMES generation explanation

**Use case:** Learning all configuration options, documentation reference.

**Deployment:**
```bash
kubectl apply -f examples/simple-tenant.yaml
```

---

### 3. [secret-credentials-tenant.yaml](./secret-credentials-tenant.yaml) 🔒 **Secure Credentials**

**RECOMMENDED for production**: Demonstrates secure credential management using Kubernetes Secrets.

**Features demonstrated:**
- Secret creation with RustFS credentials (`accesskey` and `secretkey`)
- Tenant referencing Secret via `spec.configuration.name`
- Automatic credential injection into pods as `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY`
- Production security best practices
- Alternative approaches (env var with `secretKeyRef`)
- Credential rotation instructions

**Configuration:**
- 1 pool with 2 servers × 2 volumes = 4 volumes
- Credentials stored in Secret (not hardcoded in YAML)
- Secrets encrypted at rest (if cluster configured)

**Use case:** Production deployments requiring secure credential management, compliance requirements, credential rotation.

**Security benefits:**
- ✅ Credentials not visible in Tenant YAML
- ✅ RBAC-controlled Secret access
- ✅ Compatible with external secret managers (Vault, AWS Secrets Manager, etc.)
- ✅ Supports credential rotation without YAML changes
- ✅ Audit trail for Secret access

**Deployment:**
```bash
# Create Secret and Tenant
kubectl apply -f examples/secret-credentials-tenant.yaml

# Verify credentials injected (should not show actual values)
kubectl describe pod secure-tenant-pool-0-0 | grep -A5 "Environment:"
```

**Production recommendations:**
- Use External Secrets Operator or Sealed Secrets for GitOps
- Enable Kubernetes Secret encryption at rest
- Rotate credentials quarterly
- Generate strong credentials: `openssl rand -hex 32`

---

### 4. [multi-pool-tenant.yaml](./multi-pool-tenant.yaml) 🔄 **Multi-Pool**

Multiple storage pools within a single tenant.

**Features demonstrated:**
- Multiple pools per tenant (2 pools with different configs)
- Different server/volume configurations per pool
- Environment variable configuration
- Custom scheduler

**Configuration:**
- Pool-0: 4 servers × 2 volumes = 8 volumes
- Pool-1: 2 servers × 4 volumes = 8 volumes
- 16 total volumes across 2 pools

**Use case:** Deployments needing multiple pool configurations.

**Deployment:**
```bash
kubectl apply -f examples/multi-pool-tenant.yaml
```

---

### 5. [production-ha-tenant.yaml](./production-ha-tenant.yaml) 🏢 **Production Ready**

**High-availability production configuration** with enterprise features.

**Features demonstrated:**
- High availability (16 servers)
- Custom storage classes (fast-ssd)
- Large storage capacity (100Gi per volume)
- PVC labels and annotations for backup/monitoring
- ConfigMap and Secret integration
- Resource requests and limits
- Priority class
- Lifecycle hooks (graceful shutdown)
- Custom scheduler

**Configuration:**
- 16 servers × 4 volumes = 64 PVCs
- 6.4TB total storage (64 × 100Gi)

**Use case:** Production deployments requiring HA and enterprise features.

**Deployment:**
```bash
kubectl create namespace production
kubectl apply -f examples/production-ha-tenant.yaml
```

---

### 6. [cluster-expansion-tenant.yaml](./cluster-expansion-tenant.yaml) 📈 **Cluster Expansion**

**Add capacity or migrate to new hardware** using multiple pools.

**Features demonstrated:**
- Multiple pool versions (v1, v2) in one tenant
- Gradual capacity expansion workflow
- Pool decommissioning strategy
- Hardware generation migration
- Different resources per pool generation
- Node targeting by hardware generation

**Configuration:**
- **Pool v1** (original): 8 servers × 4 volumes = 32 PVCs (64Ti)
- **Pool v2** (expansion): 16 servers × 4 volumes = 64 PVCs (320Ti)
- **Total**: 96 PVCs, ~384Ti capacity

**Use case:**
- Growing storage needs
- Hardware lifecycle management
- Zero-downtime capacity expansion
- Gradual migration to new hardware

**Deployment:**
```bash
kubectl create namespace storage
kubectl apply -f examples/cluster-expansion-tenant.yaml
```

---

### 6. [hardware-pools-tenant.yaml](./hardware-pools-tenant.yaml) 💽 **Mixed Disk Sizes**

**Utilize nodes with different disk sizes** (all same performance class).

**Features demonstrated:**
- Per-pool node selectors (target disk configurations)
- Different resource requirements per pool
- Same storage class, different disk sizes
- Heterogeneous hardware utilization

**Configuration:**
- **Large disks**: 4 servers × 4 volumes = 16 PVCs (10Ti each, 160Ti total)
- **Medium disks**: 8 servers × 4 volumes = 32 PVCs (5Ti each, 160Ti total)
- **Small disks**: 4 servers × 4 volumes = 16 PVCs (2Ti each, 32Ti total)
- **Total**: 64 PVCs, ~352Ti storage (all SSD for consistent performance)

**Use case:**
- Utilize existing heterogeneous hardware
- Different disk sizes but same performance class
- Capacity planning without hardware replacement
- Hardware lifecycle management

**⚠️ Important**: All pools use SAME storage class to avoid performance degradation.

**Deployment:**
```bash
kubectl create namespace storage
kubectl apply -f examples/hardware-pools-tenant.yaml
```

---

### 7. [geographic-pools-tenant.yaml](./geographic-pools-tenant.yaml) 🌍 **Multi-Region**

**Geographic distribution** across multiple regions for compliance and latency.

**Features demonstrated:**
- Node affinity for region targeting
- Topology spread constraints for zone distribution
- GDPR/compliance-ready labeling
- Data sovereignty enforcement
- Regional resource allocation

**Configuration:**
- **US region**: 8 servers × 4 volumes = 32 PVCs (160Ti)
- **EU region**: 8 servers × 4 volumes = 32 PVCs (160Ti, GDPR-compliant)
- **APAC region**: 8 servers × 4 volumes = 32 PVCs (160Ti)
- **Total**: 96 PVCs, ~480Ti storage across 3 regions

**Use case:**
- Global applications with regional data requirements
- GDPR and data residency compliance
- Low-latency access for regional users
- Disaster recovery across geographies

**Deployment:**
```bash
kubectl create namespace global
kubectl apply -f examples/geographic-pools-tenant.yaml
```

---

### 8. [spot-instance-tenant.yaml](./spot-instance-tenant.yaml) 💰 **Cost Optimization**

**Mix of on-demand and spot instances** for 70-90% cost savings.

**Features demonstrated:**
- On-demand pool for critical data
- Spot instance pool for elastic capacity
- Tolerations for spot taints
- Topology spread across instance types
- Priority classes for resource guarantees
- Cost-optimized architecture

**Configuration:**
- **Critical pool**: 4 servers × 4 volumes = 16 PVCs (32Ti on-demand)
- **Elastic pool**: 16 servers × 4 volumes = 64 PVCs (256Ti spot)
- **Total**: 80 PVCs, ~288Ti storage
- **Cost savings**: ~69% vs all on-demand

**Use case:**
- Production workloads with cost constraints
- Elastic capacity with reliability
- Handling spot interruptions gracefully
- Cloud cost optimization

**Deployment:**
```bash
kubectl create namespace storage
kubectl apply -f examples/spot-instance-tenant.yaml
```

---

### 9. [custom-rbac-tenant.yaml](./custom-rbac-tenant.yaml) 🔐 **Custom Security**

**Custom RBAC configurations** for advanced security requirements.

**Features demonstrated:**
- Custom ServiceAccount usage (2 configurations)
- Manual RBAC management (Role + RoleBinding)
- Operator-managed RBAC with custom SA
- Cloud workload identity integration (AWS/GCP/Azure)
- Additional permissions beyond defaults
- Security best practices

**Two configurations:**
1. Custom SA without operator RBAC (you manage everything)
2. Custom SA with operator RBAC (operator creates Role/RoleBinding)

**Use case:**
- Existing RBAC policies
- Cloud workload identity (IAM roles)
- External authentication systems
- Additional permissions needed

**Deployment:**
```bash
kubectl apply -f examples/custom-rbac-tenant.yaml
```

## Example Structure

All examples follow this basic structure:

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: <tenant-name>
  namespace: <namespace>
spec:
  # Container image (optional)
  image: rustfs/rustfs:latest

  # Storage pools (required, at least one)
  pools:
    - name: <pool-name>
      servers: <number>              # Must be > 0
      persistence:
        volumesPerServer: <number>   # Must be > 0
        # Validation: servers * volumesPerServer >= 4

  # Optional fields
  env: [...]
  scheduler: <scheduler-name>
  serviceAccountName: <sa-name>
  # ... see Tenant CRD for all fields
```

## Validation Rules

### Pool Requirements

- **Minimum volumes**: `servers * volumesPerServer >= 4` (RustFS erasure coding requirement)
- **Server count**: Must be > 0
- **Volumes per server**: Must be > 0
- **Pool name**: Must not be empty

### Valid Examples

✅ `servers: 4, volumesPerServer: 1` → 4 total volumes
✅ `servers: 2, volumesPerServer: 2` → 4 total volumes
✅ `servers: 4, volumesPerServer: 4` → 16 total volumes

### Invalid Examples

❌ `servers: 2, volumesPerServer: 1` → 2 total volumes (< 4)
❌ `servers: 1, volumesPerServer: 1` → 1 total volume (< 4)
❌ `servers: 0, volumesPerServer: 4` → Server count must be > 0

## Common Configurations

### Development (Minimal)

```yaml
spec:
  pools:
    - name: dev
      servers: 1
      persistence:
        volumesPerServer: 4  # 1 * 4 = 4 (minimum valid)
```

### Production (High Availability)

```yaml
spec:
  pools:
    - name: production
      servers: 16
      persistence:
        volumesPerServer: 4
        volumeClaimTemplate:
          accessModes: ["ReadWriteOnce"]
          resources:
            requests:
              storage: 100Gi
          storageClassName: fast-ssd
```

### Multi-Tier Storage

```yaml
spec:
  pools:
    # Hot tier - NVMe
    - name: hot
      servers: 4
      persistence:
        volumesPerServer: 8

    # Warm tier - SSD
    - name: warm
      servers: 8
      persistence:
        volumesPerServer: 4

    # Cold tier - HDD
    - name: cold
      servers: 16
      persistence:
        volumesPerServer: 2
```

## Created Resources

When you apply a Tenant, the operator creates:

1. **RBAC Resources** (conditional based on configuration)
   - Role
   - ServiceAccount
   - RoleBinding

2. **Services**
   - IO Service: `rustfs` (port 90→9000)
   - Console Service: `{tenant}-console` (port 9090)
   - Headless Service: `{tenant}-hl` (for StatefulSet DNS)

3. **StatefulSets** (one per pool)
   - Volume Claim Templates: `vol-0`, `vol-1`, ...
   - Automatic `RUSTFS_VOLUMES` environment variable
   - Volume mounts at `/data/rustfs0`, `/data/rustfs1`, ... (follows RustFS convention)

## Verifying Deployment

After applying an example:

```bash
# Check tenant status
kubectl get tenant <name>

# Check all resources
kubectl get all,pvc -l rustfs.tenant=<name>

# Check specific resources
kubectl get statefulset -l rustfs.tenant=<name>
kubectl get service -l rustfs.tenant=<name>
kubectl get pods -l rustfs.tenant=<name>

# View RUSTFS_VOLUMES configuration
kubectl get statefulset <name>-<pool> -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="RUSTFS_VOLUMES")].value}'
```

## Troubleshooting

### Tenant Not Creating Resources

Check operator logs:
```bash
kubectl logs -n rustfs-system -l app=rustfs-operator
```

### Validation Errors

Common issues:
- Pool validation: Ensure `servers * volumesPerServer >= 4`
- Empty pool name
- Zero servers or volumes

### Pods Not Starting

Check StatefulSet and Pod status:
```bash
kubectl describe statefulset <name>-<pool>
kubectl describe pod <pod-name>
```

## Additional Resources

- [API Reference](../docs/api-reference.md) - Complete field reference
- [Getting Started](../docs/getting-started.md) - Step-by-step guide
- [Troubleshooting](../docs/troubleshooting.md) - Common issues and solutions
- [Architecture](../docs/architecture.md) - Understanding the system

## Contributing Examples

To contribute a new example:

1. Create a descriptive YAML file
2. Add comprehensive comments
3. Test the configuration
4. Update this README with a description
5. Submit a pull request

Example template:
```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: example-name
  namespace: default
spec:
  # Clearly commented configuration
  pools:
    - name: pool-0
      servers: 4
      persistence:
        volumesPerServer: 4
```

---

**Version**: v0.1.0
**Last Updated**: 2025-11-05
