# FORGE Follow-Up — Remaining Tasks

**Parent spec:** `INTENT_PROMPT.md` (Project FORGE)
**Status:** FORGE was declared complete and merged to `main`. However, an audit on 2026-03-10 found several items from the original spec that were **not completed** during the FORGE execution waves.

**Validation command (unchanged):**
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```

---

## F-A: Delete `vendor/` Directory

**Original spec reference:** INTENT_PROMPT.md § "Root Directory & Repo Structure Cleanup" and § "Dead Weight in the Workspace"

**What FORGE said:** Remove `vendor/` entirely or move to `.gitignore`-d location. Vendor patches are disabled in `Cargo.toml` (commented out `[patch.crates-io]`).

**Current state:** `vendor/` still exists with 5 subdirectories:
- `vendor/errno/`
- `vendor/fs4/`
- `vendor/mft-reader-rs/`
- `vendor/stacker/`
- `vendor/winapi-util/`

The `[patch.crates-io]` section in root `Cargo.toml` is already commented out (lines 534-538), confirming these are dead.

**Steps:**
1. Copy the entire `vendor/` directory to `_trash/vendor/` for local reference:
   ```bash
   cp -r vendor/ _trash/vendor/
   ```
2. Remove `vendor/` from git tracking:
   ```bash
   git rm -r vendor/
   ```
3. Verify the workspace `exclude` list in `Cargo.toml` line 32 still references `vendor/mft-reader-rs` — remove that exclude entry since the directory is gone:
   ```toml
   # BEFORE
   exclude = [
       "vendor/mft-reader-rs",
   ]
   # AFTER
   exclude = []
   ```
4. Remove the commented-out `[patch.crates-io]` block at lines 526-538 of `Cargo.toml` (it references vendor paths that no longer exist).
5. Verify build still works:
   ```bash
   cargo check --workspace --all-targets
   ```
6. Commit: `forge-followup: remove dead vendor/ directory`

**Risk:** None — patches are already disabled.

---

## F-B: Remove `uffs-gui` from Workspace Members

**Original spec reference:** INTENT_PROMPT_V2.md § "Workspace Structure — Dead Weight & Placeholder Crates"

**What FORGE/ANVIL said:** `uffs-gui` is a 159-line placeholder that prints a banner and exits. It should NOT be a workspace member — it pulls `uffs-polars`, `uffs-mft`, `uffs-core` as dependencies for zero functionality.

**Current state:** `uffs-gui` is still listed in `Cargo.toml` line 27:
```toml
"crates/uffs-gui",      # 🪟 Graphical UI (future)
```

**Steps:**
1. Move `"crates/uffs-gui"` from `members` to `exclude` in root `Cargo.toml`:
   ```toml
   # In [workspace]
   exclude = [
       "crates/uffs-gui",
   ]
   ```
2. Remove the line from `members` (line 27).
3. Verify workspace builds:
   ```bash
   cargo check --workspace --all-targets
   ```
4. Commit: `forge-followup: exclude uffs-gui placeholder from workspace`

**Risk:** None — `uffs-gui` has zero functionality and no other crate depends on it.

---

## F-C: Clean C++ Remnant Documentation

**Original spec reference:** INTENT_PROMPT.md § "Documentation Sprawl" — listed 17 C++ files to move to `_trash/`.

**Current state:** Most were moved, but these remain:

### Still at `docs/` root:
- `docs/CPP_MFT_EXTENT_DIAGNOSTIC_TOOL_SPEC.md`

### Still at `docs/trial_runs/old_code_currently_not_used/`:
- `cpp_tree_improved_1.rs`
- `cpp_tree_improved_2.rs`
- `cpp_tree_improved_3.rs`
- `cpp_tree_improved_4.rs`
- `cpp_tree_org.rs`
- `index_improved_1.rs`
- `index_improved_2.rs`
- `index_improved_3.rs`
- `index_improved_4.rs`
- `index_org.rs`

**Steps:**
1. Copy each file to `_trash/` preserving directory structure:
   ```bash
   mkdir -p _trash/docs/trial_runs/old_code_currently_not_used
   cp docs/CPP_MFT_EXTENT_DIAGNOSTIC_TOOL_SPEC.md _trash/docs/
   cp docs/trial_runs/old_code_currently_not_used/*.rs _trash/docs/trial_runs/old_code_currently_not_used/
   ```
2. Remove from git:
   ```bash
   git rm docs/CPP_MFT_EXTENT_DIAGNOSTIC_TOOL_SPEC.md
   git rm -r docs/trial_runs/old_code_currently_not_used/
   ```
3. If `docs/trial_runs/` is now empty, remove it:
   ```bash
   rmdir docs/trial_runs/ 2>/dev/null  # only if empty
   ```
4. Commit: `forge-followup: archive remaining C++ doc remnants to _trash/`

**Risk:** None — these are historical reference files with no code dependencies.

---

## F-D: Clean `docs/` Directory Structure

**Original spec reference:** INTENT_PROMPT.md § "Documentation Sprawl" — target structure was:
```
docs/
├── architecture/       — ADRs, design decisions, module diagrams
├── dev/                — Developer guides (build, test, CI, cross-compile)
├── user/               — End-user guides (CLI usage, TUI, installation)
├── performance/        — Benchmarks, optimization notes, profiling
└── reference/          — NTFS/MFT technical reference (protocol-level docs)
```

**Current state — problems:**

1. **`docs/Augment Instructions/`** — Space in dirname (bad practice). Contains 1 file.
   - Rename to `docs/augment-instructions/` or move contents to `docs/dev/`
   ```bash
   mv "docs/Augment Instructions" docs/augment-instructions
   ```

2. **`docs/profiles/`** — Empty directory. Delete:
   ```bash
   git rm -r docs/profiles/   # or just rmdir if untracked
   ```

3. **`docs/Competition/`** — 6 files about competitor analysis. Move to `_trash/docs/Competition/`:
   ```bash
   cp -r docs/Competition/ _trash/docs/Competition/
   git rm -r docs/Competition/
   ```

4. **`docs/PHASE7_*.md`** (4 files) — Investigation artifacts, not documentation:
   - `docs/PHASE7_MAC_INSTRUCTIONS.md`
   - `docs/PHASE7_PERFORMANCE_ANALYSIS.md`
   - `docs/PHASE7_QUICK_START.md`
   - `docs/PHASE7_WINDOWS_TESTING.md`

   Move to `_trash/`:
   ```bash
   cp docs/PHASE7_*.md _trash/docs/
   git rm docs/PHASE7_*.md
   ```

5. **Loose files at `docs/` root** — Evaluate each and move to appropriate subdirectory or `_trash/`:
   - `docs/CLI_FEATURE_PARITY.md` → `docs/dev/`
   - `docs/CROSS_PLATFORM_RAW_MFT_LOADING.md` → `docs/dev/`
   - `docs/IMPLEMENTATION_PLAN.md` → `_trash/docs/` (superseded by FORGE/ANVIL)
   - `docs/MFTINDEX_OPTIMIZATION_PLAN.md` → `docs/performance/`
   - `docs/MFT_FEATURE_PARITY.md` → `docs/dev/`
   - `docs/MFT_INVESTIGATION_F_DRIVE.md` → `_trash/docs/` (investigation note)
   - `docs/MILESTONES.md` → `docs/dev/`
   - `docs/UFFS_PERFORMANCE_OPTIMIZATION_PHASE2.md` → `docs/performance/`
   - `docs/uffs-mft-optimization-plan.md` → `docs/performance/`
   - `docs/uffs_mft_optimization_plan_review.md` → `docs/performance/`
   - `docs/windows_profiling_to_mac_plan.md` → `docs/dev/`
   - `docs/xwin-msvc-rlib-size-root-cause-and-workarounds.md` → `docs/dev/`

6. Commit: `forge-followup: reorganize docs/ into target structure`

**Risk:** Low — file moves only, no code changes.

---

## F-E: Replace `num_cpus` with `std::thread::available_parallelism()`

**Original spec reference:** INTENT_PROMPT.md § "Toolchain & Dependency Hygiene" and INTENT_PROMPT_V2.md § "Dependency Modernization"

**Current state:** `num_cpus` is used in these locations:
- `crates/uffs-mft/src/io/readers/parallel/to_index_parallel.rs:83` — `num_cpus::get()`
- `crates/uffs-mft/src/commands/windows/bench.rs:391` — `num_cpus::get()`
- `crates/uffs-mft/benches/mft_read.rs:23` — `use num_cpus as _;` (unused dep marker)
- `crates/uffs-mft/src/lib.rs:84` — `num_cpus as _,` (unused dep marker)
- `crates/uffs-mft/src/main.rs:74` — `use {... num_cpus as _};`
- `crates/uffs-mft/src/reader.rs:174,387` — doc comments referencing `num_cpus`

**Steps:**
1. In each file where `num_cpus::get()` is called, replace with:
   ```rust
   std::thread::available_parallelism()
       .map_or(4, std::num::NonZero::get)
   ```
   The `map_or(4, ...)` fallback handles the rare case where the OS can't determine CPU count.

2. Remove the `use num_cpus as _;` lines from `lib.rs`, `main.rs`, and `benches/mft_read.rs`.

3. Remove `num_cpus` from `crates/uffs-mft/Cargo.toml` dependencies.

4. Remove `num_cpus` from workspace `[workspace.dependencies]` in root `Cargo.toml` (line 146).

5. Update doc comments in `reader.rs` that say "num_cpus" to say "available CPU count".

6. Verify:
   ```bash
   cargo check -p uffs-mft --all-targets
   cargo test -p uffs-mft --lib -- --nocapture
   ```

7. Commit: `forge-followup: replace num_cpus with std::thread::available_parallelism()`

**Risk:** Low — `available_parallelism()` is stable since Rust 1.59, and UFFS requires 1.85+.

---

## F-F: Decompose `justfile` (2,450 → ≤500 Lines)

**Original spec reference:** INTENT_PROMPT.md § "Monolithic Files" and INTENT_PROMPT_V2.md § "JUSTFILE — 2,450 Lines, Monolithic"

**Current state:** `justfile` is 2,450 lines — a single monolithic file.

**Steps:**
1. Create a `just/` directory at repo root.
2. Split the justfile into logical modules using `just`'s `import` or `mod` feature:
   ```
   just/
   ├── build.just       — Build recipes (build, release, dist, cross-compile)
   ├── test.just        — Test recipes (test, nextest, coverage, fuzz)
   ├── lint.just        — Lint recipes (fmt, clippy, doc)
   ├── ci.just          — CI pipeline recipes
   ├── dev.just         — Dev workflow recipes (go, watch, clean)
   └── windows.just     — Windows-specific recipes
   ```
3. Keep the root `justfile` as a thin orchestrator that imports modules:
   ```just
   import 'just/build.just'
   import 'just/test.just'
   import 'just/lint.just'
   import 'just/ci.just'
   import 'just/dev.just'
   import 'just/windows.just'
   
   # Core workflow
   default: go
   ```
4. Remove dead/obsolete recipes during the split.
5. Target: root `justfile` ≤50 lines, each module ≤300 lines.
6. Verify `just go` still works (this is the developer's primary workflow).
7. Commit: `forge-followup: decompose justfile into modular imports`

**Risk:** Medium — `just` module imports are relatively new. Test that all recipes resolve correctly. The `just --list` command should still show all available recipes.

---

## F-G: Reorganize `scripts/` Directory

**Original spec reference:** INTENT_PROMPT.md § "Root Directory & Repo Structure Cleanup" — target:
```
scripts/
├── ci/                 — CI pipeline scripts
├── dev/                — Developer utility scripts
└── windows/            — Windows-specific scripts (.ps1 files)
```

**Current state:** All scripts are flat in `scripts/`.

**Steps:**
1. List all scripts and categorize:
   ```bash
   ls scripts/
   ```
2. Create subdirectories:
   ```bash
   mkdir -p scripts/ci scripts/dev scripts/windows
   ```
3. Move scripts to appropriate locations. Examples:
   - `scripts/ci-pipeline.rs` → `scripts/ci/`
   - `scripts/build-cross-all.rs`, `scripts/build-local.rs` → `scripts/ci/`
   - `scripts/condense*.rs` → `scripts/dev/`
   - `scripts/*.ps1` → `scripts/windows/`
   - `scripts/check_file_size_policy.sh` → `scripts/ci/`
   - `scripts/verify_parity.rs` → **Keep at `scripts/verify_parity.rs`** (stability anchor per spec)
4. Update any references in `justfile`, CI workflows, or docs that point to moved scripts.
5. Verify:
   ```bash
   rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
   ```
6. Commit: `forge-followup: reorganize scripts/ into ci/dev/windows structure`

**Risk:** Medium — must update all references to moved scripts. Search for script paths in:
- `justfile`
- `.github/workflows/*.yml`
- `CLAUDE.md`
- `docs/**/*.md`

---

## Verification Checklist

After completing all F-A through F-G tasks, run:

```bash
# Structural checks
test ! -d vendor                                    # F-A: vendor/ gone
! grep -q 'uffs-gui' Cargo.toml | grep members     # F-B: uffs-gui excluded
test ! -f docs/CPP_MFT_EXTENT_DIAGNOSTIC_TOOL_SPEC.md  # F-C: C++ doc gone
test ! -d "docs/Augment Instructions"               # F-D: space-dirname gone
test ! -d docs/profiles                             # F-D: empty dir gone
! grep -q 'num_cpus' Cargo.toml                    # F-E: num_cpus removed
wc -l justfile | awk '$1 < 100'                    # F-F: justfile thin
test -d scripts/ci                                  # F-G: scripts reorganized

# Full validation
cargo check --workspace --all-targets
cargo test --workspace --lib --bins --tests
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```
