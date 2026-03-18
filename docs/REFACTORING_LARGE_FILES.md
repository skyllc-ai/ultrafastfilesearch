# Large File Refactoring Plan

**Date:** 2026-03-17
**Goal:** Remove ALL file size exceptions (>800 LOC) through proper modularization.

## Dual Goals

1. **Reduce line count** - All files must be <800 LOC (no exceptions allowed)
2. **Platform separation** - Windows-only and macOS-only code in dedicated files

---

## Files To Refactor (6151 LOC total)

| File | Current LOC | Target LOC | Status |
|------|-------------|------------|--------|
| `uffs-cli/commands/output.rs` | 1449 | <800 | 🔄 In Progress |
| `uffs-mft/io/parser/index.rs` | 1104 | <800 | Pending |
| `uffs-diag/bin/compare_scan_parity.rs` | 987 | <800 | Pending |
| `uffs-cli/commands/raw_io.rs` | 908 | <800 | Pending |
| `uffs-mft/parse/direct_index.rs` | 880 | <800 | Pending |
| `uffs-mft/parse/direct_index_extension.rs` | 824 | <800 | Pending |

---

## Refactoring Strategy

### Principles
1. **Extract tests to separate files** - Standard Rust pattern (`*_tests.rs` adjacent to source)
2. **Extract shared cross-platform code** - Common logic in dedicated modules
3. **Platform-specific code in dedicated files** - `#[cfg(windows)]` blocks → `platform/windows.rs`
4. **Preserve hot-path performance** - No indirection for performance-critical parsing loops
5. **Small atomic commits** - Each extraction is a separate commit

### Target Structure
Each large file should split into:
- **Core logic** (<400 LOC) - Main entry points and orchestration
- **Helpers/utilities** - Extracted helper functions
- **Tests** - `*_tests.rs` adjacent file
- **Platform-specific** - `platform/{windows,unix}.rs` when applicable

---

## File-by-File Analysis

### 1. `output.rs` (1448 LOC) → Target: <400 LOC + tests

**Current Structure:**
- Lines 1-990: Output formatting logic (CSV, JSON, footer, streaming)
- Lines 991-1449: Comprehensive test suite (458 LOC)

**Extraction Plan:**
```
commands/
├── output/
│   ├── mod.rs          # Core write_results, can_write_native_results (~300 LOC)
│   ├── streaming.rs    # StreamingWriter (Windows-only, ~150 LOC)
│   ├── footer.rs       # C++ baseline footer formatting (~100 LOC)
│   └── tests.rs        # All tests (~460 LOC)
└── output.rs           # REMOVE (replaced by output/)
```

**Key extractions:**
- `StreamingWriter` + `StreamingFormat` → `output/streaming.rs` (Windows-only)
- `format_cpp_footer_*` functions → `output/footer.rs`
- `#[cfg(test)] mod tests` → `output/tests.rs`

---

### 2. `raw_io.rs` (908 LOC) → Target: <400 LOC

**Current Structure:**
- Shared types: `QueryFilters`, `NativeOfflineQueryResults`
- Windows-only: `OwnedQueryFilters`, multi-drive search, MFT loading
- Cross-platform: Query building helpers

**Extraction Plan:**
```
commands/
├── raw_io/
│   ├── mod.rs              # Shared types, cross-platform query helpers (~250 LOC)
│   ├── filters.rs          # QueryFilters, OwnedQueryFilters (~150 LOC)
│   ├── windows_search.rs   # #[cfg(windows)] search functions (~350 LOC)
│   └── offline.rs          # Offline MFT file queries (~150 LOC)
└── raw_io.rs               # REMOVE (replaced by raw_io/)
```

**Key extractions:**
- `#[cfg(windows)]` blocks → `raw_io/windows_search.rs`
- Filter structs → `raw_io/filters.rs`
- `query_offline_mft_file` → `raw_io/offline.rs`

---

### 3. `compare_scan_parity.rs` (987 LOC) → Target: <400 LOC

**Current Structure:**
- Single diagnostic binary with many `#![expect(...)]` suppressions
- CSV loading, comparison logic, report generation

**Extraction Plan:**
```
bin/
├── compare_scan_parity.rs  # Main entry point, CLI parsing (~150 LOC)
lib.rs or src/parity/
├── mod.rs                  # Re-exports
├── loader.rs               # CSV loading, normalization (~200 LOC)
├── comparison.rs           # Path/size/tree metric comparison (~300 LOC)
├── report.rs               # Markdown report generation (~200 LOC)
└── stats.rs                # Statistical aggregation (~150 LOC)
```

