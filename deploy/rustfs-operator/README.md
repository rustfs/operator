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
| `operator.resources.requests.cpu` | CPU resource requests | `100m` |
| `operator.resources.requests.memory` | Memory resource requests | `128Mi` |
| `operator.resources.limits.cpu` | CPU resource limits | `500m` |
| `operator.resources.limits.memory` | Memory resource limits | `512Mi` |
| `operator.env` | Environment variables | `[{name: RUST_LOG, value: info}]` |
| `operator.nodeSelector` | Node selector for pod placement | `{}` |
| `operator.tolerations` | Tolerations for pod scheduling | `[]` |
| `operator.affinity` | Affinity rules for pod scheduling | `{}` |

### Operator STS Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `sts.enabled` | Enable the operator STS endpoint | `true` |
| `sts.audience` | Kubernetes TokenReview audience expected by the operator STS endpoint | `sts.rustfs.com` |
| `sts.port` | Operator container port for STS | `4223` |
| `sts.tls.enabled` | Serve the operator STS endpoint over TLS | `true` |
| `sts.tls.auto` | Create the operator STS TLS Secret when missing | `true` |
| `sts.service.type` | Kubernetes Service type for STS | `ClusterIP` |
| `sts.service.port` | Kubernetes Service port for STS | `4223` |

The RustFS operator STS endpoint intentionally uses an explicit Tenant route:

```text
POST /sts/{tenantNamespace}/{tenantName}
```

This differs from MinIO Operator's namespace-only route. A `PolicyBinding` still lives in the Tenant namespace, but the workload must call STS with both the Tenant namespace and the Tenant name.

The STS service is HTTPS by default. When `sts.tls.auto=true`, the operator creates the fixed `sts-tls` Secret in the operator namespace with `tls.crt`, `tls.key`, and `ca.crt`. Workloads must trust that CA. To use an externally issued certificate, pre-create `sts-tls` with a certificate signed by a CA already trusted by the workload and set `sts.tls.auto=false`.

STS only issues credentials for TLS-enabled Tenants. For Tenant upstream calls, the operator selects the Tenant HTTPS service endpoint and trusts the CA recorded in `status.certificates.tls.caSecretRef`.

Operator STS does not present a client certificate when calling the Tenant. Tenants configured with `spec.tls.certManager.caTrust.clientCaSecretRef` continue to run with server-side mTLS enabled, but Operator STS rejects those Tenants with HTTP 400 and `TenantTlsClientCertificateUnsupported`.

### RBAC Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `rbac.create` | Create RBAC resources | `true` |
| `serviceAccount.create` | Create service account | `true` |
| `serviceAccount.name` | Service account name | `""` (auto-generated) |
| `serviceAccount.annotations` | Service account annotations | `{}` |

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

## Creating Tenant Resources

After installing the operator, you can create Tenant resources. See the project root `examples/` directory for sample manifests:

```bash
kubectl apply -f examples/simple-tenant.yaml
```

## Upgrading

To upgrade the operator:

```bash
helm upgrade rustfs-operator deploy/rustfs-operator/
```

## Console UI (Frontend + Backend in K8s)

The console has a **backend** (Rust API, `/api/v1/*`) and an optional **frontend** (static web app, `console-web`). To have the browser reach the API correctly when both run in Kubernetes:

### Same-origin deployment (recommended)

Serve the frontend and the API under **one host** so the browser sends requests to the same origin (no CORS, cookies work):

1. Enable the frontend and Ingress in `values.yaml`:

   ```yaml
   console:
     enabled: true
     frontend:
       enabled: true
       image:
         repository: your-registry/console-web
         tag: latest
     ingress:
       enabled: true
       className: nginx
       hosts:
         - host: console.example.com
           paths: []   # ignored when frontend.enabled; / and /api are used
   ```

2. Build and push the frontend image from the repo root:

   ```bash
   docker build -t your-registry/console-web:latest console-web/
   docker push your-registry/console-web:latest
   ```

3. Install/upgrade the chart. The Ingress will route **`/api`** to the console backend and **`/`** to the frontend. The frontend is built with `NEXT_PUBLIC_API_BASE_URL=/api/v1` (default), so all API calls are same-origin.

No CORS configuration is needed on the backend for this setup.

### Backend CORS (when frontend is on a different host)

If the frontend is served from another host (e.g. `https://ui.example.com`) and the API at `https://api.example.com`, set allowed origins on the console backend:

```yaml
console:
  env:
    - name: CORS_ALLOWED_ORIGINS
      value: "https://ui.example.com"
```

Multiple origins (e.g. dev + prod): comma-separated, e.g. `"https://ui.example.com,http://localhost:3000"`.

## Verifying the Installation

Check that the operator is running:

```bash
kubectl get pods -n rustfs-system -l app.kubernetes.io/name=rustfs-operator
```

View operator logs:

```bash
kubectl logs -n rustfs-system -l app.kubernetes.io/name=rustfs-operator -f
```
