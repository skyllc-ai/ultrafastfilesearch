# CRUCIBLE Performance Baseline

This document records the approved performance baseline carried by the CRUCIBLE audit artifacts.
It summarizes the existing performance model and the validation commands that protect it.

## Performance model

- UFFS gains its primary speed advantage from direct NTFS MFT access instead of Windows file enumeration APIs.
- `uffs-polars` isolates Polars compilation and keeps the workspace on a single dataframe/schema boundary.
- `uffs-core` owns query-path hot spots such as pattern matching, extension filtering, and path resolution.
- `uffs-mft` supports multiple read modes (`auto`, `parallel`, `prefetch`, `streaming`) so the runtime can match SSD, HDD, and low-memory cases.
- The default fast path skips extension records for speed; `--full` remains the completeness-oriented option.

## Validation canon anchors

The approved validation canon for this baseline is:

1. `cargo build --release -p uffs-cli --bin uffs`
2. `cargo xwin check -p uffs-mft --lib --bin uffs_mft`
3. `cargo test -p uffs-mft --bin uffs_mft required_output_path`
4. `rust-script scripts/verify_parity.rs /Users/rnio/uffs_data D --regenerate`

## Current carried status

- Validation canon alignment is verified.
- Wave 1C parity artifact resolution is verified.
- The `required_output_path` regression check is still considered mandatory, but the current rerun is blocked by external disk pressure on the host (`No space left on device`, `os error 28`). This is carried forward as an environment blocker, not a performance or correctness regression.

## Cross-platform benchmark lane

When live Windows MFT validation is unavailable, the repository still retains a smaller hot-path benchmark lane for query behavior:

- `cargo bench -p uffs-core --bench query`

That lane is complementary to, not a replacement for, the Windows-specific canon above.
