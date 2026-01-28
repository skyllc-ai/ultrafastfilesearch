# Changelog Healing - 2026-01-28 12:00

## Issue: Nested Tokio Runtime Panic

### Symptom
```
thread 'main' panicked at tokio-1.49.0/src/runtime/scheduler/multi_thread/mod.rs:88:9:
Cannot start a runtime from within a runtime. This happens because a function (like `block_on`)
attempted to block the current thread while the thread is being used to drive asynchronous tasks.
```

### Root Cause Identified

Debug tracing revealed the panic occurs in `apply_directory_treesize()` which uses:
```rust
df.clone().lazy()
    .with_column(...)
    .collect()  // <-- This triggers tokio internally with new_streaming feature
```

The polars `new_streaming` feature uses tokio internally for `.collect()` on lazy frames.
When called from within an async context (our CLI runs on tokio), this causes nested runtime panic.

### Debug Output (from user test)
```
[DEBUG] read_all_index: ENTER volume=F
[DEBUG] read_all_index: INSIDE spawn_blocking volume=F
[DEBUG] read_all_index: read_mft_index_internal done
[DEBUG] read_all_index: EXIT volume=F
[DEBUG] search_single_drive: before execute_index_query
[DEBUG] execute_index_query: before query.collect()
[DEBUG] execute_index_query: after query.collect(), count=2282616
[DEBUG] execute_index_query: before results_to_dataframe
[DEBUG] results_to_dataframe: before DataFrame::new_infer_height
[DEBUG] results_to_dataframe: after DataFrame::new_infer_height
[DEBUG] results_to_dataframe: before apply_directory_treesize
<PANIC HERE>
```

### Fix Applied

Wrapped `apply_directory_treesize()` call in `tokio::task::block_in_place()`:
```rust
df = tokio::task::block_in_place(|| uffs_core::apply_directory_treesize(&df))
    .map_err(|err| anyhow::anyhow!("Failed to apply directory treesize: {err}"))?;
```

`block_in_place` allows blocking operations within a multi-threaded tokio runtime by
temporarily moving the current task off the runtime thread.

### Files Modified
- `crates/uffs-cli/src/commands.rs` - Wrapped `apply_directory_treesize` in `block_in_place`
- `.cargo/config.toml` - Switched Windows linker from `rust-lld` to `link.exe`
- `.cargo/windows.toml` - Switched Windows linker from `rust-lld` to `link.exe`

### Commits
- `304dbe36b` - debug: add targeted tracing for nested runtime panic investigation
- `a99a2fa57` - config: switch Windows linker from rust-lld to MSVC link.exe
- (pending) - fix: wrap apply_directory_treesize in block_in_place

