# Intent by Augment — Project FORGE

## Project Identity

**Project Codename:** **FORGE** — *Foundational Overhaul for Rust Grade Excellence*
**Branch:** `forge/architectural-renaissance`
**Codebase:** UFFS (Ultra Fast File Search)
**Repo:** https://github.com/githubrobbi/UltraFastFileSearch
**Language:** Rust (Edition 2024, nightly toolchain)
**Purpose:** Ultra-high-performance Windows NTFS file search via direct MFT reading + Polars DataFrames
**Author:** Robert Nio

> *"A forge transforms raw metal into precision-crafted tools. This project transforms a working but organically-grown Rust codebase into a world-class reference implementation."*

---

## High-Level Goal

Elevate this Rust workspace from a functional but organically-grown codebase to **world-class, production-grade Rust** — the kind of codebase that could serve as a reference implementation for high-performance systems programming. Every file, module, dependency, and CI pipeline should reflect 2026 best practices.

**Critical directive:** **Eliminate ALL traces of the C++ reference implementation** from the tracked repo. Parity has been achieved and validated. This means:

**Forbidden** (must be removed/rewritten):
- References to the C++ implementation, porting, parity, or comparison (comments, docs, identifiers, scripts)
- `cpp_*` modules, identifiers, and file prefixes in code (`cpp_types.rs`, `cpp_tree.rs`, `cpp_io_pipeline.rs`)
- `crates/uffs-legacy/` (dead C++ port reference crate)
- `CPP_*.md` docs and any docs whose primary purpose is C++ porting/comparison
- C++ comparison/analysis scripts (`analyze_cpp_stats.rs`, etc.)
- C++ parity/comparison tests (keep the underlying logic tests but reframe as Rust correctness tests)
- Phrases like "matches C++ behavior", "port of C++ function X", "C++ compatibility", "replacing the original C++ version", "inspired by the original C++ UFFS", "faster than the C++ implementation"

**Allowed** (do NOT remove):
- `.cpp` as a file extension in search examples, tests, filter patterns, or documentation — because UFFS searches codebases and `.cpp` is a legitimate file type
- Technical references to C/C++ *concepts* (e.g., "SoA layout", "zero-copy") that describe general techniques, not the specific reference implementation

**Specific files requiring rewrite (not just deletion):**
- **`README.md`** — Remove "Rust rewrite of UFFS, replacing the original C++ version" and "inspired by the original C++ UFFS" phrasing; describe UFFS on its own merits
- **`crates/uffs-mft/README.md`** — Remove "Rust vs C++" performance tables and "faster than the C++ implementation" language; replace with benchmarks against golden baseline or prior UFFS versions (e.g., "v0.1.30 baseline")
- **`CLAUDE.md`** — Remove `uffs-legacy` reference and `cpp_comparison` test references
- **`.gitignore`** — Remove any "legacy C++" comments
- **`scripts/verify_parity.rs`** — Rename `cpp_file` → `baseline_file`, rewrite header/messages to use "golden baseline" terminology (this script is the Validation Agent and must be modernized in Phase 1, before DoD checks can pass)
- **CI workflow comments** and **script help text** — Remove C++ implementation references

This is a Rust project. Every comment, doc string, and identifier should describe what the code *does*, not what C++ code it was ported from.

---

## Coordinator: Plan First — No Code Until Spec Approved

**This is a living spec, not an implementation order.** Before any code changes:

1. **Draft a living Spec** for FORGE with:
   - Scope and explicit non-goals
   - Acceptance criteria (commands + measurable checks)
   - Risk list + mitigation (especially around the 40K-line `uffs-mft` crate)
2. **Break the work into tasks and parallelizable waves** — each wave scoped to minimize merge conflicts
3. **Assign each task to a specialist persona** (Investigate / Implement / Verify / Critique / Debug / Code Review)
4. **Explicitly call out** which tasks are safe to parallelize vs must be serialized due to file conflicts

**Stop after producing the Spec + task plan. Wait for approval before implementation.**

### Recommended Space Strategy (Minimizes Conflicts)

The heaviest changes concentrate in `crates/uffs-mft/src/` (io.rs, index.rs, reader.rs, main.rs — all thousands of lines). Avoid parallel agents touching those files simultaneously.

