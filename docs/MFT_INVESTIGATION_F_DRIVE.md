# F: Drive MFT Investigation Log

> Living document tracking the detailed investigation into mismatched MFT/path results between the C++ UFFS implementation and the Rust UFFS stack (especially `uffs-mft` and `uffs-cli`). Focus volume: **F:**, large and heavily fragmented NTFS.

## 1. Problem Statement, Scope & Background

### 1.1 High‑level discrepancy

- C++ UFFS implementation vs Rust UFFS CLI on the same **F:** drive.
- Rust index/search results showed:
  - Many paths rendered with placeholder parents like `<dir:XXXXXX>`.
  - A large number of "missing parents" where `parent_frs` had no corresponding directory row.
  - Overall path coverage significantly worse than C++ on the same volume.

### 1.2 Initial hypothesis

- Rust-side **path resolution** or **MFT parsing** might be failing for high FRS values.
- Alternatively, the **raw MFT snapshot** produced on Windows for F: (`f_mft.raw`) could be incorrect (wrong extents / wrong clusters), especially for higher record ranges.

We decided to treat this as a forensic investigation of the **MFT processing pipeline**, from Windows raw read → `f_mft.raw` → `f_mft.parquet` → Rust path resolver.

### 1.3 Where to find more background and reference implementations

This document focuses on the *Rust* side investigation, but it sits in the context of a mature, working legacy implementation. For deeper background and a known‑good reference, use the optional repo-root, gitignored local-only legacy tree at `old_cpp_reference/`:

- **Architecture & design docs (legacy implementation):**
  - `old_cpp_reference/uffs/docs/architecture`
  - Contains exhaustive documentation for the original Ultra-Fast-File-Search, including how the legacy implementation handles:
    - `$MFT` parsing and extent mapping.
    - Path resolution and parent/child relationships.
    - Performance characteristics and design tradeoffs.

- **Reference C++ source code:**
  - `old_cpp_reference/uffs/UltraFastFileSearch-code`
  - This is the **ground truth implementation** we compare against when validating the Rust port. When in doubt about intended behavior, consult this tree to see how the legacy version:
    - Opens NTFS volumes and enumerates `$MFT`.
    - Maps VCN/LCN extents.
    - Interprets `FILE` records and builds full paths.

Together, these two locations provide the full historical and behavioral context for what a "working" `$MFT` pipeline looks like. The investigation documented here uses them as the baseline when judging whether the Rust behavior on F: is correct.

## 2. Offline Artifacts and Tooling

### 2.1 Artifacts captured from Windows

Under `docs/trial_runs/` we worked with:

- `f_mft.raw`: raw `$MFT` snapshot as produced by the Rust tooling on Windows.
- `f_mft.parquet`: parsed MFT DataFrame exported by Rust on Windows.

These two artifacts allowed investigation on macOS without direct access to F:.

### 2.2 Initial diagnostic CLIs (under `uffs-cli`)

We first added several small **diagnostic binaries** under `crates/uffs-cli/src/bin` to operate on those artifacts offline:

- `analyze_mft_parents` — Parquet-based analysis of parent/child coverage.
- `dump_mft_records` — dump headers for specific FRS from `.raw`.
- `scan_mft_magic` — scan magic values across `.raw` to find transitions from `FILE` to other patterns.

These tools were later **moved into a dedicated crate** `crates/uffs-diag` (see §7) to avoid unused-dependency warnings in `uffs-cli` and to keep diagnostics separate from the main CLI.

### 2.3 Windows-side validation & fresh F: artifacts (v0.2.36)

After implementing the `$MFT` extent-mapping fix and wiring up the `uffs-diag` crate, we performed a fresh series of **Windows-side validation steps** on F: using the newly built **v0.2.36** binaries.

#### 2.3.1 Install latest binaries from `dist/`

From the repo root on Windows, we installed the latest built binaries into `~/bin`:

- Script (executed via PowerShell):
  - Resolves `dist/latest` (symlink or text pointer) to the latest `dist/vX.Y.Z` directory.
  - Copies `uffs`, `uffs_mft`, `uffs_tui`, and `uffs_gui` as `*.exe` into `%USERPROFILE%\bin`.
- For v0.2.36 this resolved to:
  - `dist\v0.2.36\uffs\uffs-windows-x64.exe` → `~/bin/uffs.exe`
  - `dist\v0.2.36\uffs_mft\uffs_mft-windows-x64.exe` → `~/bin/uffs_mft.exe`
  - `dist\v0.2.36\uffs_tui\uffs_tui-windows-x64.exe` → `~/bin/uffs_tui.exe`
  - `dist\v0.2.36\uffs_gui\uffs_gui-windows-x64.exe` → `~/bin/uffs_gui.exe`

This ensured that all subsequent commands on F: used the **new binaries that include the corrected `$MFT` extent handling**.

#### 2.3.2 Ground truth from Windows: `fsutil` and `defrag`

We captured authoritative NTFS metadata for F: using built-in Windows tools:

- `fsutil fsinfo ntfsinfo F:`
  - Confirmed the basic geometry and MFT layout:
    - `Bytes Per Cluster  : 65,536` (64 KB).
    - `Bytes Per FileRecord Segment : 1,024`.
    - `Mft Valid Data Length : 4.44 GB`.
    - `Mft Start Lcn  : 0x0000000000794af2`.
    - `Mft2 Start Lcn : 0x0000000000174c59`.
    - `Mft Zone Start : 0x0000000000be61e0`, `Mft Zone End : 0x0000000000be65c0`.

- `defrag F: /A /V`
  - Produced an analysis-only fragmentation report:
    - Volume size: `855.79 GB`.
    - Cluster size: `64 KB`.
    - Used space: `685.46 GB`.
    - `MFT size            = 4.44 GB`.
    - `MFT record count    = 4,656,383`.
    - `Total MFT fragments = 0` (Windows’ view of fragmentation).
  - Overall volume fragmentation: `7%` fragmented space, average `1.01` fragments per file.

This provided an external baseline for comparing what **our Rust tooling** sees vs what Windows reports for the same volume.

#### 2.3.3 Fresh `$MFT` snapshot with `uffs_mft save`

Using the new `uffs_mft` binary (v0.2.36), we took a fresh raw MFT snapshot of F::

- Command:
  - `uffs_mft save --drive F --output f_mft.raw`
