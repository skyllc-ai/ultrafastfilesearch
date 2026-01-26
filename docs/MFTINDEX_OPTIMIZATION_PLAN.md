# MftIndex Optimization & Analytics Plan

**Goal**: Maximize MftIndex native performance for 95% of queries, use DataFrame only for advanced analytics.

**Performance Target**: Maintain 15-30x speedup over DataFrame path for common queries.

---

## Phase 1: Fix Missed Optimizations (IMMEDIATE)

### 1.1 Tree Columns in MftIndex Path

**Problem**: Tree metrics (descendants, treesize, tree_allocated) are computed in MftIndex but queries requesting them force DataFrame path and RECOMPUTE them.

**Impact**: 15-30x speedup for queries with tree columns

**Tasks**:
- [x] Add tree metric fields to `SearchResult` struct ✅
- [x] Populate tree metrics in `SearchResult` constructors ✅
- [x] Add tree columns to `results_to_dataframe()` conversion ✅
- [x] Remove `needs_tree_columns()` check from `should_use_index_path()` ✅
- [x] Remove tree metric recomputation code ✅
- [ ] Add tests for tree columns via MftIndex path ⏳
- [ ] Verify performance on Windows ⏳

**Files to modify**:
- `crates/uffs-core/src/index_search.rs` - Add fields to SearchResult
- `crates/uffs-cli/src/commands.rs` - Update results_to_dataframe(), remove check
- `crates/uffs-core/src/output.rs` - Update formatting

**Testing**:
```bash
# Should use MftIndex path (100-200ms, not 3-5s)
uffs "*.txt" --columns=path,name,descendants,treesize
uffs "C:\Windows\*" --columns=path,tree_allocated,descendants

# Verify correctness vs DataFrame path
uffs "*.log" --columns=descendants --mode=force-index > index.txt
uffs "*.log" --columns=descendants --mode=force-dataframe > df.txt
diff index.txt df.txt  # Should be identical
```

**Performance Target**:
- Before: 3-5s (DataFrame path)
- After: 100-200ms (MftIndex path)
- Speedup: **15-30x**

---

## Phase 2: High-Value MftIndex Features (NEXT)

### 2.1 Direct Children Iteration

**Goal**: Enable per-directory stats without DataFrame overhead

**Tasks**:
- [ ] Add `ChildIter` iterator struct to `uffs-mft`
- [ ] Implement `MftIndex::iter_children(&self, record: &FileRecord) -> ChildIter`
- [ ] Add `MftIndex::count_children(&self, frs: u64) -> u32` helper
- [ ] Add tests for children iteration
- [ ] Document usage patterns

**API Design**:
```rust
// Iterate over direct children
for child_frs in index.iter_children(&dir_record) {
    let child = index.find(child_frs).unwrap();
    // Process child...
}

// Quick count
let count = index.count_children(dir_frs);
```

**Files to create/modify**:
- `crates/uffs-mft/src/index.rs` - Add ChildIter, iter_children()
- `crates/uffs-mft/src/lib.rs` - Export new types

**Testing**:
```bash
# Future: per-directory stats command
uffs stats "C:\Windows\System32"
# Output: 3,234 files, 156 directories, 2.3 GB
```

---

### 2.2 Simple Sorting

**Goal**: Sort results by size, modified, name without DataFrame overhead

**Tasks**:
- [ ] Add `SortBy` enum to `uffs-core` (Size, Modified, Accessed, Name)
- [ ] Add `SortOrder` enum (Ascending, Descending)
- [ ] Implement sorting in `IndexQuery::execute()`
- [ ] Add `--sort-by` and `--order` CLI flags
- [ ] Optimize for common cases (size DESC, modified DESC)
- [ ] Add tests for all sort combinations

**CLI Design**:
```bash
uffs "*.log" --sort-by=size --order=desc
uffs "*.txt" --sort-by=modified --order=desc
uffs "*" --sort-by=name  # Default: ascending
```

**Performance**: O(n log n) - acceptable for result sets

**Files to modify**:
- `crates/uffs-core/src/index_search.rs` - Add sorting logic
- `crates/uffs-cli/src/cli.rs` - Add CLI flags
- `crates/uffs-cli/src/commands.rs` - Wire up sorting

---

### 2.3 Top-N Queries (Partial Sort)

**Goal**: Optimize `--limit` with `--sort-by` using partial sort

