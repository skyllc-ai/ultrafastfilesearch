# SecurityDescriptor Stream Counting Fix

**Date:** 2026-02-02  
**Status:** ✅ RESOLVED  
**Affected Component:** `crates/uffs-mft/src/parse.rs`

## Executive Summary

Fixed a parity issue between C++ and Rust MFT parsing where the root directory showed a 1-descendant and 104-byte discrepancy. The root cause was that `$SECURITY_DESCRIPTOR` attributes (type code 0x50) were being skipped in Rust but counted as streams in C++.

---

## The Issue

### Symptoms

When comparing C++ and Rust OFFLINE scan outputs for the G: drive:

| Field | C++ Root `G:\` | Rust Root `G:\` | Difference |
|-------|----------------|-----------------|------------|
| Size | 609,893,968 | 609,893,864 | **-104 bytes** |
| Descendants | 15,106 | 15,105 | **-1** |

All 15,062+ subdirectory paths matched **perfectly** - only the root was off.

### Investigation Process

1. **Verified all subdirectories match** - Used `compare_scan_parity` tool to confirm 100% match on all non-root paths
2. **Examined raw MFT bytes** - Used `dump_mft_records` to inspect FRS 5 (root directory)
3. **Found the smoking gun** - Root directory has a `$SECURITY_DESCRIPTOR` attribute at offset 0xE0:
   - Type code: `0x50` (SecurityDescriptor)
   - Length: `0x68` = **104 bytes** (exactly matching the size difference!)

### Root Cause

In the legacy implementation (`ntfs_index.hpp` lines 588-600):

```cpp
// case ntfs::AttributeTypeCode::AttributeSecurityDescriptor:  // <-- COMMENTED OUT!
case ntfs::AttributeTypeCode::AttributePropertySet:
...
default:
{
    // Creates stream for any unhandled attribute type
```

The `AttributeSecurityDescriptor` case is **commented out**, so it falls through to the `default:` case and IS counted as a stream.

In the Rust implementation (`parse.rs` line 1114):

```rust
// Skip known non-stream attributes silently
Some(
    AttributeType::StandardInformation
    | AttributeType::FileName
    | AttributeType::AttributeList
    | AttributeType::SecurityDescriptor,  // <-- BUG: Should NOT be skipped!
) => {}
```

`SecurityDescriptor` was explicitly in the skip list and NOT counted as a stream.

---

## The Fix

### Changes Made to `crates/uffs-mft/src/parse.rs`

#### 1. First Parsing Function (cpp_port algorithm)

**Added `SecurityDescriptor` to stream-creating list** (lines 1017-1026):
```rust
Some(
    AttributeType::ObjectId
    | AttributeType::VolumeName
    | AttributeType::VolumeInformation
    | AttributeType::PropertySet
    | AttributeType::Ea
    | AttributeType::EaInformation
    | AttributeType::LoggedUtilityStream
    | AttributeType::SecurityDescriptor,  // <-- ADDED
) => {
```

**Added synthetic name for unnamed SecurityDescriptor** (lines 1082-1102):
```rust
Some(AttributeType::SecurityDescriptor) => {
    String::from("$SECURITY_DESCRIPTOR")
}
```

**Removed `SecurityDescriptor` from skip list** (lines 1113-1120):
```rust
// Skip known non-stream attributes silently
// Note: SecurityDescriptor (0x50) IS counted as a stream in C++ via the default: case
Some(
    AttributeType::StandardInformation
    | AttributeType::FileName
    | AttributeType::AttributeList,
    // SecurityDescriptor REMOVED - it's now counted as a stream
) => {}
```

#### 2. Second Parsing Function (fast path)

Applied identical changes:
- Added `SecurityDescriptor` to stream-creating list (lines 1595-1607)
- Added synthetic name `$SECURITY_DESCRIPTOR` (lines 1659-1679)

---

## Validation

### Test Command

```bash
# Generate new Rust offline scan
cargo run --release -p uffs-cli -- "G:*" \
    --mft-file docs/trial_runs/g_disk/G_mft.bin \
    --parse-algo cpp_port --tree-algo cpp \
    --out /tmp/rust_offline_g_fixed.txt

# Compare with legacy baseline
cargo run --release -p uffs-diag --bin compare_scan_parity -- \
    docs/trial_runs/g_disk/cpp_g.txt \
    /tmp/rust_offline_g_fixed.txt -v
```

### Results

```
📈 FIELD-BY-FIELD COMPARISON
               Field     Compared      Matches       Rate     Max Diff
----------------------------------------------------------------------
      allocated_size       15,063       15,063  100.0000%            -
         descendants       15,063       15,063  100.0000%            -
                size       15,063       15,063  100.0000%            -
```

**Root directory now matches exactly:**

| Field | C++ | Rust (Fixed) | Status |
|-------|-----|--------------|--------|
| Size | 609,893,968 | 609,893,968 | ✅ |
| Descendants | 15,106 | 15,106 | ✅ |

### CI Pipeline

Full CI pipeline passed with all tests green:
```bash
rust-script scripts/ci/ci-pipeline.rs go -v
# ✅ All tests pass
# ✅ Build successful
# ✅ Committed and pushed as v0.2.173
```

---

## Technical Background

### Two-Channel Model in C++

The legacy implementation uses two separate channels for tree metrics:

1. **Channel A (propagation)**: Values returned by recursion and accumulated into parents - includes ALL streams
2. **Channel B (printed)**: Values stored into the record's directory stream and printed

For correct tree metrics, `total_stream_count` must include ALL streams, including internal Windows streams like `$SECURITY_DESCRIPTOR`.

### Attribute Type 0x50

`$SECURITY_DESCRIPTOR` stores the security descriptor (ACLs, ownership info) for a file or directory. It's typically a resident attribute with size around 100-200 bytes. Every file/directory has one, but it's usually only visible in the root directory's tree metrics because subdirectories' security descriptors are already accounted for in their own stream counts.

---

## Files Modified

- `crates/uffs-mft/src/parse.rs` - MFT parsing logic (both parsing functions)

## Related Documents

- `docs/architecture/Investigation/MFT_tree_metrics_parity_deep_dive.md` - Original investigation
- `docs/trial_runs/g_disk/PARITY_ANALYSIS_2026_02_02.md` - Pre-fix parity analysis
- `docs/architecture/C++_resources/UltraFastFileSearch-code/src/index/ntfs_index.hpp` - legacy baseline

