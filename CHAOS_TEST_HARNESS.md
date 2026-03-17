# Chaos Test Harness - Deterministic MFT Out-of-Order Processing

## Overview

The chaos test harness (`crates/uffs-mft/src/io/readers/parallel/tests_chaos.rs`) simulates the out-of-order record processing that occurs in Windows LIVE parsing due to:
- **IOCP overlapped I/O**: Chunks can complete in any order
- **Parallel rayon parsing**: Extension records may be processed before their base records

This allows reproducible testing of race conditions and merge bugs **without requiring Windows**.

## Architecture

```
Offline MFT File
    ↓
Split into chunks (8MB default)
    ↓
Reorder chunks (controlled chaos)
    ↓
Process through same pipeline as LIVE
    ↓
MftIndex output
```

## Chaos Strategies

1. **Random** - Seeded shuffle (most realistic)
   - Uses ChaCha8Rng for deterministic randomization
   - Same seed → same chunk order → reproducible failures

2. **Reverse** - Process chunks in reverse order
   - Simple but effective for testing
   - Guaranteed extension-before-base for end-of-drive files

3. **Interleaved** - Swap adjacent chunks
   - Controlled chaos
   - Good for boundary conditions

## Usage

### Running Tests

```bash
# Run all chaos tests (requires offline MFT)
cargo test -p uffs-mft -- chaos --ignored --nocapture

# Run specific strategy
cargo test -p uffs-mft -- test_random_order_d_drive --ignored --nocapture
cargo test -p uffs-mft -- test_reverse_order_d_drive --ignored --nocapture
cargo test -p uffs-mft -- test_interleaved_order_d_drive --ignored --nocapture
```

### Requirements

- **Offline MFT**: `/Users/rnio/uffs_data/drive_d/D_mft.bin`
- **Platform**: macOS (cross-platform testing)
- **Dependencies**: `rand`, `rand_chacha` (dev dependencies)

### Test Output

Each test shows:
- Total chunks processed
- Chunk reordering statistics
- Extension-before-base occurrences
- Final record count
- Success/failure status

Example output:
```
✅ RANDOM-ORDER parsing completed (seed=42)
   Chunks processed: 128
   Extension-before-base: 47 occurrences
   Total records: 1,234,567
```

## Finding Bugs

### Comparing with Reference

```bash
# 1. Run chaos test
cargo test -p uffs-mft -- test_random_order_d_drive --ignored --nocapture > chaos_output.txt

# 2. Compare with C++ reference
# The chaos harness outputs can be compared with:
# /Users/rnio/uffs_data/drive_d/cpp_d.txt

# 3. Look for discrepancies in:
#    - Directory sizes
#    - Extension record counts
#    - Data run totals
```

### Debugging Specific FRS

The harness logs extension-before-base events:
```rust
tracing::debug!(frs = ext_rec.frs, "Extension arrived before base");
```

Use `RUST_LOG=debug` to see these:
```bash
RUST_LOG=uffs_mft=debug cargo test -p uffs-mft -- test_random_order_d_drive --ignored --nocapture 2>&1 | grep "Extension arrived"
```

## Customizing Tests

### Different Chunk Sizes

```rust
let chaos_reader = ChaosMftReader::new(
    ChaosStrategy::Random { seed: 42 },
    2 * 1024 * 1024,  // 2MB chunks (more fine-grained chaos)
);
```

### Different Seeds

```rust
ChaosStrategy::Random { seed: 123456 }  // Try different seeds
```

### Custom Strategies

Add new variants to `ChaosStrategy`:
```rust
enum ChaosStrategy {
    // ...
    BlockSwap { block_size: usize },  // Swap N-chunk blocks
    DelayedExtensions,                 // Always process extensions last
}
```

## Known Issues

1. **Memory usage**: Large MFTs with small chunks use more memory
2. **Performance**: Chaos tests are slower than normal parsing (~2-3x)
3. **Determinism**: Only applies to chunk order, not within-chunk rayon parallelism

## Integration with CI

These tests are `#[ignore]` by default (require offline MFT). To run in CI:

```bash
# In .github/workflows/ci.yml
- name: Chaos tests
  if: env.HAS_OFFLINE_MFT == 'true'
  run: cargo test -p uffs-mft -- chaos --ignored
```

## References

- LIVE parser: `crates/uffs-mft/src/parse/direct_index.rs`
- Extension merger: `crates/uffs-mft/src/parse/direct_index_extension.rs`
- Parallel reader: `crates/uffs-mft/src/io/readers/parallel/`
- C++ reference: `_trash/cpp_*.txt`

## Troubleshooting

**Test panics with "offline MFT not found"**
- Ensure `/Users/rnio/uffs_data/drive_d/D_mft.bin` exists
- Or update the path in the test

**Compilation errors**
- Run `cargo check -p uffs-mft --tests`
- Ensure `rand` and `rand_chacha` are in `[dev-dependencies]`

**No output**
- Add `--nocapture` flag
- Use `RUST_LOG=info` or `RUST_LOG=debug`

**Non-deterministic results**
- Rayon parallelism within chunks is not controlled
- Use single-threaded mode: `RAYON_NUM_THREADS=1`
