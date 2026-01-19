#!/usr/bin/env python3
"""
Deep comparison of UFFS C++ vs Rust outputs using Polars.
Identifies structural differences, missing records, and attribute mismatches.
"""

import polars as pl
import sys
from pathlib import Path

# Configure Polars for large files
pl.Config.set_tbl_rows(20)
pl.Config.set_fmt_str_lengths(80)

def load_csv(path: str, name: str) -> pl.DataFrame:
    """Load CSV with proper type inference."""
    print(f"Loading {name}: {path}")
    df = pl.read_csv(
        path,
        ignore_errors=True,
        truncate_ragged_lines=True,
        infer_schema_length=10000,
    )
    print(f"  Loaded {df.height():,} rows, {df.width} columns")
    print(f"  Columns: {df.columns}")
    return df

def normalize_path(df: pl.DataFrame, path_col: str = "Path") -> pl.DataFrame:
    """Normalize paths for comparison: lowercase, forward slashes."""
    if path_col not in df.columns:
        print(f"  Warning: '{path_col}' column not found")
        return df
    return df.with_columns([
        pl.col(path_col).str.to_lowercase().str.replace_all(r"\\", "/").alias("path_norm"),
    ])

def extract_path_parts(df: pl.DataFrame) -> pl.DataFrame:
    """Extract drive, parent, and name from normalized path."""
    return df.with_columns([
        # Extract drive letter (first char before :)
        pl.col("path_norm").str.extract(r"^([a-z]):", 1).alias("drive"),
        # Extract parent directory (everything before last /)
        pl.col("path_norm").str.extract(r"^(.*/)[^/]*$", 1).alias("parent"),
        # Extract filename (last component after /)
        pl.col("path_norm").str.extract(r"/([^/]*)$", 1).alias("filename"),
    ])

