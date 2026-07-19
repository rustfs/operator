# RustFS Operator Helm Chart

Helm chart for deploying the RustFS Kubernetes operator.

## Prerequisites

- Kubernetes v1.30+
- Helm 3.0+

## Installing the Chart

To install the chart with the release name `rustfs-operator`:

```bash
helm install rustfs-operator deploy/rustfs-operator/
```

To install in a specific namespace:

```bash
helm install rustfs-operator deploy/rustfs-operator/ --namespace rustfs-system --create-namespace
```

## Uninstalling the Chart

To uninstall/delete the `rustfs-operator` deployment:

```bash
helm uninstall rustfs-operator --namespace rustfs-system
```

## Configuration

The following table lists the configurable parameters of the RustFS Operator chart and their default values.

### Operator Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `operator.replicas` | Number of operator replicas | `1` |
| `operator.image.repository` | Operator image repository | `rustfs/operator` |
| `operator.image.tag` | Operator image tag | `latest` |
| `operator.image.pullPolicy` | Image pull policy | `IfNotPresent` |
| `operator.imagePullSecrets` | Image pull secrets | `[]` |
| `operator.leaderElect` | Enable leader election override (`null`/unset for auto by replicas) | `null` |
| `operator.resources.requests.cpu` | CPU resource requests | `100m` |
| `operator.resources.requests.memory` | Memory resource requests | `128Mi` |
| `operator.resources.limits.cpu` | CPU resource limits | `500m` |
| `operator.resources.limits.memory` | Memory resource limits | `512Mi` |
| `operator.metrics.enabled` | Enable operator `/metrics`, `/healthz`, and `/readyz` endpoint | `true` |
| `operator.metrics.port` | Operator observability container port | `8080` |
| `operator.serviceMonitor.enabled` | Create a Prometheus Operator ServiceMonitor | `false` |
| `operator.prometheusRule.enabled` | Create Prometheus alert rules for operator and tenant storage health | `false` |
| `operator.tenantMonitor.enabled` | Poll RustFS tenant storage health and capacity metrics | `true` |
| `operator.tenantMonitor.intervalSeconds` | Tenant storage monitor interval | `300` |
| `clusterDomain` | Kubernetes cluster DNS domain used for Tenant peer URLs, generated TLS SANs, and operator STS auto TLS | `cluster.local` |
| `operator.env` | Environment variables | `[{name: RUST_LOG, value: info}]` |
| `operator.nodeSelector` | Node selector for pod placement | `{}` |
| `operator.tolerations` | Tolerations for pod scheduling | `[]` |
| `operator.affinity` | Affinity rules for pod scheduling | `{}` |

Chart-managed environment variables must not be duplicated in `operator.env`. Configure
`OPERATOR_CLUSTER_DOMAIN`, `OPERATOR_NAMESPACE`, and the chart-managed `OPERATOR_STS_*` settings
through their documented chart values so the Deployment, Service, generated certificate, and RBAC
manifests remain consistent.

### Operator STS Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `sts.enabled` | Enable the operator STS endpoint | `true` |
| `sts.audience` | Kubernetes TokenReview audience expected by the operator STS endpoint | `sts.rustfs.com` |
| `sts.port` | Operator container port for STS | `4223` |
| `sts.tls.enabled` | Serve the operator STS endpoint over TLS | `true` |
| `sts.tls.auto` | Create and replace an invalid Operator-managed STS TLS Secret with namespaced write RBAC | `true` |
| `sts.service.type` | Kubernetes Service type for STS | `ClusterIP` |
| `sts.service.port` | Kubernetes Service port for STS | `4223` |

The RustFS operator STS endpoint intentionally uses an explicit Tenant route:

```text
POST /sts/{tenantNamespace}/{tenantName}
```

This differs from MinIO Operator's namespace-only route. A `PolicyBinding` still lives in the Tenant namespace, but the workload must call STS with both the Tenant namespace and the Tenant name.

