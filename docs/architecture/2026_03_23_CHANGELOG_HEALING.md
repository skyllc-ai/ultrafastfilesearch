# Changelog Healing — 2026-03-23 Session

## Fixes Applied

### Regex End-Anchoring (Bug Fix)
- **Root cause**: Rust `regex::is_match()` does substring matching by default. Pattern `>.*\.(jpg|png|heic)` matched `icon.png.vir` because `.png` was found mid-string.
- **Fix**: Auto-append `$` to regex patterns in `compile_parsed_pattern()` if not already end-anchored.
- **Tests**: 8 new tests covering mid-filename rejection, ADS rejection, correct matches, explicit `$`, path prefix, digit patterns.
- **File**: `crates/uffs-core/src/index_search/pattern.rs`

### Regex Extension Index Optimization (New Feature)
- **What**: Extract file extensions from regex alternation groups like `>.*\.(jpg|png|heic)` to pre-filter via extension index before applying the full regex.
- **Result**: O(matches) instead of O(n) — potentially 60-350× fewer regex evaluations.
- **Function**: `extract_extensions_from_regex()` in `search/util.rs`
- **Integration**: `try_get_extension_indices()` now supports both glob and regex extension extraction.
- **Tests**: 18 new tests for extraction, rejection, normalization.

### --name-only CLI Flag (New Feature)
- **What**: Forces filename-only matching, disabling full-path substring matching for literal patterns. Matches C++ UFFS behavior.
- **Validation**: Errors if pattern contains path separators (`\` or `/`) and is not a regex.
- **Threading**: `name_only` passed through `SearchConfig` → `SingleFileStreamConfig` → `IndexStreamConfig` → overrides `is_path_pattern` at all consumption points.
- **Tests**: 5 CLI integration tests (rejects backslash, rejects fwdslash, accepts literal, accepts glob, accepts regex).
- **Files**: `args.rs`, `main.rs`, `mod.rs`, `dispatch.rs`, `single_file.rs`, `live.rs`

### MMMmmm Footer Fix
- **Root cause**: "MMMmmm that was FAST" warning triggered for regex/glob patterns with few results.
- **Fix**: Only trigger for full-scan patterns (`*`, `**`, `**/*`, empty).
- **Tests**: Split into 2 tests — full-scan shows warning, regex does not.

### cpp_pattern Double-Prefix Fix
- **Root cause**: `>F:>C:\\Users\\` — regex patterns were getting `>DRIVE:` prepended twice.
- **Fix**: Skip prefix for patterns already starting with `>`.
- **Files**: `single_file.rs`, `live.rs`

### Script Improvements
- **parity_check_live.rs**: `--pattern`, retry logic, error code decoding, `Skipped` variant
- **parity_check.rs**: `--pattern`, `--format custom`
- **verify_parity.rs**: `--pattern` parameter
- **benchmark.ps1**: Pattern quoting fix for `|` characters in regex

## Validation
- `cargo check --workspace` ✅
- `cargo test --package uffs-cli` ✅ (all name-only + regex + footer tests pass)
- `cargo test --package uffs-core` ✅ (all regex anchoring tests pass)
- Next: `just ship -v`
