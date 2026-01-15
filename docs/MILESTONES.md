# UltraFastFileSearch-Rust Milestone Tracking

## Project Overview

**Project**: UltraFastFileSearch Rust Implementation
**Start Date**: 2026-01-15
**Target Completion**: 2026-06-15 (21 weeks)
**Status**: 🟡 In Progress
**Architecture**: Cargo Workspace with 6 crates (Polars-based)

---

## Workspace Structure

```
crates/
├── uffs-polars/  🔧 Polars facade (compilation isolation)
├── uffs-mft/     📦 MFT reading → Polars DataFrame
├── uffs-core/    📦 Query engine using Polars lazy API
├── uffs-cli/     🔧 Command-line interface
├── uffs-tui/     🖥️  Terminal UI
└── uffs-gui/     🪟 Graphical UI (future)
```

---

## Milestone Summary

| Phase | Milestone | Crate(s) | Target | Status | Progress |
|-------|-----------|----------|--------|--------|----------|
| 0 | Workspace Setup | all | Week 1 | ⬜ Not Started | 0% |
| 1 | MFT Foundation | uffs-mft | Week 4 | ⬜ Not Started | 0% |
| 2 | MFT DataFrame | uffs-mft | Week 8 | ⬜ Not Started | 0% |
| 3 | Core Processing | uffs-core | Week 12 | ⬜ Not Started | 0% |
| 4 | CLI & Performance | uffs-cli | Week 16 | ⬜ Not Started | 0% |
| 5 | TUI & Polish | uffs-tui | Week 21 | ⬜ Not Started | 0% |

**Legend**: ⬜ Not Started | 🟡 In Progress | 🟢 Complete | 🔴 Blocked

---

## Phase 0: Workspace Setup (Week 0-1)

**Goal**: Establish modern Rust workspace with Polars facade
**Target Date**: Week 1
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 0.1 | Workspace Cargo.toml | - | ⬜ | `[workspace]` manifest |
| 0.2 | crates/ directory structure | - | ⬜ | 6 crate directories |
| 0.3 | **uffs-polars facade crate** | - | ⬜ | Polars compilation isolation |
| 0.4 | uffs-mft crate skeleton | - | ⬜ | lib.rs, Cargo.toml |
| 0.5 | uffs-core crate skeleton | - | ⬜ | lib.rs, Cargo.toml |
| 0.6 | uffs-cli crate skeleton | - | ⬜ | main.rs, Cargo.toml |
| 0.7 | Workspace dependencies | - | ⬜ | `[workspace.dependencies]` |
| 0.8 | rustfmt.toml | - | ⬜ | Code formatting |
| 0.9 | clippy.toml | - | ⬜ | Linting rules |
| 0.10 | GitHub Actions CI | - | ⬜ | Build, test, clippy |
| 0.11 | MSRV policy | - | ⬜ | rust-version = "1.80" (Polars) |

