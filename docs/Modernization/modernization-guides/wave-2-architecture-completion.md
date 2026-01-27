<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 2: Architecture Completion - Implementation Guide

> **Effort**: 3-5 days | **Priority**: 🔴 Critical
> **Prerequisites**: Wave 1 complete
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## ⚠️ Before You Start

1. **Create healing changelog**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%M)_CHANGELOG_HEALING.md
   ```

2. **Verify Wave 1 complete**:
   ```bash
   just check && just clippy && just test
   ```

---

## 📋 Task Checklist

- [ ] 2.1 MFT Pipeline Architecture Audit
- [ ] 2.2 Error Boundary Enforcement
- [ ] 2.3 Async Architecture Audit
- [ ] 2.4 Cache Architecture

---

## 2.1 MFT Pipeline Architecture Audit

### What You're Doing
Ensuring the MFT reading pipeline matches or exceeds C++ performance.

### Key Requirements

**Multi-Drive Parallel Indexing**:
- C++ uses single IOCP with multiple volume handles
- Rust should use tokio with parallel drive reading
- All NTFS drives indexed simultaneously

**Path Resolution Timing**:
- Path resolution MUST happen during MFT digestion
- Path column already present in DataFrame before filtering
- NOT a post-processing step

**Hard Link Expansion**:
- Default: Expand hard links to separate rows (matches Explorer)
- Power-user switch: `--no-expand-hardlinks` for unique FRS output

### Audit Checklist
```bash
# Find all MFT reading code
grep -rn "read_mft\|MftReader\|parse_mft" crates/uffs-mft/src/

# Find path resolution code
grep -rn "resolve_path\|PathResolver" crates/uffs-mft/src/

# Find hard link handling
grep -rn "hard_link\|HardLink" crates/uffs-mft/src/
```

### Architecture Diagram
```
┌─────────────────────────────────────────────────────────────┐
│                    MFT Reading Pipeline                      │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐                    │
│  │ Drive C │   │ Drive D │   │ Drive E │  ← Parallel reads  │
│  └────┬────┘   └────┬────┘   └────┬────┘                    │
│       │             │             │                          │
│       └─────────────┼─────────────┘                          │
│                     ▼                                        │
│            ┌────────────────┐                                │
│            │  MFT Parser    │  ← Parse records               │
│            └───────┬────────┘                                │
│                    ▼                                         │
│            ┌────────────────┐                                │
│            │ Path Resolver  │  ← Resolve during digestion    │
│            └───────┬────────┘                                │
│                    ▼                                         │
│            ┌────────────────┐                                │
│            │ Hard Link Exp. │  ← Expand by default           │
│            └───────┬────────┘                                │
│                    ▼                                         │
│            ┌────────────────┐                                │
│            │ Polars DF      │  ← Ready for queries           │
│            └────────────────┘                                │
└─────────────────────────────────────────────────────────────┘
```

---

## 2.2 Error Boundary Enforcement

### What You're Doing
Ensuring consistent error handling at crate boundaries.

### Rules
1. **Public APIs** return `UffsError` (not `anyhow::Error`)
2. **Internal code** can use `anyhow` for convenience
3. **Crate boundaries** convert to typed errors

### Audit Steps

**Step 1**: Find all public functions
```bash
grep -rn "pub fn\|pub async fn" crates/*/src/lib.rs
```

**Step 2**: Check return types
```bash
# Should return Result<T, UffsError> or UffsResult<T>
grep -rn "-> Result<" crates/*/src/lib.rs | grep -v UffsError
```

**Step 3**: Add context to errors
```rust
// Good: Rich context
.with_context(|| format!("Failed to read MFT from drive {}", drive))?

// Bad: No context
.map_err(UffsError::from)?
```

---

## 2.3 Async Architecture Audit

### What You're Doing
Reviewing async patterns for correctness and cancellation support.

### Audit Checklist

**Step 1**: Catalog all `tokio::spawn` calls
```bash
grep -rn "tokio::spawn" crates/
```

**Step 2**: Check for cancellation tokens
```rust
// Pattern: Use CancellationToken for long-running ops
use tokio_util::sync::CancellationToken;

async fn index_drive(drive: char, cancel: CancellationToken) -> UffsResult<()> {
    loop {
        if cancel.is_cancelled() {
            return Err(UffsError::Cancelled);
        }
        // ... work ...
    }
}
```

**Step 3**: Add graceful shutdown
```rust
// Pattern: Handle Ctrl+C gracefully
tokio::select! {
    result = index_all_drives() => result,
    _ = tokio::signal::ctrl_c() => {
        tracing::info!("Received Ctrl+C, shutting down...");
        Ok(())
    }
}
```

---

## 2.4 Cache Architecture

### What You're Doing
Optimizing the caching strategy for MFT data.

### Requirements
- **Default**: Cache enabled (fast startup)
- **Opt-out**: `--no-cache` flag for fresh data
- **Compression**: Zstd for cache files
- **Invalidation**: Based on MFT sequence numbers

### Implementation Pattern
```rust
pub struct CacheConfig {
    pub enabled: bool,           // Default: true
    pub compression: Compression, // Default: Zstd
    pub path: PathBuf,           // Default: ~/.cache/uffs/
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compression: Compression::Zstd,
            path: dirs::cache_dir().unwrap().join("uffs"),
        }
    }
}
```

---

## ✅ Wave 2 Completion Checklist

- [ ] Multi-drive parallel indexing implemented
- [ ] Path resolution happens during MFT digestion
- [ ] Hard link expansion default on, switch to disable
- [ ] All public APIs return `UffsError`
- [ ] All `tokio::spawn` calls cataloged
- [ ] Cancellation tokens for long-running ops
- [ ] Cache enabled by default with zstd compression

### Final Validation
```bash
rust-script scripts/ci-pipeline.rs go -v
```

---

*Previous: [Wave 1 - Immediate Wins](wave-1-immediate-wins.md)*
*Next: [Wave 3 - Testing Excellence](wave-3-testing-excellence.md)*

