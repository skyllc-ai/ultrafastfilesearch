# MFT Reader (Rust)

A high-performance Rust tool for reading raw NTFS Master File Table (MFT) records and exporting them to CSV format.

## Overview

This tool directly reads the MFT from an NTFS volume by bypassing standard Windows file system APIs. This approach provides:

- **Speed**: Direct volume access is significantly faster than enumerating files through the file system
- **Complete data**: Access to all MFT records including deleted files and system metadata
- **Low-level details**: Exposes MFT internals like record numbers, sequence numbers, and parent references

## Requirements

- **Windows OS**: Uses Windows-specific APIs for raw volume access
- **Administrator privileges**: Required to open the volume directly
- **Rust toolchain**: For building from source

## Building

```bash
cd mft-reader-rs
cargo build --release
```

The executable will be created at `target/release/mft-reader.exe`.

## Usage

```bash
# Basic usage - read drive C: and output to mft_records.csv
mft-reader.exe -d C

# Specify output file
mft-reader.exe -d C -o output.csv

# Output to stdout
mft-reader.exe -d C -o -

# Verbose mode (shows volume details)
mft-reader.exe -d C -v
```

### Command Line Options

| Option | Short | Description |
|--------|-------|-------------|
| `--drive` | `-d` | Drive letter to read (e.g., C, D, E) |
| `--output` | `-o` | Output CSV file path (default: mft_records.csv, use `-` for stdout) |
| `--verbose` | `-v` | Show detailed volume information |

## CSV Output Format

The output CSV contains the following columns:

| Column | Description |
|--------|-------------|
| RecordNumber | MFT record index (FRS number) |
| SequenceNumber | Record reuse counter |
| InUse | Whether the record is active |
| IsDirectory | Whether this is a directory |
| ParentRecordNumber | Parent directory's MFT record number |
| ParentSequenceNumber | Parent directory's sequence number |
| FileName | File or directory name |
| FileSize | Logical file size in bytes |
| AllocatedSize | Allocated size on disk in bytes |
| CreationTime | File creation timestamp |
| ModificationTime | Last content modification timestamp |
| AccessTime | Last access timestamp |
| ChangeTime | Last MFT record change timestamp |
| Attributes | File attribute flags (R=ReadOnly, H=Hidden, S=System, D=Directory, A=Archive, etc.) |
| AttributeFlags | Raw attribute flags in hexadecimal |
| LinkCount | Number of hard links |
| IsBaseRecord | Whether this is the base record (vs. extension record) |

## How It Works

1. **Opens the volume** using `\\.\C:` syntax for direct access
2. **Reads NTFS volume data** via `FSCTL_GET_NTFS_VOLUME_DATA` to get MFT location and sizes
3. **Gets MFT extents** using `FSCTL_GET_RETRIEVAL_POINTERS` to handle fragmented MFT
4. **Reads MFT records** directly from disk at the calculated offsets
5. **Applies USA unfixup** to restore sector end bytes (data integrity mechanism)
6. **Parses attributes** including $FILE_NAME, $STANDARD_INFORMATION, and $DATA
7. **Outputs to CSV** with formatted timestamps and attribute flags

## Technical Details

### NTFS Structures Implemented

- `NTFS_BOOT_SECTOR` - Boot sector with volume geometry
- `FILE_RECORD_SEGMENT_HEADER` - MFT record header with flags
- `ATTRIBUTE_RECORD_HEADER` - Attribute parsing support
- `FILENAME_INFORMATION` - Filename attribute ($30)
- `STANDARD_INFORMATION` - Timestamps and attributes ($10)
- Multi-sector header with USA (Update Sequence Array) unfixup

### Fragmented MFT Support

The MFT itself can be fragmented across the disk. This tool handles fragmentation by:
1. Opening `$MFT` to get its retrieval pointers
2. Building an extent map (VCN → LCN mappings)
3. Calculating correct disk offsets for each record

## Example Output

```csv
RecordNumber,SequenceNumber,InUse,IsDirectory,ParentRecordNumber,FileName,FileSize,...
0,1,true,false,5,$MFT,0,...
5,5,true,true,5,.,0,...
39,1,true,true,5,$Extend,0,...
100,5,true,false,39,desktop.ini,282,...
```

## Performance

On a typical system with ~500,000 files:
- Read time: ~5-10 seconds
- Output: ~50-100 MB CSV file
- Speed: ~50,000-100,000 records/second

## Limitations

- **Windows only**: Relies on Windows-specific APIs
- **NTFS only**: Does not support other file systems
- **No path reconstruction**: Outputs parent record numbers, not full paths
- **Administrator required**: Cannot run as standard user

## Source References

This Rust implementation is a direct port of the C++ MFT reading logic from the Ultra-Fast-File-Search project.

