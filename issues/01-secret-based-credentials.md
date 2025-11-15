# Implement Secret-based credential management

**Labels**: enhancement
**Status**: ✅ COMPLETED (2025-11-15)

## Description

Currently, the operator only supports reading RustFS credentials (accesskey/secretkey) from environment variables. We need to add support for reading credentials from Kubernetes Secrets for better security and integration with secret management systems.

## Implementation Summary

### Completed Changes

1. **Added `spec.credsSecret` field to Tenant CRD** (`src/types/v1alpha1/tenant.rs`)
   - Optional reference to Kubernetes Secret containing credentials
   - Uses standard `LocalObjectReference` type

2. **Implemented credential validation** (`src/context.rs`)
   - Added `validate_credential_secret()` function (renamed from `get_tenant_credentials()`)
   - Validates Secret exists and has required keys (`accesskey`, `secretkey`)
   - Validates UTF-8 encoding
   - Does NOT extract values (for security)
   - Added error types: `CredentialSecretNotFound`, `CredentialSecretMissingKey`, `CredentialSecretInvalidEncoding`

3. **Updated reconciliation logic** (`src/reconcile.rs`)
   - Early validation when Secret is configured
   - Records Warning events on validation failures
   - Smart retry intervals (60s for credential errors, 5s for API errors)

4. **StatefulSet credential injection** (`src/types/v1alpha1/tenant/workloads.rs`)
   - Credentials injected via `secretKeyRef` (not literal values)
   - Kubernetes resolves references at pod startup
   - Environment variables can still override

5. **Documentation**
   - Added comprehensive example: `examples/secret-credentials-tenant.yaml`
   - Updated CHANGELOG.md with breaking change documentation
   - Updated CLAUDE.md with architecture notes

### Design Decisions

- **Validation vs Runtime**: Separate validation (early feedback) from runtime injection (Kubernetes handles via secretKeyRef)
- **Security**: Credentials never loaded into operator memory
- **Optional**: Secret is optional - RustFS uses built-in defaults (`rustfsadmin`/`rustfsadmin`) if not provided
- **Priority**: Secret credentials > Environment variables > RustFS defaults

### Breaking Changes

- Renamed `spec.configuration` → `spec.credsSecret` (more descriptive name)
- Acceptable at v0.1.0 pre-release stage

## Related

- Referenced in: `src/context.rs` (validate_credential_secret function)
- Part of: Core Stability roadmap phase
- Branch: `feat/41-secret-credential-management`