- Key log output:
  - `⚠️  MFT is fragmented extents=28 sparse_extents=0 total_clusters=72756 total_records=4656384 mft_size_mb="4547.25"`.
  - `📊 Read plan generated chunks=12 records_to_read=4656384 records_skipped=0 skip_percentage="0.0%"`.
- Summary banner:
  - Total records: `4,656,384`.
  - In-use records: `2,215,620`.
  - Free records: `2,440,764`.
  - Utilization: `47.6%`.
  - Fragmentation (our view): `28 extent(s)`.
  - Output file: `C:\Users\rnio\GitHub\UltraFastFileSearch\f_mft.raw`.
  - Original MFT size: `4.44 GB`; compressed size (zstd): `~810 MB`.

This run exercises the **fixed `$MFT` extent mapping** (via `FSCTL_GET_RETRIEVAL_POINTERS`) and gives us a new `f_mft.raw` to analyze offline.

#### 2.3.4 Parquet export & high-level stats with `uffs_mft load`

We then parsed the new raw snapshot and exported it to Parquet:

- Command:
  - `uffs_mft load f_mft.raw --output f_mft.parquet`
- Key stats:
  - Total records: `4,656,384`.
  - Bytes per record: `1,024`.
  - Records exported to Parquet: `1,572,041`.
  - Output Parquet file: `C:\Users\rnio\GitHub\UltraFastFileSearch\f_mft.parquet` (≈ 61 MB).

We also ran `uffs_mft load f_mft.raw --info-only` for a quick summary:

- Records parsed: `1,572,041`.
- Directories: `268,791`.
- Files: `1,303,250`.
- Total file size: `~439.73 GB`.
- Attributes: Hidden ≈ `2,353`, System ≈ `2,922`, Sparse ≈ `27,258`.

These numbers are in the same ballpark as the Rust CLI’s `--hide-system` view on F: and give us a **volume-wide ground truth from the new raw snapshot**.

#### 2.3.5 Deep on-line scan with `uffs_mft info --deep --drive F`

Finally, we ran the on-line deep info command against F::

- Command:
  - `uffs_mft info --deep --drive F`
- Volume / MFT metrics (abridged):
  - Bytes per sector: `512`.
  - Bytes per cluster: `65,536`.
  - Bytes per MFT record: `1,024`.
  - Total clusters: `14,021,391`; volume size ≈ `855.80 GB`.
  - MFT start LCN: `7,949,042` (matches `fsutil`’s `0x794af2`).
  - MFT size: `4.44 GB` (`~0.519%` of volume).
  - Total records: `4,656,384`; in-use: `2,215,620`; free: `2,440,764`.
  - Fragmentation: `28 extent(s)`.

- Deep scan behavior:
  - Uses the MFT bitmap (no_bitmap = false) to skip unused records.
  - Retrieves `num_extents = 28` via `get_mft_extents()`.
  - Generates a parallel read plan with 12 chunks.
  - Log excerpt:
    - `Added placeholder records for missing parent directories (Vec path) total_added=7867 iterations=2`.
    - `Parallel read complete records_parsed=1,516,398 ...`.
  - Reported breakdown (on the rust side):
    - Directories: `~276,658`.
    - Files: `~1,303,250`.
    - Windows comparison section (excluding hidden/system/metadata):
      - Folders: `~275,757`.
      - Files: `~1,297,978`.
      - Total movable: `1,573,735`.

This confirms that **with the new extent mapping**, we can:

- Successfully read all 4.6M MFT records across 28 extents.
- Derive a ~1.57M-file view of F: that is internally consistent (`save`/`load`/`info --deep` agree within rounding).
- Still see a non-zero, but now explicit, count of **placeholder parents** (`total_added=7867`), which we expect to shrink further as the Rust behavior converges on C++.

## 3. Parquet‑Level Parent/Child Analysis

### 3.1 Tool: `analyze_mft_parents`

Goal: quantifying and characterizing the "missing parents" problem using only `f_mft.parquet`.

Key behavior:

- Load the Parquet into a Polars `DataFrame`.
- Validate expected columns: `frs`, `parent_frs`, and (optionally) `is_directory`.
- Build sets:
  - `frs_set` – all distinct FRS values.
  - `parent_set` – all distinct nonzero `parent_frs` values.
  - `dir_set` – FRS values that appear as directories, based on `is_directory` if present; otherwise we treat all FRS as potential directory candidates.
- Compute:
  - Parents that are referenced but do not have a **directory row** (`missing_parents`).
  - Split `missing_parents` into:
    - Those that **do** appear as some row (non-directory) in `frs_set`.
    - Those that have **no row at all** in the DataFrame.
  - Child counts per missing parent.
  - Bucketed distribution of missing parents by FRS ranges (e.g., buckets of size 100,000).

### 3.2 Findings from Parquet analysis

- There were **thousands of parent_frs values** that did not have corresponding directory rows.
- Many of these missing parents were concentrated in **high‑FRS buckets**, indicating an issue with a particular FRS range rather than random corruption.
- Some missing parents did have non-directory rows; others were entirely absent from the DataFrame.

**Conclusion at this stage:**

- The parent/child issue is real and systematic, not just occasional edge cases.
- The pathology is heavily skewed towards certain **FRS ranges**, suggesting that the corresponding `$MFT` regions may not have been read correctly from disk.

## 4. Raw MFT Record Forensics

### 4.1 Tool: `dump_mft_records`

Goal: inspect individual records in `f_mft.raw` to see whether the raw bytes look like valid NTFS FILE records.

Implementation details:

- Uses `uffs_mft::raw::{load_raw_mft, LoadRawOptions}` to load `f_mft.raw`.
- Defines a local `MultiSectorHeader` and `FileRecordSegmentHeader` struct matching `crates/uffs-mft/src/ntfs.rs` so it can run cross-platform.
- For each requested FRS:
  - Fetch the record via `raw.get_record(frs)`.
  - Interpret the first bytes as a `FileRecordSegmentHeader`.
  - Print fields such as:
    - `magic`, `usa_offset`, `usa_count`.
    - `sequence_number`, `link_count`, `first_attribute_offset`.
    - `flags` and derived booleans: `is_in_use`, `is_directory`, `is_base_record`.
    - `bytes_in_use`, `bytes_allocated`, `base_file_record_segment`.
  - Hex‑dump the first 64 bytes for manual inspection.

### 4.2 Findings from raw record inspection

We targeted FRS values known to be pathological ("missing parents" in the Parquet analysis). Example: **FRS 2,640,657**.

For that FRS (and several similar ones), `dump_mft_records` showed:

