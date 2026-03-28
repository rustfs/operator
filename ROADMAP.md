# RustFS Kubernetes Operator Roadmap

This document outlines the development roadmap for the RustFS Kubernetes Operator. The roadmap is organized by release versions and includes features, improvements, and technical debt items.

**Last Updated**: 2026-03-28
**Current Version**: 0.1.0 (pre-release)

---

## Current Status (v0.1.0)

### ✅ Completed Features

- [x] Basic Tenant CRD with multi-pool support
- [x] RBAC resource management (Role, ServiceAccount, RoleBinding)
- [x] Service creation (IO, Console, Headless)
- [x] StatefulSet generation per pool
- [x] Persistent volume management with volume claim templates
- [x] Per-pool scheduling configuration (nodeSelector, affinity, tolerations, resources)
- [x] Automatic RUSTFS_VOLUMES configuration
- [x] Required RustFS environment variables
- [x] CRD validation rules (servers, volumes, credentials)
- [x] Certificate and TLS utilities (RSA, ECDSA, Ed25519)
- [x] Kubernetes events for reconciliation actions
- [x] Test infrastructure with helper utilities
- [x] Tenant status: conditions (`Ready`, `Progressing`, `Degraded`), overall state, per-pool status from StatefulSets (successful reconcile path)
- [x] StatefulSet create/update with safe update validation and apply when spec changes
- [x] Operator HTTP console API and `console` CLI subcommand; `console-web` management UI

### 🔧 Known Issues

- [ ] No integration or E2E tests in-repo (unit tests only)
- [ ] Tenant status not always updated when reconcile returns early with an error (e.g. credential/KMS validation)
- [ ] Status subresource patch: only one conflict retry; stronger backoff optional
- [ ] TLS certificate rotation not automated
- [ ] Advanced StatefulSet rollout (rollback, extra strategy options) still open

---

## Core Stability

**Focus**: Production readiness for basic deployments

### High Priority

- [x] **Secret-based credential management** ✅ **COMPLETED** (2025-11-15)
  - ✅ Support for reading credentials from Kubernetes Secrets
  - ✅ Secure credential injection via `secretKeyRef` (credentials never loaded into operator memory)
  - ✅ Validation of Secret structure:
    - Secret exists in same namespace
    - Contains required keys (`accesskey`, `secretkey`)
    - Valid UTF-8 encoding
    - Minimum 8 characters for both keys
  - ✅ Backward compatibility with environment variables
  - ✅ Comprehensive error messages and event recording
  - ✅ Smart retry logic (60s for credential errors, 5s for API errors)
  - ✅ Production-ready examples and documentation
  - See: `examples/secret-credentials-tenant.yaml`, Issue #41

- [x] **Status conditions (happy path)** ✅ — `Ready` / `Progressing` / `Degraded`, pool-level status; see `src/reconcile.rs`
- [ ] **Status on reconciliation errors** — Surface failing state when reconcile exits early (credentials, validation, etc.); related to Issue #42
- [x] **StatefulSet update (core)** ✅ — Validate immutable fields, apply when `statefulset_needs_update`; per-pool status from STS
- [ ] **StatefulSet rollout extras** — Rollback, configurable strategies, richer revision tracking (beyond current behavior)
- [ ] **Improved error handling and observability**
  - Prometheus metrics (reconciliation duration, error rates, pool health)
  - Broader event coverage if gaps remain
  - Note: structured logging (`tracing`) and `error_policy` requeue tiers exist today

### Medium Priority

- [ ] **Configuration validation enhancements**
  - Validate storage class exists before creating PVCs
  - Check node selector labels match available nodes
  - Validate resource requests don't exceed node capacity
  - Warn on mixing storage classes (performance implications)

- [ ] **Documentation improvements**
  - API reference documentation (CRD fields)
  - Operator deployment guide (Helm chart, manifests)
  - Troubleshooting guide with common issues
  - Migration guide from direct StatefulSet deployments

### Testing & Quality

- [ ] **Integration test suite**
  - Kind/k3s-based integration tests
  - Test tenant lifecycle (create, update, delete)
  - Test pool scaling operations
  - Test error recovery scenarios

- [ ] **E2E tests**
  - Real RustFS deployment testing
  - Data persistence verification
  - Upgrade/downgrade scenarios
  - Disaster recovery testing

---

## Advanced Features

**Focus**: Advanced lifecycle management and operational features

### High Priority

- [ ] **Tenant lifecycle management**
  - Finalizers for graceful deletion
  - Orphaned resource cleanup
  - Pre-deletion validation (check for data)
  - Backup integration hooks

- [ ] **Pool lifecycle management**
  - Safe pool addition with data rebalancing awareness
  - Pool removal with decommissioning checks
  - Pool expansion (increase servers/volumes)
  - Pool migration support

- [ ] **TLS/Certificate management**
  - Automatic certificate generation (cert-manager integration)
  - Certificate rotation automation
  - Support for custom CA certificates
  - mTLS between RustFS servers

### Medium Priority

- [ ] **Monitoring and alerting**
  - RustFS metrics scraping and exposure
  - ServiceMonitor CRD for Prometheus Operator
  - Grafana dashboard templates
  - Alert rules for common issues

- [ ] **Backup and disaster recovery**
  - Integration with Velero
  - Snapshot management
  - Point-in-time recovery documentation
  - Multi-cluster replication guidance

- [ ] **Resource optimization**
  - Automatic resource right-sizing recommendations
  - Storage capacity monitoring and alerts
  - Cost optimization insights (spot instance viability)
  - Performance profiling tools

---

## Enterprise Features

**Focus**: Multi-tenancy, security, and compliance

### High Priority

- [ ] **Multi-tenancy enhancements**
  - Namespace isolation best practices
  - Resource quota integration
  - Network policy templates
  - Tenant isolation verification

