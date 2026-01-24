# Changelog Healing - 2026-01-24

## Summary
Fixed borrow checker errors and warnings in the new `SlidingIocpInline` mode implementation.

## What Failed

### 1. Borrow Checker Errors (E0499, E0502)
**Location:** `crates/uffs-mft/src/io.rs:1610-1643`

**Error:**
```
error[E0499]: cannot borrow `*index` as mutable more than once at a time
error[E0502]: cannot borrow `index.links` as immutable because it is also borrowed as mutable
```

**Root Cause:**
The `parse_record_to_index()` function held a mutable reference to `record` (from `index.get_or_create(frs)`) while simultaneously trying to:
- Call `index.add_name()` (mutable borrow)
- Access `index.links.len()` (immutable borrow)
- Call `index.links.push()` (mutable borrow)

### 2. Type Mismatch (E0308)
**Location:** `crates/uffs-mft/src/io.rs:1645`

**Error:**
```
error[E0308]: mismatched types - expected `u16`, found `u8`
```

**Root Cause:**
`record.name_count` is `u16`, but the code cast to `u8`.

### 3. Warnings
- **Dead code:** `skip_begin` and `record_count` fields in `IoOp` struct were never read
- **Dropping reference:** `drop(record)` does nothing on a reference

## How Fixed

### 1. Borrow Checker Fix
Restructured the code to avoid overlapping borrows:
1. Pre-process all additional names BEFORE getting the record reference
2. Add names to buffer and push LinkInfo entries to `index.links` first
3. Then get the record reference and update it
4. After the record reference goes out of scope, chain the links together

### 2. Type Fix
Changed `as u8` to `as u16` for `name_count`.

### 3. Warning Fixes
- Removed unused `skip_begin` and `record_count` fields from `IoOp` struct
- Removed unnecessary `drop(record)` call (reference goes out of scope naturally)

## Files Changed
- `crates/uffs-mft/src/io.rs`

## Verification
- `cargo check --package uffs-mft` passes with no errors or warnings
- CI pipeline (`rust-script scripts/ci-pipeline.rs go -v`) passes
