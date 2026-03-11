<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 1: Immediate Wins - Implementation Guide

> **Effort**: 1-2 days | **Priority**: 🔴 Critical
> **Prerequisites**: Rust toolchain installed, repository cloned
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## ⚠️ Before You Start

1. **Create healing changelog FIRST**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%M)_CHANGELOG_HEALING.md
   ```

2. **Verify clean state**:
   ```bash
   just check && just clippy && just test
   ```

---

## 📋 Task Checklist

- [x] 1.1 MSRV Policy Formalization (already done)
- [ ] 1.2 Changelog Automation
- [ ] 1.3 Semantic Versioning Checks
- [ ] 1.4 Fuzz Testing Infrastructure
- [ ] 1.5 Mutation Testing

---

## 1.1 MSRV Policy Formalization

### Status: ✅ Already Implemented

UFFS already has `rust-version = "1.85"` in Cargo.toml.

### Verification
```bash
grep "rust-version" Cargo.toml
# Should show: rust-version = "1.85"
```

---

## 1.2 Changelog Automation

### What You're Doing
Creating a CHANGELOG.md file following Keep a Changelog format.

### Step-by-Step

**Step 1**: Create `CHANGELOG.md` in repository root
```bash
touch CHANGELOG.md
```

**Step 2**: Add initial content:
```markdown
# Changelog

All notable changes to UFFS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Modernization documentation and tracking

## [0.2.114] - 2026-01-27

### Added
- Initial modernization tracking
- UFFS-specific modernization plan

[Unreleased]: https://github.com/githubrobbi/UltraFastFileSearch/compare/v0.2.114...HEAD
[0.2.114]: https://github.com/githubrobbi/UltraFastFileSearch/releases/tag/v0.2.114
```

### Verification
```bash
cat CHANGELOG.md | head -20
```

---

## 1.3 Semantic Versioning Checks

### What You're Doing
Adding cargo-semver-checks to detect breaking API changes.

### Step-by-Step

**Step 1**: Install cargo-semver-checks
```bash
cargo install cargo-semver-checks
```

**Step 2**: Test locally
```bash
cargo semver-checks check-release --workspace
```

**Step 3**: Add to CI pipeline (scripts/ci/ci-pipeline.rs)

### Verification
```bash
cargo semver-checks check-release --workspace 2>&1 | head -20
```

---

## 1.4 Fuzz Testing Infrastructure

### What You're Doing
Setting up cargo-fuzz for MFT parsing security testing.

### Step-by-Step

**Step 1**: Install cargo-fuzz
```bash
cargo install cargo-fuzz
```

**Step 2**: Initialize fuzz testing
```bash
cd crates/uffs-mft
cargo fuzz init
```

**Step 3**: Create MFT record parsing fuzz target

Create `crates/uffs-mft/fuzz/fuzz_targets/fuzz_mft_record.rs`:
```rust
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Fuzz MFT record parsing - security critical
    if data.len() >= 1024 {
        // MFT records are typically 1024 bytes
        let _ = uffs_mft::parse_mft_record(data);
    }
});
```

**Step 4**: Run fuzzer (1-5 minutes initially)
```bash
cargo +nightly fuzz run fuzz_mft_record -- -max_total_time=60
```

### Priority Fuzz Targets
- MFT record header parsing
- $FILE_NAME attribute parsing
- $DATA attribute parsing
- Path resolution logic

---

## 1.5 Mutation Testing

### What You're Doing
Using cargo-mutants to verify test quality.

### Step-by-Step

**Step 1**: Install cargo-mutants
```bash
cargo install cargo-mutants
```

**Step 2**: Run on uffs-mft first (most critical)
```bash
cargo mutants --package uffs-mft -- --release
```

**Step 3**: Interpret results
- **Killed**: Tests caught the mutation ✅
- **Survived**: Tests missed it - need better tests! ❌
- **Target**: ≥70% killed

### Verification
```bash
cargo mutants --package uffs-mft --list | head -20
```

---

## ✅ Wave 1 Completion Checklist

- [x] `rust-version = "1.85"` in Cargo.toml
- [ ] `CHANGELOG.md` exists with proper format
- [ ] `cargo semver-checks check-release --workspace` runs
- [ ] `fuzz/` directory exists in uffs-mft with targets
- [ ] `cargo mutants --package uffs-mft` shows ≥70% killed

### Final Validation
```bash
rust-script scripts/ci/ci-pipeline.rs go -v
```

---

*Next: [Wave 2 - Architecture Completion](wave-2-architecture-completion.md)*