**Tasks**:
- [ ] Detect `--limit` + `--sort-by` combination
- [ ] Implement partial sort using `select_nth_unstable_by()`
- [ ] Add benchmarks comparing full sort vs partial sort
- [ ] Document performance characteristics

**Optimization**:
```rust
// Instead of: sort all, then take N
results.sort_by(cmp);
results.truncate(limit);

// Use: partial sort (O(n + k log k) instead of O(n log n))
results.select_nth_unstable_by(limit, cmp);
results.truncate(limit);
results[..limit].sort_by(cmp);
```

**Performance Target**:
- Full sort: O(n log n) - e.g., 23M * log(23M) ≈ 500M ops
- Partial sort: O(n + k log k) - e.g., 23M + 100 * log(100) ≈ 23M ops
- Speedup: **~20x for small limits**

**Testing**:
```bash
# Should use partial sort
uffs "*.log" --sort-by=size --order=desc --limit=100

# Benchmark
hyperfine \
  'uffs "*.log" --sort-by=size --limit=100' \
  'uffs "*.log" --sort-by=size --limit=10000'
```

---

### 2.4 Extension Stats

**Goal**: Fast extension statistics using ExtensionIndex

**Tasks**:
- [ ] Add `MftIndex::extension_stats(&self) -> Vec<(String, u32)>`
- [ ] Leverage existing ExtensionIndex CSR structure
- [ ] Add `--stats` flag to show extension breakdown
- [ ] Add `--top-extensions=N` to show top N extensions
- [ ] Format output as table

**CLI Design**:
```bash
uffs --stats
# Output:
# Extension Statistics:
# .txt     1,234,567 files
# .log       456,789 files
# .dll       234,567 files
# ...

uffs --stats --top-extensions=20
```

**Performance**: O(extensions) - typically <1000, so <1ms

**Files to modify**:
- `crates/uffs-mft/src/index.rs` - Add extension_stats()
- `crates/uffs-cli/src/cli.rs` - Add --stats flag
- `crates/uffs-cli/src/commands.rs` - Implement stats command

---

### 2.5 Path Prefix Filtering (Tree-Aware)

**Goal**: Optimize queries like `C:\Users\*\Documents\*.pdf` using tree structure

**Tasks**:
- [ ] Parse path patterns to extract prefix constraints
- [ ] Add `PathPrefixFilter` to IndexQuery
- [ ] Implement tree-aware filtering (skip entire subtrees)
- [ ] Optimize for common patterns (`C:\Users\*\AppData\*`)
- [ ] Add benchmarks vs naive filtering

**Optimization Strategy**:
```rust
// Pattern: C:\Users\*\Documents\*.pdf
// 1. Find all "Users" directories under C:\
// 2. For each, find "Documents" subdirectory
// 3. Search only within those subtrees
// 4. Skip unrelated directories entirely
```

**Performance Target**:
- Naive: Scan all 23M records, filter by path regex
- Optimized: Scan only matching subtrees (e.g., 100K records)
- Speedup: **~200x for specific path patterns**

**CLI Examples**:
```bash
uffs "C:\Users\*\Documents\*.pdf"
uffs "C:\Windows\System32\drivers\*.sys"
uffs "D:\Projects\*\src\*.rs"
```

**Files to modify**:
- `crates/uffs-core/src/pattern.rs` - Parse path prefix patterns
- `crates/uffs-core/src/index_search.rs` - Add tree-aware filtering
- `crates/uffs-core/src/path.rs` - Path prefix matching

---

## Phase 3: DataFrame for Analytics (LATER)

### 3.1 SQL Support

**Goal**: Enable SQL queries for advanced analytics

**Tasks**:
- [ ] Add `sql` subcommand to CLI
- [ ] Convert MftIndex to DataFrame (reuse existing code)
- [ ] Use Polars SQL context for query execution
- [ ] Add SQL examples to documentation
- [ ] Add tests for common SQL patterns

**CLI Design**:
```bash
# Basic aggregation
uffs sql "SELECT ext, COUNT(*), SUM(size) FROM mft GROUP BY ext ORDER BY COUNT(*) DESC LIMIT 20"

# Complex query
uffs sql "
  SELECT
    parent_path,
    COUNT(*) as file_count,
    SUM(size) as total_size
  FROM mft
  WHERE ext IN ('jpg', 'png', 'gif')
  GROUP BY parent_path
  HAVING total_size > 1000000000
  ORDER BY total_size DESC
"

# Join with external data (future)
uffs sql "
  SELECT m.path, m.size, b.category
  FROM mft m
  JOIN 'file_categories.csv' b ON m.ext = b.extension
"
```