### Acceptance Criteria

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` runs (even if no tests yet)
- [ ] `cargo clippy --workspace` passes
- [ ] CI pipeline runs on push/PR
- [ ] All crates have proper Cargo.toml with workspace inheritance

---

## Phase 1: uffs-mft Foundation (Weeks 1-4)

**Goal**: Core NTFS structures and raw disk access
**Crate**: `uffs-mft`
**Target Date**: Week 4
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 1.1 | NtfsBootSector struct | - | ⬜ | Boot sector parsing |
| 1.2 | FileRecordHeader struct | - | ⬜ | MFT record header |
| 1.3 | AttributeHeader struct | - | ⬜ | Common attribute header |
| 1.4 | Resident attribute parsing | - | ⬜ | In-record data |
| 1.5 | Non-resident attribute parsing | - | ⬜ | External data runs |
| 1.6 | Windows volume opening | - | ⬜ | `\\.\X:` format |
| 1.7 | FSCTL_GET_NTFS_VOLUME_DATA | - | ⬜ | Volume metadata |
| 1.8 | FSCTL_GET_RETRIEVAL_POINTERS | - | ⬜ | MFT extents |
| 1.9 | Raw cluster reading | - | ⬜ | Direct disk I/O |
| 1.10 | Error types with thiserror | - | ⬜ | MftError enum |
| 1.11 | Unit tests | - | ⬜ | Structure parsing tests |

### Acceptance Criteria

- [ ] Can open NTFS volume with admin privileges
- [ ] Can read boot sector and extract MFT location
- [ ] Can read raw MFT clusters
- [ ] Can parse MFT record headers
- [ ] All unit tests pass
- [ ] `cargo doc --package uffs-mft` generates docs

---

## Phase 2: uffs-mft DataFrame (Weeks 5-8)

**Goal**: Complete MFT parsing with Polars DataFrame output
**Crate**: `uffs-mft`
**Target Date**: Week 8
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 2.1 | $STANDARD_INFORMATION parsing | - | ⬜ | Timestamps, flags |
| 2.2 | $FILE_NAME parsing | - | ⬜ | Name, parent ref |
| 2.3 | $DATA parsing (resident) | - | ⬜ | Small files |
| 2.4 | $DATA parsing (non-resident) | - | ⬜ | Large files |
| 2.5 | Multi-sector fixup (unfixup) | - | ⬜ | Data integrity |
| 2.6 | $BITMAP parsing | - | ⬜ | Valid record bitmap |
| 2.7 | $REPARSE_POINT parsing | - | ⬜ | Symlinks, junctions |
| 2.8 | Run list (mapping pairs) | - | ⬜ | VCN/LCN mapping |
| 2.9 | **DataFrame construction** | - | ⬜ | Build Polars DataFrame |
| 2.10 | **Parquet persistence** | - | ⬜ | save_parquet()/load_parquet() |
| 2.11 | Unit tests | - | ⬜ | Attribute parsing |

### Acceptance Criteria

- [ ] Can parse all standard NTFS attributes
- [ ] Multi-sector fixup correctly applied
- [ ] Can extract file names and parent references
- [ ] **MFT data returned as Polars DataFrame**
- [ ] **DataFrame can be saved/loaded as Parquet**
- [ ] All unit tests pass

---

## Phase 3: uffs-core Processing (Weeks 9-12)

**Goal**: Query engine using Polars lazy API
**Crate**: `uffs-core`
**Target Date**: Week 12
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 3.1 | **MftQuery builder** | - | ⬜ | Wraps LazyFrame |
| 3.2 | Polars filter predicates | - | ⬜ | size, date, type |
| 3.3 | PathResolver struct | - | ⬜ | FRS → full path |
| 3.4 | Glob to regex conversion | - | ⬜ | Pattern translation |
| 3.5 | **Polars string matching** | - | ⬜ | SIMD-accelerated |
| 3.6 | Streaming mode support | - | ⬜ | Large datasets |
| 3.7 | Table exporter | - | ⬜ | Pretty print |
| 3.8 | JSON exporter | - | ⬜ | Machine readable |
| 3.9 | CSV exporter | - | ⬜ | Spreadsheet |
| 3.10 | Unit tests | - | ⬜ | Query & matching |

### Acceptance Criteria

- [ ] **MftQuery wraps Polars LazyFrame**
- [ ] Polars lazy predicates work correctly
- [ ] Path resolution is accurate
- [ ] Export formats produce valid output
- [ ] **Streaming mode handles large datasets**
- [ ] All unit tests pass

---

## Phase 4: uffs-cli & Performance (Weeks 13-16)

**Goal**: CLI tool and performance optimization
**Crate**: `uffs-cli`
**Target Date**: Week 16
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 4.1 | CLI argument parsing | - | ⬜ | clap derive |
| 4.2 | `search` command | - | ⬜ | Pattern search |
| 4.3 | `index` command | - | ⬜ | Build/save index |
| 4.4 | `stats` command | - | ⬜ | Volume statistics |
| 4.5 | Progress indicators | - | ⬜ | indicatif |
| 4.6 | Error messages | - | ⬜ | miette |
| 4.7 | Async I/O optimization | - | ⬜ | tokio |
| 4.8 | Parallel MFT reading | - | ⬜ | Multi-drive |
| 4.9 | Benchmark suite | - | ⬜ | criterion |
| 4.10 | Performance profiling | - | ⬜ | flamegraph |
| 4.11 | Integration tests | - | ⬜ | End-to-end |

### Acceptance Criteria

- [ ] CLI accepts all documented arguments
- [ ] Progress shown during indexing
- [ ] MFT read speed ≥500 MB/s
- [ ] Index build ≤2s for 1M files
- [ ] Search latency <10ms
- [ ] All benchmarks pass

### Performance Tracking

| Metric | C++ Baseline | Current | Target | Status |
|--------|--------------|---------|--------|--------|
| MFT Read (MB/s) | 500 | - | ≥500 | ⬜ |
| Index Build (1M files) | 2.0s | - | ≤1.5s | ⬜ |
| Search Latency | 8ms | - | <5ms (SIMD) | ⬜ |
| Memory/File | 32B | - | ~45B (Polars) | ⬜ |
| Parquet Size | N/A | - | ~60% of raw | ⬜ |

---

## Phase 5: uffs-tui & Polish (Weeks 17-21)

**Goal**: Terminal UI and production readiness
**Crate**: `uffs-tui`
**Target Date**: Week 21
**Status**: ⬜ Not Started

### Deliverables

| ID | Deliverable | Owner | Status | Notes |
|----|-------------|-------|--------|-------|
| 5.1 | TUI framework setup | - | ⬜ | ratatui + crossterm |
| 5.2 | Search input widget | - | ⬜ | Real-time search |
| 5.3 | Results list widget | - | ⬜ | Scrollable list |
| 5.4 | File details panel | - | ⬜ | Size, dates, path |
| 5.5 | Progress indicators | - | ⬜ | Indexing progress |
| 5.6 | Keyboard navigation | - | ⬜ | vim-style bindings |
| 5.7 | Admin privilege check | - | ⬜ | UAC elevation |
| 5.8 | User documentation | - | ⬜ | README, --help |
| 5.9 | API documentation | - | ⬜ | rustdoc for all crates |
| 5.10 | Release builds | - | ⬜ | Optimized binaries |
| 5.11 | Cross-compilation | - | ⬜ | Windows targets |

### Acceptance Criteria

- [ ] TUI launches and displays search interface
- [ ] Real-time search updates as you type
- [ ] Keyboard navigation works smoothly
- [ ] Progress shown during indexing
- [ ] Documentation complete and accurate
- [ ] Release binaries tested on clean system

---

## Risk Register

| ID | Risk | Impact | Probability | Mitigation | Status |
|----|------|--------|-------------|------------|--------|
| R1 | Windows API complexity | High | Medium | Use `windows` crate, extensive testing | ⬜ Open |
| R2 | Performance regression | High | Low | Continuous benchmarking vs C++ | ⬜ Open |
| R3 | Memory safety with raw I/O | Critical | Medium | Buffer management, fuzzing | ⬜ Open |
| R4 | NTFS edge cases | Medium | Medium | Test on diverse volumes | ⬜ Open |
| R5 | Admin privilege issues | Medium | Low | Clear error messages, docs | ⬜ Open |
| R6 | Workspace complexity | Low | Low | Clear crate boundaries, docs | ⬜ Open |

---

## Dependencies

### Crate Dependencies

| Crate | Depends On | Key External Deps |
|-------|------------|-------------------|
| `uffs-polars` | - | polars (all features) |
| `uffs-mft` | uffs-polars | windows, tokio, bitflags |
| `uffs-core` | uffs-polars, uffs-mft | - |
| `uffs-cli` | uffs-core | clap, indicatif, miette |
| `uffs-tui` | uffs-core | ratatui, crossterm |
| `uffs-gui` | uffs-core | egui (future) |

### Phase Dependencies

```
Phase 0 (Workspace Setup + uffs-polars facade)
    ↓
