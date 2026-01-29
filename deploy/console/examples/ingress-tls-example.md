# Example: Console with Ingress and TLS

This example shows how to deploy the Console with Nginx Ingress and Let's Encrypt TLS certificates.

## Prerequisites

- Nginx Ingress Controller installed
- cert-manager installed for automatic TLS certificates
- DNS record pointing to your cluster

## Configuration

```yaml
# values-console-ingress.yaml
console:
  enabled: true
  replicas: 2  # For high availability

  # JWT secret (keep this secure!)
  jwtSecret: "REPLACE_WITH_YOUR_SECRET_HERE"

  service:
    type: ClusterIP  # No need for LoadBalancer with Ingress
    port: 9090

  ingress:
    enabled: true
    className: nginx
    annotations:
      cert-manager.io/cluster-issuer: letsencrypt-prod
      nginx.ingress.kubernetes.io/ssl-redirect: "true"
      nginx.ingress.kubernetes.io/force-ssl-redirect: "true"
      # Console uses cookies for auth
      nginx.ingress.kubernetes.io/affinity: cookie
      nginx.ingress.kubernetes.io/session-cookie-name: "console-session"
    hosts:
      - host: rustfs-console.example.com
        paths:
          - path: /
            pathType: Prefix
    tls:
      - secretName: rustfs-console-tls
        hosts:
          - rustfs-console.example.com

  resources:
    requests:
      cpu: 100m
      memory: 128Mi
    limits:
      cpu: 500m
      memory: 512Mi

  # Pod anti-affinity for HA
  affinity:
    podAntiAffinity:
      preferredDuringSchedulingIgnoredDuringExecution:
        - weight: 100
          podAffinityTerm:
            labelSelector:
              matchLabels:
                app.kubernetes.io/component: console
            topologyKey: kubernetes.io/hostname
```

## Deploy

```bash
# Create ClusterIssuer for Let's Encrypt (if not exists)
cat <<EOF | kubectl apply -f -
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: your-email@example.com
    privateKeySecretRef:
      name: letsencrypt-prod
    solvers:
      - http01:
          ingress:
            class: nginx
EOF

# Install Console
helm install rustfs-operator ../../rustfs-operator \
  -f values-console-ingress.yaml

# Wait for certificate to be issued
kubectl get certificate -w

# Verify Ingress
kubectl get ingress rustfs-operator-console
```

## Access

```bash
# Console will be available at
open https://rustfs-console.example.com
```

## Test API

```bash
# Create service account
kubectl create serviceaccount api-user
kubectl create clusterrolebinding api-user-admin \
  --clusterrole=cluster-admin \
  --serviceaccount=default:api-user

# Login
TOKEN=$(kubectl create token api-user --duration=1h)
curl -X POST https://rustfs-console.example.com/api/v1/login \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$TOKEN\"}" \
  -c cookies.txt -k

# List tenants
curl https://rustfs-console.example.com/api/v1/tenants \
  -b cookies.txt -k
```

## Cleanup

```bash
helm uninstall rustfs-operator
kubectl delete serviceaccount api-user
kubectl delete clusterrolebinding api-user-admin
```
