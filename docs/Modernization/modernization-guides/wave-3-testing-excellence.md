<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 3: Testing Excellence - Implementation Guide

> **Effort**: 2-3 days | **Priority**: 🟠 Major
> **Prerequisites**: Wave 2 complete
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## ⚠️ Before You Start

1. **Create healing changelog**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%M)_CHANGELOG_HEALING.md
   ```

2. **Verify Wave 2 complete**:
   ```bash
   just check && just clippy && just test
   ```

---

## 📋 Task Checklist

- [ ] 3.1 Coverage Target (90%)
- [ ] 3.2 MFT Parsing Tests
- [ ] 3.3 Property-Based Testing
- [ ] 3.4 Performance Regression Testing

---

## 3.1 Coverage Target

### What You're Doing
Establishing baseline coverage and increasing to 90%.

### Step-by-Step

**Step 1**: Measure current coverage
```bash
cargo llvm-cov --workspace --html
open target/llvm-cov/html/index.html
```

**Step 2**: Identify gaps
```bash
# Show uncovered lines
cargo llvm-cov --workspace --show-missing-lines
```

**Step 3**: Add coverage gate to CI
```bash
# Fail if coverage drops below 90%
cargo llvm-cov --workspace --fail-under 90
```

### Priority Coverage Areas
1. **uffs-mft**: MFT parsing (most critical)
2. **uffs-core**: Query engine
3. **uffs-cli**: Command handling

---

## 3.2 MFT Parsing Tests

### What You're Doing
Comprehensive tests for all MFT parsing scenarios.

### Test Categories

**MFT Record Parsing**:
```rust
#[test]
fn test_parse_mft_record_header() {
    let raw_record = include_bytes!("fixtures/mft_record_file.bin");
    let record = parse_mft_record(raw_record).unwrap();
    assert_eq!(record.signature, b"FILE");
    assert!(record.flags.contains(MftFlags::IN_USE));
}

#[test]
fn test_parse_mft_record_directory() {
    let raw_record = include_bytes!("fixtures/mft_record_dir.bin");
    let record = parse_mft_record(raw_record).unwrap();
    assert!(record.flags.contains(MftFlags::DIRECTORY));
}
```

**Attribute Parsing**:
```rust
#[test]
fn test_parse_file_name_attribute() {
    let attr_data = include_bytes!("fixtures/attr_file_name.bin");
    let attr = parse_attribute(attr_data).unwrap();
    match attr {
        Attribute::FileName(fn_attr) => {
            assert_eq!(fn_attr.name, "test.txt");
            assert_eq!(fn_attr.parent_ref, 5); // Root directory
        }
        _ => panic!("Expected FileName attribute"),
    }
}
```

**Path Resolution**:
```rust
#[test]
fn test_path_resolution_simple() {
    let mft = load_test_mft();
    let path = mft.resolve_path(123).unwrap();
    assert_eq!(path, r"C:\Users\Test\Documents\file.txt");
}

#[test]
fn test_path_resolution_hard_link() {
    let mft = load_test_mft();
    let paths = mft.resolve_all_paths(456).unwrap();
    assert_eq!(paths.len(), 2); // File has 2 hard links
}
```

**Hard Link Expansion**:
```rust
#[test]
fn test_hard_link_expansion_default() {
    let df = index_drive_with_options(DriveOptions::default());
    // Hard links should create separate rows
    let count = df.filter(col("frs").eq(lit(789))).count();
    assert_eq!(count, 3); // 3 hard links = 3 rows
}

#[test]
fn test_hard_link_no_expansion() {
    let opts = DriveOptions { expand_hardlinks: false, ..Default::default() };
    let df = index_drive_with_options(opts);
    let count = df.filter(col("frs").eq(lit(789))).count();
    assert_eq!(count, 1); // No expansion = 1 row per FRS
}
```

---

## 3.3 Property-Based Testing

### What You're Doing
Using proptest for edge case discovery.

### Step-by-Step

**Step 1**: Add proptest dependency
```bash
cargo add proptest --dev -p uffs-mft
```

**Step 2**: Create property tests
```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn path_resolution_never_panics(frs in 0u64..1_000_000) {
        let mft = load_test_mft();
        // Should return Ok or Err, never panic
        let _ = mft.resolve_path(frs);
    }

    #[test]
    fn filter_expression_roundtrip(expr in "[a-z]{1,10}") {
        let parsed = parse_filter(&expr);
        if let Ok(filter) = parsed {
            let serialized = filter.to_string();
            let reparsed = parse_filter(&serialized).unwrap();
            assert_eq!(filter, reparsed);
        }
    }
}
```

---

## 3.4 Performance Regression Testing

### What You're Doing
Adding criterion benchmarks with baseline management.

### Step-by-Step

**Step 1**: Create benchmark file
```rust
// benches/mft_parsing.rs
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_mft_record_parsing(c: &mut Criterion) {
    let raw_records = load_test_records();
    
    let mut group = c.benchmark_group("mft_parsing");
    group.throughput(Throughput::Elements(raw_records.len() as u64));
    
    group.bench_function("parse_records", |b| {
        b.iter(|| {
            for record in &raw_records {
                parse_mft_record(record).unwrap();
            }
        })
    });
    
    group.finish();
}

criterion_group!(benches, bench_mft_record_parsing);
criterion_main!(benches);
```

**Step 2**: Save baseline
```bash
cargo bench -- --save-baseline main
```

**Step 3**: Compare against baseline
```bash
cargo bench -- --baseline main
```

---

## ✅ Wave 3 Completion Checklist

- [ ] Coverage baseline established
- [ ] Coverage ≥ 90% for critical crates
- [ ] MFT record parsing tests complete
- [ ] Attribute parsing tests complete
- [ ] Path resolution tests complete
- [ ] Hard link expansion tests complete
- [ ] Property-based tests added
- [ ] Criterion benchmarks with baselines

### Final Validation
```bash
cargo llvm-cov --workspace --fail-under 90
rust-script scripts/ci-pipeline.rs go -v
```

---

*Previous: [Wave 2 - Architecture Completion](wave-2-architecture-completion.md)*
*Next: [Wave 4 - Documentation & API](wave-4-documentation-api.md)*