- `MultiSectorHeader.magic = 0x00000000` (not `FILE` / `RCRD` / `INDX`).
- Other header fields were obviously nonsense for a valid 1,024‑byte MFT record:
  - `usa_offset = 0`, `usa_count = 0`.
  - `sequence_number = 0`, `link_count = 0`.
  - `first_attribute_offset = 0`.
  - `flags = 0x0000`.
  - `bytes_in_use` was a huge random number (e.g. ~660M bytes) for a 1 KiB record.
  - `base_file_record_segment` was a large garbage value.
- The first 64 bytes hex dump looked like random or unrelated data, not a structured `FILE` record.

**Conclusion at this stage:**

- The **parser and header interpretation are correct**; the data at those offsets in `f_mft.raw` is not an NTFS FILE record at all.
- The root problem is now strongly suspected to be **wrong clusters in the raw MFT snapshot** (`f_mft.raw`) for high FRS ranges.

## 5. Magic Distribution Scan Across `f_mft.raw`

### 5.1 Tool: `scan_mft_magic`

Goal: map out where valid `FILE` records occur within `f_mft.raw`, and where other magic values (or zeros) dominate.

Implementation details:

- Uses `uffs_mft::raw::{load_raw_mft, LoadRawOptions}`.
- Defines a local `MultiSectorHeader` with `magic`, `usa_offset`, `usa_count`.
- For each record `frs` in `0..record_count`:
  - Reads the first `MultiSectorHeader`.
  - Classifies `magic` as one of:
    - `FILE` (`0x454C4946`),
    - `RCRD` (`0x44524352`),
    - `INDX` (`0x58444E49`),
    - `ZERO` (`0x00000000`),
    - or `OTHER`.
  - Collects counts in buckets (default `bucket_size = 100,000`).
- Prints a table for each bucket:
  - `bucket`, `FRS_start`, `FRS_end`, and counts of FILE/RCRD/INDX/ZERO/OTHER.

### 5.2 Findings from magic scan

- Lower FRS ranges of `f_mft.raw` showed expected, dominant `FILE` magic counts.
- For **high‑FRS ranges**, the distribution shifted dramatically:
  - `FILE` counts dropped sharply.
  - `RCRD`, `ZERO`, or `OTHER` dominated the buckets.
- This aligned closely with the FRS buckets where `analyze_mft_parents` had reported large numbers of missing parents.

**Conclusion at this stage:**

- `f_mft.raw` is clearly **not a continuous run of `$MFT` FILE records**; past a certain FRS, it appears to be reading other on‑disk structures (e.g. RCRD, zeros, or unrelated data).
- This strongly points to a bug in the **Windows `$MFT` extent / runlist mapping** logic rather than an error in the offline parser.

## 6. Windows `$MFT` Extent Mapping Fix

### 6.1 The suspect: `FSCTL_GET_RETRIEVAL_POINTERS` handling

The raw snapshot on Windows is built by mapping `$MFT` through `FSCTL_GET_RETRIEVAL_POINTERS` into a set of extents and then reading those clusters.

The key Rust function lives in `crates/uffs-mft/src/platform.rs` (exported by `uffs-mft::VolumeHandle` and `MftExtent`).

The earlier implementation incorrectly handled `ERROR_MORE_DATA (234)` for `FSCTL_GET_RETRIEVAL_POINTERS`:

- It would:
  - Parse whatever partial data came back.
  - Advance `StartingVcn` to `last.vcn + last.cluster_count`.
  - Grow the buffer and loop from the advanced VCN.

This is **not what the Windows API intends** for `ERROR_MORE_DATA` in this context. The correct semantics are:

- `ERROR_MORE_DATA` means the buffer was too small to hold the full `RETRIEVAL_POINTERS_BUFFER` for the requested `StartingVcn`.
- The correct approach is to **increase the buffer size and retry with the same `StartingVcn`**, not to move forward and treat the partial response as final.

### 6.2 Implemented fix

We updated `get_retrieval_pointers` to:

- On `ERROR_MORE_DATA`:
  - Double the buffer size (or otherwise increase it).
  - Retry the IOCTL with the **same** `StartingVcn`.
  - Do not parse or record partial results.
- On `ERROR_HANDLE_EOF (38)`:
  - Stop; there are no more extents.
- Only return an error if **no extents can be collected at all**.

This change ensures the **full runlist for `$MFT`** is obtained as intended, rather than a truncated or malformed set of extents that would cause us to read the wrong clusters for higher VCN/FRS values.

### 6.3 Expected impact

- Regenerating `f_mft.raw` and `f_mft.parquet` on Windows after this fix should:
  - Produce valid `FILE` records in high‑FRS regions where we previously saw `RCRD`/ZERO/OTHER.
  - Dramatically reduce or eliminate the set of "missing parents" in `analyze_mft_parents`.
  - Bring Rust path coverage and counts in line with the legacy implementation on F:.

## 7. Diagnostic Crate Split: `uffs-diag`

### 7.1 Motivation

The initial diagnostic binaries were implemented under `crates/uffs-cli/src/bin`. This caused several issues:

- They pulled in heavy dependencies (`chrono`, `clap`, `uffs_polars`, etc.) into the CLI test profile, triggering `-W unused-crate-dependencies` warnings.
- They mixed **offline diagnostics** with the main end‑user CLI, conflating responsibilities.

Given the workspace’s **strict linting policy** (no broad suppression hacks, prefer surgical fixes), we instead:

- Created a dedicated crate: `crates/uffs-diag`.
- Moved all offline diagnostics and helpers there.

### 7.2 New crate: `crates/uffs-diag`

Key points:

- Added to workspace members in `Cargo.toml` and to `[workspace.dependencies]` as `uffs-diag = { path = "crates/uffs-diag" }`.
- `Cargo.toml` for `uffs-diag` declares minimal dependencies:
  - `anyhow` for error handling.
  - `uffs-mft` for raw MFT and Windows NTFS integration.
  - `uffs-polars` for Parquet/Polars analysis.

Binaries now in `uffs-diag`:

- `analyze_mft_parents` – Parquet parent/child coverage analysis.
- `dump_mft_records` – targeted record header inspection from `.raw`.
- `scan_mft_magic` – magic distribution across `.raw`.
- `dump_mft_extents` – **new Windows-only helper**, see below.

### 7.3 CLI stubs left in `uffs-cli`

