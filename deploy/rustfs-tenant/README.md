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

# RustFS Tenant chart

Deploy a Tenant and optional S3 API / Tenant Console networking as one Helm release,
or render ordinary Kubernetes YAML for GitOps and `kubectl apply -f`.
Install the RustFS Operator and its CRD separately; this chart has no Operator dependency.
See the [networking guide](../../docs/tenant-networking.md) for plain YAML, TLS,
virtual-hosted S3, verification, and lifecycle details.

## Resource ownership

| Owner | Resources |
| --- | --- |
| This chart / your GitOps application | Tenant, optional Ingress / HTTPRoute / BackendTLSPolicy |
| RustFS Operator | Tenant Services, StatefulSets and other existing managed resources |
| Platform administrator | GatewayClass, Gateway, controllers, DNS and external certificates |
| Secret management system / administrator | Credentials, referenced TLS Secrets and CA ConfigMaps |

The chart does not change the Tenant CRD, controller RBAC, or existing Operator chart.
`ingress` and `httpRoute` are **chart values**, not new Tenant.spec fields.
The Operator Console's `console.ingress` setting belongs to the Operator chart and
is independent of the Tenant Console configured here.

## Install from this repository

1. Install the Operator and CRD using the [Operator guide](../../docs/operator-user-guide.md).
2. Create `rustfs-storage` and a `rustfs-credentials` Secret in that namespace,
   containing UTF-8 `accesskey` and `secretkey` values (minimum 8 characters each).
   Use your secret manager; do not commit credentials in values files.
3. Copy [values.yaml](values.yaml) to `tenant-values.yaml`. Set a tested image,
   storage class, pool layout and capacity for your environment. The default is
   a **single-node, single-disk development** Tenant, not an HA layout.
4. Install without external access:

```bash
helm upgrade --install rustfs ./deploy/rustfs-tenant \
  --namespace rustfs-storage --values tenant-values.yaml
```

To enable networking, edit the hostnames and platform references in one example:

```bash
# Choose Ingress OR Gateway API per endpoint.
helm upgrade --install rustfs ./deploy/rustfs-tenant \
  --namespace rustfs-storage --values tenant-values.yaml \
  --values deploy/rustfs-tenant/examples/gateway.yaml
```

The Ingress alternative is [examples/ingress.yaml](examples/ingress.yaml).
The example requires an installed IngressClass and a pre-created edge TLS Secret.
The Gateway example requires Gateway API CRDs, a controller, and an existing HTTPS
Gateway listener allowing routes from `rustfs-storage`. It does not create them.

These commands use the source chart. Use a published repository chart only after
a release containing `rustfs-tenant` is available; do not assume the existing
published `0.0.6` release already contains this addition. The release workflow
packages both charts under the release version. The Tenant chart deliberately
has no `appVersion`: it does not select a RustFS image through chart metadata.

## Values contract

| Value | Meaning |
| --- | --- |
| `tenant.metadata.name` | Tenant name; defaults to release name; DNS-1035, max 55 characters |
| `tenant.metadata.labels`, `annotations` | Native metadata maps for the Tenant |
| `tenant.spec` | Native Tenant spec, rendered without `tpl` evaluation or environment injection |
| `ingress.api`, `ingress.console` | Independent Ingress settings; disabled by default |
| `ingress.*.hosts` | Explicit DNS hostnames; no catch-all rule |
| `ingress.*.ingressClassName`, `tls` | Native Ingress fields; TLS uses existing Secrets |
| `ingress.*.labels`, `annotations` | Resource metadata, including controller-specific settings |
| `httpRoute.api`, `httpRoute.console` | Independent HTTPRoute settings; disabled by default |
| `httpRoute.*.parentRefs`, `hostnames` | Native Gateway references and explicit hostnames |
| `httpRoute.*.timeouts` | Native rule timeouts; depends on controller support |
| `httpRoute.*.labels`, `annotations` | HTTPRoute metadata |
| `httpRoute.*.backendTLS` | Optional native BackendTLSPolicy `spec.validation`; requires v1.4+ CRDs and controller support |

Namespace comes from `--namespace`. Services are always `<tenant>-io:9000` and
`<tenant>-console:9001`, matching the existing Operator contract. Each route uses
`/` with prefix matching. The chart does not add rewrites, shared Gateways, DNS,
certificates, arbitrary extra resources, or controller-specific policies.
Use separate standard manifests for advanced platform policies.

Helm merges maps but **replaces lists**, including `tenant.spec.pools` and `env`.
Keep the complete desired lists in your values file. Clearing a default map uses
`null`, e.g. `tenant.spec.credsSecret: null` when using another supported credential
source. The installed Tenant CRD remains the authority for validating native spec
fields; chart schema validation covers the chart-owned settings.

## Upgrade and remove

Keep the release name, namespace and Tenant name stable. Renaming a Tenant creates
a different storage cluster; this chart does not implement data migration.
Do not adopt an existing Tenant or user-managed networking by adding Helm ownership
annotations or using `--take-ownership`. Existing installations can keep using
plain YAML unchanged. Plan any ownership migration separately.

Set an endpoint's `enabled: false` and apply a Helm upgrade to remove its chart-owned
Ingress/HTTPRoute and its optional backend TLS policy. Disabled endpoints may
retain their configuration for later reuse. Reverting to an internal-only
deployment must disable all previously
enabled endpoints. Use complete desired values; avoid `--reuse-values` when removing
old exposure settings. Switching Ingress to HTTPRoute can interrupt traffic and
needs a planned cutover.

`helm uninstall rustfs -n rustfs-storage` deletes the chart-owned Tenant and routes;
Tenant deletion triggers the Operator/Kubernetes workload cleanup. Treat it as a
storage teardown, not a networking-only operation. Check PVC/PV retention and
backups before teardown. Referenced credential/certificate resources and shared
Gateways are not deleted by this chart.

Deleting only the Tenant CR does **not** garbage-collect the chart-owned routes:
there are intentionally no synthetic ownerReferences or lookup hooks. Use the
Helm release lifecycle, or explicitly manage every resource with GitOps.
Helm is not a continuous reconciler; use a GitOps controller if drift correction
of networking resources is required.

## Validation

From the repository root (Helm 3 required):

```bash
helm lint deploy/rustfs-tenant
cargo test --test tenant_chart
kubectl kustomize examples/networking/ingress
kubectl kustomize examples/networking/gateway
```

Rendering does not verify the installed CRDs, Gateway attachment, DNS, certificates,
or S3 traffic. Follow the [runtime verification steps](../../docs/tenant-networking.md#verify-the-data-path)
on your target platform.