The STS service is HTTPS by default. When `sts.tls.auto=true`, the operator creates the fixed `sts-tls` Secret in the operator namespace with `tls.crt`, `tls.key`, and `ca.crt`. With `rbac.create=true`, the chart creates a namespaced Role that can create Secrets and update only `sts-tls`; the ClusterRole keeps all Secret and ConfigMap access read-only. Workloads must trust that CA. To use an externally issued certificate, pre-create `sts-tls` with a certificate signed by a CA already trusted by the workload and set `sts.tls.auto=false`; the chart then omits the namespaced Secret write Role.

STS only issues credentials for TLS-enabled Tenants. For Tenant upstream calls, the operator selects the Tenant HTTPS service endpoint and trusts the CA recorded in `status.certificates.tls.caSecretRef`.

Operator STS does not present a client certificate when calling the Tenant. Tenants configured with `spec.tls.certManager.caTrust.clientCaSecretRef` continue to run with server-side mTLS enabled, but Operator STS rejects those Tenants with HTTP 400 and `TenantTlsClientCertificateUnsupported`.

When `operator.serviceMonitor.enabled=true`, the chart creates scrape targets for both the operator observability endpoint and the Console API `/metrics` endpoint.

### Tenant RPC Authentication

Use `spec.rpcSecret` to keep RustFS internode RPC authentication independent from
the administrator credentials in `spec.credsSecret`:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-rpc-auth
  namespace: storage
type: Opaque
stringData:
  rpc-secret: "replace-with-a-dedicated-rpc-secret"
---
apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: rustfs-a
  namespace: storage
spec:
  credsSecret:
    name: rustfs-admin-creds
  rpcSecret:
    name: rustfs-rpc-auth
    key: rpc-secret
```

The operator maps the selected Secret key to `RUSTFS_RPC_SECRET` in every RustFS
Pod. Before applying workloads, it verifies that the Secret and selected key
exist and that the value is valid UTF-8, non-blank, contains no NUL bytes, and is
not the RustFS default credential value (`rustfsadmin`). Keep this value stable
while rotating administrator credentials. Secret updates enqueue every Tenant
whose spec references the Secret, including when multiple Tenants share it. A
Secret update does not change the environment of already-running Pods.
Coordinated restart and hot reload are outside this feature. If `spec.rpcSecret`
is omitted, the operator does not set
`RUSTFS_RPC_SECRET`, RustFS resolves it from its own credential configuration,
and the operator does not report `RpcAuthReady` for that unmanaged value.

### Tenant Provisioning

Tenants can declare RustFS canned policies, regular users, and buckets directly in `spec.policies`, `spec.users`, and `spec.buckets`. Provisioning starts only after the Tenant workload is ready, uses `spec.credsSecret` as the RustFS admin credential source, and reports progress under `status.provisioning`.

User provisioning requires a non-empty direct policy mapping:

```yaml
spec:
  credsSecret:
    name: rustfs-admin-creds
  policies:
    - name: app-readwrite
      document:
        configMapKeyRef:
          name: app-policy
          key: policy.json
  users:
    - name: app-user
      credsSecret:
        name: rustfs-user-app-user
      policies:
        - app-readwrite
  buckets:
    - name: app-data
      objectLock: true
```

Policy ConfigMaps and user Secrets must live in the Tenant namespace. `users[].credsSecret.name` selects the credentials Secret; when omitted, the operator falls back to a Secret named after `users[].name` for compatibility with existing manifests. The operator indexes references from Tenant specs, so creating or updating a referenced object enqueues every referencing Tenant without requiring or mutating labels or requiring write access to that object. Provisioned resources are retained when removed from the Tenant spec.

### RBAC Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `rbac.create` | Create RBAC resources | `true` |
| `serviceAccount.create` | Create service account | `true` |
| `serviceAccount.name` | Service account name | `""` (auto-generated) |
| `serviceAccount.annotations` | Service account annotations | `{}` |

The generated ClusterRole grants only `get`, `list`, and `watch` for Secrets and ConfigMaps. STS auto TLS write access is isolated to a Role in the operator namespace.

### Other Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `namespace` | Namespace to deploy to | `""` (uses release namespace) |
| `commonLabels` | Labels to add to all resources | `{}` |
| `commonAnnotations` | Annotations to add to all resources | `{}` |

## Examples

### Custom Image and Tag

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --set operator.image.repository=myregistry/operator \
  --set operator.image.tag=v0.2.0
```

