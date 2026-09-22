<!--
Copyright 2026 RustFS Team

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

# Tenant networking: Kubernetes YAML and Helm

The Operator creates `<tenant>-io:9000` for S3 and `<tenant>-console:9001` for the
Tenant Console. Attach your ingress or gateway to these ClusterIP Services, not
the internal headless Service. All pools of a Tenant belong to one cluster and
share these endpoints. Existing Tenant YAML and Operator installations are unchanged.

The optional [Tenant chart](../deploy/rustfs-tenant/README.md) packages a Tenant
and networking into one release. Networking is not part of the Tenant CRD and
is not reconciled by the Operator. Both deployment methods below use the same
Service contract. Choose **one owner** for each resource: Helm or YAML/GitOps.

## Prerequisites

- An installed RustFS Operator and `rustfs.com/v1alpha1` Tenant CRD.
- A namespace (`rustfs-storage` in these examples) and an existing credential
  Secret named `rustfs-credentials` with UTF-8 `accesskey` and `secretkey`, each at
  least 8 characters. Provision secrets through your normal secret management.
- A storage class and a tested RustFS image suitable for your deployment. The
  examples use a single-node/single-disk **development** pool. Choose an appropriate
  HA pool layout and capacity for production.
- For Ingress: an installed controller / IngressClass and edge TLS Secret.
- For HTTPRoute: Gateway API `v1` CRDs, a controller, and an existing Gateway with
  an HTTPS listener and certificate covering the requested domains.
- DNS pointing the public names at the chosen ingress/gateway address.

Create the namespace before provisioning the Secret:

```bash
kubectl create namespace rustfs-storage
```

## Apply ordinary Kubernetes YAML

Copy and edit [the base Tenant](../examples/networking/base/tenant.yaml), including
namespace, name and pool settings. Then choose either
[Ingress](../examples/networking/ingress/ingress.yaml) or
[HTTPRoute](../examples/networking/gateway/httproute.yaml). The manifests contain
normal `apiVersion`, `kind`, `metadata` and `spec` fields with no Helm expressions.
If you rename the Tenant, update every route backend reference as well.

```bash
kubectl apply -f examples/networking/base/tenant.yaml

# Choose one:
kubectl apply -f examples/networking/ingress/ingress.yaml
# kubectl apply -f examples/networking/gateway/httproute.yaml
```

Or apply the equivalent Kustomize overlay after editing it:

```bash
kubectl apply -k examples/networking/ingress
# Alternative: kubectl apply -k examples/networking/gateway
```

The base creates no namespace, Secret, Gateway, or controller. Changing between
overlays does not prune the old overlay's resources with plain `kubectl apply`.
Delete obsolete Ingress/HTTPRoute objects explicitly after a planned cutover, or
use your GitOps tool's pruning configuration. Do not run both alternatives against
the same public endpoint inadvertently.

An HTTPRoute backend is an ordinary Service reference:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: rustfs-api
  namespace: rustfs-storage
spec:
  parentRefs:
    - name: shared-gateway
      namespace: gateway-system
      sectionName: https
  hostnames:
    - s3.example.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: rustfs-io
          port: 9000
```

The Gateway owner must allow the route's namespace through the listener's
`allowedRoutes`. A cross-namespace Gateway parent reference uses this attachment
permission, not ReferenceGrant. Route-to-Service references here stay in the Tenant
namespace. The chart never changes the shared Gateway's listeners or permissions.
See [Gateway API cross-namespace routing](https://gateway-api.sigs.k8s.io/guides/multiple-ns/).

## Use one Helm values file

Copy [values.yaml](../deploy/rustfs-tenant/values.yaml), put your existing Tenant
`spec` under `tenant.spec`, and configure `ingress` or `httpRoute` in the same file.
`tenant.metadata` accepts name, labels and annotations; namespace is the release
namespace. These wrapper keys belong to Helm only, not to the Kubernetes Tenant.

```bash
helm upgrade --install rustfs ./deploy/rustfs-tenant \
  --namespace rustfs-storage --values tenant-values.yaml
