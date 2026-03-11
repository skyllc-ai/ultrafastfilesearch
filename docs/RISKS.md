# CRUCIBLE Risk Register

This document captures the audit-era risks that remain relevant after the Wave 1 baseline validation.

## Verified and closed for this baseline

- Repository-wide cleanup of retired legacy naming is verified complete.
- Validation canon alignment is verified.
- Wave 1C parity artifact resolution is verified.

## Active risks

| Risk | Why it matters | Current disposition |
|------|----------------|---------------------|
| Windows-only live MFT access | Full end-to-end MFT validation requires Windows with Administrator privileges. | Accepted platform constraint; document and test around it. |
| Host resource pressure | Large workspace builds and some regression checks depend on adequate disk and memory headroom. | Active environment constraint. |
| Structural concentration in `uffs-mft` | The audit snapshot still identifies a large central crate, duplicate variants, and oversized files. | Known follow-up area; unchanged by Wave 1D. |
| CI tier split | Always-on CI does not exercise every heavy build and parity path on every change. | Mitigated by the validation canon and wave-level verification gates. |

## Carried external blocker

- `cargo test -p uffs-mft --bin uffs_mft required_output_path` is currently blocked by host disk pressure with `No space left on device` (`os error 28`). Carry this forward until the environment has capacity again.
- Treat the blocker as external to the product code unless a rerun in a healthy environment shows a real regression.
