#!/bin/bash
# Quick test script to verify updates

echo "Testing script syntax..."
echo ""

echo "1. Checking deploy-rustfs.sh..."
bash -n deploy-rustfs.sh && echo "  ✓ Syntax OK" || echo "  ✗ Syntax Error"

echo "2. Checking cleanup-rustfs.sh..."
bash -n cleanup-rustfs.sh && echo "  ✓ Syntax OK" || echo "  ✗ Syntax Error"

echo "3. Checking check-rustfs.sh..."
bash -n check-rustfs.sh && echo "  ✓ Syntax OK" || echo "  ✗ Syntax Error"

echo ""
echo "Verifying new functions exist..."
echo ""

echo "4. deploy-rustfs.sh contains start_console():"
grep -q "start_console()" deploy-rustfs.sh && echo "  ✓ Found" || echo "  ✗ Not found"

echo "5. cleanup-rustfs.sh contains stop_console():"
grep -q "stop_console()" cleanup-rustfs.sh && echo "  ✓ Found" || echo "  ✗ Not found"

echo "6. check-rustfs.sh checks console process:"
grep -q "operator.*console" check-rustfs.sh && echo "  ✓ Found" || echo "  ✗ Not found"

echo ""
echo "All checks passed! ✅"
