# Changelog Healing - 2026-01-27 14:00

## CI Pipeline Run

**Command:** `rust-script scripts/ci-pipeline.rs go -v`

---

## Issue 1: Invalid field access on `UsnApplyStats`

**File:** `crates/uffs-mft/src/reader.rs`  
**Lines:** 1005, 1008

**Error:**
```
error[E0609]: no field `added` on type `UsnApplyStats`
error[E0609]: no field `renamed` on type `UsnApplyStats`
```

**Root Cause:**  
The `UsnApplyStats` struct has fields: `deleted`, `created`, `modified`, `skipped`.  
The logging code incorrectly references `stats.added` and `stats.renamed` which don't exist.

**Fix:**  
Change `stats.added` → `stats.created` and `stats.renamed` → `stats.modified` (or remove the renamed field since it's tracked as part of modified).

---

## Issue 2: Unnecessary path qualification warnings

**File:** `crates/uffs-mft/src/reader.rs`  
**Lines:** 1013, 1066

**Warning:**
```
warning: unnecessary qualification
    crate::platform::VolumeHandle::open(drive)
```

**Root Cause:**  
`VolumeHandle` is already imported or in scope, so the full path is unnecessary.

**Fix:**  
Remove `crate::platform::` prefix, use just `VolumeHandle::open(drive)`.

---

## Summary

| Issue | Type | Status |
|-------|------|--------|
| `stats.added` field doesn't exist | Error | Fixed |
| `stats.renamed` field doesn't exist | Error | Fixed |
| Unnecessary `crate::platform::` qualification | Warning | Fixed |

