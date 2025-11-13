# Multi-Pool Use Cases

## Overview

The RustFS Operator supports multiple pools within a single Tenant, enabling advanced deployment patterns for capacity scaling, compliance, cost control, and high availability.

## ⚠️ Critical Architecture Understanding

### Unified Cluster Behavior

**All pools in a Tenant form ONE unified RustFS erasure-coded cluster**, not independent storage tiers.

**Key Points:**
1. **Single RUSTFS_VOLUMES**: All pools combined into one space-separated environment variable
2. **Uniform Data Distribution**: Erasure coding stripes data across ALL volumes in ALL pools
3. **No Storage Class Awareness**: RustFS does NOT prefer fast disks over slow disks
4. **Performance Limitation**: Cluster performs at the speed of the SLOWEST storage class

### Common Misconception

**WRONG Assumption:**
"I can create an NVMe pool for hot data and an HDD pool for cold data, and RustFS will intelligently place data on the appropriate tier."

**REALITY:**
- RustFS has NO hot/warm/cold data awareness for internal pools
- ALL data is uniformly distributed across ALL volumes
- An object will have shards on NVMe AND HDD
- Read/write performance limited by slowest tier (HDD)
- Expensive NVMe provides ZERO performance benefit

### What RustFS Tiering Actually Is

**RustFS tiering** (from `crates/ecstore/src/tier/tier.rs`):
- Transitions data to **EXTERNAL** cloud storage (S3, Azure, GCS, MinIO)
- Configured via **bucket lifecycle policies**
- NOT for internal disk class differentiation

**Example**: Transition old objects to AWS S3 Glacier for cost savings.

## Architecture

### Single Tenant, Multiple Pools

Each Tenant can contain multiple Pools:
- **One StatefulSet per Pool** with independent configuration
- **Unified distributed cluster** via combined RUSTFS_VOLUMES
- **Shared services** (IO, Console, Headless) across all pools
- **Pool-specific scheduling** for node targeting and resource allocation

### Example Structure

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: my-tenant
spec:
  pools:
    - name: pool-a
      servers: 4
      persistence: {...}
      nodeSelector: {storage-type: nvme}  # Pool-A specific
      resources: {requests: {cpu: "8"}}   # Pool-A specific

    - name: pool-b
      servers: 8
      persistence: {...}
      nodeSelector: {storage-type: ssd}   # Pool-B specific
      resources: {requests: {cpu: "4"}}   # Pool-B specific
```

## Per-Pool Configuration Options

### Storage Configuration
- **Storage Class**: Different storage types per pool (NVMe, SSD, HDD)
- **Storage Size**: Different capacities per pool
- **Volume Count**: Different server/volume ratios
- **Mount Paths**: Custom paths (default: `/data/rustfs{N}`)

### Kubernetes Scheduling
- **Node Selector**: Target specific nodes by labels
- **Affinity**: Complex node/pod affinity rules
- **Tolerations**: Schedule on tainted/dedicated nodes
- **Topology Spread**: Distribute across zones/regions
- **Resources**: CPU/memory requests and limits
- **Priority Class**: Override tenant-level priority (per-pool)

### Metadata
- **Labels**: Custom PVC labels for each pool
- **Annotations**: Backup policies, monitoring tags

## Use Case Examples

### 1. Hardware-Targeted Pools

**Scenario**: Different pools on NVMe, SSD, and HDD nodes

**Example**: [hardware-pools-tenant.yaml](../examples/hardware-pools-tenant.yaml)

**Benefits**:
- Performance optimization (hot data on NVMe)
- Cost optimization (cold data on HDD)
- Hardware utilization (match workload to hardware)
- Resource differentiation (more CPU/memory for NVMe)

**Implementation**:
```yaml
pools:
  - name: nvme-pool
    nodeSelector: {storage-type: nvme}
    resources:
      requests: {cpu: "8", memory: "32Gi"}

  - name: hdd-pool
    nodeSelector: {storage-type: hdd}
    resources:
      requests: {cpu: "2", memory: "8Gi"}