| Space | Scope | Risk Level |
|-------|-------|------------|
| **A — Plan + Guardrails** | Finalized living spec, wave plan, "done means done" checklist | None (no code) |
| **B — Repo Hygiene & Docs** | Move docs/root clutter, add `_trash/` + `.gitignore`, README C++ rewrites, docs reorganization | Low (mostly file moves) |
| **C — CI/CD Overhaul** | Fix workflows, nightly standardization, add test/clippy/coverage steps | Low (won't conflict with module refactors) |
| **D — Code Refactors** | The big `uffs-mft` and `uffs-cli` file-splitting, dependency modernization, idiomatic patterns | High (serialized within this Space) |

Merge B and C into D (or into the main FORGE branch) when ready. Space D work must be serialized — no parallel agents editing the same monolithic files.

### Non-Goals (Explicit Scope Boundaries)

Do **not** do any of the following during FORGE:
- **No new features** — this is a refactor, not a feature release
- **No behavioral changes** (other than bug fixes discovered by validation)
- **No stable toolchain pinning** — nightly is intentional
- **No GUI work** unless directly required by a refactor
- **No Polars version pinning** — continue tracking git main
- **No cross-platform runtime support** — MFT reading remains Windows-only; cross-platform is compile-check only

### Serialization Boundaries (For Coordinator Wave Planning)

These constraints help the Coordinator avoid merge conflicts when assigning parallel tasks:

| Boundary | Rule |
|----------|------|
| `crates/uffs-mft/src/io*` | Only one agent at a time |
| `crates/uffs-mft/src/index*` | Only one agent at a time |
| `crates/uffs-mft/src/reader*` | Only one agent at a time |
| `crates/uffs-mft/src/main.rs` | Only one agent at a time |
| `crates/uffs-cli/src/commands.rs` | Only one agent at a time |
| `docs/` + `_trash/` | Can run in parallel with code refactors |
| `.github/workflows/` | Can run in parallel with code refactors |
| `Cargo.toml` (root + crates) | Serialize with dependency modernization tasks |

### Encoding Constraints for Agents

Augment supports repo-stored rules in `.augment/rules/` and hierarchical `AGENTS.md` / `CLAUDE.md`. These have **length limits** — they're designed for short, high-leverage constraints, not full specs. Split the material:

- **`.augment/rules/forge.md`** (Always-included, short): Non-negotiables only — no C++ port references, spec-first workflow, perf non-regression, no blanket `#[allow]`, `forge:` commit prefix, nightly-only
- **`CLAUDE.md`** (root): Update to remove `uffs-legacy` and `cpp_comparison` references; add a pointer to `.augment/rules/forge.md` for FORGE-specific rules
- **This document** (`INTENT_PROMPT.md` → later `docs/architecture/FORGE_SPEC.md`): The full living spec with decomposition plans, DoD commands, idiom tables, etc. — referenced by agents but not always-included in context

---

## Current Architecture (As-Is)

```
Cargo workspace (resolver = "2", edition = "2024", rust-version = "1.85")
├── crates/uffs-polars/   — Polars facade (compilation isolation, git main branch)
├── crates/uffs-mft/      — MFT reading, parsing, indexing, CLI binary (GOD CRATE ~40K lines)
├── crates/uffs-core/     — Query engine, pattern matching, path resolution (~7K lines)
├── crates/uffs-cli/      — CLI binary on clap (~3.6K lines)
├── crates/uffs-tui/      — Terminal UI on ratatui (~700 lines)
├── crates/uffs-gui/      — Placeholder (~200 lines)
├── crates/uffs-legacy/   — Dead legacy C++ port reference (TO BE REMOVED)
├── crates/uffs-diag/     — Diagnostic binaries ("temporarily enabled" since Jan 2026)
├── vendor/               — Disabled patches kept "for reference"
├── scripts/              — 11 rust-script utilities
├── docs/                 — 91 markdown files (13MB), many investigative/duplicative
├── LOG/                  — 47 changelog-healing session logs
├── dist/                 — 29GB of build artifacts
└── justfile              — 2,441-line task runner
```

**Dependency graph:** `uffs-polars` ← `uffs-mft` ← `uffs-core` ← `uffs-cli` / `uffs-tui`

---

## Critical Problems to Solve

### 1. MONOLITHIC FILES — The #1 Code Smell

These files are far too large for any well-structured Rust project. Each needs to be decomposed into focused submodules:

| File | Lines | Bytes | What It Contains |
|------|-------|-------|------------------|
| `uffs-mft/src/io.rs` | 8,375 | 325KB | AlignedBuffer, extent mapping, fixup, 6+ reader implementations (Parallel, Streaming, Prefetch, Pipelined, IOCP, Bulk, SlidingWindow), record parsing, merging |
| `uffs-mft/src/index.rs` | 7,677 | 291KB | MftIndex struct, FileRecord, building, tree metrics, children sorting, extension index, USN journal application, DataFrame conversion |
| `uffs-mft/src/reader.rs` | 5,070 | 198KB | MftReader orchestrator, MftReadMode enum, multi-drive reading, benchmarking, parquet I/O, statistics |
| `uffs-mft/src/main.rs` | 5,043 | 173KB | Full CLI binary for uffs_mft with 10+ subcommands, logging setup, progress bars, formatting |
| `uffs-mft/src/cpp_types.rs` | 4,279 | 157KB | C++ compatibility types and conversion layer — **DELETE ENTIRELY** |
| `uffs-mft/src/parse.rs` | 3,856 | 162KB | MFT record parsing (zero-alloc parser, attribute iteration, SoA output) |
| `uffs-cli/src/commands.rs` | 3,040 | 108KB | All CLI command implementations in one file |
| `justfile` | 2,441 | 124KB | Monolithic task runner |

**Target:** No single `.rs` file should exceed ~500-800 lines. Each module should have one clear responsibility.

### 2. GOD CRATE: `uffs-mft` Does Too Much

`uffs-mft` currently contains:
- Low-level I/O (aligned buffers, direct disk reads, IOCP)
- NTFS structure parsing
- MFT record parsing
- In-memory indexing (MftIndex)
- Tree metrics computation
- C++ compatibility layer (`cpp_types.rs`, `cpp_tree.rs`, `cpp_io_pipeline.rs`) — **all to be deleted**
- Parquet persistence
- USN journal reading
- A full CLI binary (uffs_mft)
- Platform detection (drive type, elevation check)
- Caching layer

This should be split into focused crates or at minimum well-separated internal modules with clear boundaries.

### 3. DEAD WEIGHT IN THE WORKSPACE

- **`crates/uffs-legacy/`** — "Reference only, do not modify" but still compiled as workspace member
- **`crates/uffs-diag/`** — "Temporarily enabled" since January 2026 (2+ months)
- **`vendor/`** — Disabled patches with a comment saying "kept for reference"
- **`dist/`** — 29GB of build artifacts (keep latest 2 in git for CI/scripts; keep all locally for rollback — offline scripts rely on these to locate current relevant artifacts)
- **`LOG/`** — 47 changelog-healing logs tracked in git
- **`docs/`** — 91 markdown files (13MB), many are investigation notes, not documentation

### 4. CI/CD IS EFFECTIVELY BROKEN

- Original `ci.yml` is **disabled** (manual trigger only)
- Active `optimized-ci.yml` **skips the build step** entirely (GitHub runners OOM on Polars)
- No test execution in CI at all
- No coverage reporting active
- No clippy check in active CI
- Security audit runs but that's it
- The "local-first" approach means CI provides almost no safety net

### 5. TOOLCHAIN & DEPENDENCY HYGIENE

- `rust-toolchain.toml` uses `channel = "nightly"` (unpinned) — **intentional**: this project tracks cutting-edge Polars main + nightly Rust features and is not yet stable enough to freeze
- `once_cell` crate still in deps — unnecessary since Rust 1.80 (`std::sync::LazyLock`)
- Polars facade enables **120+ features** — audit which are actually used
- `async-recursion` and `async-trait` in workspace deps — `async-trait` can be removed **if** all async traits are used generically (no `dyn Trait`); native `async fn in trait` (Rust 1.75+) doesn't support `dyn` dispatch, so investigate before removing
- `dirs-next` — ✅ correct choice (per RUSTSEC-2020-0053, `dirs` is unmaintained; `dirs-next` is the maintained fork)
- `hostname` crate at 0.4.2 — check if still maintained
- `log` + `simplelog` — **remove both**, standardize entirely on `tracing` + `tracing-subscriber` (the modern choice)
- `colored` 3.x — **remove**: only used in `uffs-legacy` (being deleted), zero usage in active crates

### 6. POLARS FEATURE AUDIT (Selective, Not Aggressive)

The `uffs-polars` facade enables 120+ Polars features. **Do NOT blindly trim** — the long-term vision includes:
- **SQL interface** (`sql` feature) for querying the filesystem via SQL
- **MCP (Model Context Protocol)** integration — AI agents querying the file index
- **JSON/CSV/Parquet I/O** for interop with external tools
- **Aggregations, pivots, group-by** for filesystem analytics

**Keep:** Any feature that supports SQL querying, data export, aggregations, string ops, temporal ops, joins, or analytics. These will be used as UFFS evolves into an MCP-enabled filesystem query engine.

**Remove only** features that are clearly irrelevant to filesystem data (e.g., `ewma`, `ewma_by`, `business`, `month_start`, `month_end` — financial time-series features that make zero sense for file search). When in doubt, keep the feature.

### 7. DOCUMENTATION SPRAWL — Major Reorganization Needed

The `docs/` directory (91 files, 13MB) is an unorganized dumping ground:

**Structural problems:**
- Flat top-level with 18 loose `.md` files alongside subdirectories
- `docs/architecture/Investigation/` contains 43 items including `.rs` snapshots (230KB `index.rs`, 112KB `parse.rs`), `.ps1` scripts, a raw `ntfs_index.hpp` C++ header, and duplicate fix docs (`v2`, `v3`, `v4` copies)
- `docs/trial_runs/old_code_currently_not_used/` has 5 old `cpp_tree_*.rs` files
- `docs/Augment Instructions/` (space in dirname — bad practice)
- `docs/Competition/`, `docs/profiles/` — unclear purpose
- `docs/Modernization/` — contains the prior modernization attempt docs

**17 C++ files to move to trash:**
- `docs/CPP_MFT_EXTENT_DIAGNOSTIC_TOOL_SPEC.md`
- `docs/CPP_RAW_MFT_DUMP_TOOL_SPEC.md`
- `docs/architecture/CPP_IO_PIPELINE_PORT.md`
- `docs/architecture/CPP_PARSE_ALGORITHM_PORT.md`
- `docs/architecture/CPP_PARSING_PARITY.md`
- `docs/architecture/CPP_TREE_ALGORITHM_PORT.md`
- `docs/architecture/CPP_TREE_ALGORITHM_PORT_TRACKER.md`
- `docs/architecture/RUST_VS_CPP_ANALYSIS.md`
- `docs/architecture/Investigation/MFT_tree_metrics_cpp_port_parity_final_fix.md`
- `docs/architecture/Investigation/cpp_tree_internal_stream_delta_fix.rs`
- `docs/architecture/Investigation/cpp_tree_two_channel_patched.rs`
- `docs/architecture/Investigation/tree_metrics_cpp_parity_deep_dive_fix.md`
- `docs/architecture/Investigation/ntfs_index.hpp`
- `docs/trial_runs/old_code_currently_not_used/cpp_tree_improved_1.rs` (and 2, 3, 4)
- `docs/trial_runs/old_code_currently_not_used/cpp_tree_org.rs`

**Action:** Move all C++-referencing docs to `_trash/docs/` (gitignored, kept locally for reference but untracked). Do NOT permanently delete — we may need them, but they must leave the tracked repo.

> **IMPORTANT:** `.gitignore` does not untrack files already in git. To remove tracked files while keeping local copies: `cp` → `_trash/`, then `git rm` the originals. **Never `git mv` into `_trash/`** — that keeps them tracked.

**Target docs structure:**
```
docs/
├── architecture/       — ADRs, design decisions, module diagrams
├── dev/                — Developer guides (build, test, CI, cross-compile)
├── user/               — End-user guides (CLI usage, TUI, installation)
├── performance/        — Benchmarks, optimization notes, profiling
└── reference/          — NTFS/MFT technical reference (protocol-level docs)
```

Add `_trash/` to `.gitignore` — this is the graveyard for C++ docs, old investigation notes, and superseded plans.

### 8. ROOT DIRECTORY & REPO STRUCTURE CLEANUP

The repo root is cluttered with files and directories that don't belong there:

**Move to `_trash/` (gitignored, kept locally):**
| Item | Why |
|------|-----|
| `Shell/` | Personal shell configs (`.zshrc`, `.bash_profile`) — not project files |
| `~/` | Empty directory with tilde name (accidental creation) |
| `reference/` | Nearly empty (symlinks + a `problem` file) — obsolete |
| `missing_paths.txt` | Test output data from a Windows drive — not source code |
| `LOG/` (48 files) | Changelog-healing session logs — valuable history but not tracked source |
| `vendor/` | Disabled patches "kept for reference" — no longer active |
| `crates/uffs-legacy/` | Dead C++ port reference |
| `benchmarks/` | Empty (just `.gitkeep`) — remove placeholder or populate |
| C++ scripts in `scripts/` | `analyze_cpp_stats.rs`, `analyze_trial_parity.rs`, `analyze_parity_differences.*`, `compare_outputs.py` (NOTE: **keep** `verify_parity.rs` — it's the Validation Agent, just modernize its C++ terminology) |

**Consolidate config files under `.config/` or appropriate location:**
| Item | Action |
|------|--------|
| `audit.toml` | Move to `.config/audit.toml` or keep at root (cargo-audit expects root) |
| `deny.toml` | Keep at root (cargo-deny expects it) |
| `codecov.yml` | Move to `.github/codecov.yml` (standard location) |
| `release-plz.toml` | Keep at root (release-plz expects it) |
| `.geiger.toml` | Evaluate if still used, move to `.config/` if so |
| `SPDXLICENSES` | Move to `LICENSES/SPDXLICENSES` (belongs with license files) |
| `.gitmessage` | Keep (standard git config) |

**Reorganize `scripts/`:**
```
scripts/
├── ci/                 — CI pipeline scripts (ci-pipeline.rs, build-cross-all.rs, build-local.rs)
├── dev/                — Developer utility scripts (condense, profiling, test helpers)
└── windows/            — Windows-specific scripts (.ps1 files)
```
Remove all C++ analysis/parity scripts → `_trash/scripts/`

**Reorganize `build/`:**
- `build/update_all_versions.rs` (48KB!) — evaluate if still needed, refactor or move to `scripts/`
- `build/.uffs-workflow-state.json` — should be gitignored (state file)

**Clean empty directories:**
- `.cargo/` (empty), `.claude/` (empty), `.qodo/` (empty), `.idea/` (empty) — remove or gitignore

**Target clean root:**
```
UltraFastFileSearch/
├── .config/            — Tool configs (nextest.toml, coverage.toml, audit.toml, geiger.toml)
├── .github/            — CI workflows
├── .reuse/             — REUSE compliance
├── crates/             — Workspace crates (the actual code)
├── dist/               — Release artifacts (latest 2 tracked, rest gitignored)
├── docs/               — Properly organized documentation
├── scripts/            — Organized build/dev/windows scripts
├── _trash/             — Gitignored graveyard (C++ docs, legacy, logs, vendor, references)
├── Cargo.toml          — Workspace manifest
├── Cargo.lock          — Lockfile
├── CHANGELOG.md        — Release changelog
├── CLAUDE.md           — AI assistant context
├── LICENSE             — Primary license
├── LICENSES/           — All license texts + SPDXLICENSES
├── README.md           — Project README
├── REUSE.toml          — REUSE spec
├── deny.toml           — cargo-deny config (must be at root)
├── justfile            — Task runner
├── release-plz.toml    — Release automation
├── rust-toolchain.toml — Toolchain config
└── rustfmt.toml        — Formatter config
```

---

## Modernization Spec (To-Be Target State)

### Module Decomposition Plan

#### `uffs-mft/src/io.rs` (8,375 → ~6 files)
```
uffs-mft/src/io/
├── mod.rs              — Re-exports, shared types
├── aligned_buffer.rs   — AlignedBuffer, sector alignment
├── extent_map.rs       — MftExtentMap, VCN→LCN mapping, chunk generation
├── fixup.rs            — USA fixup, multi-sector fixup
├── readers/
│   ├── mod.rs          — MftRecordReader trait
│   ├── parallel.rs     — ParallelMftReader (SSD)
│   ├── streaming.rs    — StreamingMftReader (low memory)
│   ├── prefetch.rs     — PrefetchMftReader (HDD double-buffer)
│   ├── pipelined.rs    — PipelinedMftReader (I/O thread + parse thread)
│   ├── iocp.rs         — IOCP-based readers (IocpParallel, BulkIocp, SlidingWindow)
│   └── batch.rs        — BatchMftReader (bulk read-all-then-parse)
├── parser.rs           — parse_record_zero_alloc, ParsedRecord, ParsedColumns
└── merger.rs           — MftRecordMerger, extension attribute merging
```

#### `uffs-mft/src/index.rs` (7,677 → ~5 files)
```
uffs-mft/src/index/
├── mod.rs              — MftIndex struct, core types, re-exports
├── types.rs            — FileRecord, LinkInfo, ChildInfo, SizeInfo, StandardInfo
├── builder.rs          — from_parsed_records, record insertion, extension index
├── tree.rs             — Tree metrics computation (descendants, treesize)
├── children.rs         — Child sorting, directory enumeration
├── usn.rs              — USN journal delta application
└── dataframe.rs        — DataFrame conversion (to_dataframe, from_dataframe)
```

#### `uffs-mft/src/main.rs` (5,043 → separate concerns)
- Extract subcommands into individual files under `src/commands/`
- Extract logging/progress infrastructure into `src/logging.rs`
- Keep `main.rs` as thin orchestrator (<100 lines)

#### `uffs-cli/src/commands.rs` (3,040 → ~5 files)
```
uffs-cli/src/commands/
├── mod.rs          — Command enum, shared helpers
├── search.rs       — Search command
├── index.rs        — Index build command
├── info.rs         — Info/stats commands
├── raw.rs          — save-raw / load-raw commands
└── output.rs       — Output formatting helpers
```

### Crate Restructuring

1. **Move `uffs-legacy` and `uffs-diag` out of workspace members** — use `exclude` or move to a separate `tools/` directory
2. **Consider extracting `uffs-mft-index`** as a separate crate if the index grows further
3. **Remove `vendor/`** entirely or move to `.gitignore`-d location
4. **Clean `dist/`** — keep latest 2 artifacts in git for CI/scripts; gitignore the rest

### CI/CD Overhaul

**All CI steps use `dtolnay/rust-toolchain@nightly`** — never mix stable/nightly. This project uses nightly features (SIMD, Polars git main, Edition 2024).

**Reality check:** GitHub-hosted runners OOM on full Polars + workspace link. The existing `optimized-ci.yml` already skips the build step for this reason. Design CI in two tiers:

**Tier 1 — Always on PR (must pass to merge):**
1. **Format check** — `cargo +nightly fmt --check`
2. **Clippy** — `cargo +nightly clippy --workspace --all-targets -- -D warnings` (use `cargo check` for Polars-heavy crates if clippy OOMs)
3. **Tests** — `cargo +nightly nextest run --workspace` (exclude integration tests that require Windows/MFT)
4. **Security** — `cargo audit` + `cargo deny check`

**Tier 2 — Scheduled / manual trigger (weekly + on-demand):**
5. **Full build** — Use a larger runner, `sccache` with S3 backend, or pre-build Polars in a Docker image cached in GHCR
6. **Coverage** — `cargo +nightly llvm-cov` with Codecov upload
7. **Cross-compile check** — `cargo +nightly check --target x86_64-pc-windows-msvc`

**Acceptance criteria:** Tier 1 must be green on every PR. Tier 2 must be green before FORGE merge to main. Both tiers are non-negotiable for the final merge.

### Dependency Modernization

| Current | Action |
|---------|--------|
| `once_cell` | Replace with `std::sync::LazyLock` / `std::sync::OnceLock` |
| `async-trait` | Remove **if** all async traits are used generically (no `dyn Trait`). If any async traits require trait objects, keep `async-trait` and document why — native `async fn in trait` (Rust 1.75+) doesn't support `dyn` dispatch |
| `async-recursion` | Evaluate if still needed with `Box::pin` |
| `log` + `simplelog` | **Remove both** — standardize on `tracing` ecosystem |
| `colored` | **Remove** — only used in `uffs-legacy` (being deleted) |
| `dirs-next` | ✅ Keep (correct maintained fork per RUSTSEC-2020-0053) |
| `hostname` 0.4.2 | Check maintenance status, consider `gethostname` |
| Polars features | Selective audit only — keep SQL, analytics, I/O features for MCP/query vision; remove only clearly irrelevant financial features |

### Code Quality Targets

- **Max file length:** 800 lines (soft target), 500 lines (ideal). Exceptions allowed **only** with documented justification proving that splitting would harm readability or maintainability — not as a convenience shortcut
- **Max function length:** 50 lines (soft target). Longer functions are permitted **only** when a world-class Rust expert would agree that the function is genuinely more readable as one unit (e.g., a complex state machine, a match with many arms that share context). Must include a comment explaining why the function remains long
- **Cyclomatic complexity:** ≤15 per function (same exception policy — hard evidence, not quick fixes)
- **Test coverage:** ≥80% line coverage for non-platform-gated code
- **Zero `#[allow]` in production code** without a comment justifying it (same exception policy — hard evidence, not quick fixes)
- **All public API documented** with examples. **Note:** This is pre-1.0 with zero API stability guarantees — there is no legacy API to preserve. All public interfaces are a blank slate and can be redesigned, renamed, or refactored at will without versioning concerns
- **Every module has a `//!` doc comment** explaining its purpose

### Idiomatic Rust Patterns — Enforce During Refactoring

Every file touched during FORGE must be upgraded to idiomatic, world-class Rust. Replace legacy/C-style patterns with their idiomatic equivalents.

> **Note:** Some replacements below suggest crates (`thiserror`, `bytemuck`, `zerocopy`, `phf`, `arrayvec`, `scopeguard`, `itoa`, `ryu`). These are **guidance, not mandates**. Only add a new crate if it's already in the workspace deps OR justified by measurable wins (perf/safety). Prefer std-only solutions first. This table does not override Rule 6 ("No new dependencies without justification").

| Anti-Pattern | Idiomatic Replacement |
|-------------|----------------------|
| `if len > n { arr[n] }` | `arr.get(n)` / `if let Some(x) = arr.get(n)` |
| `arr[arr.len() - 1]` | `.last()` / `.last().unwrap_or(&default)` |
| `arr[0..n]` after manual bounds check | `.get(0..n)` / `.get(..n).and_then(\|s\| s.try_into().ok())` |
| `for i in 0..len { arr[i] }` | `for (i, x) in arr.iter().enumerate()` |
| Manual sector/chunk iteration | `.chunks_exact(SECTOR_SIZE)` / `.chunks(N)` |
| `if x.is_some() { x.unwrap() }` | `if let Some(v) = x` / `x.map(\|v\| ...)` |
| `if err.is_err() { return err }` | `?` operator |
| `let mut v = Vec::new(); for x in iter { v.push(f(x)) }` | `iter.map(f).collect()` |
| `if condition { Some(x) } else { None }` | `condition.then(\|\| x)` / `condition.then_some(x)` |
| `match opt { Some(x) => f(x), None => default }` | `opt.map_or(default, f)` / `opt.unwrap_or(default)` |
| `String::from("...")` in const context | `"...".to_owned()` or static `&str` |
| `clone()` to satisfy borrow checker | Restructure lifetimes, use `Cow<'_, T>`, or `Arc`/`Rc` |
| `Box<dyn Error>` | `thiserror` enums with `#[error]` derives |
| Raw pointer arithmetic in safe code | `bytemuck`, `zerocopy`, or safe slice operations |
| `unsafe` for simple casts | `TryFrom`/`TryInto`, `bytemuck::cast_slice` |
| Manual `Drop` for RAII | Verify necessity; prefer `scopeguard` or structured ownership |
| `HashMap` for small lookups (<20 keys) | `match`, const arrays, or `phf` |
| `Vec<u8>` for fixed-size buffers | `[u8; N]` / `ArrayVec<u8, N>` |
| `.to_string()` in hot paths | `write!` to pre-allocated buffer, `itoa`/`ryu` for numbers |
| Nested `if let` / `match` chains | Combine with `let-else`, `?`, or early returns |
| `pub use dep::Type` re-export chains (consumers use `crate_a::Thing` when `Thing` lives in `crate_b`) | Depend on the source crate directly — don't route through intermediaries. Makes the real dependency graph visible, prevents version drift, and avoids breakage when the middleman changes its re-exports. **Exception:** `uffs-polars` is an *intentional* facade for compile-time isolation — that re-export is by design |

**Additional style rules:**
- Prefer `let-else` (Rust 1.65+) for early returns: `let Some(x) = opt else { return; };`
- Use `std::mem::take` / `std::mem::replace` instead of clone-then-clear patterns
- Prefer `impl Into<T>` / `impl AsRef<T>` for function parameters over concrete types
- Use `#[must_use]` on functions that return values that should not be silently discarded
- Prefer `From`/`Into` trait implementations over manual conversion functions
- Use type aliases for complex generic types: `type Result<T> = std::result::Result<T, UffsError>;`
- Prefer associated constants over module-level constants when scoped to a type

### Performance Preservation

**Non-negotiable:** All optimizations must be preserved or improved:
- Zero-allocation MFT record parsing
- SoA (Struct-of-Arrays) layout for DataFrame building
- Thread-local buffers
- SSD/HDD-aware I/O strategies
- Double-buffered prefetch
- Rayon parallel parsing
- Large chunk sizes (4-8MB)
- mimalloc allocator
- Polars lazy evaluation

Any refactoring must include before/after benchmarks proving no regression.

---

## Execution Strategy

### Phase 1: Cleanup & Hygiene (Week 1)
- **Delete all C++ traces**: Remove `cpp_types.rs`, `cpp_tree.rs`, `cpp_io_pipeline.rs` (extract any still-needed logic into idiomatic Rust first). Remove `crates/uffs-legacy/` from workspace. Delete C++ comparison scripts and C++ porting docs (`docs/architecture/CPP_*.md`)
- **Modernize `verify_parity.rs` terminology** — rename `cpp_file` → `baseline_file`, rewrite header/messages to use "golden baseline" language. This must happen in Phase 1 because the DoD grep checks cannot pass until this script is cleaned
- **Rewrite READMEs** — remove C++ implementation references from `README.md`, `crates/uffs-mft/README.md`, `CLAUDE.md`, `.gitignore`
- Remove/archive dead weight (`vendor/`, trim `docs/`, manage `dist/` retention policy) — use `cp` → `_trash/` then `git rm`, never `git mv`
- Dependency audit: remove unused crates, update outdated ones
- Selective Polars feature audit (remove only clearly irrelevant financial features; keep SQL/analytics/I/O per Section 6)
- Fix CI to actually run tests and clippy

### Phase 2: Module Decomposition (Weeks 2-3)
- Split `io.rs` into `io/` module tree
- Split `index.rs` into `index/` module tree
- Split `parse.rs` into focused modules
- Split `main.rs` (uffs-mft binary) into command modules
- Split `commands.rs` (uffs-cli) into command modules
- Decompose `justfile` into focused task groups

### Phase 3: Crate Boundaries (Week 3-4)
- Evaluate whether `uffs-mft` should be split further
- Clean up public API surface (too many re-exports)
- Establish clear crate-level API contracts
- Add integration tests at crate boundaries

### Phase 4: Quality Polish (Week 4-5)
- Achieve 80%+ test coverage
- All public items documented
- Benchmark suite for regression detection
- CI pipeline fully functional with caching
- README and docs consolidated

### Validation Agent — Regression Gate

A dedicated validation agent must be invoked at **every major refactoring junction** to ensure no behavioral regressions. This is the safety net that allows aggressive refactoring with confidence.

**Command:**
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```

**What it does:**
1. Runs the freshly-built `uffs` binary against a stored MFT data file (`D_mft.bin`)
2. Produces sorted CSV output of all filesystem records
3. Computes SHA256 over sorted data rows (header/footer aware)
4. Compares against the **golden baseline snapshot** (verified-correct reference output)
5. Reports MATCH or MISMATCH with diff details

**When to run:**
- After any module split or file decomposition (Phase 2)
- After removing C++ compatibility layer files (Phase 1)
- After dependency changes that touch I/O, parsing, or indexing
- After any change to `uffs-mft` or `uffs-core` internals
- Before merging any PR back to `forge/architectural-renaissance`

**Golden baseline files** are stored externally at `/Users/rnio/uffs_data/` — these are the source of truth for correctness and must not be modified during refactoring.

**⚠ Path stability:** Keep `scripts/verify_parity.rs` at this exact path until FORGE is complete — it is the stability anchor. If relocated during scripts reorganization, provide a thin wrapper at the old path so the canonical validation command continues to work.

**Evolution:** As part of FORGE, this script should be modernized:
- Remove C++ terminology from the script itself (rename `cpp_file` → `baseline_file`, etc.)
- Support multiple drives (D, S, C, etc.) in a single validation run
- Add to CI as an optional manual validation step
- Consider converting to a proper integration test in `uffs-cli`

### Definition of Done (Machine-Checkable)

Every criterion below must pass before FORGE can be declared complete. These are concrete, runnable checks — not aspirational goals. All checks are designed to produce correct exit codes so a Verifier agent can run them.

**Build / Lint / Test (nightly only):**
```bash
cargo +nightly fmt --check                                       # Clean formatting
cargo +nightly clippy --workspace --all-targets -- -D warnings   # Zero warnings
cargo +nightly test --workspace                                  # All tests pass (or: cargo nextest run --workspace)
RUSTDOCFLAGS="-D warnings" cargo +nightly doc --workspace --no-deps  # Docs build — warnings are fatal
```

**Validation gate:**
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate  # SHA256 MATCH
```

**File size policy:**
```bash
# Inventory oversized .rs files under crates/ (truth source; documented exceptions still appear here)
find crates/ -name '*.rs' -exec wc -l {} \; \
  | awk '$1 > 800 { print "OVER:", $0; found=1 } END { exit found }'

# Enforce the actual policy: every oversized file must be explicitly allowlisted
# and carry a module-level //! Exception: comment; stale exceptions fail too
bash scripts/check_file_size_policy.sh
```

**C++ reference-implementation purge (zero hits = pass):**
These target *porting/parity/comparison semantics*, NOT the `.cpp` file extension (which is legitimate for a file search tool). Scans all tracked locations including config files and CI workflows:
```bash
# Ban port/parity/comparison language (no trailing \b — C++ ends in non-word chars)
rg -n -S -g'*.rs' -g'*.md' -g'*.yml' -g'*.toml' -i \
  'c\+\+\s*(implementation|port|parity|reference|version)|port(ed)?\s+from\s+c\+\+|matches\s+c\+\+|faster\s+than\s+(the\s+)?c\+\+' \
  crates/ scripts/ docs/ README.md CLAUDE.md .gitignore .github/ justfile \
  && { echo "ERROR: forbidden C++ reference-implementation language found"; exit 1; } || true

# Ban ANY cpp_* identifier (wildcard, not allowlist — catches future variants too)
rg -n -S -g'*.rs' '\bcpp_[A-Za-z0-9_]*\b' crates/ scripts/ \
  && { echo "ERROR: cpp_* identifiers found"; exit 1; } || true

# Confirm deleted files/crates are gone
ls crates/uffs-mft/src/cpp_*.rs 2>/dev/null && exit 1 || true   # Should not exist
ls crates/uffs-legacy/ 2>/dev/null && exit 1 || true            # Should not exist
```

**Dependency purge (direct deps only — transitive deps from other crates are acceptable):**
```bash
# No direct dependency on these crates in any workspace Cargo.toml
! rg -n '^\s*(once_cell|simplelog|colored)\s*=' Cargo.toml crates/*/Cargo.toml

# No direct usage of log macros in our code (transitive log via other crates is OK)
! rg -n -S -g'*.rs' '\blog::(trace|debug|info|warn|error)!|\buse\s+log::' crates/ scripts/
```

**Structural checks:**
```bash
test -d _trash                                      # _trash/ exists
grep -q '_trash' .gitignore                         # _trash/ is gitignored
test ! -d vendor                                    # vendor/ moved to _trash
test ! -d Shell                                     # Shell/ moved to _trash
test ! -f missing_paths.txt                         # moved to _trash
```

### Phase 5: Completion Protocol (Final)

Before declaring FORGE complete, execute these steps in order:

1. **Final sweep** — Re-read every section of this INTENT_PROMPT.md and verify each item was addressed:
   - [ ] All C++ traces eliminated (source, comments, docs, identifiers, scripts, tests, READMEs)
   - [ ] All monolithic files decomposed (no file >800 lines without justified exception)
   - [ ] Root directory cleaned per Section 8 target structure
   - [ ] `docs/` reorganized per Section 7 target structure
   - [ ] `_trash/` populated and gitignored
   - [ ] Dependencies modernized per Section 5
   - [ ] Polars features audited per Section 6
   - [ ] CI pipeline functional with nightly everywhere
   - [ ] `scripts/` reorganized, C++ scripts moved to `_trash/`
   - [ ] All public APIs documented
   - [ ] All modules have `//!` doc comments

2. **Run ALL Definition of Done checks above** — every single one must pass.

3. **Full CI pipeline:**
   ```bash
   just go
   # or: rust-script scripts/ci-pipeline.rs go -v
   ```
   Format → test → lint → build must all pass.

4. **Git workflow throughout FORGE:**
   - Commit frequently with atomic, descriptive messages (`forge: split io.rs into io/ module tree`, `forge: remove cpp_types.rs`, etc.)
   - Use `forge/` prefix for all commit messages during this project
   - Each phase should have multiple commits — this creates a traceable history and the ability to revert individual changes if something breaks
   - Never squash the FORGE branch — preserve the full refactoring history

5. **Final commit & merge:**
   ```bash
   git add -A
   git commit -m "forge: FORGE complete — architectural renaissance"
   git push origin forge/architectural-renaissance
   ```
   Then merge `forge/architectural-renaissance` → `main` via PR (preserving commit history).

---

## Rules of Engagement

1. **Never add blanket `#[allow]`** — fix the root cause
2. **Preserve all performance characteristics** — benchmark before/after
3. **Redesign public APIs freely** — this is pre-1.0 with no consumers; optimize for the best possible API design, not backward compatibility
4. **Small, atomic commits** — one concern per commit
5. **Tests first** — write/update tests before refactoring
6. **No new dependencies** without justification
7. **Every module ≤800 lines** — soft target with documented exceptions allowed per Code Quality Targets policy

---

## Key Context for AI Agents

- This is a **Windows-only tool** for its core functionality (MFT reading). All Windows I/O code is behind `#[cfg(windows)]`.
- **Polars is pulled from git main branch** (not crates.io) for SIMD/nightly features. This is intentional.
- The **`uffs-polars` facade crate** exists solely for compile-time isolation (~4min Polars compile). Do not merge it into other crates.
- The **C++ reference-implementation layer is being deleted** — `cpp_types.rs`, `cpp_tree.rs`, `cpp_io_pipeline.rs` served their purpose for parity validation and are no longer needed. Extract any still-useful algorithms into idiomatic Rust modules before deletion. Remove C++ comparison tests, scripts (`analyze_cpp_stats.rs`, etc.), and docs (`CPP_*.md`). However, `.cpp` as a file extension in search examples/tests is **legitimate** — UFFS searches codebases.
- **`unsafe` code exists** in the I/O layer for Windows API calls and aligned buffer management. Each instance must be preserved with safety documentation, or replaced with safe alternatives if possible.
- The justfile `just go` command is the developer's primary workflow — format → test → lint → build → deploy → commit.
