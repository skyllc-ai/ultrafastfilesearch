# Intent: Fix F-Drive Parity Mismatch

## Goal
Achieve 100% parity between C++ (`uffs.com`) and Rust (`uffs.exe`) output for **F-drive** MFT scans. D-drive and S-drive already pass parity verification.

## Current Status

### ✅ PASSING: D-drive and S-drive
```
Golden baseline (sorted): 028356d4c9298ca8ef790229f4d4270ea29827ad155051e01181181fa34a531e
Rust output (sorted):    028356d4c9298ca8ef790229f4d4270ea29827ad155051e01181181fa34a531e
RESULT: FULL OUTPUT MATCH AFTER LINE-SORT NORMALIZATION
```

### ❌ FAILING: F-drive
```
Golden baseline (sorted): 8361188993bd0fded64465a4cbf5eed0443f1bcb2fc45c06b655bbbf63d8ca32
Rust output (sorted):    6028e26ea37d4d1f5b4cd59b43dc5bab595613d1fd6e795ee3fba6ddc82076ab
Lines that differ: 2221315 (out of 2221321 total)
```

## Evidence

### 1. Timestamp Differences (1 hour offset)
```
Line 5:
  BASELINE: "F:\!Install.txt",...,2014-10-23 17:42:58,...
  RUST:     "F:\!Install.txt",...,2014-10-23 16:42:58,...
```
Rust timestamps are **1 hour behind** baseline consistently.

### 2. Tree Metrics Difference (root folder size)
```
Line 6:
  BASELINE: "F:\","","F:\",1951818953632,...
  RUST:     "F:\","","F:\",1951820068930,...
```
Difference: ~1.1MB in cumulative folder size.

### 3. Capture Metadata (trial_run.md)
```
**Started:** 2026-03-11T22:18:32.7876612-07:00
```
F-drive was captured on **March 11, 2026** with **PDT (-7)** timezone.

## Verification Command
```bash
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data F --regenerate
```

## Data Files
- MFT binary: `/Users/rnio/uffs_data/drive_f/F_mft.bin`
- C++ baseline: `/Users/rnio/uffs_data/drive_f/cpp_f.txt`
- Rust output: `/Users/rnio/uffs_data/drive_f/verify_rust_f.txt`
- Capture log: `/Users/rnio/uffs_data/drive_f/trial_run.md`

## Key Codebase Locations
- `crates/uffs-mft/` — MFT parsing, tree metrics calculation
- `crates/uffs-core/` — Path resolution, query engine
- `crates/uffs-cli/` — CLI scan command, CSV output
- `scripts/verify_parity.rs` — Parity verification tool

## Constraints
1. **No suppression hacks** — Don't hide problems with `#[allow(...)]`
2. **Preserve D/S parity** — Any fix must not break working drives
3. **Match C++ behavior exactly** — The C++ output is the golden baseline
4. **Timestamps in local time** — MFT stores UTC, output converts to local

## Hypotheses to Investigate

### H1: Timezone offset detection
The auto-detect reads `trial_run.md` for `-07:00` but something may be wrong in how it's applied to the uffs scan command.

### H2: Tree metrics accumulation
The ~1.1MB difference in root folder size suggests a subtle difference in how sizes are accumulated. Check:
- Hardlink handling
- ADS (Alternate Data Streams) size inclusion
- Sparse/compressed file sizes

### H3: MFT parsing edge case
F-drive may have records that D/S don't have (different Windows version, different apps installed).

## Success Criteria
```
rust-script scripts/verify_parity.rs /Users/rnio/uffs_data F --regenerate

RESULT: FULL OUTPUT MATCH AFTER LINE-SORT NORMALIZATION
  Exact line order differs (different traversal order), but content matches.
```

