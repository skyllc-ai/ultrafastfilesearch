# Healing log (draft) - MFT parent placeholders / missing parents

## Context

- Symptom: On F: drive, Rust UFFS CLI produced many `<dir:FRS>` / `<unknown:FRS>` placeholder parents and under-reported file counts compared to C++.
- Investigation showed thousands of `parent_frs` values in `f_mft.parquet` without corresponding directory rows, especially in high-FRS ranges.
- Cross-checks against `f_mft_reference.csv` from the vendored `mft-reader-rs` showed that many of these missing parents were in-use, base directory records in the reference output.
- We previously fixed a major `$MFT` extent bug and introduced placeholder parent rows in the Rust reader to match C++ `at()` behavior.

## Changes in this round

1. **Diag: header + full parse inspection for specific FRS**

   - Added `crates/uffs-diag/src/bin/inspect_mft_record_flow.rs` to:
     - Load `f_mft.raw` via `uffs_mft::raw::load_raw_mft`.
     - For selected FRS, dump the local `FileRecordSegmentHeader` fields (magic, USA, flags, base reference).
     - On Windows, call into the real `apply_fixup` + `parse_record_full` pipeline to see exactly how the core reader treats the record.
   - Added `crates/uffs-diag/src/bin/uffs_mft_helpers_windows.rs` (Windows-only) to host the helper that runs `apply_fixup` and `parse_record_full` on a single FRS.

2. **Diag: magic distribution scanner**

   - Implemented `crates/uffs-diag/src/bin/scan_mft_magic.rs` to scan all records in `f_mft.raw` and classify the NTFS magic (`FILE`, `RCRD`, `INDX`, `ZERO`, `OTHER`) by buckets of FRS.
   - This showed that in some earlier snapshots, high FRS ranges had few `FILE` records and many `RCRD`/`ZERO` entries, which correlated with missing parents.

3. **Parent placeholder creation in the reader**

   - In `crates/uffs-mft/src/io.rs` we now have `create_placeholder_record(frs: u64) -> ParsedRecord` and `add_missing_parent_placeholders_to_vec` / `ParsedColumns::add_missing_parent_placeholders`.
   - These functions:
     - Detect any `parent_frs` referenced by parsed records that do not have a corresponding row.
     - Synthesize a minimal directory record with name `<dir:FRS>`, parent FRS defaulting to 5 (root) and `is_directory = true`.
     - Repeat until closure so that chains of missing parents also get placeholders.
   - This matches C++ `at()` semantics and dramatically reduces `<unknown:FRS>` failures in `FastPathResolver`.

4. **Path resolver understanding**

   - Re‑reviewed `crates/uffs-core/src/path_resolver.rs`:
     - `FastPathResolver::build` constructs a Vec-backed FRS→(parent, name) map from the full MFT DataFrame.
     - When a parent FRS is missing entirely, `format_partial_path` emits `<unknown:FRS>` with a partial path suffix.
   - With parent placeholders present in the DataFrame, these `<unknown:FRS>` cases should become rare and traceable to genuinely unrecoverable parents.

## Validation (so far)

- `cargo check -p uffs-diag --bin inspect_mft_record_flow` passes.
- `cargo run -p uffs-diag --bin inspect_mft_record_flow -- docs/trial_runs/UltraFastFileSearch/f_mft.raw <frs...>`:
  - Confirms header sanity for selected FRS and, on Windows, shows `parse_record_full` outcomes (Base/Extension/Skip).
- `cargo run -p uffs-diag --bin scan_mft_magic -- docs/trial_runs/UltraFastFileSearch/f_mft.raw [bucket]`:
  - Confirms `FILE` magic distribution and highlights problematic FRS buckets.
- `cargo run -p uffs-diag --release --bin cross_check_mft_reference -- docs/trial_runs/UltraFastFileSearch/f_mft_reference.csv docs/trial_runs/UltraFastFileSearch/f_mft.parquet`:
  - For joined FRS, `IsDirectory` (CSV) and `is_directory` (Parquet) agree 100%.
  - There remain reference-only parents with many children in Parquet; placeholders in Rust are used to keep paths resolvable while we continue investigating raw header / extent causes for those gaps.

## Next steps

- On Windows, run `inspect_mft_record_flow` for high-impact parent FRS (e.g., 2640657, 2631176, 2628892, 2628924, 2627024) on a freshly captured `f_mft.raw` that matches the CSV snapshot.
- If `apply_fixup` + `parse_record_full` still drop any in-use base directories that the reference reader keeps, adjust `parse_record_full` / merger semantics to accept them (with tests).
- Re‑run `cross_check_mft_reference` and `analyze_mft_parents` with synchronized artifacts to verify that missing parent counts are bounded and explainable.
- Once raw + parse semantics are proven, tighten path resolver behavior and document remaining `<dir:FRS>` / `<unknown:FRS>` cases as genuine on-disk anomalies rather than reader bugs.