**Performance**: 3-5s for DataFrame conversion + SQL execution (acceptable for analytics)

**Files to modify**:
- `crates/uffs-cli/src/cli.rs` - Add `sql` subcommand
- `crates/uffs-cli/src/commands.rs` - Implement SQL execution
- Add dependency: `polars-sql` crate

**Testing**:
```bash
# Verify SQL results match manual aggregation
uffs sql "SELECT COUNT(*) FROM mft WHERE ext = 'txt'"
uffs "*.txt" --count-only  # Should match
```

---

### 3.2 Stats Command

**Goal**: Pre-built analytics queries for common use cases

**Tasks**:
- [ ] Add `stats` subcommand with multiple modes
- [ ] Implement `--group-by` for aggregation
- [ ] Add `--top=N` for top-N results
- [ ] Support multiple aggregation functions (count, sum, avg, min, max)
- [ ] Format output as tables with human-readable sizes

**CLI Design**:
```bash
# Extension statistics
uffs stats --group-by=extension --top=20
# Output:
# Extension  Count      Total Size  Avg Size
# .txt       1,234,567  45.2 GB     38.4 KB
# .log       456,789    123.4 GB    283.2 KB
# .dll       234,567    12.3 GB     55.1 KB

# Directory statistics
uffs stats --group-by=parent --top=20 --sort-by=size
# Output:
# Directory                    Files   Total Size
# C:\Windows\System32          3,456   2.3 GB
# C:\Program Files\Microsoft   2,345   1.8 GB

# File type statistics
uffs stats --group-by=type
# Output:
# Type        Count      Total Size
# Files       4,234,567  456.7 GB
# Directories 234,567    -

# Time-based statistics
uffs stats --group-by=year --field=modified
# Output:
# Year  Files      Total Size
# 2024  1,234,567  123.4 GB
# 2023  2,345,678  234.5 GB
```

**Performance**: 3-5s (DataFrame path, acceptable for analytics)

**Files to modify**:
- `crates/uffs-cli/src/cli.rs` - Add `stats` subcommand
- `crates/uffs-cli/src/commands.rs` - Implement stats logic
- `crates/uffs-core/src/stats.rs` - Create stats module

---

### 3.3 Export Command

**Goal**: Export results to various formats for external analysis

**Tasks**:
- [ ] Add `export` subcommand
- [ ] Support formats: Parquet, CSV, JSON, NDJSON
- [ ] Add compression options (gzip, zstd, snappy)
- [ ] Support streaming export for large datasets
- [ ] Add progress reporting for large exports

**CLI Design**:
```bash
# Export to Parquet (default compression)
uffs "*.txt" export results.parquet

# Export to CSV
uffs "*.log" export results.csv

# Export to JSON (pretty-printed)
uffs "C:\Users\*\Documents\*" export results.json --pretty

# Export with compression
uffs "*" export all_files.parquet --compression=zstd

# Export with specific columns
uffs "*.pdf" export pdfs.csv --columns=path,size,modified

# Streaming export (for large datasets)
uffs "*" export all_files.parquet --streaming
```

**Supported Formats**:
- **Parquet**: Best for data science workflows (Python, R, Spark)
- **CSV**: Universal compatibility
- **JSON**: Web applications, APIs
- **NDJSON**: Streaming, line-delimited JSON

**Performance**:
- Parquet: ~1-2s for 1M records (compressed)
- CSV: ~3-5s for 1M records
- JSON: ~5-10s for 1M records (larger file size)

**Files to modify**:
- `crates/uffs-cli/src/cli.rs` - Add `export` subcommand
- `crates/uffs-cli/src/commands.rs` - Implement export logic
- `crates/uffs-core/src/export.rs` - Create export module

---


## Milestones & Tracking

### Milestone 1: Tree Columns Fix (Week 1)
**Goal**: Fix missed optimization for tree columns

**Status**: 🟢 Complete (2026-01-26)

