<!--
Copyright 2025 RustFS Team

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# RustFS Operator User Guide

This guide is a technical manual for installing, configuring, and operating the RustFS Kubernetes Operator.

Chinese version: [operator-user-guide.zh-CN.md](operator-user-guide.zh-CN.md)

## 1. Overview

RustFS Operator manages RustFS object storage clusters on Kubernetes. Users describe the desired storage cluster with a namespaced `Tenant` custom resource, and the operator reconciles Kubernetes resources needed to run RustFS.

The operator provides:

- `Tenant` CRD (`rustfs.com/v1alpha1`) for declaring RustFS pools, persistence, scheduling, credentials, TLS, logging, encryption, and bootstrap provisioning.
- Controller reconciliation for Tenant-owned RBAC, Services, StatefulSets, PVC templates, status conditions, and Kubernetes Events.
- Helm chart under `deploy/rustfs-operator/` for production-style installation.
- Operator Console API and UI for management workflows.
- Optional operator STS endpoint for workload identity based temporary RustFS credentials.
- Optional tenant provisioning for canned policies, regular users, and buckets.
- Metrics, health probes, and optional Prometheus Operator resources.

Important service separation:

| Component | Purpose | Default port |
|-----------|---------|--------------|
| RustFS S3 API inside a Tenant | S3-compatible object storage access | `9000` |
| RustFS Tenant Console | Web console for one RustFS Tenant | `9001` |
| Operator Console API/UI | Operator management API and UI | `9090` |
| Operator STS | Temporary credentials endpoint | `4223` |
| Operator observability endpoint | `/metrics`, `/healthz`, `/readyz` | `8080` |

## 2. Architecture Model

A `Tenant` is one RustFS cluster. A Tenant can contain one or more pools, but all pools in the same Tenant form one unified RustFS cluster. Do not use pools as hot/warm/cold storage tiers. If you need separate performance, isolation, lifecycle, or administrative boundaries, create separate Tenants.

When a Tenant is applied, the operator creates and owns:

- one ServiceAccount, Role, and RoleBinding when Tenant RBAC is enabled;
- one headless Service named `{tenant}-hl` for StatefulSet peer DNS;
- one S3 Service named `{tenant}-io` on port `9000`;
- one Tenant Console Service named `{tenant}-console` on port `9001`;
- one StatefulSet per pool;
- PVC templates named `vol-0`, `vol-1`, and so on;
- generated RustFS environment variables such as `RUSTFS_VOLUMES`, `RUSTFS_ADDRESS`, `RUSTFS_CONSOLE_ADDRESS`, and `RUSTFS_CONSOLE_ENABLE`.

## 3. Prerequisites

- Kubernetes v1.30 or newer.
- Helm 3.0 or newer for chart installation.
- A StorageClass that can satisfy the Tenant PVCs.
- `kubectl` configured for the target cluster.
- Access to the configured operator and RustFS images.
- Optional: Prometheus Operator when enabling `ServiceMonitor` or `PrometheusRule`.
- Optional: cert-manager when using cert-manager managed Tenant TLS.

## 4. Install the Operator

Install with the included Helm chart:

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace
```

Verify the operator and Console pods:

```bash
kubectl get pods -n rustfs-system
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=operator \
  -f
```

Upgrade an existing installation:

```bash
helm upgrade rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system
```

Uninstall:

```bash
helm uninstall rustfs-operator --namespace rustfs-system
```

## 5. Helm Configuration

Use a values file for repeatable installation:

```bash
helm upgrade --install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace \
  -f values.yaml
