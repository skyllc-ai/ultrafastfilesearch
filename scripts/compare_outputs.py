#!/usr/bin/env python3
"""Compare uffs C++ vs Rust output files."""

import sys
from pathlib import Path


def normalize_path(path: str) -> str:
    """Normalize path for comparison (handle case, slashes)."""
    return path.strip().lower().replace("\\", "/")


def load_paths(filepath: Path) -> set[str]:
    """Load and normalize paths from output file."""
    paths = set()
    with open(filepath, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith(("Indexing", "Searching", "Found", "Time:", "Error")):
                paths.add(normalize_path(line))
    return paths


def main():
    cpp_file = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("output_cpp.txt")
    rust_file = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("output_rust.txt")

    if not cpp_file.exists() or not rust_file.exists():
        print(f"Error: Need both {cpp_file} and {rust_file}")
        sys.exit(1)

    cpp_paths = load_paths(cpp_file)
    rust_paths = load_paths(rust_file)

    cpp_only = cpp_paths - rust_paths
    rust_only = rust_paths - cpp_paths
    common = cpp_paths & rust_paths

    print("=" * 60)
    print("UFFS Output Comparison: C++ vs Rust")
    print("=" * 60)
    print(f"\nFile sizes:")
    print(f"  C++:  {cpp_file.stat().st_size:,} bytes")
    print(f"  Rust: {rust_file.stat().st_size:,} bytes")

    print(f"\nPath counts:")
    print(f"  C++ total:    {len(cpp_paths):,}")
    print(f"  Rust total:   {len(rust_paths):,}")
    print(f"  Common:       {len(common):,}")
    print(f"  C++ only:     {len(cpp_only):,}")
    print(f"  Rust only:    {len(rust_only):,}")

    if len(cpp_paths) > 0:
        match_pct = len(common) / len(cpp_paths) * 100
        print(f"\n  Match rate:   {match_pct:.2f}% (vs C++ baseline)")

    if cpp_only:
        print(f"\n--- In C++ but NOT in Rust (first 10) ---")
        for p in sorted(cpp_only)[:10]:
            print(f"  {p}")
        if len(cpp_only) > 10:
            print(f"  ... and {len(cpp_only) - 10} more")

    if rust_only:
        print(f"\n--- In Rust but NOT in C++ (first 10) ---")
        for p in sorted(rust_only)[:10]:
            print(f"  {p}")
        if len(rust_only) > 10:
            print(f"  ... and {len(rust_only) - 10} more")

    if not cpp_only and not rust_only:
        print("\n✓ PERFECT MATCH - Both outputs contain identical paths!")

    print()


if __name__ == "__main__":
    main()