**Tasks**:
- [x] Identify the issue (DONE - documented in this plan)
- [x] Add tree metrics to SearchResult (descendants, treesize, tree_allocated)
- [x] Update SearchResult constructors to populate from FileRecord
- [x] Update results_to_dataframe() to include tree columns
- [x] Remove tree metric recomputation code
- [x] Remove needs_tree_columns() check from should_use_index_path()
- [x] Update tree column addition logic to skip existing columns
- [x] Code compiles successfully (cargo clippy passed)
- [ ] Build release binary and test on Windows
- [ ] Verify 15-30x speedup with benchmarks

**Success Criteria**:
- ✅ `uffs "*.txt" --columns=descendants` uses MftIndex path
- ⏳ Performance: 100-200ms (not 3-5s) - pending Windows testing
- ⏳ Results match DataFrame path exactly - pending verification

**Deliverable**: Tree columns work via fast MftIndex path

**Implementation Details**:
- Modified `crates/uffs-core/src/index_search.rs`:
  - Added `descendants`, `treesize`, `tree_allocated` fields to `SearchResult`
  - Updated `from_record()` and `from_expanded()` to populate tree metrics
- Modified `crates/uffs-cli/src/commands.rs`:
  - Added tree metric vectors to `results_to_dataframe()`
  - Removed tree metric recomputation code (lines 1293-1308)
  - Removed `output_config` parameter from `should_use_index_path()`
  - Updated tree column addition logic to skip columns that already exist
- Tree metrics are now always available from MftIndex without DataFrame overhead

---

### Milestone 2: Children Iteration & Sorting (Week 2-3)
**Goal**: Enable per-directory stats and result sorting

**Status**: 🔴 Not Started

**Tasks**:
- [ ] Implement ChildIter
- [ ] Add iter_children() API
- [ ] Implement simple sorting
- [ ] Add --sort-by CLI flag
- [ ] Add tests for both features

**Success Criteria**:
- Can iterate children of any directory
- Can sort results by size/modified/name
- Performance: O(n log n) for sorting

**Deliverable**: `uffs "*.log" --sort-by=size --order=desc` works

---

### Milestone 3: Top-N & Extension Stats (Week 4)
**Goal**: Optimize limited queries and add extension statistics

**Status**: 🔴 Not Started

**Tasks**:
- [ ] Implement partial sort for --limit
- [ ] Add extension_stats() method
- [ ] Add --stats CLI flag
- [ ] Benchmark partial sort vs full sort
- [ ] Add tests

**Success Criteria**:
- `--limit` with `--sort-by` uses partial sort
- 20x speedup for small limits
- Extension stats show top extensions

**Deliverable**: `uffs --stats --top-extensions=20` works

---

### Milestone 4: Path Prefix Filtering (Week 5-6)
**Goal**: Tree-aware path filtering for massive speedups

**Status**: 🔴 Not Started

**Tasks**:
- [ ] Parse path prefix patterns
- [ ] Implement tree-aware filtering
- [ ] Add benchmarks
- [ ] Optimize for common patterns
- [ ] Add tests

**Success Criteria**:
- `uffs "C:\Users\*\Documents\*.pdf"` skips unrelated subtrees
- 100-200x speedup for specific path patterns
- Results match naive filtering

**Deliverable**: Path prefix queries are dramatically faster

---

### Milestone 5: SQL Support (Week 7-8)
**Goal**: Enable SQL queries for advanced analytics

**Status**: 🔴 Not Started

**Tasks**:
- [ ] Add sql subcommand
- [ ] Integrate polars-sql
- [ ] Add SQL examples
- [ ] Add tests for common patterns
- [ ] Document SQL schema

**Success Criteria**:
- Can execute SQL queries on MFT data
- Results are correct
- Performance: 3-5s (acceptable for analytics)

**Deliverable**: `uffs sql "SELECT ..."` works

---

### Milestone 6: Stats & Export Commands (Week 9-10)
**Goal**: Pre-built analytics and export capabilities

**Status**: 🔴 Not Started

**Tasks**:
- [ ] Implement stats subcommand
- [ ] Implement export subcommand
- [ ] Support multiple formats (Parquet, CSV, JSON)
- [ ] Add compression options
- [ ] Add tests

**Success Criteria**:
- Stats command provides useful aggregations
- Export works for all formats
- Compression reduces file size

**Deliverable**: `uffs stats --group-by=extension` and `uffs export results.parquet` work