```

Common chart sections:

| Section | Purpose |
|---------|---------|
| `operator` | Operator Deployment replicas, image, resources, probes, metrics, scheduling, leader election, and tenant monitoring. |
| `sts` | Operator STS endpoint, service port, TokenReview audience, and TLS handling. |
| `serviceAccount` / `rbac` | Operator ServiceAccount and RBAC creation. |
| `console` | Operator Console backend/UI Deployment, service, session cookie secret, ingress, resources, and optional split frontend. |
| `namespace` | Namespace override for chart resources; defaults to the Helm release namespace. |
| `commonLabels` / `commonAnnotations` | Labels and annotations added to chart-managed resources. |

Example production-oriented values:

```yaml
operator:
  replicas: 2
  image:
    repository: registry.example.com/rustfs/operator
    tag: v0.1.0
  resources:
    requests:
      cpu: 200m
      memory: 256Mi
    limits:
      cpu: 1000m
      memory: 1Gi
  tenantMonitor:
    enabled: true
    intervalSeconds: 300
  serviceMonitor:
    enabled: true

console:
  enabled: true
  replicas: 2
  jwtSecret: "<stable-base64-or-random-secret>"
  ingress:
    enabled: true
    className: nginx
    hosts:
      - host: console.example.com

sts:
  enabled: true
  audience: sts.rustfs.com
  tls:
    enabled: true
    auto: true
```

Notes:

- `operator.leaderElect` can be unset. The chart enables leader election automatically when `operator.replicas > 1`.
- Keep `console.jwtSecret` stable when running multiple Console replicas. If unset, the chart generates or reuses a Secret.
- Keep `CONSOLE_COOKIE_SECURE` enabled for production HTTPS. Only disable it for local HTTP testing.
- `sts.tls.auto=true` lets the operator create the `sts-tls` Secret when missing.

## 6. Create a Tenant

A minimal development Tenant:

```yaml
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: dev-minimal
  namespace: default
spec:
  image: rustfs/rustfs:latest
  pools:
    - name: dev-pool
      servers: 1
      persistence:
        volumesPerServer: 1
```

Apply and verify:

```bash
kubectl apply -f tenant.yaml
kubectl get tenant dev-minimal
kubectl get pods,pvc,svc -l rustfs.tenant=dev-minimal
```

Wait for pods:

```bash
kubectl wait --for=condition=ready pod \
  -l rustfs.tenant=dev-minimal \
  --timeout=300s
```

Access the Tenant S3 API:

```bash
kubectl port-forward svc/dev-minimal-io 9000:9000
```

Access the Tenant Console:

```bash
kubectl port-forward svc/dev-minimal-console 9001:9001
```

Use examples in `examples/` as starting points:

| Example | Use case |
|---------|----------|
| `examples/minimal-dev-tenant.yaml` | Smallest valid development Tenant. |
| `examples/secret-credentials-tenant.yaml` | Secret-based admin credentials. |
| `examples/provisioning-tenant.yaml` | Bootstrap policies, users, and buckets. |
| `examples/production-ha-tenant.yaml` | High-availability production-style layout. |
| `examples/multi-pool-tenant.yaml` | Multiple pools in one unified Tenant cluster. |
| `examples/custom-rbac-tenant.yaml` | Custom ServiceAccount and RBAC patterns. |

## 7. Tenant Configuration Reference

### 7.1 Tenant Identity

Tenant names must be DNS-1035 compatible and no longer than 55 characters because the operator derives Service names such as `{tenant}-console`.

Use lowercase names that start with a letter and contain only lowercase letters, digits, and `-`.

### 7.2 Pool Configuration

`spec.pools` is required. Each pool creates one StatefulSet.

Key fields:

| Field | Purpose |
|-------|---------|
| `name` | Pool name used in labels, StatefulSet names, and peer DNS. Must be unique in the Tenant. |
| `servers` | Number of RustFS pods in the pool. Must be greater than `0`. Immutable after creation. |
| `persistence.volumesPerServer` | Number of PVCs mounted into each server. Must be greater than `0`. Immutable after creation. |
| `persistence.volumeClaimTemplate` | PVC spec used for each generated volume. Set storage size, access modes, and StorageClass here. |
| `persistence.path` | Base mount path. Defaults to `/data`; mounted paths become `{path}/rustfs0`, `{path}/rustfs1`, and so on. |
| `nodeSelector`, `affinity`, `tolerations`, `topologySpreadConstraints` | Pool-level scheduling controls. |
| `resources` | Container resource requests and limits for the pool. |
| `priorityClassName` | Pool-level priority class override. |

Operator admission checks:

- `servers` and `persistence.volumesPerServer` must be greater than `0`.
- Pool names must be unique.
- Pool peer DNS labels must fit Kubernetes DNS label limits.
- Existing pool `servers` and `volumesPerServer` cannot be changed in place.

The operator does not validate whether a RustFS storage layout, erasure set size, or storage class parity is supported. RustFS performs those checks when the Tenant workload starts.

Example:

```yaml
spec:
  pools:
    - name: pool-0
      servers: 4
      persistence:
        volumesPerServer: 4
        volumeClaimTemplate:
          accessModes: ["ReadWriteOnce"]
          resources:
            requests:
              storage: 100Gi
          storageClassName: fast-ssd
      resources:
        requests:
          cpu: "2"
          memory: 8Gi
        limits:
          cpu: "4"
          memory: 16Gi
