# Improve StatefulSet reconciliation and update handling

**Labels**: enhancement

## Description

While StatefulSet creation works correctly, the update and reconciliation logic needs refinement to handle changes to Tenant specifications properly and manage rollouts safely.

## Current Behavior

- StatefulSet creation works correctly
- Updates and modifications need refinement
- Limited rollout management and safety checks

## Desired Behavior

Implement robust StatefulSet reconciliation:
1. Detect and apply configuration changes correctly
2. Manage rolling updates with proper pod management policy
3. Handle StatefulSet spec changes (replicas, image, env vars, resources)
4. Validate changes before applying (e.g., prevent unsafe volume changes)
5. Track rollout progress in Tenant status
6. Handle rollback scenarios

## Implementation Considerations

- Implement proper diff detection for StatefulSet specs
- Use `podManagementPolicy` (Parallel/OrderedReady) correctly
- Add validation for breaking changes (volume topology, storage class)
- Update status with rollout progress
- Consider using `updateStrategy` field appropriately
- Add integration tests for update scenarios

## Priority

**High** - Core Stability (from ROADMAP.md)

## Related

- StatefulSet creation: `src/types/v1alpha1/tenant.rs:new_statefulset()`
- Reconciliation logic: `src/reconcile.rs`
- Part of: Core Stability roadmap phase