---

## Performance Targets Summary

| Feature | Before | After | Speedup |
|---------|--------|-------|---------|
| Tree columns | 3-5s (DF) | 100-200ms (Index) | **15-30x** |
| Simple sorting | 3-5s (DF) | 200-300ms (Index) | **10-15x** |
| Top-N (limit=100) | 200-300ms (full sort) | 10-20ms (partial) | **10-20x** |
| Extension stats | 3-5s (DF) | <1ms (Index) | **3000x+** |
| Path prefix filter | 3-5s (scan all) | 10-50ms (subtree) | **100-200x** |
| SQL queries | N/A | 3-5s (DF) | New feature |
| Stats command | N/A | 3-5s (DF) | New feature |
| Export | N/A | 1-10s (format-dependent) | New feature |

---

## Testing Strategy

### Unit Tests
- [ ] Tree columns in SearchResult
- [ ] Children iteration
- [ ] Sorting (all combinations)
- [ ] Partial sort correctness
- [ ] Extension stats accuracy
- [ ] Path prefix parsing
- [ ] SQL query execution
- [ ] Export format correctness

### Integration Tests
- [ ] End-to-end CLI tests for all features
- [ ] Multi-drive scenarios
- [ ] Large dataset tests (23M records)
- [ ] Edge cases (empty results, single result)

### Performance Tests
- [ ] Benchmark tree columns (Index vs DF)
- [ ] Benchmark sorting (full vs partial)
- [ ] Benchmark path prefix filtering
- [ ] Benchmark extension stats
- [ ] Regression tests (ensure no slowdowns)

### Correctness Tests
- [ ] Compare Index path vs DataFrame path results
- [ ] Verify sorting is stable
- [ ] Verify partial sort matches full sort
- [ ] Verify SQL results match manual aggregation

---

## Dependencies

### External Crates
- `polars-sql` - For SQL support (Phase 3.1)
- Existing: `polars`, `polars-lazy`, `polars-io`

### Internal Dependencies
- Phase 2.3 (Top-N) depends on Phase 2.2 (Sorting)
- Phase 3.2 (Stats) depends on Phase 3.1 (SQL) for some aggregations
- Phase 3.3 (Export) depends on DataFrame conversion

---

## Documentation Updates

### User Documentation
- [ ] Update README with new features
- [ ] Add examples for all new CLI flags
- [ ] Document SQL schema for `uffs sql`
- [ ] Add performance comparison tables
- [ ] Create "When to use MftIndex vs DataFrame" guide

### Developer Documentation
- [ ] Document ChildIter API
- [ ] Document sorting implementation
- [ ] Document partial sort optimization
- [ ] Document path prefix filtering algorithm
- [ ] Add architecture diagrams

---

## Risk Mitigation

### Performance Risks
- **Risk**: Sorting large result sets (23M records) may be slow
- **Mitigation**: Implement partial sort for --limit, warn users for large sorts

### Correctness Risks
- **Risk**: Tree-aware filtering may miss files
- **Mitigation**: Extensive testing, compare with naive filtering

### Compatibility Risks
- **Risk**: SQL syntax may not match user expectations
- **Mitigation**: Document Polars SQL dialect, provide examples

---

## Success Metrics

### Performance
- ✅ 95% of queries use MftIndex path (not DataFrame)
- ✅ Average query time: <200ms (vs 3-5s before)
- ✅ Top-N queries: <50ms for limit ≤ 1000

### Usability
- ✅ Users can sort results without DataFrame overhead
- ✅ Users can get extension stats instantly
- ✅ Users can export results to standard formats

### Completeness
- ✅ All high-value MftIndex features implemented
- ✅ DataFrame path available for advanced analytics
- ✅ Clear documentation on when to use each path

---

## Next Steps

1. **Review this plan** with stakeholders
2. **Start with Milestone 1** (Tree columns fix) - highest impact, lowest effort
3. **Iterate through milestones** in order
4. **Track progress** using this document
5. **Update benchmarks** after each milestone
6. **Document learnings** in CHANGELOG_HEALING.md

---

**Last Updated**: 2026-01-26
**Status**: In Progress - Milestone 1 Complete (code), pending Windows testing
**Current Milestone**: Milestone 1 (Tree Columns Fix) - Testing Phase
**Next Milestone**: Milestone 2 (Children Iteration & Sorting)
