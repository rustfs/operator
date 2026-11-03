# RustFS Operator Deployment

This directory contains separate charts for installing the RustFS Operator and deploying Tenants.

## Quick Start

Install the operator from the public [Helm repository](https://operator.rustfs.com).
Before installing, prepare the `sts-tls` Secret described in the
[chart installation guide](rustfs-operator/README.md#installing-the-chart).

```bash
helm repo add rustfs-operator https://operator.rustfs.com
helm repo update
helm search repo rustfs-operator/rustfs-operator --versions
helm install rustfs-operator rustfs-operator/rustfs-operator \
  --version 0.0.8 \
  --namespace rustfs-system \
  --create-namespace
```

The example selects published chart version `0.0.8`. Select another available
version with `--version`. For GitOps deployments, see the
[Argo CD Application example](rustfs-operator/README.md#argo-cd).

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
- Access to the Operator image selected by the chart version or your Helm values

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