```

### 2. Geographic Distribution

**Scenario**: Pools in different regions for compliance/latency

**Example**: [geographic-pools-tenant.yaml](../examples/geographic-pools-tenant.yaml)

**Benefits**:
- GDPR compliance (EU data stays in EU)
- Data sovereignty enforcement
- Low latency for regional users
- Disaster recovery across regions

**Implementation**:
```yaml
pools:
  - name: us-region
    affinity:
      nodeAffinity:
        requiredDuringScheduling:
          nodeSelectorTerms:
            - matchExpressions:
                - key: topology.kubernetes.io/region
                  operator: In
                  values: ["us-east-1"]
    topologySpreadConstraints:
      - maxSkew: 1
        topologyKey: topology.kubernetes.io/zone
```

### 3. Cost Optimization (Spot Instances)

**Scenario**: Mix of on-demand and spot instances

**Example**: [spot-instance-tenant.yaml](../examples/spot-instance-tenant.yaml)

**Benefits**:
- 70-90% cost reduction
- Critical data on guaranteed instances
- Elastic capacity on cheaper spot instances
- Automatic failure handling via erasure coding

**Implementation**:
```yaml
pools:
  - name: critical-pool
    nodeSelector: {instance-lifecycle: on-demand}
    priorityClassName: system-cluster-critical

  - name: elastic-pool
    nodeSelector: {instance-lifecycle: spot}
    tolerations:
      - key: "spot-instance"
        operator: "Equal"
        value: "true"
        effect: "NoSchedule"
```

### 4. Workload Separation

**Scenario**: Different pools for different workload types

**Benefits**:
- Batch processing isolation from real-time
- Performance guarantees per workload
- Resource allocation by priority

**Implementation**:
```yaml
pools:
  - name: realtime-pool
    servers: 4
    nodeSelector: {workload-type: realtime}
    resources:
      requests: {cpu: "8", memory: "32Gi"}
    priorityClassName: high-priority

  - name: batch-pool
    servers: 16
    nodeSelector: {workload-type: batch}
    resources:
      requests: {cpu: "4", memory: "16Gi"}
```

### 5. Multi-Tenant SaaS

**Scenario**: Separate pools per customer tier

**Benefits**:
- SLA guarantees for premium customers
- Hardware isolation
- Security boundaries
- Cost differentiation

**Implementation**:
```yaml
pools:
  - name: enterprise-pool
    nodeSelector: {tenant-tier: enterprise}
    tolerations:
      - key: "enterprise-only"
        effect: "NoSchedule"
    resources:
      requests: {cpu: "8", memory: "32Gi"}

  - name: standard-pool
    nodeSelector: {tenant-tier: standard}
    resources:
      requests: {cpu: "2", memory: "8Gi"}
```

### 6. Failure Domain Separation

**Scenario**: Pools distributed across availability zones

**Benefits**:
- Survive entire zone failures
- Network locality within zones
- Balanced distribution

**Implementation**:
```yaml
pools:
  - name: zone-a
    affinity:
      nodeAffinity:
        requiredDuringScheduling:
          nodeSelectorTerms:
            - matchExpressions:
                - key: topology.kubernetes.io/zone
                  operator: In
                  values: ["us-east-1a"]

  - name: zone-b
    affinity:
      nodeAffinity:
        requiredDuringScheduling:
          nodeSelectorTerms:
            - matchExpressions:
                - key: topology.kubernetes.io/zone
                  operator: In
                  values: ["us-east-1b"]
