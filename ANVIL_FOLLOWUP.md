# ANVIL Follow-Up — Remaining Tasks

**Parent spec:** `INTENT_PROMPT_V2.md` (Project ANVIL)
**Status:** ANVIL was declared signed off (T21 complete). However, an audit on 2026-03-10 found several spec targets that were **not fully met** during the ANVIL execution waves.

**Validation command (unchanged):**
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```

---

## A-1: Adopt `zerocopy` for Safe NTFS Structure Parsing

**Original spec reference:** INTENT_PROMPT_V2.md § "Unsafe Code Modernization — Phase 1: Adopt `zerocopy`"

**What ANVIL said:** Replace `core::ptr::read()` with `zerocopy::FromBytes` at 60+ sites. This eliminates entire classes of alignment and validity bugs.

**Current state:** **Zero adoption.** Neither `zerocopy` nor `bytemuck` appears in any `Cargo.toml`. There are **31 `core::ptr::read` / `ptr::read_unaligned` call sites** in non-test production code across these files:

| File | Count | Structures Read |
|------|-------|-----------------|
| `crates/uffs-mft/src/io/parser/fragment.rs` | 4 | `FileRecordSegmentHeader`, `AttributeRecordHeader`, `StandardInformation`, `FileNameAttribute` |
| `crates/uffs-mft/src/io/parser/index.rs` | 4 | Same set |
| `crates/uffs-mft/src/io/parser/fragment_extension.rs` | 3 | `FileRecordSegmentHeader`, `AttributeRecordHeader`, `FileNameAttribute` |
| `crates/uffs-mft/src/io/parser/index_extension.rs` | 3 | Same set |
| `crates/uffs-mft/src/parse/full.rs` | 3 | `FileRecordSegmentHeader`, `AttributeRecordHeader`, `ReparsePointHeader` |
| `crates/uffs-mft/src/parse/forensic/base.rs` | 2 | `AttributeRecordHeader`, `ReparsePointHeader` |
| `crates/uffs-mft/src/parse/forensic/extension.rs` | 1 | `AttributeRecordHeader` |
| `crates/uffs-mft/src/parse/attribute_helpers.rs` | 3 | `StandardInformationFixed`, `StandardInformation`, `FileNameAttribute` |
| `crates/uffs-mft/src/parse/fixup.rs` | 1 | `MultiSectorHeader` |
| `crates/uffs-mft/src/parse/forensic.rs` | 1 | `FileRecordSegmentHeader` |
| `crates/uffs-mft/src/platform/volume.rs` | 1 | `NtfsBootSector` |
| `crates/uffs-mft/src/platform/system.rs` | 1 | `StorageDeviceDescriptor` |
| `crates/uffs-mft/src/usn.rs` | 1 | `USN_RECORD_V2` (via `ptr::read_unaligned`) |
| `crates/uffs-diag/src/bin/dump_mft_records.rs` | 1 | `FileRecordSegmentHeader` |
| `crates/uffs-diag/src/bin/scan_mft_magic.rs` | 1 | `MultiSectorHeader` |
| `crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` | 1 | `FileRecordSegmentHeader` |

**Steps:**

### Step 1: Add `zerocopy` to workspace dependencies
In root `Cargo.toml`, add under `[workspace.dependencies]`:
```toml
zerocopy = { version = "0.8", features = ["derive"] }
```

In `crates/uffs-mft/Cargo.toml`, add:
```toml
zerocopy = { workspace = true }
```

### Step 2: Derive `FromBytes` on NTFS structures
In `crates/uffs-mft/src/ntfs/` (the NTFS structure definitions), add derives to each `#[repr(C)]` struct:

```rust
use zerocopy::{FromBytes, Immutable, KnownLayout};

#[derive(FromBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct FileRecordSegmentHeader {
    // ... existing fields unchanged ...
}
```

Do this for ALL structures that are currently read via `ptr::read`:
- `FileRecordSegmentHeader`
- `AttributeRecordHeader`
- `StandardInformation`
- `StandardInformationFixed`
- `FileNameAttribute`
- `MultiSectorHeader`
- `NtfsBootSector`
- `ReparsePointHeader`
- `StorageDeviceDescriptor` (Windows-only)

