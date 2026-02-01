#!/bin/bash
# Analyze the 119 extra paths in Rust scan vs C++ baseline

set -e

WORK_DIR="docs/trial_runs/d_disk"
CPP_FILE="$WORK_DIR/cpp_d.txt"
RUST_FILE="$WORK_DIR/scan_output.txt"
OUTPUT_DIR="$WORK_DIR/parity_analysis"

echo "🔍 Analyzing Parity Differences: Rust vs C++ Baseline"
echo ""

# Create output directory
mkdir -p "$OUTPUT_DIR"

echo "📊 Step 1: Extracting paths from both files..."
echo "  This may take a few minutes for 7M+ lines..."

# Extract path from C++ file (column 1, remove quotes)
# C++ format: "D:\path\to\file","name","parent",...
echo "  Extracting C++ paths..."
cut -d',' -f1 "$CPP_FILE" | sed 's/^"//;s/"$//' | sort > "$OUTPUT_DIR/cpp_paths_sorted.txt"

# Extract path from Rust file (column 5)
# Rust format: frs,parent_frs,name,ext,path,...
echo "  Extracting Rust paths..."
cut -d',' -f5 "$RUST_FILE" | sort > "$OUTPUT_DIR/rust_paths_sorted.txt"

echo ""
echo "📊 Step 2: Finding differences..."

# Paths in Rust but NOT in C++ (the extra 119 paths)
comm -13 "$OUTPUT_DIR/cpp_paths_sorted.txt" "$OUTPUT_DIR/rust_paths_sorted.txt" > "$OUTPUT_DIR/extra_in_rust.txt"

# Paths in C++ but NOT in Rust (should be 0 if we have full parity)
comm -23 "$OUTPUT_DIR/cpp_paths_sorted.txt" "$OUTPUT_DIR/rust_paths_sorted.txt" > "$OUTPUT_DIR/missing_in_rust.txt"

echo ""
echo "📊 Results:"
echo "  Paths in Rust but NOT in C++: $(wc -l < "$OUTPUT_DIR/extra_in_rust.txt")"
echo "  Paths in C++ but NOT in Rust: $(wc -l < "$OUTPUT_DIR/missing_in_rust.txt")"
echo ""

# Analyze the extra paths in Rust
if [ -s "$OUTPUT_DIR/extra_in_rust.txt" ]; then
    echo "🔍 Analyzing the extra paths in Rust..."
    echo ""
    
    # Show first 20 extra paths
    echo "📄 First 20 extra paths in Rust:"
    head -20 "$OUTPUT_DIR/extra_in_rust.txt"
    echo ""
    
    # Analyze patterns
    echo "📊 Pattern analysis of extra paths:"
    echo ""
    
    # Check for ADS (Alternate Data Streams)
    ADS_COUNT=$(grep -c ':' "$OUTPUT_DIR/extra_in_rust.txt" || true)
    echo "  Paths with ':' (potential ADS): $ADS_COUNT"
    
    # Check for directories (ending with \)
    DIR_COUNT=$(grep -c '\\$' "$OUTPUT_DIR/extra_in_rust.txt" || true)
    echo "  Paths ending with '\\' (directories): $DIR_COUNT"
    
    # Check for specific file types
    echo ""
    echo "  File type breakdown:"
    echo "    .bin files: $(grep -c '\.bin' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    .exe files: $(grep -c '\.exe' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    .dll files: $(grep -c '\.dll' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    .txt files: $(grep -c '\.txt' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    Zone.Identifier: $(grep -c 'Zone\.Identifier' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    com.dropbox.attrs: $(grep -c 'com\.dropbox\.attrs' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    
    echo ""
    echo "  Path patterns:"
    echo "    Rust target dirs: $(grep -c 'target\\\\' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    Dropbox paths: $(grep -c 'Dropbox' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
    echo "    System paths: $(grep -c 'Windows\\\\' "$OUTPUT_DIR/extra_in_rust.txt" || true)"
fi

# Analyze missing paths in Rust (should be 0)
if [ -s "$OUTPUT_DIR/missing_in_rust.txt" ]; then
    echo ""
    echo "⚠️  WARNING: Paths missing in Rust scan:"
    echo ""
    head -20 "$OUTPUT_DIR/missing_in_rust.txt"
fi

echo ""
echo "✅ Analysis complete!"
echo ""
echo "📁 Output files:"
echo "  Extra in Rust:   $OUTPUT_DIR/extra_in_rust.txt"
echo "  Missing in Rust: $OUTPUT_DIR/missing_in_rust.txt"
echo "  C++ paths:       $OUTPUT_DIR/cpp_paths_sorted.txt"
echo "  Rust paths:      $OUTPUT_DIR/rust_paths_sorted.txt"
echo ""

