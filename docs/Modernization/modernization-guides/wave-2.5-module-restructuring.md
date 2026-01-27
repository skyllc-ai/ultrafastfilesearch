<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 2.5: Module Restructuring - Implementation Guide

> **Effort**: 3-5 days | **Priority**: 🔴 Critical
> **Prerequisites**: Wave 2 complete
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## 🎯 Goal

**Best Practice**: Each file = **one primary type or responsibility**, typically **200-500 lines**.

Files over 1000 lines become hard to navigate, review, and maintain. This wave splits
oversized files into focused modules while preserving the public API.

---

## 📊 File Size Analysis

### 🔴 CRITICAL (>2000 lines - MUST split)

| File | Lines | Target Structure |
|------|-------|------------------|
| `uffs-mft/src/io.rs` | 6,623 | → `io/` directory (8-10 files) |
| `uffs-mft/src/index.rs` | 5,690 | → `index/` directory (6-8 files) |
| `uffs-mft/src/main.rs` | 4,543 | → Extract to `cli/` module |
| `uffs-mft/src/reader.rs` | 4,475 | → `reader/` directory (4-6 files) |
| `uffs-cli/src/commands.rs` | 2,520 | → `commands/` directory (5-7 files) |

### 🟠 SHOULD SPLIT (1000-2000 lines)

| File | Lines | Target Structure |
|------|-------|------------------|
| `uffs-mft/src/platform.rs` | 1,872 | → `platform/` directory |
| `uffs-mft/src/parse.rs` | 1,861 | → `parse/` directory |
| `uffs-mft/src/ntfs.rs` | 1,457 | → `ntfs/` directory |
| `uffs-core/src/index_search.rs` | 1,334 | → `search/` directory |
| `uffs-core/src/path_resolver.rs` | 1,107 | → Split by responsibility |
| `uffs-mft/src/raw.rs` | 1,041 | → `raw/` directory |

### 🟡 CONSIDER SPLITTING (500-1000 lines)

| File | Lines | Action |
|------|-------|--------|
| `uffs-core/src/compiled_pattern.rs` | 974 | Split if multiple types |
| `uffs-core/src/tree.rs` | 854 | Split if multiple types |
| `uffs-core/src/output.rs` | 790 | Split by output format |
| `uffs-legacy/src/.../utils_impl.rs` | 704 | Legacy - lower priority |
| `uffs-core/src/query.rs` | 679 | Split if multiple types |
| `uffs-core/src/extensions.rs` | 616 | Split by extension type |
| `uffs-cli/src/main.rs` | 519 | OK - main files can be larger |

---

## 📋 Task Checklist

- [ ] 2.5.1 Split `io.rs` → `io/` module
- [ ] 2.5.2 Split `index.rs` → `index/` module
- [ ] 2.5.3 Split `reader.rs` → `reader/` module
- [ ] 2.5.4 Refactor `main.rs` (uffs-mft) → extract CLI logic
- [ ] 2.5.5 Split `commands.rs` → `commands/` module
- [ ] 2.5.6 Split remaining 1000+ line files
- [ ] 2.5.7 Review 500-1000 line files

---

## 🔧 Restructuring Pattern

### Before: Single Large File
```
src/
├── lib.rs
└── io.rs  (6,623 lines - everything in one file)
```

### After: Module Directory
```
src/
├── lib.rs
└── io/
    ├── mod.rs              # Re-exports only (~50 lines)
    ├── aligned_buffer.rs   # AlignedBuffer (~200 lines)
    ├── batch_reader.rs     # BatchMftReader (~400 lines)
    ├── parallel_reader.rs  # ParallelMftReader (~600 lines)
    ├── streaming_reader.rs # StreamingMftReader (~300 lines)
    ├── pipelined_reader.rs # PipelinedMftReader (~400 lines)
    ├── iocp_reader.rs      # IocpMftReader (~500 lines)
    ├── extent_map.rs       # MftExtentMap (~300 lines)
    ├── chunks.rs           # ReadChunk, generate_read_chunks (~200 lines)
    ├── merger.rs           # MftRecordMerger (~300 lines)
    └── tests.rs            # All tests (~500 lines)
```

### mod.rs Pattern
```rust
//! I/O operations for MFT reading.

mod aligned_buffer;
mod batch_reader;
mod chunks;
mod extent_map;
mod iocp_reader;
mod merger;
mod parallel_reader;
mod pipelined_reader;
mod streaming_reader;

#[cfg(test)]
mod tests;

// Re-export public API (preserves backward compatibility)
pub use aligned_buffer::AlignedBuffer;
pub use batch_reader::BatchMftReader;
pub use chunks::{ReadChunk, generate_read_chunks};
pub use extent_map::MftExtentMap;
pub use iocp_reader::IocpMftReader;
pub use merger::MftRecordMerger;
pub use parallel_reader::ParallelMftReader;
pub use pipelined_reader::PipelinedMftReader;
pub use streaming_reader::StreamingMftReader;
```

---

## ⚠️ Rules

1. **Preserve public API** - All existing `pub use` must continue to work
2. **One type per file** - Each struct/enum gets its own file
3. **Tests stay with code** - Or move to `tests.rs` in same directory
4. **mod.rs is thin** - Only `mod` declarations and `pub use` re-exports
5. **Run CI after each file** - Don't batch too many changes

---

## ✅ Wave 2.5 Completion Checklist

- [ ] No file over 1000 lines (except benchmarks/tests)
- [ ] All modules follow `mod.rs` + subfiles pattern
- [ ] Public API unchanged (no breaking changes)
- [ ] All tests pass
- [ ] CI pipeline green

### Final Validation
```bash
rust-script scripts/ci-pipeline.rs go -v
```

---

*Previous: [Wave 2 - Architecture Completion](wave-2-architecture-completion.md)*
*Next: [Wave 3 - Testing Excellence](wave-3-testing-excellence.md)*