- [ ] **Security hardening**
  - Pod Security Standards compliance (restricted profile)
  - Seccomp and AppArmor profiles
  - Read-only root filesystem support
  - Non-root container support
  - Secrets encryption at rest

- [ ] **Compliance and audit**
  - Audit logging for all operator actions
  - Compliance report generation (PCI, HIPAA, SOC2)
  - RBAC audit tools
  - Security scanning integration (Trivy, Snyk)

### Medium Priority

- [ ] **Advanced scheduling**
  - Cluster autoscaler integration
  - Pod disruption budgets
  - Priority classes for critical workloads
  - Custom scheduler support

- [ ] **Networking enhancements**
  - Ingress/Gateway API integration
  - Service mesh compatibility (Istio, Linkerd)
  - Network policy generation
  - External DNS integration

- [ ] **Storage enhancements**
  - Storage class auto-detection
  - Volume expansion support
  - Snapshot scheduling
  - Tiering policy management (RustFS lifecycle)

---

## Production Ready

**Focus**: Stability, documentation, and ecosystem integration

### Release Criteria

- [ ] **Stability requirements**
  - 3 months without critical bugs
  - 95%+ test coverage
  - Performance benchmarks published
  - Upgrade path from all 0.x versions

- [ ] **Documentation completeness**
  - Complete API documentation
  - Production deployment guides
  - Architecture deep-dive
  - Runbooks for common operations
  - Video tutorials and demos

- [ ] **Ecosystem integration**
  - OperatorHub.io listing
  - Artifact Hub listing
  - Helm chart repository
  - OLM (Operator Lifecycle Manager) support
  - Kustomize examples

- [ ] **Community and support**
  - Active community channels (Slack, Discord, forum)
  - Regular release cadence (monthly)
  - Public roadmap with user voting
  - Commercial support options documented

---

## Future Considerations (Post-1.0)

### Under Discussion

- **GitOps integration**: ArgoCD/Flux declarative configuration
- **Multi-cluster management**: Federated tenant deployments
- **Advanced replication**: Cross-cluster data replication
- **AI/ML workload optimization**: Specialized configurations for AI storage patterns
- **Edge deployment support**: Lightweight operator for edge Kubernetes
- **Operator SDK migration**: Consider migrating to operator-sdk framework
- **Custom admission webhooks**: Additional validation and mutation logic
- **Backup operator integration**: Dedicated backup operator with CRD

---

## Technical Debt and Refactoring

### High Priority

- [ ] Refactor reconciliation loop for better testability
- [ ] Extract stateful set generation into separate module
- [ ] Improve error types with more context
- [ ] Add comprehensive inline documentation
- [ ] Standardize naming conventions across codebase

### Medium Priority

- [ ] Consider using `kube-runtime` finalizers API
- [x] ~~**k8s-openapi / kube from crates.io**~~ — Using crates.io versions (see `Cargo.toml`); keep pinned upgrades deliberate
- [ ] Performance profiling and optimization
- [ ] Memory usage analysis and optimization
- [ ] Reduce binary size (investigate dependencies)

### Low Priority

- [ ] Migrate build system to modern Rust practices
- [ ] Consider async runtime optimizations
- [ ] Evaluate alternative Kubernetes client libraries
- [ ] Code generation for boilerplate reduction

---

## Community and Contribution Goals

### Community Building

- [x] **CONTRIBUTING.md** and contributor workflow (`make pre-commit`)
- [x] **Pull request template** (`.github/pull_request_template.md`)
- [ ] GitHub issue templates and `good-first-issue` labels
- [ ] Regular community meetings (monthly)
- [x] **Core developer docs** (`docs/DEVELOPMENT.md`, `CLAUDE.md`, `docs/architecture-decisions.md`) — expand as needed

### Ecosystem Partnerships

- [ ] Collaborate with RustFS core team
- [ ] Partner with Kubernetes SIG Storage
- [ ] Engage with CNCF projects (cert-manager, external-secrets)
- [ ] Work with cloud providers for validation
- [ ] Collaborate with observability vendors (Datadog, New Relic)

---

## Dependencies and Prerequisites

### Minimum Requirements

- **Kubernetes**: v1.27+ (current target: v1.30)
- **Rust**: 1.91+ (edition 2024)
- **RustFS**: Version compatibility matrix TBD
- **kube** / **k8s-openapi**: crates.io versions in `Cargo.toml`

### Optional Dependencies

- **cert-manager**: v1.12+ (for TLS automation)
- **Prometheus Operator**: v0.68+ (for monitoring)
- **Velero**: v1.12+ (for backup)
- **external-secrets**: v0.9+ (for secret management)

---

## How to Contribute to This Roadmap

We welcome community input on this roadmap. You can:

1. **Vote on features**: Comment on issues with 👍 for features you need
2. **Propose new features**: Open an issue with the `enhancement` label
3. **Discuss priorities**: Join our community meetings
4. **Share use cases**: Help us understand your deployment scenarios
5. **Contribute code**: Pick up items marked as `good-first-issue`

**Discussion Forum**: https://github.com/orgs/rustfs/discussions
**Issue Tracker**: https://github.com/rustfs/operator/issues

---

## Success Metrics

We track these metrics to measure progress:

- **Stability**: Mean time between failures (MTBF)
- **Performance**: Reconciliation time, resource usage
- **Quality**: Test coverage, bug count, security vulnerabilities
- **Adoption**: GitHub stars, downloads, production deployments
- **Community**: Contributors, PR velocity, issue resolution time

---

**Note**: This roadmap is a living document and subject to change based on community feedback, RustFS evolution, and Kubernetes ecosystem developments. Dates are estimates and may shift based on priorities and available resources.
