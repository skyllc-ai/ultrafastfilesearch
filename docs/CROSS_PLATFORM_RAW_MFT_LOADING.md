# Cross-Platform Raw MFT Loading

## Current Status

**Problem**: The `uffs_mft load` command currently only works on Windows, even though it's loading from a saved file (not accessing a live volume).

**Root Cause**: The entire `crates/uffs-mft/src/io.rs` module is gated with `#[cfg(windows)]` because it contains both:
1. **Windows-specific I/O code** (direct volume access, IOCP, etc.)
2. **Cross-platform parsing code** (`apply_fixup()`, `parse_record()`, etc.)

The parsing functions don't actually use any Windows APIs - they just parse NTFS byte structures. But because they're in the same module as the Windows-specific I/O code, they're not available on macOS/Linux.

## Impact

This prevents the **brilliant debugging workflow** the user suggested:
1. Save raw MFT on Windows → `uffs_mft save G: G_mft.bin`
2. Copy to macOS → `scp G_mft.bin mac:/path/to/repo/docs/trial_runs/`
3. Debug on macOS → `cargo run --bin uffs_mft -- load G_mft.bin --build-index`

Currently step 3 fails with:
```
Error: uffs_mft only works on Windows.
```

## Workaround (Current)

For now, the user must run the load command **on Windows**:

```powershell
# On Windows
~\bin\uffs_mft.exe load G_mft.bin --build-index
```

This will show:
- Index build time
- Tree metrics sample (first 10 directories)
- Root directory tree metrics
- Debug logging from `compute_tree_metrics()`

This should help diagnose why tree metrics are showing zeros.

## Long-Term Solution

**Refactor parsing code into cross-platform module:**

1. Create new module `crates/uffs-mft/src/parse.rs` (NOT gated with `#[cfg(windows)]`)
2. Move cross-platform parsing functions:
   - `apply_fixup()`
   - `parse_record()`
   - `parse_record_full()`
   - `parse_record_zero_alloc()`
   - `ParsedRecord` struct
   - `ParseResult` enum
   - NTFS structure definitions (if not already in `ntfs.rs`)
3. Keep Windows-specific I/O code in `io.rs`:
   - `read_mft_record()`
   - `read_mft_chunk()`
   - IOCP-related code
   - Volume handle management
4. Update imports throughout the codebase

**Benefits:**
- ✅ Enable cross-platform raw MFT loading
- ✅ Enable macOS/Linux debugging with saved MFT files
- ✅ Cleaner separation of concerns
- ✅ Better testability (can test parsing without Windows)

**Effort**: ~2-3 hours of careful refactoring

## Next Steps

1. **Immediate**: Copy binaries to Windows and run load command there
2. **Short-term**: Debug tree metrics issue using Windows-based workflow
3. **Long-term**: Refactor parsing code to enable cross-platform support

## Files Affected

- `crates/uffs-mft/src/io.rs` - Currently `#[cfg(windows)]`, contains both I/O and parsing
- `crates/uffs-mft/src/reader.rs` - Uses parsing functions
- `crates/uffs-mft/src/index.rs` - Uses `ParsedRecord`
- `crates/uffs-mft/src/main.rs` - `cmd_load()` currently `#[cfg(windows)]`

## Related Issues

- Tree metrics showing zeros (current debugging focus)
- Cross-platform testing limitations
- Inability to use macOS tooling for debugging