Phase 1 (uffs-mft Foundation)
    ↓
Phase 2 (uffs-mft DataFrame)
    ↓
Phase 3 (uffs-core Processing with Polars)
    ↓
Phase 4 (uffs-cli & Performance)
    ↓
Phase 5 (uffs-tui & Polish)
```

---

## Weekly Progress Log

### Week 0 (2026-01-15) - Planning

- [x] Analyzed C++ codebase
- [x] Created implementation plan
- [x] Created milestone document
- [x] Refactored for workspace architecture
- [x] **Refactored for Polars-based architecture**
- [ ] Set up workspace structure with uffs-polars facade
- [ ] Establish CI/CD pipeline

### Week 1 - TBD

_Progress updates will be added here_

---

## Change Log

| Date | Change | Reason |
|------|--------|--------|
| 2026-01-15 | Initial document creation | Project kickoff |
| 2026-01-15 | Refactored for workspace architecture | Modular crate design |
| 2026-01-15 | **Refactored for Polars-based architecture** | SIMD, parallelism, Parquet persistence |

---

## Appendix A: Workspace Structure

```
UltraFastFileSearch-Rust/
├── Cargo.toml                      # Workspace manifest
├── crates/
│   ├── uffs-polars/                # 🔧 Polars facade (compiles ONCE)
│   │   ├── Cargo.toml              # All Polars features here
│   │   └── src/
│   │       └── lib.rs              # Re-exports polars::prelude::*
│   │
│   ├── uffs-mft/                   # 📦 MFT reading → DataFrame
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs              # Public API
│   │       ├── reader.rs           # MftReader
│   │       ├── dataframe.rs        # DataFrame construction
│   │       ├── ntfs/               # NTFS structures
│   │       │   ├── mod.rs
│   │       │   ├── boot_sector.rs
│   │       │   ├── file_record.rs
│   │       │   ├── attributes.rs
│   │       │   └── run_list.rs
│   │       ├── io/                 # Low-level I/O
│   │       │   ├── mod.rs
│   │       │   ├── volume.rs
│   │       │   └── async_read.rs
│   │       └── platform/
│   │           ├── mod.rs
│   │           └── windows.rs
│   │
│   ├── uffs-core/                  # 📦 Query engine (Polars lazy)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── query.rs            # MftQuery (wraps LazyFrame)
│   │       ├── path_resolver.rs    # Path reconstruction
│   │       ├── glob.rs             # Glob to regex
│   │       └── export.rs           # Table, JSON, CSV
│   │
│   ├── uffs-cli/                   # 🔧 CLI binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── commands/
│   │           ├── mod.rs
│   │           ├── search.rs
│   │           ├── index.rs
│   │           └── stats.rs
│   │
│   ├── uffs-tui/                   # 🖥️ TUI binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── widgets/
│   │
│   └── uffs-gui/                   # 🪟 GUI binary (future)
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
│
├── examples/                       # Usage examples
├── benches/                        # Benchmarks
└── docs/                           # Documentation
```

---

## Appendix B: Key Metrics Dashboard

```
┌─────────────────────────────────────────────────────────────────┐
│                    PROJECT HEALTH DASHBOARD                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Overall Progress: ██░░░░░░░░░░░░░░░░░░ 5%                      │
│                                                                  │
│  Phase 0: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 3: ░░░░░░░░░░░░ 0%   │
│  Phase 1: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 4: ░░░░░░░░░░░░ 0%   │
│  Phase 2: ░░░░░░░░░░░░░░░░░░░░ 0%    Phase 5: ░░░░░░░░░░░░ 0%   │
│                                                                  │
│  Crates: 0/6 complete              Tests: 0 passing / 0 total   │
│  Open Risks: 6                      Blockers: 0                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Appendix C: Resources

### Project Resources

- **C++ Reference**: `old_cpp/uffs/UltraFastFileSearch-code/`
- **Architecture Doc**: `docs/architecture/Suggested Rust Source Code Structure.docx`
- **Implementation Plan**: `docs/IMPLEMENTATION_PLAN.md`

### External References

- [NTFS Documentation (Microsoft)](https://docs.microsoft.com/en-us/windows/win32/fileio/master-file-table)
- [NTFS Internals](https://flatcap.github.io/linux-ntfs/ntfs/)
- [Rust `windows` crate](https://docs.rs/windows)
- [Tokio async runtime](https://tokio.rs)
- [ratatui TUI framework](https://ratatui.rs)
- [Cargo Workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html)
- [Polars User Guide](https://docs.pola.rs/)
- [Polars Rust API](https://docs.rs/polars)
- [Parquet Format](https://parquet.apache.org/)

