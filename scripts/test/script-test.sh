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

echo ""
echo "All script syntax checks passed! ✅"