### Step 3: Replace each `ptr::read` call site
**Before** (unsafe):
```rust
let header: FileRecordSegmentHeader = unsafe { core::ptr::read(data.as_ptr().cast()) };
```

**After** (safe):
```rust
use zerocopy::FromBytes;
let header = FileRecordSegmentHeader::read_from_prefix(data)
    .map_err(|e| MftError::Parse(format!("invalid header: {e}")))?
    .0;
```

Or for fixed-size reads where you know the buffer is large enough:
```rust
let (header, _rest) = FileRecordSegmentHeader::ref_from_prefix(data)
    .map_err(|e| MftError::Parse(format!("invalid header: {e}")))?;
```

### Step 4: Handle `ptr::read_unaligned` specially
For `usn.rs:372` which uses `ptr::read_unaligned` (the `USN_RECORD_V2` may not be aligned), `zerocopy::FromBytes::read_from_prefix` handles unaligned data natively — it copies bytes, just like `read_unaligned`.

### Step 5: Validate after EACH file
After converting each file, run:
```bash
cargo check -p uffs-mft --all-targets
cargo test -p uffs-mft --lib -- --nocapture
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```

**Recommended conversion order** (start with lowest-risk, highest-impact):
1. `parse/fixup.rs` (1 site, simplest)
2. `parse/attribute_helpers.rs` (3 sites)
3. `parse/forensic.rs` + `parse/forensic/base.rs` + `parse/forensic/extension.rs` (4 sites)
4. `parse/full.rs` (3 sites)
5. `io/parser/index.rs` + `io/parser/index_extension.rs` (7 sites)
6. `io/parser/fragment.rs` + `io/parser/fragment_extension.rs` (7 sites)
7. `platform/volume.rs` (1 site — `NtfsBootSector`)
8. `platform/system.rs` (1 site — `StorageDeviceDescriptor`)
9. `usn.rs` (1 site — `USN_RECORD_V2`)

**Risk:** Medium — changing how bytes are interpreted can expose latent bugs. The parity gate after each file is the safety net.

**Expected outcome:** Eliminate ~31 `unsafe` blocks, reducing the total from 142 to ~111.

---

## A-2: Eliminate Production `unwrap()` / `expect()` / `panic!()`

**Original spec reference:** INTENT_PROMPT_V2.md § "Error Handling" — targets: zero `unwrap()`, ≤5 `expect()`, zero `panic!` in non-test production code.

**Current state:**
- **20 `unwrap()` calls** in non-test code (many in doc examples and benchmarks — acceptable in those contexts, but audit each)
- **23 `expect()` calls** in non-test code (target ≤5)
- **6 `panic!` calls** in non-test code

### `panic!` sites (all 6 — fix these first):

| File | Line | Context | Fix |
|------|------|---------|-----|
| `crates/uffs-cli/src/main.rs` | 639 | `panic!("conflicting search source flags should fail")` | This is inside a `#[test]` function — **actually OK**, the grep just doesn't filter nested test code. Verify manually. |
| `crates/uffs-cli/src/main.rs` | 661 | `panic!("expected index subcommand")` | Same — inside `#[test]`. Verify. |
| `crates/uffs-mft/src/reader.rs` | 528 | `Err(e) => panic!("Unexpected error: {:?}", e)` | Inside `#[test]`. Verify. |
| `crates/uffs-mft/src/reader.rs` | 557 | Same pattern | Inside `#[test]`. Verify. |
| `crates/uffs-mft/src/reader.rs` | 581 | Same pattern | Inside `#[test]`. Verify. |
| `crates/uffs-mft/src/reader.rs` | 598 | Same pattern | Inside `#[test]`. Verify. |

**Action:** Manually verify each. If all 6 are inside `#[cfg(test)]` modules, this target is already met. The grep filter missed nested test code.

### `expect()` sites — the ones that need fixing:

**Acceptable** (tracing subscriber init in `main()` — irrecoverable):
- `crates/uffs-cli/src/main.rs:436` — tracing subscriber init ✅
- `crates/uffs-mft/src/logging.rs:88` — tracing subscriber init ✅
- `crates/uffs-tui/src/main.rs:147` — tracing subscriber init ✅
- `crates/uffs-gui/src/main.rs:118` — tracing subscriber init ✅

