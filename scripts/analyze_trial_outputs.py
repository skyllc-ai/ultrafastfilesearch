#!/usr/bin/env python3
"""
Analyze UFFS trial run outputs - compare C++ vs Rust outputs for exact parity.

Usage:
    python scripts/analyze_trial_outputs.py <trial_dir>
    python scripts/analyze_trial_outputs.py docs/trial_runs/d_disk

Expected files in trial_dir:
    - cpp_d.txt (or cpp_<drive>.txt) - C++ reference output
    - rust_new_d.txt (or rust_new_<drive>.txt) - Rust with new tree algo

The script will:
1. Parse both files and extract all paths
2. Compare line-by-line for exact match
3. Report any differences with context
4. Analyze patterns in differences (if any)
"""

import sys
import os
from pathlib import Path
from collections import defaultdict
import re


def parse_uffs_output(filepath: Path) -> list[dict]:
    """Parse UFFS tab-separated output into list of records."""
    records = []
    with open(filepath, 'r', encoding='utf-8', errors='replace') as f:
        for line_num, line in enumerate(f, 1):
            line = line.rstrip('\n\r')
            if not line:
                continue
            # Tab-separated: path, type, size, descendants, date, ...
            parts = line.split('\t')
            if len(parts) >= 4:
                records.append({
                    'line_num': line_num,
                    'path': parts[0],
                    'type': parts[1] if len(parts) > 1 else '',
                    'size': parts[2] if len(parts) > 2 else '',
                    'descendants': parts[3] if len(parts) > 3 else '',
                    'raw': line
                })
    return records


def normalize_path(path: str) -> str:
    """Normalize path for comparison."""
    return path.lower().replace('\\', '/').rstrip('/')


def compare_outputs(cpp_file: Path, rust_file: Path) -> dict:
    """Compare C++ and Rust outputs, return detailed comparison."""
    print(f"\n{'='*70}")
    print(f"Comparing:")
    print(f"  C++:  {cpp_file}")
    print(f"  Rust: {rust_file}")
    print(f"{'='*70}")

    cpp_records = parse_uffs_output(cpp_file)
    rust_records = parse_uffs_output(rust_file)

    print(f"\nRecord counts:")
    print(f"  C++:  {len(cpp_records):,}")
    print(f"  Rust: {len(rust_records):,}")
    print(f"  Diff: {len(rust_records) - len(cpp_records):+,}")

    # Build path sets for comparison
    cpp_paths = {normalize_path(r['path']): r for r in cpp_records}
    rust_paths = {normalize_path(r['path']): r for r in rust_records}

    cpp_only = set(cpp_paths.keys()) - set(rust_paths.keys())
    rust_only = set(rust_paths.keys()) - set(cpp_paths.keys())
    common = set(cpp_paths.keys()) & set(rust_paths.keys())

    print(f"\nPath comparison:")
    print(f"  Common paths:  {len(common):,}")
    print(f"  C++ only:      {len(cpp_only):,}")
    print(f"  Rust only:     {len(rust_only):,}")

    if len(cpp_paths) > 0:
        match_pct = len(common) / len(cpp_paths) * 100
        print(f"  Match rate:    {match_pct:.4f}%")

    # Check for attribute differences in common paths
    attr_diffs = []
    for path in common:
        cpp_rec = cpp_paths[path]
        rust_rec = rust_paths[path]
        if cpp_rec['size'] != rust_rec['size'] or cpp_rec['descendants'] != rust_rec['descendants']:
            attr_diffs.append({
                'path': path,
                'cpp_size': cpp_rec['size'],
                'rust_size': rust_rec['size'],
                'cpp_desc': cpp_rec['descendants'],
                'rust_desc': rust_rec['descendants']
            })

    if attr_diffs:
        print(f"\n⚠️  Attribute differences in common paths: {len(attr_diffs)}")
        print(f"\nFirst 10 attribute differences:")
        for diff in attr_diffs[:10]:
            print(f"  {diff['path']}")
            print(f"    Size: C++={diff['cpp_size']} vs Rust={diff['rust_size']}")
            print(f"    Desc: C++={diff['cpp_desc']} vs Rust={diff['rust_desc']}")

    # Report missing paths
    if cpp_only:
        print(f"\n❌ Paths in C++ but NOT in Rust (first 20):")
        for p in sorted(cpp_only)[:20]:
            print(f"  {p}")
        if len(cpp_only) > 20:
            print(f"  ... and {len(cpp_only) - 20} more")

    if rust_only:
        print(f"\n❌ Paths in Rust but NOT in C++ (first 20):")
        for p in sorted(rust_only)[:20]:
            print(f"  {p}")
        if len(rust_only) > 20:
            print(f"  ... and {len(rust_only) - 20} more")

    # Perfect match check
    if not cpp_only and not rust_only and not attr_diffs:
        print(f"\n✅ PERFECT MATCH - All {len(common):,} paths match exactly!")
        return {'status': 'perfect', 'common': len(common)}

    return {
        'status': 'differences',
        'common': len(common),
        'cpp_only': len(cpp_only),
        'rust_only': len(rust_only),
        'attr_diffs': len(attr_diffs)
    }


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    trial_dir = Path(sys.argv[1])
    if not trial_dir.exists():
        print(f"Error: Directory not found: {trial_dir}")
        sys.exit(1)

    # Find output files
    cpp_files = list(trial_dir.glob("cpp_*.txt"))
    rust_new_files = list(trial_dir.glob("rust_new_*.txt"))

    print(f"Trial directory: {trial_dir}")
    print(f"Found {len(cpp_files)} C++ files, {len(rust_new_files)} Rust (new) files")

    # Match by drive letter
    for cpp_file in sorted(cpp_files):
        drive = cpp_file.stem.replace('cpp_', '')
        rust_file = trial_dir / f"rust_new_{drive}.txt"
        if rust_file.exists():
            compare_outputs(cpp_file, rust_file)
        else:
            print(f"\n⚠️  No matching Rust file for {cpp_file.name}")


if __name__ == "__main__":
    main()