### Architecture Documentation

- **[MFT Reading Deep Dive](../docs/architecture/02-mft-reading-deep-dive.md)** - Comprehensive documentation of the MFT reading architecture, including:
  - NTFS on-disk structures
  - USA (Update Sequence Array) unfixup algorithm
  - Retrieval pointer handling for fragmented MFT
  - Record parsing flow
  - Performance optimizations

### C++ Source Files

The following C++ source files were used as reference for this implementation:

| File | Description |
|------|-------------|
| `UltraFastFileSearch-code/file.cpp` | Main MFT reading implementation |
| Lines 939-967 | `NTFS_BOOT_SECTOR` structure |
| Lines 969-992 | `MULTI_SECTOR_HEADER` and `unfixup()` algorithm |
| Lines 1014-1044 | `ATTRIBUTE_RECORD_HEADER` structure |
| Lines 1056-1075 | `FILE_RECORD_SEGMENT_HEADER` structure |
| Lines 1076-1100 | `FILENAME_INFORMATION` and `STANDARD_INFORMATION` |
| Lines 1497-1529 | `get_retrieval_pointers()` for fragmented MFT |
| Lines 2367-2377 | MFT record parsing logic |

## C++ to Rust Comparison

This implementation has been verified to have **100% logic parity** with the original C++ code.

### Structure Mapping

| C++ Structure | Rust Structure | Location |
|---------------|----------------|----------|
| `NTFS_BOOT_SECTOR` | `NtfsBootSector` | `ntfs.rs:9-51` |
| `MULTI_SECTOR_HEADER` | `MultiSectorHeader` | `ntfs.rs:53-95` |
| `FILE_RECORD_SEGMENT_HEADER` | `FileRecordSegmentHeader` | `ntfs.rs:175-208` |
| `ATTRIBUTE_RECORD_HEADER` | `AttributeRecordHeader` | `ntfs.rs:210-221` |
| `RESIDENT` | `ResidentAttributeData` | `ntfs.rs:223-230` |
| `NONRESIDENT` | `NonResidentAttributeData` | `ntfs.rs:232-244` |
| `FILENAME_INFORMATION` | `FilenameInformation` | `ntfs.rs:246-290` |
| `STANDARD_INFORMATION` | `StandardInformation` | `ntfs.rs:292-302` |

### Algorithm Verification

| Algorithm | C++ | Rust | Status |
|-----------|-----|------|--------|
| USA Unfixup | `i * 512 - sizeof(unsigned short)` | `i * 512 - 2` | ✅ Match |
| File record size calc | `clusters >= 0 ? clusters * sectors * bytes : 1 << -clusters` | Same | ✅ Match |
| Magic number check | `Magic == 'ELIF'` | `magic == 0x454C4946` | ✅ Match |
| IN_USE flag | `Flags & 0x0001` | `flags & 0x0001` | ✅ Match |
| DIRECTORY flag | `Flags & 0x0002` | `flags & 0x0002` | ✅ Match |
| Parent FRS extraction | Lower 48 bits of ParentDirectory | `& 0x0000_FFFF_FFFF_FFFF` | ✅ Match |
| Attribute iteration | `FirstAttributeOffset` + `Length` | Same | ✅ Match |
| End marker detection | `Type == 0xFFFFFFFF` | `type_code == 0xFFFFFFFF` | ✅ Match |

### Attribute Type Codes

| Attribute | C++ Value | Rust Value | Status |
|-----------|-----------|------------|--------|
| $STANDARD_INFORMATION | `0x10` | `0x10` | ✅ |
| $FILE_NAME | `0x30` | `0x30` | ✅ |
| $DATA | `0x80` | `0x80` | ✅ |
| $INDEX_ROOT | `0x90` | `0x90` | ✅ |
| $ATTRIBUTE_END | `0xFFFFFFFF` | `0xFFFFFFFF` | ✅ |

### Key Implementation Notes

1. **Packed Structures**: Both use `#pragma pack(1)` (C++) and `#[repr(C, packed)]` (Rust) to ensure correct memory layout matching NTFS on-disk format.

2. **USA Unfixup**: The algorithm is byte-for-byte identical:
   - Iterate from `i=1` to `USACount`
   - Calculate offset as `i * 512 - 2` (sector size minus u16)
   - Verify check value equals `usa[0]`
   - Replace with `usa[i]`

3. **Retrieval Pointers**: Both use `FSCTL_GET_RETRIEVAL_POINTERS` to handle fragmented MFT, building VCN→LCN extent maps.

4. **Endianness**: Both assume little-endian (x86/x64), which matches NTFS on-disk format.

## License

Part of the Ultra-Fast-File-Search project.