**Need fixing** (replace with `?` or `ok_or_else`):
- `crates/uffs-mft/src/progress.rs:17` — `indicatif` template parsing. Replace with:
  ```rust
  ProgressStyle::with_template("...")?
  // or: .map_err(|e| MftError::Config(format!("invalid template: {e}")))?
  ```
- `crates/uffs-cli/src/main.rs:565` — `expect("CLI help should render successfully")`. Replace with `?`.
- `crates/uffs-cli/src/main.rs:566` — `expect("CLI help should be valid UTF-8")`. Replace with `?`.
- `crates/uffs-cli/src/main.rs:618` — `expect("default search args should parse")`. Verify if this is test code.
- `crates/uffs-cli/src/main.rs:649` — `expect("index args should parse")`. Verify if this is test code.
- `crates/uffs-cli/src/main.rs:671` — `expect("index subcommand should exist")`. Verify if this is test code.

**Note:** Many of the `expect()` calls at lines 618, 649, 671 in `main.rs` may be inside `#[test]` functions. Open the file and verify.

**All benchmark `expect()` calls** (in `crates/uffs-core/benches/*.rs`) are acceptable — benchmarks are not production code.

### `unwrap()` audit:

Most of the 20 hits are in:
- Doc comment examples (`//!` lines) — **Acceptable**, these are documentation
- Benchmark files — **Acceptable**
- Test-adjacent code the grep didn't filter — **Verify each**

**Steps:**
1. Open each file and manually verify whether the call is in test/bench/doc context.
2. For any genuinely in production code, replace with `?` or `.ok_or_else(|| ...)`.
3. Commit: `anvil-followup: eliminate remaining production unwrap/expect/panic`

**Risk:** Low if you verify context carefully. The `main.rs` test functions are the likeliest false positives.

---

## A-3: Add `// SAFETY:` Comments to All `unsafe` Blocks

**Original spec reference:** INTENT_PROMPT_V2.md § "Code Quality Targets" — "Every `unsafe` block has `// SAFETY:` comment"

**Current state:** 142 `unsafe` blocks in non-test code, only **52 `// SAFETY:` comments** (~37% coverage). That means ~90 blocks are missing safety documentation.

**Steps:**
1. Find all undocumented unsafe blocks:
   ```bash
   # This finds unsafe blocks NOT preceded by a // SAFETY: comment
   grep -rn "unsafe {" crates/ --include='*.rs' | grep -v test | grep -v tests | grep -v benches
   ```
2. For each block, add a `// SAFETY:` comment on the line(s) immediately before the `unsafe` keyword explaining:
   - **What invariant** the code relies on (e.g., "buffer is at least `size_of::<Header>()` bytes")
   - **Why it's correct** (e.g., "length checked on line N above")
   - **What could go wrong** if the invariant is violated

3. The bulk of undocumented unsafe is in:
   - `crates/uffs-mft/src/platform/system.rs` (~15 blocks) — Windows FFI calls
   - `crates/uffs-mft/src/platform/volume.rs` (~12 blocks) — Volume handle operations
   - `crates/uffs-mft/src/reader/dataframe_read.rs` (~4 blocks) — `CloseHandle` calls
   - `crates/uffs-mft/src/reader/index_read.rs` (~4 blocks) — `CloseHandle` calls
   - `crates/uffs-mft/src/reader/persistence.rs` (~2 blocks) — Seek/read operations
   - `crates/uffs-mft/src/usn.rs` (~5 blocks) — USN journal reading
   - `crates/uffs-mft/src/platform/extents.rs` (~2 blocks) — Extent mapping
   - `crates/uffs-mft/src/io/readers/` (~many blocks) — Reader implementations

4. **Template for Windows FFI calls:**
   ```rust
   // SAFETY: `handle` is a valid, open volume handle obtained from `CreateFileW`
   // in `VolumeHandle::open`. The buffer is stack-allocated with sufficient size.
   // The Windows API may write up to `buffer.len()` bytes; we check the return
   // code before reading from the buffer.
   unsafe { DeviceIoControl(...) }
   ```