```

## Technical Details

### How Pools are Combined

From `workloads.rs:37-66`:
```rust
fn rustfs_volumes_env_value(&self) -> Result<String, types::error::Error> {
    let volume_specs: Vec<String> = self.spec.pools.iter()
        .map(|pool| {
            format!(
                "http://{}-{}-{{0...{}}}.{}.{}.svc.cluster.local:9000{}/rustfs{{0...{}}}",
                tenant_name, pool_name, servers-1, headless, namespace, path, volumes-1
            )
        })
        .collect();
    Ok(volume_specs.join(" "))  // Space-separated
}
```

**Result**: All pools combined into single RUSTFS_VOLUMES environment variable.

**Example** (2 pools):
```
http://tenant-pool-a-{0...3}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...7} http://tenant-pool-b-{0...7}.tenant-hl.ns.svc.cluster.local:9000/data/rustfs{0...3}
```

RustFS then treats all 12 servers (4 from pool-a + 8 from pool-b) as a unified distributed cluster.

### Scheduling Field Propagation

From `workloads.rs:236-250`:
```rust
spec: Some(corev1::PodSpec {
    service_account_name: Some(self.service_account_name()),
    containers: vec![container],
    scheduler_name: self.spec.scheduler.clone(),
    priority_class_name: pool.priority_class_name.clone()
        .or_else(|| self.spec.priority_class_name.clone()),
    node_selector: pool.node_selector.clone(),
    affinity: pool.affinity.clone(),
    tolerations: pool.tolerations.clone(),
    topology_spread_constraints: pool.topology_spread_constraints.clone(),
    ..Default::default()
}),
```

**Pool-level** fields override or extend **tenant-level** settings.

## Best Practices

### 1. Ensure Minimum Viable Configuration

Each pool must satisfy: `servers * volumesPerServer >= 4`

**Example**:
- ✅ 1 server × 4 volumes = 4 ✓
- ✅ 2 servers × 2 volumes = 4 ✓
- ❌ 2 servers × 1 volume = 2 ✗

### 2. Plan for Failures

With multi-pool, plan for worst-case:
- What if entire pool goes down?
- With erasure coding, can you afford to lose N/2 volumes?
- Ensure critical data has redundancy across pools

### 3. Label Nodes Appropriately

Use clear, consistent node labels:
```bash
# Good
kubectl label node <node> storage-type=nvme
kubectl label node <node> instance-lifecycle=spot
kubectl label node <node> topology.kubernetes.io/region=us-east-1

# Avoid
kubectl label node <node> type=1  # Unclear
```

### 4. Use Topology Spread for High Availability

Distribute pool pods across failure domains:
```yaml
topologySpreadConstraints:
  - maxSkew: 1
    topologyKey: topology.kubernetes.io/zone
    whenUnsatisfiable: DoNotSchedule
    labelSelector:
      matchLabels:
        rustfs.pool: my-pool-name
```

### 5. Monitor Pool Health Separately

```bash
# Check pods per pool
kubectl get pods -l rustfs.pool=nvme-pool
kubectl get pods -l rustfs.pool=ssd-pool
kubectl get pods -l rustfs.pool=hdd-pool

# Check distribution
kubectl get pods -l rustfs.tenant=my-tenant -o wide
```

## Limitations

### Current Limitations

1. **No Per-Pool Status**: Status tracking is tenant-level only
2. **Shared Services**: All pools share same IO/Console services
3. **Single RUSTFS_VOLUMES**: All pools in one environment variable
4. **No Dynamic Pool Addition**: Must update tenant spec (no hot-add)

### Design Constraints

1. **Erasure Coding**: Each pool must meet 4-volume minimum
2. **StatefulSet Per Pool**: Each pool creates separate StatefulSet
3. **Shared Headless Service**: All pools use same headless service for DNS
4. **Unified Cluster**: RustFS treats all pools as one cluster

## Troubleshooting

### Pool Pods Not Scheduling

**Symptom**: Pods stuck in Pending state

**Check**:
```bash
kubectl describe pod <pod-name>
# Look for: "0/N nodes are available: N node(s) didn't match Node-Selector"
```

**Solution**: Verify node labels match pool's nodeSelector

### Uneven Distribution

**Symptom**: All pods in one zone

**Check**:
```bash
kubectl get pods -l rustfs.pool=my-pool -o wide
```

**Solution**: Add topology spread constraints to pool

### Resource Starvation

**Symptom**: Some pools not getting resources

**Check**:
```bash
kubectl describe nodes
# Look for resource pressure
```

**Solution**: Set appropriate resource requests per pool

## Related Documentation

- [Pool Configuration](../docs/pool-configuration.md)
- [Hardware-Targeted Example](../examples/hardware-pools-tenant.yaml)
- [Geographic Distribution Example](../examples/geographic-pools-tenant.yaml)
- [Spot Instance Example](../examples/spot-instance-tenant.yaml)

---

**Version**: v0.2.0 (with pool scheduling fields)
**Last Updated**: 2025-11-08