To preserve discoverability and avoid breaking existing usage patterns, we left tiny stubs in `crates/uffs-cli/src/bin` with the same names, e.g.:

- `scan_mft_magic.rs` in `uffs-cli` now just prints a message:
  - Tells the user the tool moved to `uffs-diag`.
  - Suggests running: `cargo run -p uffs-diag --bin scan_mft_magic -- <args>`.

This removes the heavier diag dependencies from the CLI’s build surface while preserving the user‑facing names.

### 7.4 Handling lint warnings cleanly

Given the workspace’s `unused_crate_dependencies = "warn"` and clippy `cargo` lints at `deny`, we:

- Kept dependencies only where they are actually used.
- Where a crate is intentionally version‑locked but not directly used by a given binary (e.g. `uffs_polars` being pulled into `uffs-diag` for consistency), we added **small, targeted** `use crate as _;` patterns with comments explaining why, e.g.:
  - In `analyze_mft_parents`: `use uffs_mft as _;` to keep diagnostics version‑locked with the core MFT reader.
  - In `dump_mft_records` and `scan_mft_magic`: `use uffs_polars as _;` so diag tools share the same Polars facade version.
- This avoids broad `#[allow(unused_crate_dependencies)]` and keeps reasoning explicit.

## 8. New Windows Helper: `dump_mft_extents`

### 8.1 Purpose

You explicitly requested a **tiny Windows helper** to surface the `$MFT` extents Rust sees, so they can be compared directly with tools like `fsutil file queryextents` or `ntfsinfo` on F:.

Binary: `crates/uffs-diag/src/bin/dump_mft_extents.rs`

### 8.2 Behavior

On **Windows**:

- Usage:
  - `dump_mft_extents F` (drive letter without colon).
- Steps:
  1. Parse the drive letter (`A`–`Z`, case‑insensitive).
  2. Call `VolumeHandle::open(drive)` from `uffs-mft`.
  3. Retrieve `volume_data = handle.volume_data()` and print:
     - `bytes_per_sector`, `bytes_per_cluster`.
     - `bytes_per_file_record_segment`, `clusters_per_file_record_segment`.
     - `mft_valid_data_length`, `mft_start_lcn`, `mft2_start_lcn`, `mft_zone_start`, `mft_zone_end`.
  4. Call `handle.get_mft_extents()` and print each `MftExtent`:
     - `idx`, `vcn`, `cluster_count`, `lcn`, plus derived `byte_offset` and `byte_size`.
  5. Compute and print summary:
     - `extent_count`.
     - `total_clusters`, `total_bytes`.
     - `approx_records = total_bytes / bytes_per_file_record_segment`.

On **non‑Windows** builds:

- The binary compiles but `main` is a trivial stub that prints:
  - "dump_mft_extents is only supported on Windows targets."
- This keeps the workspace building cleanly on macOS/Linux while ensuring the real logic is gated under `#[cfg(windows)]`.

### 8.3 Linting and dependency handling

- On Windows, we import `std::env`, `anyhow::{Context, Result}`, and `uffs_mft::{MftExtent, VolumeHandle}` under `#[cfg(windows)]`.
- On non‑Windows, we include:
  - `#[cfg(not(windows))] use { anyhow as _, uffs_mft as _, uffs_polars as _, };`
  - This keeps diag dependencies version‑locked and satisfies `unused_crate_dependencies` without blanket allows.

## 9. Current Status & Next Steps

### 9.1 Current status

As of this document’s creation:

- **Extent mapping fix** for `$MFT` on Windows is implemented in `uffs-mft` (correct `ERROR_MORE_DATA` handling in `get_retrieval_pointers`).
- **Diagnostic tooling** is consolidated in `crates/uffs-diag`:
  - `analyze_mft_parents` works on `.parquet`.
  - `dump_mft_records` and `scan_mft_magic` work on `.raw`.
  - `dump_mft_extents` exposes `$MFT` extents on Windows.
- `uffs-diag` builds cleanly under the workspace’s strict lint regime.
- `uffs-cli` no longer carries the heavy offline diagnostics in its own bin directory; only stubs remain.
- The full CI pipeline (`./scripts/ci-pipeline.rs go --coverage-report`) currently passes with the latest workspace state (last successful run reported version `0.2.38`).

In addition, we have performed a **full end-to-end comparison** on F: using the production C++ binary and the Rust CLI, then analyzed the differences with the `analyze_diff` tool:

- C++ run (production, on Windows):
  - Command: `uffs.exe "*" --hide-system --no-bitmap --drive F > cpp_f.txt`
- Rust run (current Rust CLI on Windows):
  - Command: `uffs.exe "*" --hide-system --no-bitmap --drive F > rust_f.txt`
- Deep comparison on macOS using `analyze_diff`:
  - Command (with absolute paths into `docs/trial_runs/UltraFastFileSearch/`):
    - `cargo run --release --bin analyze_diff cpp_f.txt rust_f.txt`

Key high-level findings from this diff:

- C++: **2,369,731** unique paths.
- Rust: **1,577,217** unique paths.
- Exact matches: **59,365** paths.
- C++-only paths: **2,310,366**.
- Rust-only paths: **1,517,852**.
- Overall match rate: **~2.5%**.

The diff also shows:

- A very large set of **parent directories present only in the C++ output**:
  - `387,732` parent directories in C++ that do not appear in Rust.
  - Heavy concentrations of missing files under certain trees (top examples):
    - `f:/users/rnio/pictures/icloud photos/photos/` – **191,128** files missing in Rust.
    - `f:/windows/softwaredistribution/download/.../windows11.0-kb5055528-x64/` – **68,352** files missing.
    - `f:/windows/winsxs/manifests/` – **26,861** files missing.
    - Plus many other high-volume system/app trees).
- Rust output contains many entries with placeholder parents like `f:/<dir:...>/...`, while C++ shows concrete directory paths for the same logical content.

Taken together with the earlier raw/MFT analysis, this diff confirms that **Rust is currently missing a huge fraction of F:’s directory tree** and is “inventing” pseudo-parents (`<dir:XXXXXX>`) where it cannot resolve true parents.

### 9.2 Planned / Recommended next steps (on Windows, with F:)

1. **Regenerate artifacts with the fixed extent mapping**:
   - Rebuild the Rust tools on Windows with the updated `uffs-mft`.
   - Run the main indexing CLI against F: to regenerate:
     - `docs/trial_runs/f_mft.raw`.
     - `docs/trial_runs/f_mft.parquet`.

