# uffs-mft Lint Cleanup Changelog

**Date:** 2026-03-08
**Crate:** uffs-mft
**Scope:** Remove all 246 blanket `#[allow(...)]` suppressions

## Summary

Converted all 246 `#[allow(...)]` attributes across 14 source files to narrow `#[expect(lint, reason = "...")]` with meaningful justifications. Multi-lint `#[allow(a, b, c)]` blocks were split into separate `#[expect]` lines. No code logic was changed — only attribute annotations were modified.

## Changes by File

### lib.rs (1 suppression)

- `module_name_repetitions` → `#[expect]` with reason "re-exports use crate-prefixed names for clarity"

### cache.rs (1 suppression)

- `large_enum_variant` → `#[expect]` with reason for variant size difference

### usn.rs (2 suppressions)

- `struct_excessive_bools` on `FileChange` → `#[expect]` with reason "independent change flags from USN journal records"
- `unsafe_code` on `windows_impl` module → `#[expect]` with reason "FFI: Windows API calls"

### cpp_tree.rs (2 suppressions)

- Crate-root `indexing_slicing` → `#![expect]` with reason "C++ port uses direct indexing for performance parity"
- `single_call_fn` → `#[expect]` with reason "extracted for clarity"

### raw.rs (~14 suppressions)

- `cast_possible_truncation`, `shadow_reuse`, `unused_mut`, `dead_code`, `single_call_fn`, `indexing_slicing` across functions and test modules

### ntfs.rs (~20 suppressions)

- `cast_sign_loss`, `indexing_slicing`, `unsafe_code`, `cast_possible_truncation`, `missing_const_for_fn`, `similar_names`, `cast_possible_wrap`, `single_call_fn`, `struct_excessive_bools`, `missing_assert_message`

### parse.rs (~23 suppressions)

- 7-lint crate-root `#![allow(...)]` block split into 7 individual `#![expect]` lines
- Item-level: `struct_excessive_bools`, `cast_possible_truncation`, `unsafe_code`, `single_call_fn`, `missing_asserts_for_indexing`, `cognitive_complexity`, `too_many_lines`, `indexing_slicing`

### cpp_types.rs (~30 suppressions)

- Crate-root `indexing_slicing` → `#![expect]`
- 6 helper cast functions with `cast_possible_truncation`/`cast_sign_loss`
- ~16 method-level allows converted
- 5 test modules (size_tests, usa_fixup_tests, attribute_parsing_tests, stream_parsing_tests, extension_record_tests) — each module-level `#[allow(...)]` block converted to individual `#[expect]` lines

### index.rs (~48 suppressions)

- Test module with 14 lints converted to individual `#[expect]` lines
- Multi-line blocks for `compute_tree_metrics_impl` (10 lints), `display_stats` (5 lints), `from_parsed_records` (4 lints), `deserialize` (3 lints)
- Item-level: `cast_possible_truncation`, `indexing_slicing`, `single_call_fn`, `too_many_lines`, `cognitive_complexity`, `unsafe_code`

### main.rs (~18 suppressions → 34 expect lines)

- Multi-lint allows split into separate `#[expect]` lines
- `too_many_lines`, `single_call_fn`, `unsafe_code`, `print_stderr`, `print_stdout`, `dead_code`, `unused_async`

### reader.rs (~38 suppressions → 51 expect lines)

- `unused_async` (19 instances): "async for API parity with windows"
- `unsafe_code` (8 instances): "FFI: CloseHandle on valid overlapped handle"
- `too_many_lines` (3 instances): sequential I/O pipeline reasons
- `dead_code` (3 instances): kept as fallback/reference
- `cast_precision_loss`, `float_arithmetic`, `cast_possible_wrap`, `struct_excessive_bools`, `too_many_arguments`, `single_call_fn`, `cast_possible_truncation`

### io.rs (~28 suppressions → 36 expect lines)

- `unsafe_code` (20+ instances): FFI reasons for ReadFile, SetFilePointerEx, IOCP, CloseHandle, CreateIoCompletionPort
- `too_many_lines` (4 instances): monolithic parsers, parallel I/O orchestration
- `cast_possible_truncation` (4 instances): "NTFS field sizes are bounded by u16/u32 record layout"
- `unused_imports` (2 instances): "used in inline parsing mode"

### platform.rs (~19 suppressions)

- All `unsafe_code` for Windows FFI: CreateFileW, DeviceIoControl, CloseHandle, GetLogicalDrives, GetVolumeInformationW, OpenProcessToken, GetTokenInformation
- Thread safety impls: "windows file handles are thread-safe kernel objects"

### cpp_io_pipeline.rs (1 suppression → 2 expect lines)

- `unsafe_code, too_many_lines` split into:
  - `#[expect(unsafe_code, reason = "FFI: windows IOCP API")]`
  - `#[expect(clippy::too_many_lines, reason = "IOCP sliding-window loop is inherently complex")]`

## Validation

- **Zero `#[allow(` remaining** in `crates/uffs-mft/src/` (verified with grep)
- **`rustfmt --check`** passes on all 14 files
- **`cargo clippy`** blocked by pre-existing polars-arrow dependency build failure (toolchain compatibility issue with nightly-2025-12-15, unrelated to this change)

## Files Modified

14 files changed, 1270 insertions(+), 401 deletions(-)
