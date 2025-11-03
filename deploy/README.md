# RustFS Operator Deployment

This directory contains the Helm chart for deploying the RustFS Kubernetes operator.

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

# Create a sample tenant
kubectl apply -f deploy/rustfs-operator/examples/simple-tenant.yaml

# View tenants
kubectl get tenants --all-namespaces
```

## Uninstall

```bash
helm uninstall rustfs-operator --namespace rustfs-system
```
