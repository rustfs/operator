# Feature: Add Health Checks (Liveness, Readiness, Startup Probes)

## 📋 Summary

This PR introduces comprehensive health check mechanisms for RustFS StatefulSet pods. By implementing Liveness, Readiness, and Startup probes, we significantly enhance the reliability, availability, and self-healing capabilities of the RustFS cluster managed by the operator.

## 🚀 Key Changes

### 1. CRD Schema Update (`src/types/v1alpha1/tenant.rs`)
- **New Fields**: Added `livenessProbe`, `readinessProbe`, and `startupProbe` to the `TenantSpec` struct.
- **Type**: These fields use the standard Kubernetes `corev1::Probe` type, allowing full customization (httpGet, exec, tcpSocket, thresholds, etc.).
- **Optional**: All fields are optional to maintain backward compatibility.

### 2. Intelligent StatefulSet Generation (`src/types/v1alpha1/tenant/workloads.rs`)
- **Probe Injection**: The `new_statefulset` method now injects these probes into the RustFS container definition.
- **Smart Defaults**: To ensure out-of-the-box reliability, the operator applies optimized default values if the user does not specify custom probes:
    - **Liveness Probe**: Checks `/rustfs/health/live` on port 9000.
        - `initialDelaySeconds`: 120s (Gives ample time for initialization)
        - `periodSeconds`: 15s
    - **Readiness Probe**: Checks `/rustfs/health/ready` on port 9000.
        - `initialDelaySeconds`: 30s
        - `periodSeconds`: 10s
    - **Startup Probe**: Checks `/rustfs/health/startup` on port 9000.
        - `failureThreshold`: 30 (Allows up to 5 minutes for slow startups before killing the pod)

### 3. Enhanced Reconciliation & Update Logic
- **Update Detection**: Updated `statefulset_needs_update` to include deep comparison of probe configurations.
- **Rolling Updates**: Changing probe settings in the Tenant CRD will now correctly trigger a rolling update of the StatefulSet, ensuring the new health check policies are applied.

## 🧪 Testing Verification

All tests passed successfully (`cargo test`).

- **New Unit Tests**:
    - `test_default_probes_applied`: Confirms that smart defaults are applied when CRD fields are missing.
    - `test_custom_probes_override`: Confirms that user-provided configurations take precedence over defaults.
    - `test_probe_update_detection`: Confirms that modifying probe parameters triggers a reconciliation update.
- **Regression Testing**: Verified that existing tests (RBAC, ServiceAccount, Labels, etc.) continue to pass.

## 📦 Impact

- **Reliability**: Pods that deadlock or become unresponsive will now be automatically restarted by Kubernetes.
- **Availability**: Traffic will not be routed to pods that are not ready (e.g., during startup or temporary failure).
- **UX**: Users get production-ready defaults without needing complex configuration, but retain full control if needed.

## ✅ Checklist

- [x] Code compiles successfully.
- [x] `cargo fmt` has been run.
- [x] `cargo clippy` passes without errors.
- [x] New unit tests added and passing (43/43 tests passed).
- [x] Documentation (CRD fields) is self-explanatory.