def main():
    if len(sys.argv) < 3:
        print("Usage: python analyze_diff.py cpp.txt rust.txt")
        sys.exit(1)
    
    cpp_path, rust_path = sys.argv[1], sys.argv[2]
    
    print("=" * 70)
    print("UFFS Deep Comparison Analysis")
    print("=" * 70)
    
    # Load data
    cpp = load_csv(cpp_path, "C++")
    rust = load_csv(rust_path, "Rust")
    
    print("\n" + "=" * 70)
    print("STEP 1: Column Comparison")
    print("=" * 70)
    cpp_cols = set(cpp.columns)
    rust_cols = set(rust.columns)
    print(f"C++ columns ({len(cpp_cols)}):  {sorted(cpp_cols)}")
    print(f"Rust columns ({len(rust_cols)}): {sorted(rust_cols)}")
    print(f"Only in C++:  {sorted(cpp_cols - rust_cols)}")
    print(f"Only in Rust: {sorted(rust_cols - cpp_cols)}")
    common_cols = cpp_cols & rust_cols
    print(f"Common columns: {sorted(common_cols)}")
    
    print("\n" + "=" * 70)
    print("STEP 2: Path Analysis")
    print("=" * 70)
    
    # Normalize paths
    cpp = normalize_path(cpp)
    rust = normalize_path(rust)
    
    # Check for null/empty paths
    cpp_null_paths = cpp.filter(pl.col("path_norm").is_null() | (pl.col("path_norm") == "")).height
    rust_null_paths = rust.filter(pl.col("path_norm").is_null() | (pl.col("path_norm") == "")).height
    print(f"C++ null/empty paths: {cpp_null_paths:,}")
    print(f"Rust null/empty paths: {rust_null_paths:,}")
    
    # Sample paths from each
    print("\nSample C++ paths:")
    print(cpp.select("path_norm").head(5))
    print("\nSample Rust paths:")
    print(rust.select("path_norm").head(5))
    
    print("\n" + "=" * 70)
    print("STEP 3: Drive-level Comparison")
    print("=" * 70)
    
    cpp = extract_path_parts(cpp)
    rust = extract_path_parts(rust)
    
    cpp_drives = cpp.group_by("drive").len().sort("drive")
    rust_drives = rust.group_by("drive").len().sort("drive")
    print("C++ by drive:")
    print(cpp_drives)
    print("\nRust by drive:")
    print(rust_drives)
    
    print("\n" + "=" * 70)
    print("STEP 4: Path Matching Analysis")
    print("=" * 70)
    
    # Create sets for fast lookup
    cpp_paths = set(cpp["path_norm"].drop_nulls().to_list())
    rust_paths = set(rust["path_norm"].drop_nulls().to_list())
    
    common = cpp_paths & rust_paths
    cpp_only = cpp_paths - rust_paths
    rust_only = rust_paths - cpp_paths
    
    print(f"Total C++ unique paths:  {len(cpp_paths):,}")
    print(f"Total Rust unique paths: {len(rust_paths):,}")
    print(f"Exact matches:           {len(common):,}")
    print(f"C++ only:                {len(cpp_only):,}")
    print(f"Rust only:               {len(rust_only):,}")
    print(f"Match rate:              {100*len(common)/len(cpp_paths):.2f}%")
    
    # Sample missing paths
    if cpp_only:
        print(f"\nSample paths in C++ but NOT in Rust (first 20):")
        for p in sorted(cpp_only)[:20]:
            print(f"  {p}")
    
    if rust_only:
        print(f"\nSample paths in Rust but NOT in C++ (first 20):")
        for p in sorted(rust_only)[:20]:
            print(f"  {p}")

    print("\n" + "=" * 70)
    print("STEP 5: Missing Path Pattern Analysis")
    print("=" * 70)
    
    # Analyze patterns in missing paths
    cpp_only_df = cpp.filter(pl.col("path_norm").is_in(list(cpp_only)[:100000]))
    
    # Check if missing paths have specific characteristics
    print("Missing paths by drive:")
    print(cpp_only_df.group_by("drive").len().sort("len", descending=True))
    
    # Check for $-prefixed names (system files)
    if "Name" in cpp_only_df.columns:
        system_files = cpp_only_df.filter(pl.col("Name").str.starts_with("$"))
        print(f"\nMissing $-prefixed (system) files: {system_files.height:,}")
    
    # Check parent patterns
    print("\nTop 20 missing parent directories:")
    parents = cpp_only_df.group_by("parent").len().sort("len", descending=True).head(20)
    print(parents)

    print("\n" + "=" * 70)
    print("STEP 6: Attribute Comparison on Matching Records")
    print("=" * 70)

    if len(common) > 0:
        # Join on normalized path to compare attributes
        cpp_common = cpp.filter(pl.col("path_norm").is_in(list(common)[:50000]))
        rust_common = rust.filter(pl.col("path_norm").is_in(list(common)[:50000]))

        # Rename columns for join
        cpp_renamed = cpp_common.select([
            pl.col("path_norm"),
            *[pl.col(c).alias(f"cpp_{c}") for c in cpp_common.columns if c != "path_norm"]
        ])
        rust_renamed = rust_common.select([
            pl.col("path_norm"),
            *[pl.col(c).alias(f"rust_{c}") for c in rust_common.columns if c != "path_norm"]
        ])

        joined = cpp_renamed.join(rust_renamed, on="path_norm", how="inner")
        print(f"Joined {joined.height:,} matching records for attribute comparison")

        # Compare key attributes
        attrs_to_compare = ["Name", "Size", "Created", "Last Written", "Directory Flag"]
        for attr in attrs_to_compare:
            cpp_col = f"cpp_{attr}"
            rust_col = f"rust_{attr}"
            if cpp_col in joined.columns and rust_col in joined.columns:
                # Cast both to string for comparison
                mismatches = joined.filter(
                    pl.col(cpp_col).cast(pl.Utf8) != pl.col(rust_col).cast(pl.Utf8)
                )
                print(f"  {attr}: {mismatches.height:,} mismatches out of {joined.height:,}")
                if mismatches.height > 0 and mismatches.height <= 5:
                    print(f"    Sample: {mismatches.select(['path_norm', cpp_col, rust_col]).head(3)}")

    print("\n" + "=" * 70)
    print("STEP 7: Root Cause Hypothesis")
    print("=" * 70)

    # Check if Rust is missing entire directory trees
    rust_parents = set(rust["parent"].drop_nulls().to_list())
    cpp_parents = set(cpp["parent"].drop_nulls().to_list())
    missing_parents = cpp_parents - rust_parents

    print(f"Parent directories in C++ but not in Rust: {len(missing_parents):,}")
    if missing_parents:
        print("Sample missing parent directories:")
        for p in sorted(missing_parents)[:20]:
            print(f"  {p}")

    # Check if Rust has path resolution issues (paths with <unknown>)
    rust_unknown = rust.filter(pl.col("path_norm").str.contains("<unknown>"))
    print(f"\nRust paths with '<unknown>': {rust_unknown.height:,}")
    if rust_unknown.height > 0:
        print(rust_unknown.select("path_norm").head(10))

    # Check for very short paths (might indicate root-level issues)
    rust_short = rust.filter(pl.col("path_norm").str.len_chars() < 10)
    cpp_short = cpp.filter(pl.col("path_norm").str.len_chars() < 10)
    print(f"\nC++ paths < 10 chars: {cpp_short.height:,}")
    print(f"Rust paths < 10 chars: {rust_short.height:,}")

    print("\n" + "=" * 70)
    print("SUMMARY & RECOMMENDATIONS")
    print("=" * 70)

    total_cpp = len(cpp_paths)
    total_rust = len(rust_paths)
    missing_pct = 100 * len(cpp_only) / total_cpp if total_cpp > 0 else 0

    print(f"""
Analysis Complete:
  - C++ found {total_cpp:,} unique paths
  - Rust found {total_rust:,} unique paths
  - Missing from Rust: {len(cpp_only):,} ({missing_pct:.1f}%)
  - Extra in Rust: {len(rust_only):,}

Likely Issues:
  1. Path resolution: Rust may have <unknown> paths that don't match
  2. Missing parent directories: {len(missing_parents):,} parent dirs not in Rust
  3. System files: Check if $-prefixed files are being filtered
  4. Multi-drive handling: Check if all drives are being scanned
""")

if __name__ == "__main__":
    main()