2. **Verify `$MFT` extents vs Windows tools**:
   - Run:
     - `cargo run -p uffs-diag --bin dump_mft_extents -- F`
   - Compare to `fsutil file queryextents` / `ntfsinfo` output for `$MFT` on F:.
   - Confirm the extents match in VCN/LCN layout.

3. **Re-run offline diagnostics on the new artifacts**:
   - `cargo run -p uffs-diag --bin scan_mft_magic -- docs/trial_runs/f_mft.raw 100000`
   - `cargo run -p uffs-diag --bin dump_mft_records -- docs/trial_runs/f_mft.raw <a few high FRS>`
   - `cargo run -p uffs-diag --bin analyze_mft_parents -- docs/trial_runs/f_mft.parquet`

4. **Compare Rust vs C++ path coverage again**:
   - Re-run equivalent queries on Rust and legacy implementations using the newly indexed F:.
   - Confirm path counts, directory coverage, and absence of `<dir:XXXXXX>` placeholders align.

5. **Refine any remaining edge cases**:
   - If discrepancies remain, use the same diagnostic stack:
     - `dump_mft_records` for specific problematic FRS.
     - `dump_mft_extents` to confirm local runlist layout.
     - Targeted diffs against C++ behavior.

## 10. Post-fix diagnostics on latest F: artifacts

### 10.1 Updated `analyze_mft_parents` results

After regenerating `f_mft.raw` / `f_mft.parquet` with the corrected `$MFT` extent mapping and re-running the comparison scans on F:, we ran the parent/child analyzer on the **latest** Parquet snapshot:

- Command:
  - `cargo run -p uffs-diag --release --bin analyze_mft_parents -- docs/trial_runs/UltraFastFileSearch/f_mft.parquet`

Key metrics (rounded):

- Total rows in `f_mft.parquet`: ~**1.57M**.
- Distinct `frs`: ~**1.51M**.
- Distinct non-zero `parent_frs`: ~**238k**.
- Directory FRS (`is_directory = true`): ~**268k**.
- **Missing parents**: **8,500–8,600** distinct `parent_frs` values that are referenced by children but have **no directory row** in the table.

The analyzer also reports the **top missing parents by child count**. On the current F: snapshot, the most extreme examples are:

- `parent_frs = 2,640,657` → ~11.8k children.
- `parent_frs = 2,631,176` → ~10.5k children.
- `parent_frs = 2,628,892` → ~4.3k children.
- `parent_frs = 2,628,924` → ~3.0k children.
- `parent_frs = 2,627,024` → ~2.9k children.

Bucketed by FRS ranges (e.g. 100k-sized buckets), these missing parents are **heavily concentrated in high-FRS regions** (around the 2.6M range and above), not uniformly spread across the MFT.

### 10.2 Updated `scan_mft_magic` results

We re-ran the magic-distribution scanner on the latest raw snapshot to correlate missing parents with the on-disk record types:

- Command:
  - `cargo run -p uffs-diag --release --bin scan_mft_magic -- docs/trial_runs/UltraFastFileSearch/f_mft.raw 100000`

Findings (abridged):

- The **lower FRS ranges** still show the expected dominance of `FILE` magic – i.e. contiguous, healthy MFT regions.
- In the **high-FRS buckets** where `analyze_mft_parents` reports many missing parents, `scan_mft_magic` shows a very different distribution:
  - `FILE` counts drop sharply.
  - `RCRD`, `ZERO`, and other non-`FILE` magic values dominate.

This confirms that, for many of the problematic parent FRS values, the corresponding raw records in `f_mft.raw` are **not base `FILE` records** at all; they are either zeroed, log (`RCRD`), or other non-FILE structures.

### 10.3 Targeted `dump_mft_records` on top missing parents

To understand these high-impact missing parents, we inspected specific FRS directly from the latest raw snapshot:

- Command:
  - `cargo run -p uffs-diag --release --bin dump_mft_records -- docs/trial_runs/UltraFastFileSearch/f_mft.raw 2640657 2631176 2628892 2628924 2627024`

Representative findings:

- **FRS 2,640,657**:
  - `magic = 0x00000000` (`ZERO`), `usa_offset = 0`, `usa_count = 0`.
  - Header fields (e.g. `bytes_in_use`, `base_file_record_segment`) contain nonsensical values for a 1 KiB record.
  - Interpreting this as a `FILE` record would be incorrect; it is effectively garbage from an MFT perspective.

- **FRS 2,631,176; 2,628,892; 2,628,924; 2,627,024**:
  - `magic = 0x44524352` (`RCRD` – log-file style records), not `FILE`.
  - Some are `is_in_use = true`, others `false`.
  - All have **non-zero `base_file_record_segment`**, i.e. they are *extension* records tied to some base file-record segment, not standalone base records.

Across these samples we see a consistent pattern:

- Many of the highest-impact **missing parents** are **not valid base directory FILE records** in the latest `f_mft.raw`.
- From the raw MFT’s point of view, it is therefore reasonable (and correct) that `uffs-mft` does not emit a directory row for those FRS in `f_mft.parquet`.

However, the legacy implementation is still able to resolve real directory paths for children that name these FRS as `parent_frs`, while the current Rust path resolver falls back to placeholders like `<dir:2640657>`.

### 10.4 Working hypothesis

Given the above, the remaining discrepancy between C++ and Rust on F: is now believed to be **semantic**, not I/O-related:

- The earlier `$MFT` extent mapping bug (wrong clusters at high FRS) has been fixed, and the new raw snapshot behaves consistently with Windows’ reported geometry.
- The new diagnostics show that high-FRS parents with many children often correspond to **extension or log records**, not base directory FILE records.
- The C++ pipeline likely has richer logic around:
  - Following `base_file_record_segment` from extensions back to their base records.
  - Interpreting stale or recycled parent references using sequence numbers.
  - Handling directory detection when the “obvious” base record is missing or non-directory.

The Rust pipeline currently treats `parent_frs` more literally and only considers base `FILE` records marked as directories when building the directory table. This explains:

- Why `analyze_mft_parents` reports thousands of missing parents clustered in specific FRS regions.
- Why path reconstruction ends up with `<dir:FRS>` components for children whose `parent_frs` does not correspond to a directory row in `f_mft.parquet`.

## 11. Reference MFT reader (`mft-reader-rs`) and cross-checks

To better understand the C++ semantics around these tricky high-FRS parents, the C++ team provided a **Rust reference MFT reader** that is intended to be a **1:1 port of the C++ MFT-reading logic** (but without full path reconstruction):

