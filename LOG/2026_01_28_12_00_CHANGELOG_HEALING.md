# Changelog Healing - 2026-01-28 12:00

## Issue: Nested Tokio Runtime Panic

### Symptom
```
thread 'main' panicked at tokio-1.49.0/src/runtime/scheduler/multi_thread/mod.rs:88:9:
Cannot start a runtime from within a runtime. This happens because a function (like `block_on`) 
attempted to block the current thread while the thread is being used to drive asynchronous tasks.
```

### Investigation

Added debug tracing to pinpoint the exact location of the panic:

1. **`read_all_index`** - Wrapped in `spawn_blocking`, completes successfully
2. **`execute_index_query`** - Added debug lines before/after `query.collect()` and `results_to_dataframe`
3. **`results_to_dataframe`** - Added debug lines before/after:
   - `DataFrame::new_infer_height`
   - `apply_directory_treesize`
   - `add_path_only_column`

### Debug Output (from user test)
```
[DEBUG] read_all_index: ENTER volume=F
[DEBUG] read_all_index: INSIDE spawn_blocking volume=F
FRS 5: first_stream.size=0, total_size=0, stream_count=1
[DEBUG] read_all_index: read_mft_index_internal done
[DEBUG] read_all_index: EXIT volume=F
<PANIC HERE>
```

This shows the panic occurs AFTER `read_all_index` returns, somewhere in the query/dataframe processing.

### Files Modified
- `crates/uffs-cli/src/commands.rs` - Added targeted debug eprintln statements
- `crates/uffs-mft/src/cache.rs` - Added debug tracing in `load_or_build_dataframe_cached`
- `crates/uffs-mft/src/reader.rs` - Added debug tracing in `read_all_index`

### Approach
Using targeted `#[allow(clippy::print_stderr)]` on specific functions rather than blanket module-level allow, per coding rules.

### Next Steps
Run with new debug output to identify exact panic location, then wrap the offending polars operation in `spawn_blocking`.