```

### 7.3 Credentials

For production, use `spec.credsSecret`. The Secret must be in the same namespace as the Tenant and contain UTF-8 `accesskey` and `secretkey` keys. Both values must be at least 8 characters.

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-admin-creds
  namespace: storage
type: Opaque
stringData:
  accesskey: "replace-with-access-key"
  secretkey: "replace-with-secret-key"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  credsSecret:
    name: rustfs-admin-creds
  pools:
    - name: pool-0
      servers: 2
      persistence:
        volumesPerServer: 2
```

Credential priority:

1. `spec.credsSecret`.
2. Explicit `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY` in `spec.env`.
3. RustFS built-in defaults. Use defaults only for development.

### 7.4 Workload Settings

Useful Tenant-level fields:

| Field | Purpose |
|-------|---------|
| `image` | RustFS server image. Defaults to the operator's configured fallback. |
| `imagePullSecret` | Image pull Secret reference. |
| `imagePullPolicy` | RustFS image pull policy. |
| `scheduler` | Custom scheduler name. |
| `env` | Additional RustFS container environment variables. Do not override operator-managed variables. |
| `serviceAccountName` | Custom ServiceAccount for RustFS pods. |
| `createServiceAccountRbac` | Whether the operator should create Role/RoleBinding for the Tenant ServiceAccount. |
| `priorityClassName` | Tenant-level priority class. |
| `lifecycle` | Kubernetes container lifecycle hooks. |
| `podManagementPolicy` | StatefulSet pod management policy. |
| `podDeletionPolicyWhenNodeIsDown` | Node-down pod deletion behavior. |
| `securityContext` | Pod SecurityContext override for RustFS pods. |

The operator reserves these environment variables and manages them automatically:

- `RUSTFS_VOLUMES`
- `RUSTFS_ADDRESS`
- `RUSTFS_CONSOLE_ADDRESS`
- `RUSTFS_CONSOLE_ENABLE`
- `RUSTFS_KMS_*` variables; use `spec.encryption` instead.
- TLS-related RustFS variables when Tenant TLS is enabled.

For a single-pool single-node single-disk Tenant, `RUSTFS_VOLUMES` is rendered as the local data path, for example `/data/rustfs0`. Multi-pool tenants and other layouts render peer DNS URLs through the Tenant headless Service and are validated by RustFS at runtime.

`podDeletionPolicyWhenNodeIsDown` accepts:

- `DoNothing`: do not delete pods automatically.
- `Delete`: request a normal pod delete.
- `ForceDelete`: force delete the pod with `gracePeriodSeconds=0`.
- `DeleteStatefulSetPod`: Longhorn-compatible force delete for StatefulSet pods stuck on down nodes.
- `DeleteDeploymentPod`: Longhorn-compatible force delete for Deployment pods stuck on down nodes.
- `DeleteBothStatefulSetAndDeploymentPod`: Longhorn-compatible force delete for both StatefulSet and Deployment pods.