5. **Template for `CloseHandle` calls:**
   ```rust
   // SAFETY: `overlapped_handle` was obtained from `CreateEventW` earlier in this
   // function and has not been closed yet. After this call, the handle is invalid
   // and must not be reused.
   unsafe { CloseHandle(overlapped_handle) }.ok();
   ```

6. Verify docs build cleanly (the `undocumented_unsafe_blocks = "deny"` lint will catch any you missed):
   ```bash
   cargo clippy -p uffs-mft --all-targets -- -D clippy::undocumented_unsafe_blocks
   ```

7. Commit: `anvil-followup: document all unsafe blocks with SAFETY comments`

**Risk:** None — documentation-only change. No behavior change.

---

## A-4: Split Remaining 3 Oversized Files

**Original spec reference:** INTENT_PROMPT_V2.md § "File Decomposition Plan" — target: every `.rs` file ≤500 lines (ideal) / ≤800 lines (absolute max).

**Current state:** 3 files exceed 800 lines:

| File | Lines | Issue |
|------|-------|-------|
| `crates/uffs-mft/src/reader/multi_drive.rs` | **1,307** | Multi-volume orchestration |
| `crates/uffs-cli/src/commands/search.rs` | **1,268** | Search command handler |
| `crates/uffs-diag/src/bin/compare_scan_parity.rs` | **987** | Diagnostic binary |

Additionally, **37 files** are between 500-800 lines — above the ideal target.

### A-4a: Split `multi_drive.rs` (1,307 lines)

**Recommended decomposition:**
```
crates/uffs-mft/src/reader/multi_drive/
├── mod.rs              — MultiDriveMftReader struct, orchestrator (<300 lines)
├── config.rs           — Drive configuration, selection logic
├── merge.rs            — Result merging across drives
└── progress.rs         — Multi-drive progress tracking
```

**Steps:**
1. Read `multi_drive.rs` and identify logical boundaries.
2. Extract types/functions into submodules.
3. Keep `mod.rs` as a thin orchestrator with re-exports.
4. Update `reader.rs` `mod multi_drive;` to work with the directory module.
5. Validate:
   ```bash
   cargo check -p uffs-mft --all-targets
   rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
   ```

### A-4b: Split `commands/search.rs` (1,268 lines)

**Recommended decomposition:**
```
crates/uffs-cli/src/commands/search/
├── mod.rs              — SearchCommand entry point, arg handling (<300 lines)
├── execute.rs          — Search execution logic
├── display.rs          — Result display/formatting
└── filters.rs          — Filter construction from CLI args
```

**Steps:**
1. Read `search.rs` and identify the search flow stages.
2. Extract into submodules.
3. Validate:
   ```bash
   cargo check -p uffs-cli --all-targets
   cargo test -p uffs-cli --lib -- --nocapture
   ```

### A-4c: Evaluate `compare_scan_parity.rs` (987 lines)

This is a diagnostic binary in `uffs-diag`. Two options:
- **If still useful:** Split into a module with helpers.
- **If obsolete:** Move to `_trash/` and remove from `uffs-diag`.

Since `uffs-diag` has **0 tests**, evaluate whether this binary is still needed at all.

**Risk:** Medium for A-4a (touches `uffs-mft` reader path — parity gate required). Low for A-4b and A-4c.

---

## A-5: Expand `uffs-cli` Test Coverage

**Original spec reference:** INTENT_PROMPT_V2.md § "Test Strategy — Priority 1: `uffs-cli` tests (5 → ≥30)"

**Current state:** 17 `#[test]` functions in `uffs-cli` (10 in `main.rs`, 3 in `search.rs`, 4 in `output.rs`). Target was ≥30.

**Steps:**
1. Add `assert_cmd` to dev-dependencies of `crates/uffs-cli/Cargo.toml`:
   ```toml
   [dev-dependencies]
   assert_cmd = "2"
   predicates = "3"
   ```
2. Create `crates/uffs-cli/tests/` directory for integration tests.
3. Write tests for:
   - **Argument parsing** (each subcommand: search, index, info, raw-io, stats) — 5 tests
   - **Help output** (`uffs --help`, `uffs search --help`, etc.) — 3 tests
   - **Error paths** (missing required args, invalid patterns, conflicting flags) — 5 tests
   - **Output format validation** (verify `--format csv`, `--format json`, `--format table` produce expected structure) — 3 tests
