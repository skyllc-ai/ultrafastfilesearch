# uffs-mft

**Direct NTFS Master File Table (MFT) reader for Windows.**

This crate provides ultra-fast, low-level access to the NTFS MFT, bypassing Windows file enumeration APIs entirely. It reads raw disk sectors and parses MFT records in parallel, outputting a Polars DataFrame ready for analysis.

## Features

- **Direct disk access** - Reads raw clusters, bypassing filesystem overhead
- **Bitmap optimization** - Skips free/unused records (often 10-30% of MFT)
- **Parallel parsing** - Uses rayon to parse records across all CPU cores
- **Polars DataFrame output** - Columnar format with SIMD operations
- **Comprehensive logging** - Rich tracing output for debugging and analysis

## Binary: `uffs_mft`

The crate includes a standalone binary for MFT operations:

```bash
# Build the binary
cargo build --release -p uffs-mft

# The binary will be at: target/release/uffs_mft.exe
```

### Commands

#### `info` - Show MFT Information (Fast)

Displays volume geometry and MFT metadata without reading all records (~10ms):

```bash
uffs_mft info --drive C
```

Output:
```
═══════════════════════════════════════════════════════════════
                    MFT INFO (Lightweight)
                    Drive: C:
═══════════════════════════════════════════════════════════════

📐 VOLUME GEOMETRY
  Bytes per sector:     512
  Bytes per cluster:    4096
  Bytes per MFT record: 1024
  Total clusters:       244190646
  Volume size:          931.51 GB

📁 MFT STRUCTURE
  MFT start LCN:        786432
  MFT size:             512.00 MB
  MFT % of volume:      0.054%
  Total records:        524288
  In-use records:       450000
  Free records:         74288
  Utilization:          85.8%
  Fragmentation:        1 extent(s) ✅

✅ HEALTH STATUS: Good (based on metadata)

💡 TIP: Use --deep for detailed file statistics.

⏱️  Completed in 8.2ms
═══════════════════════════════════════════════════════════════
```

#### `info --deep` - Full MFT Analysis

Reads and parses all MFT records for comprehensive statistics (~10-30s):

```bash
uffs_mft info --drive C --deep
```

Additional output with `--deep`:
```
📊 DEEP SCAN: Reading all MFT records...

📊 FILE SYSTEM STATISTICS
  Parsed records:       450000
  Directories:          50000
  Files:                400000

🏷️  ATTRIBUTE FLAGS
  Hidden:               1200
  System:               500
  Read-only:            150
  Archive:              380000
  Compressed:           100
  Encrypted:            50
  Sparse:               20
  Reparse points:       10

🔗 EXTENDED ATTRIBUTES
  Files with ADS:       25 (Alternate Data Streams)
  Files with hardlinks: 150

💾 STORAGE ANALYSIS
  Total file size:      450.25 GB
  Total allocated:      465.50 GB
  Slack space:          15616.00 MB (3.3%)

⏱️  Deep scan completed in 12.45s
```

#### `read` - Export MFT to Parquet

Reads the MFT and exports to a Parquet file:

```bash
uffs_mft read --drive C --output mft.parquet
```

The output Parquet contains all file metadata:
- `record_number`, `parent_record_number`
- `name`, `path` (reconstructed)
- `size`, `allocated_size`
- `created`, `modified`, `accessed`
- `is_directory`, `is_hidden`, `is_system`, `is_compressed`, etc.

#### `drives` - List NTFS Drives

```bash
uffs_mft drives
```

Output:
```
NTFS drives:
  C: (931.5 GB, ~524288 MFT records)
  D: (1863.0 GB, ~1048576 MFT records)
```

## Library Usage

```rust
use uffs_mft::MftReader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Open MFT reader for C: drive
    let reader = MftReader::open('C').await?;
    
    // Read all records into a Polars DataFrame
    let df = reader.read_all().await?;
    
    println!("Read {} records", df.height());
    println!("{}", df.head(Some(10)));
    
    Ok(())
}
```

## MFT Data Levels

| Level | What it is | Size | Speed |
|-------|------------|------|-------|
| **Boot Sector** | Volume geometry | 512 bytes | Instant |
| **$MFT Record 0** | MFT extent map | 1 KB | Instant |
| **$MFT Bitmap** | In-use record flags | ~64 KB | <10ms |
| **Full MFT** | All file records | 500 MB - 5 GB | 5-30s |

## Requirements

- **Windows only** - Uses Windows APIs for raw disk access
- **Administrator privileges** - Required for direct MFT access
- **Rust 1.85+** - Edition 2024

## License

MPL-2.0 - See [LICENSE](../../LICENSE) for details.