**Note:** This is a diagnostic tool. Consider if it should become a library + thin binary.

---

### 4. `io/parser/index.rs` (1104 LOC) → Target: <600 LOC (hot path)

**Current Structure:**
- Single `parse_record_to_index()` function handling all NTFS attribute types
- Performance-critical IOCP hot path

**Analysis:**
This file has `#![allow(clippy::all, ...)]` because it's a monolithic hot-path parser.
The code **must remain inlined** for performance, but we can still:

**Extraction Plan:**
```
io/parser/
├── index.rs                # Main parse loop, attribute dispatch (~500 LOC)
├── index_helpers.rs        # Helper functions (stream counting, etc.) (~200 LOC)
├── index_attributes.rs     # Attribute-specific parsing logic (~300 LOC)
└── index_extension.rs      # Already exists (extension records)
```

**Constraint:** Keep the hot loop in `index.rs`. Extract only helper code that doesn't add call overhead.

---

### 5. `parse/direct_index.rs` (880 LOC) → Target: <500 LOC

**Current Structure:**
- Cross-platform version of `io/parser/index.rs`
- Same monolithic parser pattern

**Extraction Plan:**
```
parse/
├── direct_index.rs                 # Main parse loop (~450 LOC)
├── direct_index_helpers.rs         # Shared helpers with io/parser version (~200 LOC)
├── direct_index_extension.rs       # Already exists
├── direct_index_extension_tests.rs # Already exists
└── direct_index_tests.rs           # Add test file if needed
```

**Key insight:** `io/parser/index.rs` and `parse/direct_index.rs` share ~80% code. Consider extracting shared helpers into `parse/common_parser.rs` or `ntfs/parser_core.rs`.

---

### 6. `parse/direct_index_extension.rs` (824 LOC) → Target: <500 LOC

**Current Structure:**
- Extension record parser for direct-to-index path
- Mirrors the attribute handling in `direct_index.rs`

**Extraction Plan:**
```
parse/
├── direct_index_extension.rs       # Main extension parse (~400 LOC)
├── direct_index_extension_attrs.rs # Attribute handlers (~250 LOC)
├── direct_index_extension_tests.rs # Already exists (242 LOC)
```

---

## Execution Order

**Phase 1: Test Extraction (Low Risk)**
1. `output.rs` → Extract tests to `output/tests.rs`
2. `compare_scan_parity.rs` → Extract into library modules

**Phase 2: Platform Separation**
3. `raw_io.rs` → Split Windows/cross-platform code
4. `output.rs` → Extract `StreamingWriter` (Windows-only)

**Phase 3: Parser Consolidation (Higher Risk)**
5. Identify shared code between `io/parser/index.rs` and `parse/direct_index.rs`
6. Extract shared helpers without adding hot-path overhead
7. Split extension parser attributes

---

## Success Criteria

- [ ] All files < 800 LOC (preferably < 500 LOC)
- [ ] No blanket `#![allow(clippy::all)]` suppressions
- [ ] All existing tests pass
- [ ] CI pipeline passes: `rust-script scripts/ci/ci-pipeline.rs go -v`
- [ ] File size exceptions file is empty or removed
- [ ] Performance benchmarks show no regression

---

## Notes

- The parser files (`index.rs`, `direct_index.rs`, `direct_index_extension.rs`) are intentionally monolithic for performance. Any extraction must be verified to not impact IOCP throughput.
- Consider using `#[inline(always)]` on extracted helpers to eliminate call overhead.
- The diagnostic tool (`compare_scan_parity.rs`) is less performance-sensitive and can be refactored more aggressively.

---

## References

- `scripts/ci/file_size_exceptions.txt` - Current exceptions list
- `CLAUDE.md` - Project coding standards
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/

---

## Progress Tracking

### Phase 1: Test Extraction
- [ ] `output.rs` tests → `output/tests.rs`
- [ ] `compare_scan_parity.rs` → library structure

### Phase 2: Platform Separation
- [ ] `raw_io.rs` → module split
- [ ] `output.rs` streaming → separate module

### Phase 3: Parser Consolidation
- [ ] Identify `io/parser/index.rs` ↔ `parse/direct_index.rs` shared code
- [ ] Extract common helpers
- [ ] Split extension parser

### Final
- [ ] Remove `scripts/ci/file_size_exceptions.txt`
- [ ] Full CI validation

