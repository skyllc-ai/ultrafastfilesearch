<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 6: Advanced Tooling - Implementation Guide

> **Effort**: 1-2 days | **Priority**: 🟢 Enhancement
> **Prerequisites**: Wave 5 complete
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## ⚠️ Before You Start

1. **Create healing changelog**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%MM)_CHANGELOG_HEALING.md
   ```

2. **Verify Wave 5 complete**:
   ```bash
   just check && just clippy && just test
   ```

---

## 📋 Task Checklist

- [ ] 6.1 tokio-console Integration
- [ ] 6.2 Unused Dependency Detection
- [ ] 6.3 Build Caching with sccache
- [ ] 6.4 cargo-expand Documentation

---

## 6.1 tokio-console Integration

### What You're Doing
Adding tokio-console for async debugging during development.

### Step-by-Step

**Step 1**: Add console-subscriber dependency
```bash
cargo add console-subscriber --dev -p uffs-cli
```

**Step 2**: Add feature flag
```toml
# Cargo.toml
[features]
tokio-console = ["console-subscriber"]
```

**Step 3**: Initialize in main
```rust
#[cfg(feature = "tokio-console")]
fn init_console() {
    console_subscriber::init();
}

#[tokio::main]
async fn main() {
    #[cfg(feature = "tokio-console")]
    init_console();
    
    // ... rest of main
}
```

**Step 4**: Run with console
```bash
# Terminal 1: Start tokio-console
tokio-console

# Terminal 2: Run app with console feature
RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio-console -- index
```

### What You Can Debug
- Task spawn/drop lifecycle
- Task poll times
- Waker statistics
- Resource contention

---

## 6.2 Unused Dependency Detection

### What You're Doing
Adding cargo-machete to CI for automatic unused dependency detection.

### Step-by-Step

**Step 1**: Install cargo-machete
```bash
cargo install cargo-machete
```

**Step 2**: Run locally
```bash
cargo machete
```

**Step 3**: Add to CI pipeline

Add to `scripts/ci/ci-pipeline.rs`:
```rust
// Check for unused dependencies
run_command("cargo", &["machete", "--skip-target-dir"])?;
```

**Step 4**: Add justfile recipe
```just
# Check for unused dependencies
machete:
    @printf "\033[0;34m🔪 Checking for unused dependencies...\033[0m\n"
    cargo machete
```

### Handling False Positives
```toml
# Cargo.toml
[package.metadata.cargo-machete]
ignored = ["some-crate"]  # If truly needed but not detected
```

---

## 6.3 Build Caching with sccache

### What You're Doing
Enabling sccache for faster local development builds.

### Step-by-Step

**Step 1**: Install sccache
```bash
cargo install sccache
```

**Step 2**: Configure environment
```bash
# Add to ~/.zshrc or ~/.bashrc
export RUSTC_WRAPPER=sccache
export SCCACHE_CACHE_SIZE="10G"
```

**Step 3**: Verify it's working
```bash
sccache --show-stats
cargo build --release
sccache --show-stats  # Should show cache hits
```

**Step 4**: Add justfile recipe
```just
# Show sccache statistics
sccache-stats:
    @printf "\033[0;34m📊 sccache statistics:\033[0m\n"
    sccache --show-stats

# Clear sccache
sccache-clear:
    @printf "\033[0;34m🧹 Clearing sccache...\033[0m\n"
    sccache --zero-stats
```

### Expected Speedup
- First build: Normal time
- Subsequent builds: 2-5x faster for unchanged crates

---

## 6.4 cargo-expand Documentation

### What You're Doing
Documenting macro debugging workflow for complex derive macros.

### Step-by-Step

**Step 1**: Install cargo-expand
```bash
cargo install cargo-expand
```

**Step 2**: Basic usage
```bash
# Expand all macros in a file
cargo expand --package uffs-mft --lib

# Expand specific item
cargo expand --package uffs-mft --lib -- MftRecord
```

**Step 3**: Add justfile recipe
```just
# Expand macros for debugging
expand CRATE ITEM="":
    @printf "\033[0;34m🔍 Expanding macros in {{ CRATE }}...\033[0m\n"
    @if [ -z "{{ ITEM }}" ]; then \
        cargo expand --package {{ CRATE }} --lib; \
    else \
        cargo expand --package {{ CRATE }} --lib -- {{ ITEM }}; \
    fi
```

**Step 4**: Document common patterns

Create `docs/debugging-macros.md`:
```markdown
# Debugging Macros in UFFS

## Common Derive Macros

### thiserror
```bash
cargo expand --package uffs-core -- UffsError
```

### clap
```bash
cargo expand --package uffs-cli -- Cli
```

### serde
```bash
cargo expand --package uffs-mft -- MftRecord
```
```

---

## ✅ Wave 6 Completion Checklist

- [ ] console-subscriber feature added
- [ ] tokio-console workflow documented
- [ ] cargo-machete in CI pipeline
- [ ] sccache configured and documented
- [ ] cargo-expand recipes in justfile
- [ ] Macro debugging guide created

### Final Validation
```bash
cargo machete
sccache --show-stats
rust-script scripts/ci/ci-pipeline.rs go -v
```

---

## 🎉 Modernization Complete!

Congratulations! You've completed all 6 waves of the UFFS modernization plan.

### Summary of Achievements
- ✅ Wave 1: MSRV, Changelog, Semver, Fuzz, Mutation testing
- ✅ Wave 2: MFT pipeline, Error handling, Async, Caching
- ✅ Wave 3: 90% coverage, Property tests, Performance baselines
- ✅ Wave 4: 100% rustdoc, CLI docs, MFT field reference
- ✅ Wave 5: Performance baselines, Tracing, Memory profiling
- ✅ Wave 6: tokio-console, machete, sccache, cargo-expand

### Next Steps
1. Update [MODERNIZATION_TRACKER.md](../MODERNIZATION_TRACKER.md) with completion dates
2. Create a release with all improvements
3. Continue monitoring metrics and improving

---

*Previous: [Wave 5 - Performance & Observability](wave-5-performance-observability.md)*
*Back to: [README](README.md)*

