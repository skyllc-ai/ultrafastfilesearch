# CHANGELOG_HEALING_CRUCIBLE

## Scope

This file now serves as the truthful CRUCIBLE evidence ledger.

- Wave 1 originally created the CRUCIBLE artifact set.
- This backfill refresh records what can be proven from the existing Wave 1-4
  history.
- It is **not** a pristine pre-execution baseline capture.

## Proven historical evidence

### Wave 1

- Commit `beed91d26` — `anvil: Wave 1 baseline cleanup, parity path, and audit artifacts`
- Created `docs/ARCHITECTURE.md`, `docs/PERFORMANCE.md`, `docs/RISKS.md`, and
  `LOG/CHANGELOG_HEALING_CRUCIBLE.md`
- Updated `scripts/verify_parity.rs` as part of the parity-path work

### Wave 2

- Commit `ffafcd3cc` — `anvil: Wave 2 bounded orchestration concurrency`
- Changed `crates/uffs-cli/src/commands/search.rs`
- Changed `crates/uffs-mft/src/reader/multi_drive.rs`

### Wave 3

- Commit `fc53707bb` — `anvil: Wave 3 timeout and shutdown hardening`
- Hardened CLI/TUI/runtime error and shutdown paths across current production
  entrypoints and supporting MFT modules

### Wave 4

- Commit `312ff5fe6` — `anvil: Wave 4 observability and docs hardening`
- Refreshed `docs/ARCHITECTURE.md` and `docs/RISKS.md`
- Added the observability-layer code changes that those docs now describe

## Carried validation status

- Validation canon alignment remains documented in `docs/PERFORMANCE.md`.
- Wave 1 parity artifact resolution remains carried as complete.
- The `required_output_path` regression command remains part of the canon but is
  still carried as an environment blocker when host disk pressure triggers
  `No space left on device` (`os error 28`).

## Backfill added in this task

- Created `docs/CRUCIBLE_AUDIT_REPORT.md` to package the surviving Wave 1-4
  evidence into a truthful audit report.
- Added the audit report to the `docs/README.md` CRUCIBLE artifact index.

## Non-claims

- This repository does not contain a trustworthy in-repo "Run 0" CRUCIBLE
  baseline with full CI/parity command output captured before Waves 1-4.
- `CRUCIBLE_FOLLOWUP.md` should be treated as stale execution guidance, not as
  the authoritative record of what actually happened on this branch.

## Notes

- No source code or dependency changes were made in this task.
- No commit or push was performed.
