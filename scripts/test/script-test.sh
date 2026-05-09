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

echo ""
echo "All script checks passed! ✅"
