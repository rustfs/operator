# Example: Basic Console Deployment with LoadBalancer

This example shows how to deploy the RustFS Operator Console with a LoadBalancer service for external access.

## Configuration

```yaml
# values-console-loadbalancer.yaml
console:
  enabled: true
  replicas: 1

  # Generate JWT secret: openssl rand -base64 32
  jwtSecret: "REPLACE_WITH_YOUR_SECRET_HERE"

  service:
    type: LoadBalancer
    port: 9090
    # Optional: Restrict source IPs
    loadBalancerSourceRanges:
      - 10.0.0.0/8
      - 192.168.0.0/16

  resources:
    requests:
      cpu: 100m
      memory: 128Mi
    limits:
      cpu: 500m
      memory: 512Mi
```

## Deploy

```bash
# Install with Console enabled
helm install rustfs-operator ../../rustfs-operator \
  -f values-console-loadbalancer.yaml

# Wait for LoadBalancer IP
kubectl get svc rustfs-operator-console -w

# Get the external IP
CONSOLE_IP=$(kubectl get svc rustfs-operator-console -o jsonpath='{.status.loadBalancer.ingress[0].ip}')
echo "Console available at: http://${CONSOLE_IP}:9090"
```

## Test

```bash
# Health check
curl http://${CONSOLE_IP}:9090/healthz

# Create a test user token
kubectl create serviceaccount console-test-user
kubectl create clusterrolebinding console-test-admin \
  --clusterrole=cluster-admin \
  --serviceaccount=default:console-test-user

# Get token and login
TOKEN=$(kubectl create token console-test-user --duration=1h)
curl -X POST http://${CONSOLE_IP}:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$TOKEN\"}" \
  -c cookies.txt

# List tenants
curl http://${CONSOLE_IP}:9090/api/v1/tenants -b cookies.txt
```

## Cleanup

```bash
helm uninstall rustfs-operator
kubectl delete serviceaccount console-test-user
kubectl delete clusterrolebinding console-test-admin
```