```

To review/render regular Kubernetes manifests without installing a Helm release:

```bash
helm template rustfs ./deploy/rustfs-tenant \
  --namespace rustfs-storage --values tenant-values.yaml > tenant-stack.yaml
kubectl apply --dry-run=server -f tenant-stack.yaml
kubectl apply -f tenant-stack.yaml
```

In this mode there is **no Helm release** to upgrade/uninstall. Track the rendered
resources with YAML/GitOps; removing a document from a file does not delete its
previously applied resource. Direct `kubectl delete tenant` also leaves the
separately managed routes behind.

## S3 and Console routing

Prefer separate domains, e.g. `s3.example.com` and `console.example.com`, both
forwarding `/` unchanged. Preserve the original Host, path encoding and query
parameters: they participate in S3 signing. Do not mount S3 behind `/s3` with a
rewrite or put a browser-login redirect in front of the S3 endpoint. Configure
body-size limits, streaming/buffering and timeouts for your object sizes using
the chosen controller's documented policies; these are not portable Ingress settings.

The Console domain forwards to port 9001, including its UI and management API
paths. Do not forward only the static `/rustfs/console` subtree. If your image
supports a custom `RUSTFS_CONSOLE_PREFIX`, configure it through `Tenant.spec.env`;
it does not relocate all admin endpoints. Do not change the listener ports away
from the Operator Service contract.

For browser/OIDC deployments, supported RustFS images accept
`RUSTFS_BROWSER_REDIRECT_URL=https://console.example.com` (origin only, no path).
Use `spec.env` in YAML or `tenant.spec.env` in chart values. Validate the OIDC
provider callback and proxy headers. Node-local in-flight OIDC state may require
controller-specific session affinity; an external redirect URL alone does not
provide affinity. The chart does not silently inject env vars or affinity policies.

### Path-style and virtual-hosted-style S3

Path-style (`https://s3.example.com/bucket/key`) uses the base route examples.
Configure your SDK/client to use path-style addressing when needed.

For `https://bucket.s3.example.com/key`, configure all of:

1. A RustFS image supporting `RUSTFS_SERVER_DOMAINS`, set to `s3.example.com`.
2. Both `s3.example.com` and `*.s3.example.com` in HTTPRoute `hostnames`, or separate
   Ingress `rules` (chart values: `ingress.api.hosts`).
3. DNS and edge certificates covering the base and wildcard domain. A wildcard
   certificate does not cover the base domain or arbitrary multi-label bucket names.

The chart [virtual-hosted overlay](../deploy/rustfs-tenant/examples/virtual-hosted.yaml)
can be layered over its Gateway example. Merge any other required env entries:
Helm replaces the entire `env` list. For plain YAML, edit `spec.env` and route
hostnames directly; no Operator feature switch is required.

## TLS termination and backend encryption

Public HTTPS and backend TLS are separate connections. The default examples
terminate TLS at the edge and forward HTTP to the Tenant. Configure network
isolation appropriate to your environment; use backend encryption when required.

For encrypted Gateway-to-RustFS connections:

1. Configure the existing Tenant `spec.tls` and certificate/CA resources. The
   [chart TLS overlay](../deploy/rustfs-tenant/examples/backend-tls.yaml) shows
   cert-manager with an existing Issuer. Keep the generated internal SANs and
   include the validation names for both Services. Adjust cluster domain, namespace
   and Tenant name together. The Operator remains responsible for this existing
   Tenant TLS lifecycle.
2. Supply a trusted CA ConfigMap with key `ca.crt` in the Tenant namespace, using
   the CA that actually issued the backend certificate. Do not disable verification.
3. Use a Gateway implementation supporting `BackendTLSPolicy` and install Gateway
   API v1.4+ CRDs (the policy uses `gateway.networking.k8s.io/v1`). Set
   `httpRoute.api.backendTLS` and `httpRoute.console.backendTLS`, or apply the
   [plain policies](../examples/networking/gateway/backend-tls.yaml) after editing.
   Both RustFS listeners inherit the Tenant TLS settings; configure both exposed
   backends accordingly. Do not infer HTTP/HTTPS solely from a Service port's name.

