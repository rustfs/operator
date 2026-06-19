# RustFS Operator E2E Harness

This crate provides the Rust-native Kind e2e harness and shared primitives used by the separate real-cluster fault-test runner.

The harness is intentionally separated from the main operator crate so e2e-only dependencies stay scoped to the `e2e/` manifest while still being validated by `make e2e-check` and the default `make pre-commit` path. It is driven through the reduced live entrypoints `e2e-live-create`, `e2e-live-run`, `e2e-live-update`, and `e2e-live-delete`.

## Architecture

The harness is split into four top-level domains:

- `manifests/`: e2e-owned static manifests such as the dedicated Kind config.
- `framework/`: reusable infrastructure primitives.
- `cases/`: release test-case inventory grouped by product boundary.
- `tests/`: executable suite entrypoints; live tests are ignored by default and run only through explicit Make targets.

```text
e2e/
  Cargo.toml
  manifests/
    kind-rustfs-e2e.yaml  dedicated 1 control-plane + 3 worker Kind cluster
  src/
    lib.rs
    bin/rustfs-e2e.rs  Makefile-internal helper for live workflow steps
    framework/
      config.rs          dedicated Kind e2e configuration
      fault_config.rs    real-cluster fault-test configuration and safety checks
      command.rs         safe subprocess wrapper for kind/docker/kubectl
      kind.rs            Kind cluster lifecycle and host mount preparation
      kubectl.rs         kubectl command construction boundary
      live.rs            live-run guardrails and context safety
      tools.rs           local host tool inventory
      kube_client.rs     kube-rs client boundary
      console_client.rs  reqwest Console API boundary
      wait.rs            timeout/polling helpers and Tenant Ready wait
      artifacts.rs       failure artifact collection boundary
      port_forward.rs    kubectl port-forward boundary
      images.rs          operator/console/rustfs image set boundary
      resources.rs       namespace/Secret/Tenant apply boundary
      storage.rs         local StorageClass/PV preparation boundary
      assertions.rs      Kubernetes and Tenant status assertions
      tenant_factory.rs  Kind-local and real-cluster Tenant templates
    cases/
      smoke.rs           install and readiness checks
      operator.rs        Tenant status and observed-generation checks
      console.rs         Console API health/readiness/OpenAPI checks
  tests/
    smoke.rs             ignored live smoke entrypoints
    operator.rs          ignored live Operator assertion
    console.rs           ignored live Console API assertion
    faults.rs            real-cluster destructive fault-injection suite with scenario-selected runners; not part of e2e case inventory
```

## Boundary rules

1. `framework::command` is the only layer that should execute host commands directly.
2. `framework::kubectl` is the shell/Kubernetes YAML boundary and must always pin `--context`.
3. `framework::kube_client` is the typed Kubernetes API boundary.
4. `framework::console_client` is the HTTP boundary for Console API tests.
5. `framework::storage` owns Kind local PV setup; `framework::resources` owns shared namespace/Secret/Tenant lifecycle.
6. `framework::live` owns live-run opt-in and dedicated-context checks.
7. `cases/*` should describe behavior and call framework helpers; avoid shell details there.
8. Kind e2e cases remain in `cases/*`; real-cluster fault tests are intentionally excluded from that inventory.
9. Fault tests use `FaultTestConfig`, reject Kind contexts, require a dedicated namespace and StorageClass, and never use Kind local-volume assumptions.
10. The fault-test runner creates its namespace with ownership metadata. Existing namespaces must already have the matching manager label and Tenant annotation before destructive reset is allowed.

## Safety defaults

Default configuration targets a dedicated Kind cluster:

```text
cluster:          rustfs-e2e
context:          kind-rustfs-e2e
operator ns:      rustfs-system
test namespace:   rustfs-e2e-smoke
tenant name:      e2e-tenant
console URL:      http://127.0.0.1:19090
rustfs image:      rustfs/rustfs:latest
storage class:    local-storage
PV count:         12
kind config:      e2e/manifests/kind-rustfs-e2e.yaml
```

Live tests are `#[ignore]` and run through the reduced Make workflow. The Makefile injects `RUSTFS_E2E_LIVE=1` internally, so the common flow does not need the environment prefix:

```bash
make e2e-live-run
```

The harness refuses to run live tests unless the active Kubernetes context matches the configured dedicated Kind context.

Fault tests have separate safety defaults and environment variables:

```text
context:          current non-Kind kubectl context
test namespace:   rustfs-fault-test
tenant name:      fault-test-tenant
storage class:    required via RUSTFS_FAULT_TEST_STORAGE_CLASS
artifacts:        target/fault-tests/artifacts
```

Run them independently from the Kind lifecycle:

```bash
RUSTFS_FAULT_TEST_STORAGE_CLASS=<storage-class> make fault-test
```

The runner creates an absent namespace through `kubectl create` before applying the credential Secret and Tenant. It refuses to reset or claim an existing namespace unless these values already match:

```text
app.kubernetes.io/managed-by=rustfs-operator-fault-test
rustfs.com/fault-test-tenant=<configured-tenant>
```

## Non-live validation

```bash
make e2e-check
```

This runs e2e formatting, non-live tests, and clippy. Live tests remain `#[ignore]` and require the live commands below.

## Usage (four common entry points)

- `make e2e-live-create`:
  Creates the dedicated live environment: builds the e2e image, removes old `kind-rustfs-e2e`, cleans dedicated storage, then performs create + image load.
- `make e2e-live-run`:
  Runs all live suites (smoke/operator/console) in an existing live environment.
- `make e2e-live-update`:
  Rebuilds the e2e image and updates it into the live environment (`load + rollout`).
  Prerequisite: control-plane components must already be deployed (usually after `make e2e-live-run`).
- `make e2e-live-delete`:
  Deletes the live cluster and cleans dedicated storage at `/tmp/rustfs-e2e-storage-{1,2,3}`.

Image builds use Docker host network internally to avoid local bridge DNS resolution issues for npm/crates registries; the exposed user entry points remain only these four commands.

Recommended workflow:

```bash
# Initial setup
make e2e-live-create

# Run all suites once (deploys control plane and creates tenant)
make e2e-live-run

# Rebuild image and restart deployment after code changes
make e2e-live-update

# Run suites again after rollout
make e2e-live-run

# Clean up
make e2e-live-delete
```
