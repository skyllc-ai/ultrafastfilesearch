# UFFS Engine Architecture Documentation

**Version**: 0.3.62 (Rust 1.85+ / Edition 2024)
**Last Updated**: 2026-03-23

This documentation series provides a comprehensive architectural reference for the UFFS (Ultra Fast File Search) Rust engine — a high-performance NTFS MFT reader that indexes millions of files in seconds.

A competent developer reading these documents should be able to understand, maintain, extend, or reimplement the core engine from scratch.

---

## Quick Links

| I want to... | Go to... |
|--------------|----------|
| Understand the overall architecture | [01 - Architecture Overview](01-overview.md) |
| Learn how MFT reading works | [02 - MFT Reading Pipeline](02-mft-reading.md) |
| Parse NTFS records from raw bytes | [03 - NTFS Structures & Parsing](03-ntfs-parsing.md) |
| Understand the in-memory index | [04 - In-Memory Index](04-indexing.md) |
| Understand the threading model | [05 - Concurrency Model](05-concurrency.md) |
| Add or modify search patterns | [06 - Pattern Matching & Search](06-pattern-search.md) |
| Customize output formatting | [07 - Output & Streaming](07-output-streaming.md) |
| Use the CLI | [08 - CLI Reference](08-cli.md) |
| Profile and optimize performance | [09 - Performance & Benchmarking](09-performance.md) |
| Build the project | [10 - Build System & CI](10-build-ci.md) |

---

## Document Map

### Core Engine (read in order)

| # | Document | Scope |
|---|----------|-------|
| 01 | [Architecture Overview](01-overview.md) | System context, crate map, data flow, memory layout, glossary |
| 02 | [MFT Reading Pipeline](02-mft-reading.md) | Volume access, IOCP, bitmap skip, extent mapping, drive-type tuning |
| 03 | [NTFS Structures & Parsing](03-ntfs-parsing.md) | Boot sector, FILE records, USA fixup, attributes, data runs |
| 04 | [In-Memory Index](04-indexing.md) | `MftIndex`, `FileRecord`, names buffer, tree metrics, path resolution |
| 05 | [Concurrency Model](05-concurrency.md) | Tokio, IOCP, Rayon, multi-drive parallelism, lock-free hot path |

### Search & Output

| # | Document | Scope |
|---|----------|-------|
| 06 | [Pattern Matching & Search](06-pattern-search.md) | Glob/regex/literal, extension index, `IndexQuery`, smart-case |
| 07 | [Output & Streaming](07-output-streaming.md) | Columns, formats, multi-drive streaming, path resolution |
| 08 | [CLI Reference](08-cli.md) | All flags, pattern syntax, attribute/date filters, subcommands |

### Operations

| # | Document | Scope |
|---|----------|-------|
| 09 | [Performance & Benchmarking](09-performance.md) | Benchmarks, optimization layers, profiling |
| 10 | [Build System & CI](10-build-ci.md) | Cargo profiles, cross-compilation, testing, CI pipeline, unsafe code safety |
| 11 | [Performance Deep Dive](11-performance-deep-dive.md) | The "secret sauce" — every optimization with measured impact, real benchmark data, evolution history |
| 12 | [Forensics & Diagnostics](12-forensics-diagnostics.md) | Forensic mode, `uffs-diag` tools, MFT analysis workflows, USN journal |

---

## Target Audience

**Primary:** Developers who want to:
- Understand exactly how UFFS achieves its performance
- Contribute to or maintain the codebase
- Build similar tools for NTFS or other file systems

**Prerequisites assumed:**
- Proficiency in Rust (ownership, lifetimes, async/await)
- Basic understanding of file systems and disk I/O
- Familiarity with Windows development concepts (helpful but not required)

---

## Level of Detail

Each document provides:
1. **Complete implementation details** — not just "what" but exactly "how"
2. **Code references** — pointing to specific source files and types
3. **ASCII architecture diagrams** — visual data flow and threading models
4. **Performance analysis** — where time is spent and why
5. **Edge cases** — extension records, hard links, corrupted records, etc.

---

## Crate Overview

```
uffs-cli ──► uffs-core ──► uffs-mft ──► uffs-polars ──► polars
   │              │              │
   │              │              ├── ntfs/        NTFS on-disk structures
   │              │              ├── parse/       Record parsing (cross-platform)
   │              │              ├── io/          I/O readers + inline parsers
   │              │              ├── index/       In-memory index + tree metrics
   │              │              ├── platform/    Volume, bitmap, drive detection
   │              │              ├── reader/      Orchestration + multi-drive
   │              │              └── cache/       Parquet + zstd persistence
   │              │
   │              ├── pattern/          Pattern parsing
   │              ├── compiled_pattern/ Glob classification
   │              ├── index_search/     Direct MftIndex search
   │              ├── path_resolver/    FRS → full path
   │              ├── output/           Formatting + columns
   │              └── tree/             Tree metric computation
   │
   └── commands/              CLI subcommands + search orchestration
```

---

## Version History

| Date | Change |
|------|--------|
| 2026-03-23 | Initial engine documentation set (10 documents) |