Force deletion can have data consistency implications. Use it only when the storage backend and operational procedure are designed for that failure mode. Force deletion requires the Node object to be deleted or marked with an effective `node.kubernetes.io/out-of-service` taint that the target Pod does not tolerate, so volume detach fencing is explicit.

### 7.5 TLS

Tenant TLS is configured under `spec.tls`.

Important fields:

| Field | Purpose |
|-------|---------|
| `mode` | `disabled` or `certManager` for current usable configurations. `external` is reserved and currently blocks reconciliation. |
| `mountPath` | TLS mount path. Defaults to `/var/run/rustfs/tls`. |
| `rotationStrategy` | `Rollout` is supported. `HotReload` is accepted by the CRD but currently blocks reconciliation. |
| `enableInternodeHttps` | Use HTTPS for RustFS peer communication. |
| `requireSanMatch` | Require generated DNS names to match certificate SANs. Defaults to `true`. |
| `certManager` | Certificate settings when using cert-manager. `secretName` is required for `mode: certManager`. |

For cert-manager managed certificates:

```yaml
spec:
  tls:
    mode: certManager
    rotationStrategy: Rollout
    enableInternodeHttps: true
    certManager:
      manageCertificate: true
      secretName: rustfs-a-server-tls
      issuerRef:
        group: cert-manager.io
        kind: Issuer
        name: rustfs-issuer
      includeGeneratedDnsNames: true
```

When `manageCertificate: true`, `issuerRef` is also required. The operator creates or reconciles the cert-manager `Certificate`, waits for the referenced Secret, validates `tls.crt` and `tls.key`, and uses `ca.crt` unless another CA trust source is configured.

### 7.6 Logging

Tenant logging is configured under `spec.logging`.

Modes:

| Mode | Purpose |
|------|---------|
| `stdout` | Default and recommended. Kubernetes collects logs from stdout/stderr. |
| `emptyDir` | Temporary local log storage for debugging. Logs are lost on pod restart. |
| `persistent` | PVC-backed logs. Use only with external storage independent of RustFS. |

Do not store RustFS startup logs in RustFS itself. That creates a circular dependency because the storage service is not available during startup.

Example:

```yaml
spec:
  logging:
    mode: stdout
```

### 7.7 Encryption / KMS

Tenant encryption is configured under `spec.encryption`.

Supported backends:

| Backend | Purpose |
|---------|---------|
| `local` | File-based local KMS key directory and local master key. The directory must be absolute, must be in a subdirectory under a RustFS data PVC mount, and the Tenant must have exactly one RustFS server replica across all pools. |
| `vault` | HashiCorp Vault endpoint. Requires a Secret containing `vault-token`. |

Local KMS does not use `kmsSecret`; if you set one, it is ignored. Configure the local master key with `spec.encryption.local.masterKeySecretRef`, which maps to RustFS `RUSTFS_KMS_LOCAL_MASTER_KEY`. By default, Local KMS stores key files under the first data PVC mount, for example `/data/rustfs0/.kms-keys` when `persistence.path` is `/data`. Use Vault KMS for multi-server Tenants. `allowInsecureDevDefaults: true` maps to `RUSTFS_KMS_ALLOW_INSECURE_DEV_DEFAULTS=true` and should only be used for development because RustFS may store Local KMS key material as plaintext-dev-only.

Upgrade note: older operator versions defaulted Local KMS to `/data/kms-keys`, which was not on a data PVC. The operator does not migrate key files automatically. For an existing Local KMS Tenant that still uses the old implicit default, or that explicitly set the legacy path, the operator blocks reconciliation or StatefulSet rollout until you copy any existing key files and `.master-key.salt` into the new PVC-backed directory, keep using the same local master key Secret, then set `spec.encryption.local.keyDirectory` explicitly to that subdirectory.