### 11.1 What the reference reader does

- Opens the NTFS volume (e.g. `\\.\\F:`) and reads `$MFT` using:
  - `FSCTL_GET_NTFS_VOLUME_DATA` for geometry.
  - `FSCTL_GET_RETRIEVAL_POINTERS` for MFT extents (with correct `ERROR_MORE_DATA` semantics).
- Reads every MFT record using the resulting runlist, including USA unfixup.
- Parses key attributes (`$FILE_NAME`, `$STANDARD_INFORMATION`, `$DATA`).
- Exports one CSV row per record, with fields like:
  - `RecordNumber`, `SequenceNumber`, `IsInUse`, `IsDirectory`, `IsBaseRecord`.
  - `ParentRecordNumber`, `ParentSequenceNumber` (from `$FILE_NAME`).
  - File name, timestamps, file sizes, attribute flags, etc.

This gives us a **direct, C++-equivalent view** of the raw MFT records, separate from `uffs-mft`.

### 11.2 Windows-side run

On the Windows machine that has the F: drive attached, we built and ran the reference reader and captured its view of `$MFT`:

1. Build and run the reference reader against F::
   - From the local checkout containing the reference reader on Windows:
     - `cargo run --release -- -d F -o f_mft_reference.csv -v`

2. Copy the resulting CSV back into this repo on macOS, under:
   - `docs/trial_runs/UltraFastFileSearch/f_mft_reference.csv`

The resulting `f_mft_reference.csv` contains ~4.65M rows (matching the `MFT record count` reported by `defrag` / `uffs_mft info --deep`) and serves as the C++-semantics baseline for our offline checks.

### 11.3 Offline cross-checks (macOS, current status)

With `f_mft_reference.csv` and `f_mft.parquet` side-by-side, we implemented a dedicated diagnostic binary in `crates/uffs-diag` to perform the cross-check:

- Tool: `cross_check_mft_reference` (`crates/uffs-diag/src/bin/cross_check_mft_reference.rs`).
- Command:
  - `cargo run -p uffs-diag --release --bin cross_check_mft_reference -- docs/trial_runs/UltraFastFileSearch/f_mft_reference.csv docs/trial_runs/UltraFastFileSearch/f_mft.parquet`

High-level metrics from this cross-check:

- Reference CSV rows: **4,656,383**.
- Parquet rows: **1,572,041**.
- Joined rows on FRS: **1,572,041** (i.e. every Parquet FRS has a corresponding reference row).
- Directory flag agreement:
  - For all joined rows, the reference `IsDirectory` flag and the Rust `is_directory` column **agree 100%** (0 mismatches).

We also approximate base-record status on the Parquet side (based on `base_file_record_segment` being zero) and confirm that this agrees with the reference `IsBaseRecord` flag wherever that information is available. In other words, for all FRS that **both** systems emit, `uffs-mft`’s classification of directories and base records matches the reference reader.

The remaining discrepancies are therefore entirely about **FRS that appear only in the reference CSV**. To analyze those, `cross_check_mft_reference` also prints, for a small fixed set of high-impact parent FRS (e.g. `2640657`, `2631176`, `2628892`, `2628924`, `2627024`):

- The reference row for that FRS (including `IsInUse`, `IsDirectory`, `IsBaseRecord`, and parent information).
- All children in `f_mft.parquet` whose `parent_frs` equals that FRS.

For these high-impact parents we observe the following pattern:

- In the **reference CSV**, the FRS is marked as an *in-use directory base record* and is the parent of thousands of children.
- In `f_mft.parquet`, there is **no row at all** with that FRS; we only see many children referencing it via `parent_frs`.
- In the **raw snapshot** (`f_mft.raw`), tools like `dump_mft_records` and the newer `inspect_mft_record_flow` (see below) show that the corresponding records are **not valid base `FILE` records**:
  - Many have `magic = 0x00000000` (`ZERO`) and obviously bogus header fields.
  - Others have `magic = 'RCRD'` and behave like log/extension records, often with non-zero `base_file_record_segment`.

This strongly suggests that the reference reader/C++ pipeline is willing to treat some **historical or log-derived information** as the effective parent directory for these children, whereas `uffs-mft` currently restricts itself to what appears as a valid base `FILE` record in the offline snapshot. To keep the Rust behavior explicit and debuggable, we:

- Continue to avoid fabricating real directory rows for FRS whose on-disk snapshot is clearly not a valid base `FILE` record.
- Instead, ensure that any such `parent_frs` get a **synthetic placeholder directory row** in the DataFrame (e.g. `name = "<dir:2640657>"`, `is_directory = true`), so that path resolution has a stable node to attach children to.

To support this investigation we added another diagnostic binary, `inspect_mft_record_flow` (`crates/uffs-diag/src/bin/inspect_mft_record_flow.rs`):

- It loads `f_mft.raw` via `uffs-mft::raw::load_raw_mft` and, for selected FRS values, interprets the header using a local `FileRecordSegmentHeader` layout (cross-platform).
- It prints header fields (magic, flags, bytes-in-use, base-file-record segment, USA offset/count, etc.) and, on Windows, calls a small helper (`run_fixup_and_parse_for_frs` in `crates/uffs-diag/src/uffs_mft_helpers_windows.rs`) that runs `apply_fixup` + `parse_record_full` on the same bytes.
- When run against the high-impact parent FRS listed above, this tool confirms the same story seen in §10:
  - Either the record fails multi-sector fixup entirely, or
  - It parses as an extension/log-style record rather than a base `FILE` directory record.

Together, these cross-checks establish that **where `uffs-mft` produces a row, it matches the reference semantics**, and that the remaining differences are tied to FRS where the offline raw snapshot and the reference CSV genuinely disagree about what lives at that record number.

---

This document is intended to be a **living log**. As we continue the investigation (especially if we capture newer F: snapshots or additional reference-reader runs), we will append:

- Correlated findings between `f_mft.parquet` and the reference CSV.
- Any remaining mismatches and their root causes.
- Final validation steps confirming Rust == C++ behavior on real-world F: workloads.

## 12. C++ Raw MFT Dump Tool and Byte-Level Comparison

### 12.1 Motivation

We now have high confidence that, for every FRS where both the reference CSV and `f_mft.parquet` contain a row, the Rust MFT reader (`uffs-mft`) agrees with the C++ semantics (directory flags, base-record vs extension). The remaining discrepancies are driven by FRS where:

