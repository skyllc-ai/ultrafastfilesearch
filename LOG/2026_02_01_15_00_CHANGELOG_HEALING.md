# Changelog Healing - 2026-02-01 15:00

## Issue: Timestamp 1-Hour Offset Between C++ and Rust Output

### Problem
When comparing C++ and Rust offline output, timestamps showed a consistent +1 hour difference:

| Field | C++ | Rust | Difference |
|-------|-----|------|------------|
| Created | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |
| Modified | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |
| Accessed | 2024-05-20 **19:49:45** | 2024-05-20 **20:49:45** | +1 hour |

### Root Cause Analysis

**C++ behavior** (`time_utils.hpp` lines 30-66):
1. `get_time_zone_bias()` calls `GetSystemTimeAsFileTime()` to get current UTC time
2. Calls `FileTimeToLocalFileTime()` to convert to local time
3. Returns `ft_local - ft` (the current timezone offset in 100ns intervals)
4. This offset is calculated **ONCE** and applied to **ALL** timestamps

**Key insight:** Windows' `FileTimeToLocalFileTime()` uses the **CURRENT** DST status, not the historical DST status for the timestamp's date. So if you're in PST (winter) and convert a timestamp from May (when PDT was active), Windows still applies the PST offset.

**Rust behavior** (`output.rs` lines 584-588, before fix):
1. Uses `chrono::DateTime::from_timestamp()` to create UTC datetime
2. Uses `Local.from_utc_datetime()` which applies **HISTORICAL** DST rules
3. So a May timestamp gets PDT offset, a January timestamp gets PST offset

This caused a 1-hour difference for timestamps from dates when DST status differed from current.

### Fix Applied

Modified `crates/uffs-core/src/output.rs`:

1. Added `timezone_offset_secs: i32` field to `OutputConfig` struct
2. In `Default::default()`, compute the current timezone offset once using `chrono::Local::now().offset().local_minus_utc()`
3. In `format_value()`, use `chrono::FixedOffset::east_opt(self.timezone_offset_secs)` instead of `Local.from_utc_datetime()`

This matches C++ behavior: the same timezone offset is applied to ALL timestamps, regardless of the timestamp's date.

### Verification

After fix, timestamps match exactly:
- **C++:** `2024-05-20 19:49:45`
- **Rust:** `2024-05-20 19:49:45` ✅

Full comparison shows 100% path parity with 7,058,030 common paths.

### Files Changed
- `crates/uffs-core/src/output.rs` - Added fixed timezone offset handling

### Tests
- All 145 uffs-core tests pass
- Offline comparison shows timestamp parity