Local example:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-local-kms
  namespace: storage
type: Opaque
stringData:
  local-master-key: "replace-with-a-strong-local-kms-master-key"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-local
  namespace: storage
spec:
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
  encryption:
    enabled: true
    backend: local
    local:
      keyDirectory: /data/rustfs0/.kms-keys
      masterKeySecretRef:
        name: rustfs-local-kms
        key: local-master-key
    defaultKeyId: tenant-default
```

Vault example:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-kms
  namespace: storage
type: Opaque
stringData:
  vault-token: "replace-with-vault-token"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  pools:
    - name: pool-0
      servers: 2
      persistence:
        volumesPerServer: 2
  encryption:
    enabled: true
    backend: vault
    vault:
      endpoint: https://vault.example.com:8200
    kmsSecret:
      name: rustfs-kms
    defaultKeyId: tenant-default
```

### 7.8 Bootstrap Provisioning

The operator can create RustFS policies, users, and buckets after the Tenant workload is ready. Configure:

- `spec.credsSecret` for RustFS admin credentials.
- `spec.policies` for canned policies sourced from ConfigMaps.
- `spec.users` for regular users. Each user must have at least one direct policy mapping.
- `spec.buckets` for buckets and optional object lock.

ConfigMaps and user Secrets must live in the Tenant namespace. If managed outside the Operator Console, label them with `rustfs.tenant=<tenant-name>` so updates enqueue the owning Tenant.

For each `spec.users[]` entry, the operator reads a Secret with the same name as the user. The Secret must contain `accesskey` and `secretkey`, or the MinIO-compatible keys `CONSOLE_ACCESS_KEY` and `CONSOLE_SECRET_KEY`. If both key formats are present, their values must match. User access keys must be at least 8 characters and must not contain whitespace, `=`, or `,`; user secret keys must be at least 8 characters.

Updating a user Secret's `secretkey` rotates that RustFS user's credential. The `accesskey` is immutable after the first successful reconciliation; use a new user entry and Secret when it must change, then migrate clients before removing the old entry.

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-policy
  namespace: storage
  labels:
    rustfs.tenant: rustfs-a
