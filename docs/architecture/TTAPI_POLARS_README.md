<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025 SKY, LLC.

TTAPI - Tastytrade API High-Performance Options Trading Platform
Contact: skylegal@nios.net for licensing inquiries
-->

# ttapi-polars

**Pre-compiled Polars Wrapper for High-Performance Data Processing**

[![Crates.io](https://img.shields.io/crates/v/ttapi-polars.svg)](https://crates.io/crates/ttapi-polars)
[![Documentation](https://docs.rs/ttapi-polars/badge.svg)](https://docs.rs/ttapi-polars)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

## Overview

`ttapi-polars` is a pre-compiled wrapper crate for the Polars data processing library. This crate isolates Polars compilation to prevent recompilation during development and provides a stable interface for high-performance data operations within the TTAPI ecosystem.

## Purpose

This crate serves as a **compilation isolation layer** for Polars, providing several key benefits:

- **Faster Development**: Prevents Polars recompilation during code changes
- **Stable Interface**: Consistent Polars API across the workspace
- **Memory Optimization**: Optimized for large dataset processing
- **Performance**: Pre-compiled for maximum runtime performance

## Features

- **DataFrame Operations**: High-performance data manipulation
- **Lazy Evaluation**: Memory-efficient processing of large datasets
- **Columnar Storage**: Optimized for analytical workloads
- **Parallel Processing**: Multi-threaded operations by default
- **Memory Management**: Smart memory usage for large datasets
- **Type Safety**: Rust's type system ensures data integrity

## Quick Start

```rust
use ttapi_polars::prelude::*;

fn main() -> PolarsResult<()> {
    // Create a DataFrame
    let df = df! {
        "symbol" => ["AAPL", "MSFT", "GOOGL"],
        "price" => [150.0, 300.0, 2500.0],
        "volume" => [1000000, 800000, 500000],
    }?;

    // Perform operations
    let result = df
        .lazy()
        .filter(col("price").gt(lit(200.0)))
        .select([
            col("symbol"),
            col("price"),
            (col("price") * col("volume")).alias("market_value")
        ])
        .collect()?;

    println!("{}", result);
    Ok(())
}
```

## Architecture

```
ttapi-polars/
├── src/
│   └── lib.rs              # Polars re-exports and utilities
├── Cargo.toml              # Polars dependency configuration
└── README.md               # This file
```

## Re-exported Modules

This crate re-exports the most commonly used Polars modules:

```rust
// Core Polars functionality
pub use polars::prelude::*;

// Specific modules for advanced usage
pub use polars::{
    chunked_array,
    datatypes,
    error,
    frame,
    lazy,
    series,
    time,
};
```

## Usage Patterns

### Basic DataFrame Operations

```rust
use ttapi_polars::prelude::*;

fn process_market_data() -> PolarsResult<DataFrame> {
    let df = df! {
        "timestamp" => [1640995200i64, 1640995260, 1640995320],
        "symbol" => ["AAPL", "AAPL", "AAPL"],
        "price" => [150.0, 151.0, 149.5],
        "volume" => [1000, 1500, 800],
    }?;

    // Calculate VWAP (Volume Weighted Average Price)
    let result = df
        .lazy()
        .with_columns([
            (col("price") * col("volume")).alias("dollar_volume")
        ])
        .group_by([col("symbol")])
        .agg([
            col("dollar_volume").sum().alias("total_dollar_volume"),
            col("volume").sum().alias("total_volume"),
        ])
        .with_columns([
            (col("total_dollar_volume") / col("total_volume")).alias("vwap")
        ])
        .collect()?;

    Ok(result)
}
```

### Large Dataset Processing

```rust
use ttapi_polars::prelude::*;

fn process_option_chains() -> PolarsResult<DataFrame> {
    // Process large option chain data efficiently
    LazyFrame::scan_parquet("option_chains.parquet", ScanArgsParquet::default())?
        .filter(
            col("expiration_date").gt(lit("2024-01-01"))
                .and(col("volume").gt(lit(0)))
        )
        .select([
            col("symbol"),
            col("strike"),
            col("call_put"),
            col("bid"),
            col("ask"),
            ((col("bid") + col("ask")) / lit(2.0)).alias("mid_price"),
        ])
        .group_by([col("symbol"), col("call_put")])
        .agg([
            col("mid_price").mean().alias("avg_mid_price"),
            col("strike").count().alias("contract_count"),
        ])
        .collect()
}
```

### Memory-Efficient Streaming

```rust
use ttapi_polars::prelude::*;

fn stream_large_dataset(file_path: &str) -> PolarsResult<()> {
    // Process data in chunks to manage memory
    let lazy_df = LazyFrame::scan_parquet(file_path, ScanArgsParquet::default())?;

    // Use streaming to process data without loading everything into memory
    let result = lazy_df
        .with_streaming(true)  // Enable streaming mode
        .filter(col("volume").gt(lit(1000)))
        .group_by([col("symbol")])
        .agg([
            col("price").mean().alias("avg_price"),
            col("volume").sum().alias("total_volume"),
        ])
        .collect()?;

    println!("Processed {} symbols", result.height());
    Ok(())
}
```

## Performance Optimization

### Memory Management

```rust
use ttapi_polars::prelude::*;

// Configure Polars for optimal memory usage
fn configure_polars_memory() {
    // Set memory pool size (useful for large datasets)
    std::env::set_var("POLARS_MAX_THREADS", "4");

    // Enable string caching for repeated string operations
    polars::enable_string_cache();
}
```

### Parallel Processing

```rust
use ttapi_polars::prelude::*;

fn parallel_processing_example() -> PolarsResult<DataFrame> {
    let df = df! {
        "group" => (0..1000000).map(|i| i % 100).collect::<Vec<_>>(),
        "value" => (0..1000000).map(|i| i as f64).collect::<Vec<_>>(),
    }?;

    // Polars automatically parallelizes operations
    let result = df
        .lazy()
        .group_by([col("group")])
        .agg([
            col("value").sum().alias("sum"),
            col("value").mean().alias("mean"),
            col("value").std(1).alias("std"),
        ])
        .collect()?;

    Ok(result)
}
```

## Integration with TTAPI

This crate integrates seamlessly with other TTAPI components:

### With ttapi-platform

```rust
use ttapi_platform::Platform;
use ttapi_polars::prelude::*;

async fn analyze_portfolio_data() -> Result<(), Box<dyn std::error::Error>> {
    let platform = Platform::with_defaults().await?;

    // Get portfolio data from platform
    let positions = platform.accounts().get_positions().await?;

    // Convert to Polars DataFrame for analysis
    let df = df! {
        "symbol" => positions.iter().map(|p| p.symbol.clone()).collect::<Vec<_>>(),
        "quantity" => positions.iter().map(|p| p.quantity).collect::<Vec<_>>(),
        "market_value" => positions.iter().map(|p| p.market_value).collect::<Vec<_>>(),
    }?;

    // Perform analysis
    let analysis = df
        .lazy()
        .with_columns([
            (col("market_value") / col("market_value").sum()).alias("weight")
        ])
        .filter(col("weight").gt(lit(0.05))) // Positions > 5% of portfolio
        .collect()?;

    println!("Large positions:\n{}", analysis);
    Ok(())
}
```

### With ttapi-client

```rust
use ttapi_client::Client;
use ttapi_polars::prelude::*;

async fn save_data_as_parquet() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::with_defaults()?;

    // Get data from API
    let instruments = client.auth().get("/api/instruments").await?;

    // Convert to DataFrame
    let df = df! {
        "symbol" => instruments.iter().map(|i| i.symbol.clone()).collect::<Vec<_>>(),
        "price" => instruments.iter().map(|i| i.last_price).collect::<Vec<_>>(),
    }?;

    // Save as Parquet for efficient storage
    let mut file = std::fs::File::create("instruments.parquet")?;
    ParquetWriter::new(&mut file).finish(&mut df.clone())?;

    Ok(())
}
```

## Compilation Benefits

### Before ttapi-polars (Problems)
- **Slow Development**: Every code change triggered Polars recompilation
- **Long Build Times**: 5+ minutes for simple changes
- **Resource Intensive**: High CPU/memory usage during compilation

### After ttapi-polars (Solutions)
- **Fast Development**: Polars compiled once, reused across workspace
- **Quick Builds**: 25-30 seconds for most changes
- **Resource Efficient**: Minimal recompilation overhead

## Data Volume Support

This crate is optimized for TTAPI's data volumes:

- **Small Datasets**: ~2,000 records (instruments, accounts) - In-memory processing
- **Medium Datasets**: ~30,000 records (quotes, trades) - Efficient memory usage
- **Large Datasets**: ~30,000,000 records (option chains) - Streaming processing

## Best Practices

1. **Use Lazy Evaluation**: Always prefer `lazy()` for complex operations
2. **Enable Streaming**: Use `with_streaming(true)` for large datasets
3. **Optimize Memory**: Configure thread count based on available resources
4. **Cache Strings**: Enable string caching for repeated string operations
5. **Batch Operations**: Group multiple operations together for efficiency

## Error Handling

```rust
use ttapi_polars::prelude::*;

fn safe_data_processing() -> PolarsResult<DataFrame> {
    let df = df! {
        "values" => [1.0, 2.0, f64::NAN, 4.0],
    }?;

    // Handle potential errors gracefully
    let result = df
        .lazy()
        .with_columns([
            col("values").fill_nan(lit(0.0)).alias("clean_values")
        ])
        .select([col("clean_values")])
        .collect()?;

    Ok(result)
}
```

## License

This project is licensed under the Mozilla Public License 2.0 - see the [LICENSE](../../LICENSE) file for details.

## Contributing

Please read [CONTRIBUTING.md](../../CONTRIBUTING.md) for details on our code of conduct and the process for submitting pull requests.

---

**Part of the TTAPI High-Performance Options Trading Platform**
Contact: skylegal@nios.net for licensing inquiries


---

[← Back to TTAPI Architecture Overview](../../docs/architecture/CURRENT_ARCHITECTURAL_STATE.md)
