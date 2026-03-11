#!/bin/bash
# Quick parity check for the new scan

set -e

echo "🔍 Quick Parity Check: New Scan vs C++ Baseline"
echo ""

# Count lines
echo "📊 Line counts:"
echo "  C++ baseline:  $(wc -l < docs/trial_runs/d_disk/cpp_d.txt)"
echo "  Old Rust scan: $(wc -l < docs/trial_runs/d_disk/rust_d.txt)"
echo "  New scan:      $(wc -l < docs/trial_runs/d_disk/scan_output.txt)"
echo ""

# Check for specific missing files
echo "🔍 Checking for previously missing files:"
echo ""

# Check dep-graph.bin
if grep -q "dep-graph.bin" docs/trial_runs/d_disk/scan_output.txt; then
    echo "  ✅ dep-graph.bin files FOUND"
else
    echo "  ❌ dep-graph.bin files MISSING"
fi

# Check query-cache.bin
if grep -q "query-cache.bin" docs/trial_runs/d_disk/scan_output.txt; then
    echo "  ✅ query-cache.bin files FOUND"
else
    echo "  ❌ query-cache.bin files MISSING"
fi

# Check work-products.bin
if grep -q "work-products.bin" docs/trial_runs/d_disk/scan_output.txt; then
    echo "  ✅ work-products.bin files FOUND"
else
    echo "  ❌ work-products.bin files MISSING"
fi

# Check Zone.Identifier
if grep -q "Zone.Identifier" docs/trial_runs/d_disk/scan_output.txt; then
    echo "  ✅ Zone.Identifier files FOUND"
else
    echo "  ❌ Zone.Identifier files MISSING"
fi

echo ""
echo "✅ Quick check complete!"

