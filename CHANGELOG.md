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

# Changelog

All notable changes to RustFS Operator are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Tenant `spec.network` for Service IP families and IPv6 listen addresses, plus dual-stack binds
  for operator observability, STS, and Console sockets.
- Tenant `spec.hostUsers` and OpenShift `hostUsers: false` defaults for `restricted-v3`.
- Tenant bucket canned anonymous access and ConfigMap-sourced bucket policies.

### Fixed

- Provisioning now requeues transient RustFS admin/S3 and Kubernetes failures instead of leaving
  policies, users, and buckets failed until an unrelated object change.

### Changed

- Documented that distinct-physical-disk erasure failures and a separate data-plane operator are
  outside this controller's scope.

## [0.0.6] - 2026-08-22

### Added

- Restricted Pod Security defaults for generated RustFS workloads.
- Configurable Kubernetes cluster DNS domains and generated TLS SAN coverage.
- Kubernetes STS support with PolicyBinding authorization and managed or external TLS.
- OpenShift installation support that delegates UID and FSGroup selection to SCC admission.
- Tenant credential, RPC authentication, KMS, certificate, and provisioning lifecycle validation.

### Changed

- Removed legacy Tenant workload Roles and RoleBindings and disabled automatic ServiceAccount token
  mounting for generated RustFS workloads.
- Changed `sts.tls.auto` from `true` to `false`; installations must now provide the STS TLS Secret
  unless Operator-managed certificate generation is explicitly enabled.
- Restricted Console to one replica with a `Recreate` deployment strategy because sessions are
  process-local. Console restarts and session Secret rotation invalidate active sessions.
- Tightened validation for credentials, security contexts, public TLS SANs, pool volume counts, and
  immutable PVC template fields.
- Required an explicit runtime-image acknowledgement when a Tenant overrides the default RustFS
  image.
- Defaulted chart-managed Operator and Console images to the immutable chart `appVersion` instead
  of the mutable `latest` tag.

### Fixed

- Made repeated blocked status updates idempotent and hardened leader-election loss handling.
- Tolerated transient node lookup failures while preserving Pod cleanup safety.
- Protected existing RustFS users during provisioning reconciliation.
- Corrected monitoring responses wrapped by the RustFS API.
- Corrected STS SigV4 query encoding, bounded session duration, and rotated managed TLS certificates.
- Revoked Console sessions on logout and prevented pool volume-count overflow.
- Added finalizer RBAC required by Kubernetes and OpenShift admission.

### Security

- Applied authentication to an explicit protected Console API router instead of relying on a
  fail-open path allowlist.
- Added admission limits for unauthenticated Console login and STS requests.
- Required cryptographically strong Console session keys and rejected empty credential Secrets.
- Bounded generated TLS SAN work and HTTP metrics label cardinality.

### Upgrade notes

#### Apply CRDs before upgrading the controller

Helm does not upgrade CRDs already installed from a chart's `crds/` directory. Apply both packaged
CRDs before the Helm upgrade:

```bash
kubectl apply --server-side --force-conflicts \
  --field-manager=rustfs-operator-crd-upgrade \
  -f deploy/rustfs-operator/crds/tenant-crd.yaml
kubectl apply --server-side --force-conflicts \
  --field-manager=rustfs-operator-crd-upgrade \
  -f deploy/rustfs-operator/crds/policybinding-crd.yaml
```

#### Review Tenant Kubernetes API access

The Operator removes legacy Tenant workload RBAC and renders
`automountServiceAccountToken: false`. Standard RustFS workloads do not need Kubernetes API access.
Custom sidecars or scripts that do need it must use a user-owned ServiceAccount, least-privilege
RBAC, and an explicit projected token. This migration changes the StatefulSet Pod template and
causes a rolling restart.

#### Choose the STS TLS owner

The new default is `sts.tls.auto=false`. Pre-create the configured STS TLS Secret with `tls.crt`,
`tls.key`, and `ca.crt`, or explicitly preserve the previous behavior with:

```yaml
sts:
  tls:
    auto: true
```

#### Plan Console session interruption

The Console now uses one replica and a `Recreate` rollout. Plan for a brief Console interruption;
users must authenticate again after a restart or session Secret rotation. Tenant data-plane traffic
is unaffected.

#### Rollback considerations

- Back up Tenant resources, Helm values, and the installed CRDs before upgrading.
- Do not downgrade CRDs automatically; keep the newer schema unless compatibility with the older
  controller has been verified.
- An older Operator may recreate legacy Tenant RBAC and revert the ServiceAccount token setting,
  causing another Tenant rollout and restoring broader Kubernetes API access.
- Pin Operator and RustFS images independently, then verify Tenant readiness and S3 read/write data
  before and after any rollback.

[Unreleased]: https://github.com/rustfs/operator/compare/0.0.6...HEAD
[0.0.6]: https://github.com/rustfs/operator/compare/0.0.5...0.0.6