data:
  policy.json: |
    {
      "Version": "2012-10-17",
      "Statement": [
        {
          "Effect": "Allow",
          "Action": ["s3:ListBucket", "s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
          "Resource": ["arn:aws:s3:::app-data", "arn:aws:s3:::app-data/*"]
        }
      ]
    }
---
apiVersion: v1
kind: Secret
metadata:
  name: app-user
  namespace: storage
  labels:
    rustfs.tenant: rustfs-a
type: Opaque
stringData:
  accesskey: appuser01
  secretkey: appuser01secret
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  credsSecret:
    name: rustfs-admin-creds
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 4
  policies:
    - name: app-readwrite
      document:
        configMapKeyRef:
          name: app-policy
          key: policy.json
  users:
    - name: app-user
      policies:
        - app-readwrite
  buckets:
    - name: app-data
      objectLock: true
```

Deletion behavior is conservative: provisioned resources are retained when removed from the Tenant spec.

### 7.9 Pool Lifecycle

`spec.poolLifecycle` controls explicit pool lifecycle requests. The current PVC retention policy is `Retain`.

Example decommission request:

```yaml
spec:
  poolLifecycle:
    pvcRetentionPolicy: Retain
    decommissionRequests:
      - poolName: pool-old
        requestId: decommission-pool-old-20250623
        action: Start
        reason: "capacity migrated to pool-new"
```

Use pool lifecycle operations carefully. Keep a backup and verify RustFS-level decommission behavior before removing capacity.

## 8. Operator Console

The Helm chart enables the Operator Console by default with `console.enabled=true`.

Recommended same-origin deployment:

```yaml
console:
  enabled: true
  ingress:
    enabled: true
    className: nginx
    hosts:
      - host: console.example.com
```

The unified operator image serves both `/` and `/api/v1` from the Console service. No backend CORS configuration is needed for this mode.

Console login uses a Kubernetes ServiceAccount bearer token. For the chart-managed Console ServiceAccount:

```bash
kubectl -n rustfs-system create token rustfs-operator-console --duration=24h
```

Paste the token into the login form. The Console stores the validated token in an encrypted session cookie.

For local port-forward testing:

```bash
kubectl -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090
```

Open `http://127.0.0.1:19090`.

## 9. Operator STS

The operator STS endpoint lets a Kubernetes workload exchange a projected ServiceAccount token for temporary RustFS credentials, authorized by a `PolicyBinding`.

STS route:

```text
POST /sts/{tenantNamespace}/{tenantName}
```

Create a `PolicyBinding` in the target Tenant namespace:

```yaml
apiVersion: sts.rustfs.com/v1alpha1
kind: PolicyBinding
metadata:
  name: reports-readonly
  namespace: storage
spec:
  application:
    namespace: reports
    serviceaccount: reports-api
  policies:
    - readonly
```

The workload ServiceAccount token audience must match `sts.audience`, which defaults to `sts.rustfs.com`.

```yaml
volumes:
  - name: rustfs-sts-token
    projected:
      sources:
        - serviceAccountToken:
            path: token
            audience: sts.rustfs.com
            expirationSeconds: 3600
```

Call STS from the workload:

```bash
TOKEN="$(cat /var/run/secrets/rustfs-sts/token)"

curl -sS -X POST \
  --cacert /var/run/secrets/rustfs-sts-ca/ca.crt \
  "https://rustfs-operator-sts.rustfs-system.svc:4223/sts/storage/rustfs-a" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  --data-urlencode "Version=2011-06-15" \
  --data-urlencode "Action=AssumeRoleWithWebIdentity" \
  --data-urlencode "WebIdentityToken=${TOKEN}" \
  --data-urlencode "DurationSeconds=3600"
```

Current STS constraints:

- STS only issues credentials for TLS-enabled Tenants.
- Operator STS uses the explicit Tenant route with both namespace and name.
- The `PolicyBinding` must reference at least one policy.
- Every policy referenced by the matched `PolicyBinding` must resolve to a valid RustFS policy document.
- Caller-supplied `Policy` request parameters are rejected; issued credentials use the matched `PolicyBinding` policies.
- Tenants requiring client certificates for upstream Tenant calls are rejected by Operator STS.

## 10. Monitoring and Status

Check Tenant status:

```bash
kubectl get tenant -A
kubectl describe tenant -n <namespace> <tenant>
```

The operator reports `status.currentState` values such as:

- `Ready`
- `Reconciling`
- `Blocked`
- `Degraded`
- `NotReady`
- `Unknown`

Important conditions include:

- `Ready`
- `Reconciling`
- `Degraded`
- `SpecValid`
- `CredentialsReady`
- `KmsReady`
- `TlsReady`
- `PoolsReady`
- `WorkloadsReady`
- `ProvisioningReady`

Check chart-managed observability:

```bash
kubectl -n rustfs-system port-forward svc/rustfs-operator-metrics 18080:8080
curl http://127.0.0.1:18080/healthz
curl http://127.0.0.1:18080/readyz
curl http://127.0.0.1:18080/metrics
```

Enable Prometheus Operator integration:

```yaml
operator:
  serviceMonitor:
    enabled: true
  prometheusRule:
    enabled: true
```

## 11. Operations

### Change RustFS Image

```yaml
spec:
  image: rustfs/rustfs:v1.0.0
```

The operator reconciles StatefulSets and reports rollout status in Tenant conditions and pool status.

### Change Storage Capacity

PVC expansion depends on the StorageClass and Kubernetes environment. Do not change immutable pool shape fields (`servers` and `volumesPerServer`) in place. To add capacity, add a new pool when appropriate and follow RustFS decommission and migration procedures.

### Restart Tenant Pods

Use Kubernetes primitives:

```bash
kubectl rollout restart statefulset -n <namespace> -l rustfs.tenant=<tenant>
kubectl rollout status statefulset -n <namespace> -l rustfs.tenant=<tenant>
```

### Rotate Admin Credentials

Update the referenced Secret and restart Tenant StatefulSets so pods consume the new Secret values:

```bash
kubectl create secret generic rustfs-admin-creds \
  -n <namespace> \
  --from-literal=accesskey=<new-access-key> \
  --from-literal=secretkey=<new-secret-key> \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl rollout restart statefulset -n <namespace> -l rustfs.tenant=<tenant>
```

## 12. Troubleshooting

### Tenant is Blocked

```bash
kubectl describe tenant -n <namespace> <tenant>
kubectl get events -n <namespace> --sort-by=.lastTimestamp
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=operator
```

Common blocked reasons:

| Reason | Check |
|--------|-------|
| `InvalidTenantName` | Tenant name length and DNS-1035 format. |
| `InvalidPoolSpec` | Pool count, total volume count, pool name, and immutable fields. |
| `CredentialSecretNotFound` | Secret exists in the Tenant namespace. |
| `CredentialSecretMissingKey` | Secret contains `accesskey` and `secretkey`. |
| `CredentialSecretTooShort` | Both credential values are at least 8 characters. |
| `KmsSecretNotFound` / `KmsSecretMissingKey` | KMS Secret exists and contains required keys, such as Vault `vault-token` or the Local KMS `masterKeySecretRef.key`. |
| `CertManagerCrdMissing` / `CertManagerIssuerNotFound` | cert-manager is installed and the issuer exists. |
| `StatefulSetUpdateValidationFailed` | An immutable StatefulSet or pool-shape field was changed. |
| `ProvisioningFailed` | Check `status.provisioning`, policy ConfigMaps, user Secrets, and RustFS admin credentials. |

### Pods are not Ready

```bash
kubectl get pods -n <namespace> -l rustfs.tenant=<tenant>
kubectl describe pod -n <namespace> -l rustfs.tenant=<tenant>
kubectl logs -n <namespace> -l rustfs.tenant=<tenant>
```

Check PVC binding, StorageClass availability, image pull errors, node selectors, tolerations, and resource requests.

### S3 API is not reachable

Verify the Tenant S3 service and endpoints:

```bash
kubectl get svc,endpoints -n <namespace> <tenant>-io
kubectl port-forward -n <namespace> svc/<tenant>-io 9000:9000
```

### Console login fails

For the Operator Console, verify the ServiceAccount token and Console logs:

```bash
kubectl -n rustfs-system create token rustfs-operator-console --duration=24h
kubectl logs -n rustfs-system \
  -l app.kubernetes.io/name=rustfs-operator,app.kubernetes.io/component=console
```

For the RustFS Tenant Console, use the Tenant admin credentials from `spec.credsSecret` or configured RustFS environment variables.

## 13. Best Practices

- Use `spec.credsSecret` or an external secret manager for production credentials.
- Enable Kubernetes Secret encryption at rest.
- Use one StorageClass performance class within a Tenant unless you have a deliberate RustFS layout reason.
- Do not model hot/warm/cold tiers as pools inside one Tenant.
- Use separate Tenants for separate clusters, administrative boundaries, or performance isolation.
- Keep the Operator Console on HTTPS in production.
- Keep `console.jwtSecret` stable for multi-replica Console deployments.
- Use `ServiceMonitor` and `PrometheusRule` only when Prometheus Operator is installed.
- Keep Tenant examples under version control, but never commit raw Secret values.
- Check `status.conditions` before debugging lower-level StatefulSets.

## 14. Related Documentation

- [Project README](../README.md)
- [Deployment guide](../deploy/README.md)
- [Helm chart README](../deploy/rustfs-operator/README.md)
- [Tenant examples](../examples/README.md)
- [Console frontend README](../console-web/README.md)