```bash
helm template rustfs ./deploy/rustfs-tenant -n rustfs-storage \
  -f deploy/rustfs-tenant/examples/gateway.yaml \
  -f deploy/rustfs-tenant/examples/backend-tls.yaml
```

`backendTLS` is the native policy `spec.validation` object (hostname and CA
references or `wellKnownCACertificates: System`). It configures the gateway, not
the RustFS listener. Explicit CA references are recommended for private PKI;
controller support for system CAs varies. See [Gateway TLS configuration](https://gateway-api.sigs.k8s.io/guides/tls/).

For Ingress with an HTTPS backend, set the controller-specific upstream protocol,
CA trust and TLS verification name through annotations or separately managed policy
resources. Merely setting `ingress.*.tls` only configures edge TLS. There is no
portable Ingress field that replaces BackendTLSPolicy. Keep controller-specific
resources outside the storage Operator.

## Verify the data path

First validate and inspect Kubernetes resources:

```bash
kubectl apply --dry-run=server -f tenant-stack.yaml
kubectl -n rustfs-storage get tenants,pods,svc
kubectl -n rustfs-storage describe ingress rustfs-api
# Gateway alternative:
kubectl -n rustfs-storage get httproute rustfs-api -o yaml
kubectl -n rustfs-storage get httproute rustfs-console -o yaml
```

For HTTPRoute, check the intended parent/listener reports `Accepted=True` and
`ResolvedRefs=True` for the current generation; also inspect Gateway and any
BackendTLSPolicy conditions. This does not by itself prove DNS or TLS reachability.
Use the command appropriate to your chosen Ingress/Gateway deployment.

With an AWS CLI profile containing test credentials, verify against a disposable
bucket (choose a unique name and configure the intended addressing style):

```bash
aws --profile rustfs --endpoint-url https://s3.example.com s3 mb s3://network-check-unique
printf 'network check\n' > /tmp/rustfs-network-check.txt
aws --profile rustfs --endpoint-url https://s3.example.com s3 cp \
  /tmp/rustfs-network-check.txt s3://network-check-unique/probe.txt
aws --profile rustfs --endpoint-url https://s3.example.com s3 cp \
  s3://network-check-unique/probe.txt /tmp/rustfs-network-check-downloaded.txt
cmp /tmp/rustfs-network-check.txt /tmp/rustfs-network-check-downloaded.txt
aws --profile rustfs --endpoint-url https://s3.example.com s3 presign \
  s3://network-check-unique/probe.txt --expires-in 60
```

Download the returned presigned URL and compare the contents. Then test a large
multipart upload/download using your workload's part sizes and timeouts, and
verify checksums. Repeat with virtual-hosted addressing if enabled. Verify the
Console login and, when used, the complete OIDC callback flow through the public
domain. Remove the disposable objects and bucket afterward.

The chart contract tests validate rendered resource boundaries, Service references,
TLS policy targets and invalid configurations. They do not replace these runtime
tests. Helm `--wait` is not a guarantee of Tenant or external endpoint readiness.

## Compatibility and lifecycle

Existing Tenant YAML, Operator chart values, CRD fields, RBAC and controller
behavior are unchanged. Default chart installation creates no external route and
no Gateway API resource, so Gateway CRDs are not required for internal-only or
Ingress deployments. Plain YAML remains a first-class option.

Keep existing resources under their existing owner. This chart does not implement
in-place adoption, renaming, data migration or seamless ingress-controller cutovers.
Do not use `helm --take-ownership` as a migration shortcut. Route ownership follows
the Helm release / GitOps application, not Tenant garbage collection. See the
[chart lifecycle instructions](../deploy/rustfs-tenant/README.md#upgrade-and-remove)
before disabling networking or uninstalling a release.
