#!/bin/bash
# Quick test script to verify script syntax and paths (run from project root or anywhere)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

echo "Testing script syntax (project root: $PROJECT_ROOT)..."
echo ""

echo "1. Checking scripts/deploy/deploy-rustfs.sh..."
bash -n scripts/deploy/deploy-rustfs.sh && echo "  ✓ Syntax OK" || { echo "  ✗ Syntax Error"; exit 1; }

echo "2. Checking scripts/cleanup/cleanup-rustfs.sh..."
bash -n scripts/cleanup/cleanup-rustfs.sh && echo "  ✓ Syntax OK" || { echo "  ✗ Syntax Error"; exit 1; }

echo "3. Checking scripts/check/check-rustfs.sh..."
bash -n scripts/check/check-rustfs.sh && echo "  ✓ Syntax OK" || { echo "  ✗ Syntax Error"; exit 1; }

echo "4. Checking scripts/deploy/deploy-rustfs-4node.sh..."
bash -n scripts/deploy/deploy-rustfs-4node.sh && echo "  ✓ Syntax OK" || { echo "  ✗ Syntax Error"; exit 1; }

echo "5. Checking scripts/cleanup/cleanup-rustfs-4node.sh..."
bash -n scripts/cleanup/cleanup-rustfs-4node.sh && echo "  ✓ Syntax OK" || { echo "  ✗ Syntax Error"; exit 1; }

echo "6. Checking 4-node tenant uses local Kind-friendly image pull policy..."
grep -q '^  imagePullPolicy: IfNotPresent$' examples/tenant-4nodes.yaml \
  && echo "  ✓ Tenant imagePullPolicy is IfNotPresent" \
  || { echo "  ✗ examples/tenant-4nodes.yaml must set imagePullPolicy: IfNotPresent for local Kind demos"; exit 1; }

echo "7. Checking 4-node deploy fails fast when RustFS server image is unavailable..."
grep -q 'rustfs/rustfs:latest not found locally' scripts/deploy/deploy-rustfs-4node.sh \
  && grep -q 'Failed to load rustfs/rustfs:latest into Kind' scripts/deploy/deploy-rustfs-4node.sh \
  && echo "  ✓ RustFS server image load is fail-fast" \
  || { echo "  ✗ deploy-rustfs-4node.sh must fail fast when rustfs/rustfs:latest cannot be loaded"; exit 1; }

echo "8. Checking 4-node tenant enables RustFS local-disk bypass for Kind only..."
grep -q '^    - name: RUSTFS_UNSAFE_BYPASS_DISK_CHECK$' examples/tenant-4nodes.yaml \
  && grep -q '^      value: "true"$' examples/tenant-4nodes.yaml \
  && echo "  ✓ Kind demo bypasses same-disk safety check explicitly" \
  || { echo "  ✗ examples/tenant-4nodes.yaml must set RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true for local Kind PVs"; exit 1; }

echo "9. Checking Rust-native e2e harness skeleton exists..."
for required_file in \
  e2e/Cargo.toml \
  e2e/README.md \
  e2e/manifests/kind-rustfs-e2e.yaml \
  e2e/src/lib.rs \
  e2e/src/framework/mod.rs \
  e2e/src/framework/config.rs \
  e2e/src/framework/command.rs \
  e2e/src/framework/kind.rs \
  e2e/src/framework/kubectl.rs \
  e2e/src/framework/live.rs \
  e2e/src/framework/tools.rs \
  e2e/src/framework/kube_client.rs \
  e2e/src/framework/console_client.rs \
  e2e/src/framework/wait.rs \
  e2e/src/framework/artifacts.rs \
  e2e/src/framework/port_forward.rs \
  e2e/src/framework/resources.rs \
  e2e/src/framework/storage.rs \
  e2e/src/framework/deploy.rs \
  e2e/src/framework/images.rs \
  e2e/src/framework/assertions.rs \
  e2e/src/framework/tenant_factory.rs \
  e2e/src/cases/mod.rs \
  e2e/src/cases/smoke.rs \
  e2e/src/cases/operator.rs \
  e2e/src/cases/console.rs \
  e2e/src/bin/rustfs-e2e.rs \
  e2e/tests/smoke.rs \
  e2e/tests/operator.rs \
  e2e/tests/console.rs \
  e2e/tests/faults.rs; do
  test -f "$required_file" || { echo "  ✗ Missing $required_file"; exit 1; }
done
echo "  ✓ Rust-native e2e harness skeleton files exist"

echo "10. Checking reduced e2e Makefile entrypoints are exposed..."
actual_e2e_targets=$(grep -E '^e2e-[A-Za-z0-9_-]+:' Makefile | cut -d: -f1 | sort | tr '\n' ' ' | sed 's/ $//')
expected_e2e_targets=$(printf '%s\n' e2e-check e2e-live-create e2e-live-delete e2e-live-run e2e-live-update | sort | tr '\n' ' ' | sed 's/ $//')
if [ "$actual_e2e_targets" = "$expected_e2e_targets" ]; then
  echo "  ✓ reduced e2e Makefile targets exist"
else
  echo "  ✗ Makefile must expose only e2e-check plus the four live entrypoints"
  echo "    expected: $expected_e2e_targets"
  echo "    actual:   $actual_e2e_targets"
  exit 1
fi

assert_cert_manager_crd_discovery_rbac() {
  local manifest="$1"
  python3 - "$manifest" <<'PY'
import pathlib
import re
import sys

manifest = pathlib.Path(sys.argv[1])
pattern = re.compile(
    r'^  - apiGroups: \["apiextensions\.k8s\.io"\]\n'
    r'    resources: \["customresourcedefinitions"\]\n'
    r'    resourceNames: \["certificates\.cert-manager\.io"\]\n'
    r'    verbs: \["get"\]$',
    re.MULTILINE,
)
if not pattern.search(manifest.read_text()):
    sys.exit(1)
PY
}

echo "11. Checking Operator RBAC can discover the cert-manager Certificate CRD..."
for manifest in \
  deploy/k8s-dev/operator-rbac.yaml \
  deploy/rustfs-operator/templates/clusterrole.yaml; do
  assert_cert_manager_crd_discovery_rbac "$manifest" \
    || { echo "  ✗ $manifest must grant get on customresourcedefinitions/certificates.cert-manager.io"; exit 1; }
done
echo "  ✓ Operator RBAC grants scoped cert-manager Certificate CRD discovery"

echo ""
echo "All script checks passed! ✅"
