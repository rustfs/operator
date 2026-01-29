# RustFS Operator Console Deployment Guide

## Overview

The RustFS Operator Console provides a web-based management interface for RustFS Tenants deployed in Kubernetes. It offers a REST API for managing tenants, viewing events, and monitoring cluster resources.

## Architecture

The Console is deployed as a separate Deployment alongside the Operator:
- **Operator**: Watches Tenant CRDs and reconciles Kubernetes resources
- **Console**: Provides REST API for management operations

Both components use the same Docker image but run different commands:
- Operator: `./operator server`
- Console: `./operator console --port 9090`

## Deployment Methods

### Option 1: Helm Chart (Recommended)

The Console is integrated into the main Helm chart and can be enabled via `values.yaml`.

#### Install with Console enabled:

```bash
helm install rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true \
  --set console.service.type=LoadBalancer
```

#### Upgrade existing installation to enable Console:

```bash
helm upgrade rustfs-operator deploy/rustfs-operator \
  --set console.enabled=true
```

#### Custom configuration:

Create a `custom-values.yaml`:

```yaml
console:
  enabled: true

  # Number of replicas
  replicas: 2

  # JWT secret for session signing (recommended: generate with openssl rand -base64 32)
  jwtSecret: "your-secure-random-secret-here"

  # Service configuration
  service:
    type: LoadBalancer
    port: 9090
    annotations:
      service.beta.kubernetes.io/aws-load-balancer-type: "nlb"

  # Ingress configuration
  ingress:
    enabled: true
    className: nginx
    annotations:
      cert-manager.io/cluster-issuer: letsencrypt-prod
    hosts:
      - host: rustfs-console.example.com
        paths:
          - path: /
            pathType: Prefix
    tls:
      - secretName: rustfs-console-tls
        hosts:
          - rustfs-console.example.com

  # Resource limits
  resources:
    requests:
      cpu: 100m
      memory: 128Mi
    limits:
      cpu: 500m
      memory: 512Mi
```

Apply the configuration:

```bash
helm upgrade --install rustfs-operator deploy/rustfs-operator \
  -f custom-values.yaml
```

### Option 2: kubectl apply (Standalone)

For manual deployment or customization, you can use standalone YAML files.

See `deploy/console/` directory for standalone deployment manifests.

## Accessing the Console

### Via Service (ClusterIP)

```bash
# Port forward to local machine
kubectl port-forward svc/rustfs-operator-console 9090:9090

# Access at http://localhost:9090
```

### Via LoadBalancer

```bash
# Get the external IP
kubectl get svc rustfs-operator-console

# Access at http://<EXTERNAL-IP>:9090
```

### Via Ingress

Access via the configured hostname (e.g., `https://rustfs-console.example.com`)

## API Endpoints

### Health & Readiness

- `GET /healthz` - Health check
- `GET /readyz` - Readiness check

### Authentication

- `POST /api/v1/login` - Login with Kubernetes token
  ```json
  {
    "token": "eyJhbGciOiJSUzI1NiIsImtpZCI6..."
  }
  ```

- `POST /api/v1/logout` - Logout and clear session
- `GET /api/v1/session` - Check session status

### Tenant Management

- `GET /api/v1/tenants` - List all tenants
- `GET /api/v1/namespaces/{ns}/tenants` - List tenants in namespace
- `GET /api/v1/namespaces/{ns}/tenants/{name}` - Get tenant details
- `POST /api/v1/namespaces/{ns}/tenants` - Create tenant
- `DELETE /api/v1/namespaces/{ns}/tenants/{name}` - Delete tenant

### Events

- `GET /api/v1/namespaces/{ns}/tenants/{name}/events` - List tenant events

### Cluster Resources

- `GET /api/v1/nodes` - List cluster nodes
- `GET /api/v1/namespaces` - List namespaces
- `POST /api/v1/namespaces` - Create namespace
- `GET /api/v1/cluster/resources` - Get cluster resource summary

## Authentication

The Console uses JWT-based authentication with Kubernetes ServiceAccount tokens:

