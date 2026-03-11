# CRUCIBLE Architecture Baseline

This document captures the approved CRUCIBLE architecture baseline for Wave 1D.
It is a documentation snapshot only and does not change runtime behavior.

## Validated baseline

- Repository-wide cleanup of retired legacy naming is verified complete.
- Validation canon alignment is verified.
- Wave 1C parity artifact resolution is verified.
- `cargo test -p uffs-mft --bin uffs_mft required_output_path` remains part of the validation canon, but the current rerun is externally blocked by host disk pressure (`No space left on device`, `os error 28`) and is not treated as a product regression.

## Workspace shape

| Layer | Crates | Role |
|------|--------|------|
| Data facade | `uffs-polars` | Single Polars entry point and column/schema home |
| NTFS/MFT engine | `uffs-mft` | Windows-only live MFT access, parsing, index building, `uffs_mft` binary |
| Query layer | `uffs-core` | Path resolution, pattern matching, tree metrics, search helpers |
| Frontends | `uffs-cli`, `uffs-tui`, `uffs-gui` | User-facing workflows |
| Diagnostics | `uffs-diag` | Diagnostic and analysis tooling |

The current dependency flow is intentionally layered:

`uffs-polars <- uffs-mft <- uffs-core <- frontends`

## Architectural anchors

- Live NTFS reads remain Windows-only and require elevated access.
- Non-Windows hosts remain valid for development, cross-compilation, offline index work, and query-path validation.
- `scripts/verify_parity.rs` remains the canonical parity gate referenced by the validation canon.
- The repository still optimizes around a large `uffs-mft` core, with the audit identifying duplication and oversized files as structural follow-up items rather than Wave 1D work.

## Related canonical docs

- [`docs/README.md`](README.md)
- [`docs/architecture/README.md`](architecture/README.md)
- [`docs/PERFORMANCE.md`](PERFORMANCE.md)
- [`docs/RISKS.md`](RISKS.md)
