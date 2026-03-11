<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🌊 Wave 5: Performance & Observability - Implementation Guide

> **Effort**: 2-3 days | **Priority**: 🟡 Moderate
> **Prerequisites**: Wave 4 complete
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)

---

## ⚠️ Before You Start

1. **Create healing changelog**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%M)_CHANGELOG_HEALING.md
   ```

2. **Verify Wave 4 complete**:
   ```bash
   just check && just clippy && just test
   ```

---

## 📋 Task Checklist

- [ ] 5.1 Performance Baselines
- [ ] 5.2 Tracing & Telemetry
- [ ] 5.3 Memory Profiling
- [ ] 5.4 Flamegraph Automation

---

## 5.1 Performance Baselines

### What You're Doing
Establishing and tracking performance metrics.

### Key Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| MFT Read Speed | ≥1M records/sec | Criterion benchmark |
| Path Resolution | Match C++ | Criterion benchmark |
| Query Execution | <100ms for 1M files | hyperfine |
| Startup Time | <100ms | hyperfine |
| Peak Memory | ≤ C++ baseline | heaptrack/dhat |

### Step-by-Step

**Step 1**: Create benchmark suite
```rust
// benches/performance.rs
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_mft_read_speed(c: &mut Criterion) {
    let mut group = c.benchmark_group("mft_read");
    
    // Measure records per second
    group.throughput(Throughput::Elements(1_000_000));
    group.bench_function("read_1m_records", |b| {
        b.iter(|| {
            // Read MFT from test drive
            index_drive('C').unwrap()
        })
    });
    
    group.finish();
}

fn bench_path_resolution(c: &mut Criterion) {
    let mft = load_test_mft();
    let frs_list: Vec<u64> = (0..100_000).collect();
    
    let mut group = c.benchmark_group("path_resolution");
    group.throughput(Throughput::Elements(frs_list.len() as u64));
    
    group.bench_function("resolve_100k_paths", |b| {
        b.iter(|| {
            for frs in &frs_list {
                let _ = mft.resolve_path(*frs);
            }
        })
    });
    
    group.finish();
}

criterion_group!(benches, bench_mft_read_speed, bench_path_resolution);
criterion_main!(benches);
```

**Step 2**: Measure with hyperfine
```bash
# Startup time
hyperfine --warmup 3 'target/release/uffs --help'

# Query execution
hyperfine --warmup 1 'target/release/uffs "*.rs"'
```

**Step 3**: Compare with C++ baseline
```bash
# Run legacy version
hyperfine 'uffs.com "*.rs"'

# Run Rust version
hyperfine 'target/release/uffs "*.rs"'
```

---

## 5.2 Tracing & Telemetry

### What You're Doing
Adding structured tracing for performance analysis.

### Step-by-Step

**Step 1**: Add tracing spans
```rust
use tracing::{instrument, info_span, Instrument};

#[instrument(skip(self), fields(drive = %drive))]
pub async fn index_drive(&self, drive: char) -> UffsResult<DataFrame> {
    let read_span = info_span!("mft_read", records = tracing::field::Empty);
    
    let records = async {
        let mft = self.read_mft(drive).await?;
        tracing::Span::current().record("records", mft.len());
        Ok(mft)
    }
    .instrument(read_span)
    .await?;
    
    let resolve_span = info_span!("path_resolution");
    let df = async {
        self.resolve_paths(records).await
    }
    .instrument(resolve_span)
    .await?;
    
    Ok(df)
}
```

**Step 2**: Configure tracing subscriber
```rust
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("uffs=info"));
    
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();
}
```

**Step 3**: Add file logging for debugging
```rust
use tracing_appender::rolling::{RollingFileAppender, Rotation};

let file_appender = RollingFileAppender::new(
    Rotation::DAILY,
    "logs",
    "uffs.log",
);

tracing_subscriber::registry()
    .with(fmt::layer().with_writer(file_appender))
    .init();
```

---

## 5.3 Memory Profiling

### What You're Doing
Measuring and optimizing memory usage.

### Targets

| Metric | Target |
|--------|--------|
| Peak RSS during indexing | ≤ C++ baseline |
| Memory per million files | < 500 MB |
| DataFrame memory efficiency | Optimal column types |

### Step-by-Step

**Step 1**: Use dhat for heap profiling
```bash
cargo add dhat --dev -p uffs-cli
```

```rust
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    
    // ... rest of main
}
```

**Step 2**: Run with profiling
```bash
cargo run --release --features dhat-heap -- index
# Outputs dhat-heap.json
```

**Step 3**: Optimize DataFrame column types
```rust
// Use smallest possible types
let df = df.with_column(
    col("size").cast(DataType::UInt64)  // Not Int64
)?;

// Use categorical for repeated strings
let df = df.with_column(
    col("extension").cast(DataType::Categorical(None, CategoricalOrdering::Lexical))
)?;
```

---

## 5.4 Flamegraph Automation

### What You're Doing
Adding flamegraph generation for profiling hot paths.

### Step-by-Step

**Step 1**: Install flamegraph
```bash
cargo install flamegraph
```

**Step 2**: Add justfile recipe
```just
# Generate flamegraph for indexing
flamegraph-index:
    @printf "\033[0;34m🔥 Generating flamegraph for index operation...\033[0m\n"
    cargo flamegraph --bin uffs -- index

# Generate flamegraph for search
flamegraph-search PATTERN:
    @printf "\033[0;34m🔥 Generating flamegraph for search: {{ PATTERN }}...\033[0m\n"
    cargo flamegraph --bin uffs -- search "{{ PATTERN }}"
```

**Step 3**: Run and analyze
```bash
just flamegraph-index
open flamegraph.svg
```

---

## ✅ Wave 5 Completion Checklist

- [ ] Criterion benchmarks for MFT read speed
- [ ] Criterion benchmarks for path resolution
- [ ] hyperfine measurements documented
- [ ] Tracing spans added to critical paths
- [ ] File logging configured
- [ ] Memory profiling with dhat
- [ ] DataFrame column types optimized
- [ ] Flamegraph recipes in justfile

### Final Validation
```bash
cargo bench
rust-script scripts/ci/ci-pipeline.rs go -v
```

---

*Previous: [Wave 4 - Documentation & API](wave-4-documentation-api.md)*
*Next: [Wave 6 - Advanced Tooling](wave-6-advanced-tooling.md)*