- The **reference reader** reports an in-use directory base record with many children, but
- Our **offline raw snapshot** (`f_mft.raw`) contains either zeroed or `RCRD`/log-style records at the same location.

To completely rule out any residual differences in **how we read the raw bytes from disk** (extent mapping, VCN/LCN translation, etc.), we will ask the C++ team to build an independent, very low-level `$MFT` dump tool in C++.

The goal of this tool is to generate a second raw snapshot ("C++ view of `$MFT`") using the same Windows APIs and patterns the legacy implementation trusts in production. We can then compare that file bit-for-bit against the raw snapshot produced by `uffs-mft`.

### 12.2 C++ raw dump tool design

We have documented the full specification for this tool in:

- `docs/CPP_RAW_MFT_DUMP_TOOL_SPEC.md`

Key points for the C++ tool (no access to the Rust repo required on their side):

- Platform: Windows, run from PowerShell with administrator rights.
- Inputs:
  - Drive letter, e.g. `F`.
  - Output path for the raw snapshot, e.g. `C:\\...\\f_mft_cpp.raw`.
- Behavior:
  1. Open the NTFS volume (`\\.\\F:`) and `$MFT` stream.
  2. Use `FSCTL_GET_NTFS_VOLUME_DATA` to obtain:
     - `BytesPerCluster`, `BytesPerFileRecordSegment`, `MftValidDataLength`, `MftStartLcn`.
  3. Use `FSCTL_GET_RETRIEVAL_POINTERS` on `$MFT` with **correct `ERROR_MORE_DATA` handling** to obtain the full runlist of MFT extents.
  4. Read all clusters belonging to `$MFT` in VCN order into a contiguous buffer using `SetFilePointerEx` + `ReadFile`.
  5. Interpret this buffer as `record_count = total_bytes / BytesPerFileRecordSegment` consecutive records (FRS 0..record_count-1), *without* applying multi-sector fixup or parsing.
  6. Write the result using our existing `UFFS-MFT` file format:
     - 64-byte header (magic `"UFFS-MFT"`, version 1, flags 0, record size, record count, original size, compressed size = 0).
     - Followed by the raw bytes (`record_size * record_count`).

The spec file includes explicit C++-style header-writing pseudocode and the exact layout we expect.

### 12.3 How we will use the C++ snapshot