### Increased Resources

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --set operator.resources.requests.cpu=200m \
  --set operator.resources.requests.memory=256Mi \
  --set operator.resources.limits.cpu=1000m \
  --set operator.resources.limits.memory=1Gi
```

### Leader Election for Helm Deployments

With the chart default behavior, `leaderElect` is automatically enabled when
`operator.replicas > 1` and disabled when `operator.replicas <= 1`:

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --set operator.replicas=3
```

Override explicitly if needed (for example, to force single-leader mode in all cases):

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --set operator.replicas=3 \
  --set operator.leaderElect=false
```

### Using a Values File

Create a custom `values.yaml`:

```yaml
operator:
  replicas: 2
  image:
    repository: myregistry/rustfs-operator
    tag: v0.2.0
  resources:
    requests:
      cpu: 200m
      memory: 256Mi
    limits:
      cpu: 1000m
      memory: 1Gi
  env:
    - name: RUST_LOG
      value: debug
  leaderElect:
```

Install with your custom values:

```bash
helm install rustfs-operator deploy/rustfs-operator/ -f custom-values.yaml
```

### STS PolicyBinding and Workload Token

Create a `PolicyBinding` in the target Tenant namespace. The binding authorizes one workload ServiceAccount to request temporary credentials for policies already defined in RustFS:

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

The workload should mount a projected ServiceAccount token with an audience matching `sts.audience`:

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: reports-api
  namespace: reports
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: reports-api
  namespace: reports
spec:
  replicas: 1
  selector:
    matchLabels:
      app: reports-api
  template:
    metadata:
      labels:
        app: reports-api
    spec:
      serviceAccountName: reports-api
      containers:
        - name: app
          image: example/reports-api:latest
          volumeMounts:
            - name: rustfs-sts-token
              mountPath: /var/run/secrets/rustfs-sts
              readOnly: true
      volumes:
        - name: rustfs-sts-token
          projected:
            sources:
              - serviceAccountToken:
                  path: token
                  audience: sts.rustfs.com
                  expirationSeconds: 3600
```

The workload then calls the operator STS service with the target Tenant namespace and Tenant name:

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

Operator STS derives the issued session policy from the matched `PolicyBinding` policies. Every referenced policy must exist and resolve to a valid RustFS policy document. Caller-supplied `Policy` request parameters are rejected until the operator can prove they only narrow the `PolicyBinding` permissions.

## Creating Tenant Resources

After installing the operator, you can create Tenant resources. See the project root `examples/` directory for sample manifests:

```bash
kubectl apply -f examples/simple-tenant.yaml
```

## Upgrading

To upgrade the operator:

```bash
kubectl apply --server-side --force-conflicts \
  --field-manager=rustfs-operator-crd-upgrade \
  -f deploy/rustfs-operator/crds/tenant.yaml
kubectl apply --server-side --force-conflicts \
  --field-manager=rustfs-operator-crd-upgrade \
  -f deploy/rustfs-operator/crds/policybinding-crd.yaml
helm upgrade rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system
```

Helm does not upgrade existing CRDs from a chart's `crds/` directory. Apply the
cluster-scoped CRDs first so the API server accepts fields introduced by the
new Operator version. The dedicated field manager deliberately takes ownership
of the chart-managed CRD fields, including CRDs originally created by Helm.

This release adds secure defaults to generated RustFS Pods and containers.
Existing compatible Tenants whose StatefulSet templates do not already contain
those values will roll on their next reconciliation. Schedule the upgrade in a
maintenance window: a single-replica Tenant can be unavailable during restart,
and a multi-replica Tenant temporarily runs with reduced capacity. Verify every
Tenant image first. Known incompatible images are blocked before rollout, and
mutable tags, digest references, or custom repositories are blocked under an
effective `RuntimeDefault` profile unless the Tenant carries an image-bound
acknowledgement. Before upgrade, either pin a verified RustFS beta.9-or-later
release tag, or verify the effective image and set
`operator.rustfs.com/runtime-default-image-ack` to that exact image reference:

```yaml
metadata:
  annotations:
    operator.rustfs.com/runtime-default-image-ack: "registry.example.com/rustfs/rustfs@sha256:<digest>"
spec:
  image: "registry.example.com/rustfs/rustfs@sha256:<digest>"
```