4. Target: at least 13 new tests to reach ≥30 total.
5. Commit: `anvil-followup: expand uffs-cli test coverage to 30+`

**Risk:** Low — test-only changes.

---

## A-6: Evaluate and Test or Archive `uffs-diag`

**Original spec reference:** INTENT_PROMPT_V2.md § "Test Strategy — Priority 3: `uffs-diag` — decide: test or archive"

**Current state:** 3,567 lines of Rust across 9 binaries, **0 tests**.

**Steps:**
1. List all binaries in `crates/uffs-diag/src/bin/`:
   ```bash
   ls crates/uffs-diag/src/bin/
   ```
2. For each binary, determine:
   - Is it still used? When was it last modified?
   - Does it have a clear purpose documented in a `//!` module doc?
3. **If archiving:** Move `uffs-diag` from `members` to `exclude` in root `Cargo.toml`:
   ```toml
   exclude = [
       "crates/uffs-diag",
   ]
   ```
4. **If keeping:** Add at minimum:
   - A crate-level `README.md` explaining what each binary does
   - At least one smoke test per binary (argument parsing, help output)
   - `//!` module docs on any binary missing them
5. Commit: `anvil-followup: archive or test uffs-diag diagnostic tools`

**Risk:** Low — either way, no production code changes.

---

## A-7: Consolidate Redundant Dependencies

**Original spec reference:** INTENT_PROMPT_V2.md § "Dependency Hygiene"

**Current state — redundancies still present:**

| Redundancy | Action |
|-----------|--------|
| `glob` (0.3.3) alongside `globset` (0.4.18) | Remove `glob`, use `globset` exclusively. Grep for `use glob::` to find call sites. |
| `walkdir` (2.5.0) + `jwalk` (0.8.1) + `ignore` (0.4.25) | Audit which crates use which. Consolidate to one (likely `jwalk` for parallel or `ignore` for gitignore-aware). |
| `num-format` (0.4.4) | Evaluate if still needed. If only for comma formatting, consider `itoa` + manual separator. |
| `console` (0.16.2) | Check if `indicatif` already re-exports what's needed from `console`. |

**Steps (for each):**
1. Search for usage:
   ```bash
   grep -rn "use glob::\|extern crate glob" crates/ --include='*.rs'
   grep -rn "use walkdir::\|use jwalk::\|use ignore::" crates/ --include='*.rs'
   grep -rn "use num_format::" crates/ --include='*.rs'
   grep -rn "use console::" crates/ --include='*.rs'
   ```
2. For each redundant crate, replace call sites with the preferred alternative.
3. Remove from `[workspace.dependencies]` in root `Cargo.toml` and from crate-level `Cargo.toml` files.
4. Verify: `cargo check --workspace --all-targets`
5. Commit one per dependency: `anvil-followup: remove redundant glob dependency`, etc.

**Risk:** Low per dependency, but do them one at a time to isolate failures.

---

## Verification Checklist

After completing all A-1 through A-7 tasks, run:

```bash
# Safety metrics
grep -rn "core::ptr::read" crates/ --include='*.rs' | grep -v test | wc -l   # Target: 0
grep -rn "unsafe {" crates/ --include='*.rs' | grep -v test | wc -l           # Target: ≤111
grep -rn "// SAFETY:" crates/ --include='*.rs' | wc -l                        # Target: matches unsafe count

# Error handling
grep -rn "\.unwrap()" crates/ --include='*.rs' | grep -v test | grep -v "unwrap_or" | wc -l  # Target: 0 production
grep -rn "\.expect(" crates/ --include='*.rs' | grep -v test | wc -l          # Target: ≤5 production

# File sizes
find crates/ -name '*.rs' -exec wc -l {} \; | awk '$1 > 800 { print "OVER:", $0 }'  # Target: 0

# Tests
grep -rn '#\[test\]' crates/uffs-cli/ --include='*.rs' | wc -l                # Target: ≥30
grep -rn '#\[test\]' crates/ --include='*.rs' | wc -l                          # Target: ≥330

# Full validation
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins --tests
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate
```