Once the C++ tool is available and has produced a snapshot (for example:

- `docs/trial_runs/UltraFastFileSearch/f_mft_cpp.raw`

we will:

1. Load both `f_mft_cpp.raw` and `f_mft.raw` via `uffs-mft::raw::load_raw_mft`.
2. Verify that their headers (`record_size`, `record_count`, `original_size`) are consistent with each other and with `FSCTL_GET_NTFS_VOLUME_DATA`.
3. Compare the data regions:
   - Compute hashes or checksums over the entire data region.
   - If needed, compare record-by-record to locate any FRS ranges where the bytes differ.
4. Reuse existing diagnostics (`scan_mft_magic`, `dump_mft_records`, `inspect_mft_record_flow`) on both snapshots to understand whether magic distributions and header fields match across tools.

If the two snapshots match bit-for-bit for the vast majority of the MFT (especially in the high-FRS ranges where we currently see discrepancies), we can assert that Rust and C++ are reading the same raw bytes, and that any remaining behavioral differences arise from higher-level interpretation. If they do not match, the per-FRS diff will pinpoint exactly where the two stacks diverge at the raw I/O level, guiding further fixes in `uffs-mft`'s Windows reader.

### 12.4 C++ Raw Dump Tool Implementation

The C++ team implemented the raw MFT dump tool as specified. The tool:

- Uses `FSCTL_GET_NTFS_VOLUME_DATA` for geometry
- Uses `FSCTL_GET_RETRIEVAL_POINTERS` for extent mapping
- Writes the same 64-byte header format as Rust (`UFFS-MFT` magic, version 1, flags 0)
- Outputs raw MFT bytes without compression

### 12.5 Extent Retrieval Bug Fix (Phase 1)

Before comparing raw dumps, we first fixed a critical bug in Rust's extent retrieval:

**Bugs identified and fixed in `crates/uffs-mft/src/platform.rs`:**

| Bug | Before | After |
|-----|--------|-------|
| Path format | `\\.\F:\$MFT` | `F:\$MFT` |
| File access flags | `FILE_READ_ATTRIBUTES.0` | `0` |
| File flags | `FILE_FLAG_OPEN_REPARSE_POINT \| FILE_FLAG_NO_BUFFERING` | `FILE_FLAGS_AND_ATTRIBUTES(0)` |
| HRESULT extraction | Compared full HRESULT (`0x800700EA`) against Win32 code (`234`) | Extract Win32 error: `hresult & 0xFFFF` for FACILITY_WIN32 |

**Commit:** `f3f356dff` - "fix: correct MFT extent retrieval on Windows"

After this fix, `dump_mft_extents F` correctly shows all 28 extents, matching the C++ output exactly:

```
MFT extents (VCN, clusters, LCN):
 idx      VCN      clusters           LCN         byte_offset        byte_size
   0          0       3202        7949042   520948416512      209846272
   1       3202       3345        8378124   549068734464      219217920
   ...
  27      70496       2260       12474645   817538334720      148111360

Summary:
  extent_count      = 28
  total_clusters    = 72756
  total_bytes       = 4768137216
  approx_records    = 4656384
```

### 12.6 Raw Dump Comparison Results (Phase 2 - Current)

With extent retrieval fixed, we captured fresh raw dumps from both C++ and Rust on the same read-only F: drive:

**Capture commands:**
```powershell
# C++ dump (using production uffs.com)
uffs.com --dump-mft=F --output=f_mft_cpp.raw

# Rust dump (with extent fix)
uffs_mft.exe save --drive F --output f_mft_rust_fixed.raw --no-compress
```

**Rust output confirmed all 28 extents:**
```
⚠️  MFT is fragmented extents=28 sparse_extents=0 total_clusters=72756 total_records=4656384
📊 Read plan generated chunks=12 records_to_read=4656384 records_skipped=0
```

**Comparison results:**
```
=== Raw MFT Comparison ===
Header A: version=1, flags=0, record_size=1024, record_count=4656384
Header B: version=1, flags=0, record_size=1024, record_count=4656384

Total records:  4656384
Same records:   2307904  (49.6%)
Diff records:   2348480  (50.4%)
Total differing bytes: 1347964739
Fraction of differing bytes: 0.283

First 20 differing records (FRS, differing_bytes_in_record):
  FRS 1071680: 227 bytes differ
  FRS 1071681: 217 bytes differ
  FRS 1071682: 272 bytes differ
  ...
```

### 12.7 Analysis: Extent Reading Bug

**Critical observation:** The first differing record is **FRS 1,071,680**, which is exactly the first record of **extent 5**.

**Extent boundary calculation:**
- Records per cluster = 65,536 / 1,024 = 64
- Extents 0-4 total clusters = 3,202 + 3,345 + 1,865 + 4,683 + 3,650 = 16,745
- Extents 0-4 total records = 16,745 × 64 = **1,071,680**

This means:
- **Extents 0-4**: All 1,071,680 records match perfectly ✓
- **Extents 5-27**: All 2,348,480 records differ ✗

**Key insight:** The differences are **partial** (227-465 bytes per record), not complete zeros or garbage. This indicates Rust is reading **valid MFT data from wrong disk locations**, not failing to read at all.

**Root cause hypothesis:**

The extent retrieval is now correct (all 28 extents with correct VCN/LCN/cluster values). However, the **read path** that translates these extents into disk I/O is buggy. Specifically, the code in `generate_read_chunks()` or `read_chunk()` in `crates/uffs-mft/src/io.rs` is likely:

1. Miscalculating `disk_offset` from extent LCN for extents 5+, OR
2. Using wrong extent indexing when transitioning between extents, OR
3. Not correctly handling the VCN→LCN mapping during chunk generation

**Evidence supporting this:**
- Extent 5 has LCN=3,367,678 (byte_offset=220,704,145,408)
- Extent 4 has LCN=9,469,388 (byte_offset=620,585,811,968)
- The MFT "jumps backwards" on disk at extent 5 (LCN decreases)
- If the read code assumes monotonically increasing LCNs, it would read from wrong locations

### 12.8 Current Status and Next Steps

**Phase 1 (Extent Retrieval): COMPLETE ✓**
- Rust now correctly retrieves all 28 MFT extents
- Extent data matches the legacy baseline exactly

**Phase 2 (Extent Reading): IN PROGRESS**
- Bug identified: Read path produces wrong data starting at extent 5
- Location: `crates/uffs-mft/src/io.rs` - `generate_read_chunks()` and/or `read_chunk()`
- Next: Examine how `disk_offset` is calculated from extent LCN values

**Planned investigation:**
1. Review `generate_read_chunks()` logic for VCN→LCN→disk_offset translation
2. Add diagnostic logging to show which extent/LCN is used for each FRS range
3. Compare with C++ read logic to identify the discrepancy
4. Fix the bug and re-run comparison to achieve 100% match

## 13. Root Cause Analysis and Fix

### 13.1 Root Cause: `merge_adjacent_chunks()` Bug

After detailed analysis of the code, the root cause was identified in the `merge_adjacent_chunks()` function in `crates/uffs-mft/src/io.rs`.

**The Bug:**

The function was designed to merge adjacent read chunks to reduce I/O syscall overhead. However, it incorrectly merged chunks from **different MFT extents** when:
1. The FRS numbers were contiguous (e.g., extent 4 ends at FRS 1,071,679, extent 5 starts at FRS 1,071,680)
2. But the disk locations were **NOT** contiguous (extent 4 at LCN 9,469,388, extent 5 at LCN 3,367,678)

The problematic code:

```rust
let current_end_offset = current.disk_offset + current.record_count * u64::from(record_size);
let gap_bytes = next.disk_offset.saturating_sub(current_end_offset);  // BUG HERE!
let gap_records = gap_bytes / u64::from(record_size);
```

**Why `saturating_sub` caused the bug:**

When `next.disk_offset < current_end_offset` (i.e., the next extent is BEFORE the current extent on disk), `saturating_sub` returns **0** instead of a large negative number. This made the gap appear to be zero, causing the merge condition to pass:

```rust
if gap_records <= threshold && frs_gap <= threshold && merged_bytes <= MAX_CHUNK_BYTES {
    // Incorrectly merges chunks from different extents!
    current.record_count = new_record_count;
}
```

**The Result:**

When reading the merged chunk:
- The code would seek to `current.disk_offset` (correct for extent 4)
- But read `new_record_count` records (spanning both extent 4 AND extent 5)
- This read past the end of extent 4 into whatever data happened to be on disk
- That data was NOT the MFT records for extent 5 (which are at a completely different LCN)

This explains why:
- Extents 0-4 matched perfectly (no merging across extent boundaries in that range)
- Extents 5-27 all differed (merged chunks read wrong disk locations)
- The differences were partial (227-465 bytes per record) - valid MFT data, just from wrong locations

### 13.2 The Fix

The fix ensures chunks are only merged when they are **physically contiguous on disk**, not just when FRS numbers are contiguous:

```rust
// Check for physical contiguity: next chunk must start at or very close to
// where current chunk ends. We check BOTH directions to catch non-contiguous
// extents regardless of their relative disk positions.
let is_physically_contiguous = if next.disk_offset >= current_end_offset {
    // Normal case: next chunk is after current on disk
    let gap_bytes = next.disk_offset - current_end_offset;
    gap_bytes <= threshold * u64::from(record_size)
} else {
    // Next chunk is BEFORE current on disk - NOT contiguous!
    // This happens with fragmented MFTs where extents are scattered.
    false
};

// Only merge if BOTH physically contiguous AND FRS contiguous
if is_physically_contiguous && is_frs_contiguous && merged_bytes <= MAX_CHUNK_BYTES {
    // Safe to merge
}
```

**Key changes:**
1. Explicitly check if `next.disk_offset >= current_end_offset` before calculating gap
2. If next chunk is before current on disk, immediately mark as NOT contiguous
3. Require BOTH physical AND FRS contiguity for merging

### 13.3 Expected Impact

With this fix:
- Each MFT extent will be read from its correct LCN
- No cross-extent merging will occur for fragmented MFTs
- The raw dump should now match C++ bit-for-bit for all 4.6M records
- Path resolution should work correctly for all files on F:

### 13.4 Next Steps

1. **Rebuild binaries** with the fix on Windows
2. **Regenerate raw dumps** from both C++ and Rust on F:
3. **Compare dumps** to verify 100% match
4. **Re-run full comparison** of search results between C++ and Rust
5. **Validate path resolution** - no more `<dir:XXXXXX>` placeholders