1. **Login**: Users provide their Kubernetes ServiceAccount token
2. **Validation**: Console validates the token by making a test API call to Kubernetes
3. **Session**: Console generates a JWT session token (12-hour expiry)
4. **Cookie**: Session token stored in HttpOnly cookie
5. **Authorization**: All API requests use the user's Kubernetes token for authorization

### Getting a Kubernetes Token

```bash
# Create a ServiceAccount
kubectl create serviceaccount console-user

# Create ClusterRoleBinding (for admin access)
kubectl create clusterrolebinding console-user-admin \
  --clusterrole=cluster-admin \
  --serviceaccount=default:console-user

# Get the token
kubectl create token console-user --duration=24h
```

### Login Example

```bash
TOKEN=$(kubectl create token console-user --duration=24h)

curl -X POST http://localhost:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$TOKEN\"}" \
  -c cookies.txt

# Subsequent requests use the cookie
curl http://localhost:9090/api/v1/tenants \
  -b cookies.txt
```

## RBAC Permissions

The Console ServiceAccount has the following permissions:

- **Tenants**: Full CRUD operations
- **Namespaces**: List and create
- **Services, Pods, ConfigMaps, Secrets**: Read-only
- **Nodes**: Read-only
- **Events**: Read-only
- **StatefulSets**: Read-only
- **PersistentVolumeClaims**: Read-only

Users authenticate with their own Kubernetes tokens, so actual permissions depend on the user's RBAC roles.

## Security Considerations

1. **JWT Secret**: Always set a strong random JWT secret in production
   ```bash
   openssl rand -base64 32
   ```

2. **TLS/HTTPS**: Enable Ingress with TLS for production deployments

3. **Network Policies**: Restrict Console access to specific namespaces/pods

4. **RBAC**: Console requires cluster-wide read access and tenant management permissions

5. **Session Expiry**: Default 12-hour session timeout (configurable in code)

6. **CORS**: Configure allowed origins based on your frontend deployment

## Monitoring

### Prometheus Metrics

(To be implemented - placeholder for future enhancement)

### Logs

```bash
# View Console logs
kubectl logs -l app.kubernetes.io/component=console -f

# Set log level
helm upgrade rustfs-operator deploy/rustfs-operator \
  --set console.logLevel=debug
```

## Troubleshooting

### Console Pod Not Starting

```bash
# Check pod status
kubectl get pods -l app.kubernetes.io/component=console

# View events
kubectl describe pod -l app.kubernetes.io/component=console

# Check logs
kubectl logs -l app.kubernetes.io/component=console
```

### Authentication Failures

- Verify Kubernetes token is valid: `kubectl auth can-i get tenants --as=system:serviceaccount:default:console-user`
- Check Console ServiceAccount has proper RBAC permissions
- Verify JWT_SECRET is consistent across Console replicas

### CORS Errors

- Update CORS configuration in `src/console/server.rs`
- Rebuild and redeploy the image
- Or use Ingress annotations to handle CORS

## Configuration Reference

See `deploy/rustfs-operator/values.yaml` for complete configuration options:

```yaml
console:
  enabled: true|false           # Enable/disable Console
  replicas: 1                   # Number of replicas
  port: 9090                    # Console port
  logLevel: info                # Log level
  jwtSecret: ""                 # JWT signing secret

  image:
    repository: rustfs/operator
    tag: latest
    pullPolicy: IfNotPresent

  resources: {}                 # Resource requests/limits
  nodeSelector: {}              # Node selection
  tolerations: []               # Pod tolerations
  affinity: {}                  # Pod affinity

  service:
    type: ClusterIP             # Service type
    port: 9090                  # Service port

  ingress:
    enabled: false              # Enable Ingress
    className: ""               # Ingress class
    hosts: []                   # Ingress hosts
    tls: []                     # TLS configuration
```

## Examples

See `deploy/console/examples/` for:
- Basic deployment
- LoadBalancer service
- Ingress with TLS
- Multi-replica setup
- Custom RBAC roles