The annotation must change when the image reference changes and cannot override
a known-incompatible official alpha or beta.1 through beta.8 reference that is
not digest-qualified. For `tag@digest`, Kubernetes pulls by digest; after
verifying that exact digest, acknowledge the complete reference. Mutable tags can
change content without changing the annotation, so prefer an immutable digest
in production.

The built-in RustFS image fallback also changes from the mutable `latest` tag to
`rustfs/rustfs:1.0.0-beta.10`. Tenants without `spec.image` and without a
`TENANT_RUSTFS_IMAGE` Operator environment override roll to that pinned release
on reconciliation. Set `spec.image` explicitly to control future server upgrades.

Treat this as a one-way workload security migration. Once this Operator has
reconciled a Tenant, do not downgrade directly to a version that predates the
restricted defaults: restricted admission rejects the older workload template,
while clusters without that admission can roll back to weaker settings. Recover
by rolling forward to this version or a newer fixed version.

Existing manifests that omit `users[].credsSecret` remain compatible. Wait for
the new Operator rollout to complete before relying on an explicit user Secret
reference; older binaries continue using the same-name Secret convention.

## Console UI

The published `rustfs/operator` image contains both the Console backend (Rust API,
`/api/v1/*`) and the exported `console-web` static frontend. By default the chart
deploys one Console service that serves both `/` and `/api/v1` from the same pod,
so browser requests are same-origin and do not need CORS.

### Same-origin deployment (recommended)

Serve the Console service under **one HTTPS host**:

1. Enable the Console and Ingress in `values.yaml`:

   ```yaml
   console:
     enabled: true
     ingress:
       enabled: true
       className: nginx
       hosts:
         - host: console.example.com
   ```

2. Install/upgrade the chart. The Ingress routes `/` and `/api/v1` to the Console
   service. The embedded frontend is built with `NEXT_PUBLIC_API_BASE_URL=/api/v1`
   by default. If you intentionally test over plain HTTP, set
   `CONSOLE_COOKIE_SECURE=false` in `console.env`; do not use that setting for
   production.

No CORS configuration is needed on the backend for this setup.

Console sessions are encrypted stateless cookies. Users paste a Kubernetes
ServiceAccount bearer token only during login; after validation, the Console
stores that token inside an encrypted `session` cookie for later API requests.
If you run multiple Console replicas, keep `console.jwtSecret` stable and shared
across all replicas. The chart reuses the existing generated Secret on upgrade
when `console.jwtSecret` is not set.

### Backend CORS (when frontend is on a different host)

If the frontend is served from another host (e.g. `https://ui.example.com`) and the API at `https://api.example.com`, set allowed origins on the console backend:

```yaml
console:
  env:
    - name: CORS_ALLOWED_ORIGINS
      value: "https://ui.example.com"
    # Required when the frontend and API are cross-site, so browsers send the
    # encrypted session cookie on credentialed CORS requests.
    - name: CONSOLE_COOKIE_SAME_SITE
      value: "None"
```

Multiple origins (e.g. dev + prod): comma-separated, e.g. `"https://ui.example.com,http://localhost:3000"`.

### Legacy Split Frontend

`console.frontend.enabled=true` still deploys a separate `console-web` image for
installations that intentionally keep frontend and backend images separate. In
that mode the Ingress routes `/api` to the Console backend and `/` to the split
frontend service.

### Console login token

The Console login form expects a Kubernetes ServiceAccount bearer token. For
the chart-managed Console ServiceAccount, generate a short-lived token with:

```bash
kubectl -n rustfs-system create token rustfs-operator-console --duration=24h
```

Paste the printed token into the Console login form. Use the namespace and
ServiceAccount name from your Helm release if they differ from the defaults;
the Helm install notes print the exact command for the deployed release.

## Verifying the Installation

Check that the operator is running:

```bash
kubectl get pods -n rustfs-system -l app.kubernetes.io/name=rustfs-operator
```

View operator logs:

```bash
kubectl logs -n rustfs-system -l app.kubernetes.io/name=rustfs-operator -f
```
