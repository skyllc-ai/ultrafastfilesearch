#!/usr/bin/env python3
"""Compare uffs C++ vs Rust output files."""

import re
import sys
from collections import Counter
from pathlib import Path


def extract_path(line: str) -> str | None:
    """Extract the path from a line, handling various formats."""
    line = line.strip()
    if not line:
        return None

    # Skip header/info lines
    skip_patterns = ("Indexing", "Searching", "Found", "Time:", "Error",
                     "Finished", "finished", '"drive"', '"path"')
    if any(line.startswith(p) for p in skip_patterns):
        return None

    # Try to extract path from CSV format: "path","name","dir",...
    # C++ format: "c:/path/file.txt","file.txt","c:/path/",...
    if line.startswith('"'):
        match = re.match(r'^"([^"]+)"', line)
        if match:
            return match.group(1)

    # Plain path (no quotes)
    if line[0].isalpha() and (len(line) > 1 and line[1] in ':'):
        # Looks like a drive path
        return line.split(',')[0].strip('"')

    return line


def normalize_path(path: str) -> str:
    """Normalize path for comparison (handle case, slashes)."""
    return path.lower().replace("\\", "/").rstrip("/")


def extract_drive(path: str) -> str | None:
    """Extract drive letter from path."""
    if len(path) >= 2 and path[0].isalpha() and path[1] == ':':
        return path[0].upper()
    return None


def load_paths(filepath: Path) -> tuple[set[str], Counter]:
    """Load and normalize paths from output file. Returns (paths, drive_counts)."""
    paths = set()
    drives = Counter()

    with open(filepath, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            raw_path = extract_path(line)
            if raw_path:
                norm = normalize_path(raw_path)
                paths.add(norm)
                drive = extract_drive(norm)
                if drive:
                    drives[drive] += 1

    return paths, drives


def main():
    cpp_file = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("output_cpp.txt")
    rust_file = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("output_rust.txt")

    if not cpp_file.exists() or not rust_file.exists():
        print(f"Error: Need both {cpp_file} and {rust_file}")
        sys.exit(1)

    cpp_paths, cpp_drives = load_paths(cpp_file)
    rust_paths, rust_drives = load_paths(rust_file)

    cpp_only = cpp_paths - rust_paths
    rust_only = rust_paths - cpp_paths
    common = cpp_paths & rust_paths

    print("=" * 60)
    print("UFFS Output Comparison: C++ vs Rust")
    print("=" * 60)

    print(f"\nFile sizes:")
    print(f"  C++:  {cpp_file.stat().st_size:,} bytes")
    print(f"  Rust: {rust_file.stat().st_size:,} bytes")

    print(f"\n--- Drive Coverage ---")
    all_drives = sorted(set(cpp_drives.keys()) | set(rust_drives.keys()))
    print(f"  {'Drive':<8} {'C++':>12} {'Rust':>12} {'Diff':>12}")
    print(f"  {'-'*6:<8} {'-'*10:>12} {'-'*10:>12} {'-'*10:>12}")
    for drv in all_drives:
        cpp_cnt = cpp_drives.get(drv, 0)
        rust_cnt = rust_drives.get(drv, 0)
        diff = rust_cnt - cpp_cnt
        diff_str = f"{diff:+,}" if diff != 0 else "="
        print(f"  {drv}:       {cpp_cnt:>12,} {rust_cnt:>12,} {diff_str:>12}")

    cpp_total = sum(cpp_drives.values())
    rust_total = sum(rust_drives.values())
    diff_total = rust_total - cpp_total
    diff_str = f"{diff_total:+,}" if diff_total != 0 else "="
    print(f"  {'-'*6:<8} {'-'*10:>12} {'-'*10:>12} {'-'*10:>12}")
    print(f"  {'TOTAL':<8} {cpp_total:>12,} {rust_total:>12,} {diff_str:>12}")

    print(f"\n--- Path Comparison ---")
    print(f"  C++ unique paths:   {len(cpp_paths):,}")
    print(f"  Rust unique paths:  {len(rust_paths):,}")
    print(f"  Common:             {len(common):,}")
    print(f"  C++ only:           {len(cpp_only):,}")
    print(f"  Rust only:          {len(rust_only):,}")

    if len(cpp_paths) > 0:
        match_pct = len(common) / len(cpp_paths) * 100
        print(f"  Match rate:         {match_pct:.2f}% (vs C++ baseline)")

    if cpp_only:
        print(f"\n--- Sample: In C++ but NOT in Rust (first 10) ---")
        for p in sorted(cpp_only)[:10]:
            print(f"  {p}")
        if len(cpp_only) > 10:
            print(f"  ... and {len(cpp_only) - 10} more")

    if rust_only:
        print(f"\n--- Sample: In Rust but NOT in C++ (first 10) ---")
        for p in sorted(rust_only)[:10]:
            print(f"  {p}")
        if len(rust_only) > 10:
            print(f"  ... and {len(rust_only) - 10} more")

    if not cpp_only and not rust_only:
        print("\n✓ PERFECT MATCH - Both outputs contain identical paths!")

    print()


if __name__ == "__main__":
    main()

