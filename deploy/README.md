# RustFS Operator Deployment

This directory contains separate charts for installing the RustFS Operator and deploying Tenants.

## Quick Start

Install the operator using Helm:

```bash
helm install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace
```

## What's Included

- **rustfs-operator/** - Helm chart for the operator
  - Configurable deployment settings
  - RBAC resources
  - CRD installation
  - Example Tenant resources

- **rustfs-tenant/** - Optional Tenant deployment chart with API/Console Ingress or HTTPRoute.
  It requires an existing Operator; it does not change existing Tenant YAML or install a Gateway.

## Tenant networking

Use [plain Kubernetes YAML/Kustomize](../docs/tenant-networking.md) or the
[separate Tenant chart](rustfs-tenant/README.md). All external endpoints are disabled
by default in the chart. Existing deployments do not need to migrate.

## Documentation

See the [Helm chart README](rustfs-operator/README.md) for detailed configuration options and usage examples.

## Prerequisites

- Kubernetes cluster (v1.30+)
- Helm 3.0+
- The `rustfs/operator:latest` container image loaded or available in your registry

## Verify Installation

After installing with Helm:

```bash
# Check operator pods
kubectl get pods -n rustfs-system

# View operator logs
kubectl logs -n rustfs-system -l app.kubernetes.io/name=rustfs-operator -f

# Create a sample tenant (from project root)
kubectl apply -f examples/simple-tenant.yaml

# View tenants
kubectl get tenants --all-namespaces
```

## Uninstall

```bash
helm uninstall rustfs-operator --namespace rustfs-system
```
