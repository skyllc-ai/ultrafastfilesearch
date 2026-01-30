# C++ Tree Algorithm Port - Implementation Tracker

> **Last Updated**: 2026-01-30  
> **Status**: 🟡 In Progress  
> **Branch**: `feature/cpp-tree-algorithm-port`  
> **Reference**: [CPP_TREE_ALGORITHM_PORT.md](./CPP_TREE_ALGORITHM_PORT.md)

---

## Quick Status

| Phase | Status | Progress | Notes |
|-------|--------|----------|-------|
| Phase 1: Study C++ Code | ✅ Complete | 100% | Documented in main guide |
| Phase 2: Design Rust Structures | ⬜ Not Started | 0% | |
| Phase 3: Build Child Entries | ⬜ Not Started | 0% | |
| Phase 4: Implement Tree Metrics | ⬜ Not Started | 0% | |
| Phase 5: Testing | ⬜ Not Started | 0% | |
| Phase 6: Integration | ⬜ Not Started | 0% | |

**Overall Progress**: ████░░░░░░░░░░░░░░░░ 17%

---

## Phase 1: Study C++ Code ✅

**Status**: Complete  
**Completed**: 2026-01-30

### Deliverables

- [x] Document `ChildInfo` structure (lines 3819-3827)
- [x] Document child entry creation during parsing (lines 4480-4490)
- [x] Document tree traversal algorithm (lines 4670-4812)
- [x] Document delta formula (lines 4729-4743)
- [x] Document stream processing (lines 4748-4798)
- [x] Document FRS representation differences (C++ 32-bit vs Rust 64-bit)

### Key Findings

1. C++ uses recursive DFS from root (FRS 5)
2. Delta formula: `value * (i + 1) / n - value * i / n`
3. Each stream adds +1 to treesize
4. `bulkiness` field is used for heap-based filtering
5. C++ uses 32-bit FRS, Rust correctly uses 64-bit

---

## Phase 2: Design Rust Structures ⬜

**Status**: Not Started  
**Estimated**: 1-2 hours

### Tasks

- [ ] Create `crates/uffs-mft/src/cpp_tree.rs` module
- [ ] Define `PackedFileSize` struct (6-byte packed)
- [ ] Define `CppSizeInfo` struct with `bulkiness` and `treesize`
- [ ] Define `CppChildInfo` struct with 64-bit FRS
- [ ] Define `PreprocessResult` struct
- [ ] Add module to `lib.rs`

### Acceptance Criteria

- [ ] All structures compile without warnings
- [ ] Size assertions match expected layout
- [ ] Unit tests for structure creation

---

## Phase 3: Build Child Entries ⬜

**Status**: Not Started  
**Estimated**: 2-3 hours

### Tasks

- [ ] Add `childinfos: Vec<CppChildInfo>` to `MftIndex`
- [ ] Add `first_child: u32` to `FileRecord`
- [ ] Modify `from_parsed_records()` to build child linked list
- [ ] Ensure `name_index` is set BEFORE incrementing `name_count`
- [ ] Handle hardlinks correctly (separate ChildInfo per link)

### Acceptance Criteria

- [ ] Child linked list matches C++ structure
- [ ] Hardlinks create separate entries
- [ ] Root (FRS 5) self-reference handled

---

## Phase 4: Implement Tree Metrics ⬜

**Status**: Not Started  
**Estimated**: 3-4 hours

### Tasks

- [ ] Implement `compute_tree_metrics_cpp_port()` method
- [ ] Implement recursive DFS traversal
- [ ] Implement delta formula exactly as C++
- [ ] Implement stream processing (+1 treesize per stream)
- [ ] Handle default stream accumulation
- [ ] Add debug logging option

### Acceptance Criteria

- [ ] Algorithm matches C++ pseudocode exactly
- [ ] Delta formula produces identical results
- [ ] No stack overflow on deep trees (50+ levels)

---

## Phase 5: Testing ⬜

**Status**: Not Started  
**Estimated**: 2-3 hours

### Tasks

- [ ] Implement delta formula unit tests
- [ ] Implement bulkiness algorithm tests
- [ ] Implement tree traversal tests
- [ ] Implement edge case tests (orphans, deep trees, wide trees)
- [ ] Run comparison against C++ output using `trial_run.ps1`
- [ ] Run benchmark comparison using `benchmark_tree_comparison.ps1`

### Acceptance Criteria

- [ ] All unit tests pass
- [ ] Root descendants match C++ 100%
- [ ] Root treesize match C++ 100%
- [ ] All subdirectory metrics match C++ 100%
- [ ] Performance within 2x of C++

---

## Phase 6: Integration ⬜

**Status**: Not Started  
**Estimated**: 1 hour

### Tasks

- [ ] Wire up `TreeAlgorithm::CppPort` to new implementation
- [ ] Remove placeholder warning message
- [ ] Update CLI help text
- [ ] Verify `UFFS_TREE_ALGO=cpp_port` works
- [ ] Update documentation

### Acceptance Criteria

- [ ] Can switch between algorithms via environment variable
- [ ] No regressions in existing functionality
- [ ] Documentation is complete

---

## Blockers & Issues

| ID | Issue | Status | Resolution |
|----|-------|--------|------------|
| - | None currently | - | - |

---

## Performance Benchmarks

| Date | Drive | C++ Preprocess | Rust Tree Metrics | Speedup | Notes |
|------|-------|----------------|-------------------|---------|-------|
| - | - | - | - | - | No benchmarks yet |

---

## Notes & Decisions

### 2026-01-30
- Created implementation guide and tracker
- Decided on Option A (Transformer) approach first
- Using 64-bit FRS in Rust (improvement over C++ 32-bit)
- Added benchmarking infrastructure (`benchmark-tree` command)

---

## References

- [CPP_TREE_ALGORITHM_PORT.md](./CPP_TREE_ALGORITHM_PORT.md) - Full implementation guide
- [trial_run.ps1](./Investigation/trial_run.ps1) - Rust vs C++ comparison tool
- [benchmark_tree_comparison.ps1](./Investigation/benchmark_tree_comparison.ps1) - Performance comparison

