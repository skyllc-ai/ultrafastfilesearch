# Changelog Healing - 2026-01-18 07:00

## Issue: `clippy::float_arithmetic` error in `MftStats::slack_percentage()`

### What Failed

```
error: floating-point arithmetic detected
   --> crates/uffs-mft/src/reader.rs:134:13
    |
134 |             (self.slack_space() as f64 / self.total_allocated_size as f64) * 100.0
    |             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: `-D clippy::float-arithmetic` implied by `-D warnings`
```

### Root Cause Analysis

The `slack_percentage()` function returns a human-readable percentage (e.g., 45.67%) which
inherently requires floating-point representation. The project has `float_arithmetic = "warn"`
in Cargo.toml, but CI runs with `-D warnings` which promotes this to an error.

### Why Float Arithmetic is Necessary Here

1. **Percentages are fractional values**: 45.67% cannot be represented as an integer
2. **Display/presentation function**: This is not core business logic, it's for human consumption
3. **Established pattern**: The codebase already uses targeted `#[allow(...)]` on similar functions:
   - `MftProgress::percentage()` (line 302-306)
   - `MftProgress::speed_mbps()` (line 310-318)
   - Various functions in `uffs-cli/src/commands.rs`

### Fix Applied

Added targeted `#[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]` to the
`slack_percentage()` function with a clear documentation comment explaining why float
arithmetic is unavoidable for this display function.

This follows the established pattern in the codebase for presentation-layer functions
that compute human-readable values (percentages, speeds, sizes in MB/GB).

### Verification

```bash
cargo clippy -p uffs-mft  # Passes with no warnings
cargo test -p uffs-mft    # All 14 tests pass
```

### Files Modified

- `crates/uffs-mft/src/reader.rs`: Updated `slack_percentage()` with proper allow attribute

