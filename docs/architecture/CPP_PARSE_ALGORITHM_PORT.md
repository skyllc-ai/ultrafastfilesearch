# C++ Parse Algorithm Port - Implementation Guide

> **Goal**: Implement a 100% faithful port of the C++ MFT parsing algorithm as an **alternative** to the current Rust implementation, with a switch to toggle between them.

**Branch**: `feature/cpp-parsing-algorithm-port`  
**Status**: Scaffolding complete, placeholder implementation in place  
**Date**: 2026-01-31

---

## 1. Resources

> **IMPORTANT**: This implementation is based EXCLUSIVELY on C++ source code and C++ documentation.
> All resources are located in `docs/architecture/C++_resources/`.

### 1.1 Primary C++ Source Files

| File | Purpose | Key Lines |
|------|---------|-----------|
| `UltraFastFileSearch-code/src/index/ntfs_index.hpp` | Main MFT parsing implementation | 500-920 |
| `UltraFastFileSearch-code/src/core/ntfs_record_types.hpp` | Record, LinkInfo, StreamInfo, ChildInfo structures | 1-129 |
| `UltraFastFileSearch-code/src/core/packed_file_size.hpp` | file_size_type (6-byte packed), SizeInfo | 1-61 |
| `UltraFastFileSearch-code/src/core/standard_info.hpp` | StandardInfo with bitfield attributes | 1-80 |
| `UltraFastFileSearch-code/src/io/mft_reader.hpp` | Async MFT reading with IOCP | 1-400 |

### 1.2 C++ Architecture Documentation

| Document | Purpose |
|----------|---------|
| `docs/architecture/04-mft-parsing.md` | **KEY**: Complete MFT parsing flow, USA fixup, attribute parsing |
| `docs/architecture/07-indexing.md` | In-memory index structures |
| `docs/architecture/01-overview.md` | System architecture overview |

### 1.3 Critical C++ Code Sections in ntfs_index.hpp

| Section | Lines | Description |
|---------|-------|-------------|
| Main parsing loop | 513-728 | Iterates MFT records, parses attributes |
| Magic number check | 518 | `frsh->MultiSectorHeader.Magic == 'ELIF'` |
| USA fixup | (implicit) | Called before parsing |
| $STANDARD_INFORMATION | 531-542 | Timestamps and file attributes |
| $FILE_NAME | 543-587 | Filename, parent reference, hard links |
| Stream attributes | 590-722 | $DATA, $INDEX_ROOT, $INDEX_ALLOCATION, $BITMAP |
| Extension records | 521 | `BaseFileRecordSegment` handling |

---

## 2. C++ Parsing Algorithm Deep Dive

### 2.1 FILE Record Structure

Every MFT record starts with a `FILE_RECORD_SEGMENT_HEADER`:

```cpp
#pragma pack(push, 1)
struct MULTI_SECTOR_HEADER {
    unsigned long Magic;           // 'FILE' = 0x454C4946 (stored as 'ELIF')
    unsigned short USAOffset;      // Offset to Update Sequence Array
    unsigned short USACount;       // Number of USA entries
};

struct FILE_RECORD_SEGMENT_HEADER {
    MULTI_SECTOR_HEADER MultiSectorHeader;
    unsigned long long LogFileSequenceNumber;
    unsigned short SequenceNumber;
    unsigned short LinkCount;
    unsigned short FirstAttributeOffset;
    unsigned short Flags;          // FRH_IN_USE (0x0001), FRH_DIRECTORY (0x0002)
    unsigned long BytesInUse;
    unsigned long BytesAllocated;
    unsigned long long BaseFileRecordSegment;  // Non-zero for extension records
    unsigned short NextAttributeNumber;
    unsigned short SegmentNumberUpper_or_Reserved;
    unsigned long SegmentNumberLower;
    
    // Navigation helpers
    ATTRIBUTE_RECORD_HEADER* begin() {
        return reinterpret_cast<ATTRIBUTE_RECORD_HEADER*>(
            reinterpret_cast<unsigned char*>(this) + this->FirstAttributeOffset);
    }
    
    void* end(size_t max_buffer_size = ~size_t()) {
        return reinterpret_cast<unsigned char*>(this) + 
            (max_buffer_size < this->BytesInUse ? max_buffer_size : this->BytesInUse);
    }
};
#pragma pack(pop)
```

**Key Constants:**
```cpp
enum FILE_RECORD_HEADER_FLAGS {
    FRH_IN_USE    = 0x0001,  // Record contains valid file/directory
    FRH_DIRECTORY = 0x0002,  // Record is a directory
};
```

### 2.2 Magic Number Byte Order

**Critical Detail**: The magic number is stored in little-endian format:

```
On disk:  46 49 4C 45  ('F' 'I' 'L' 'E')
In memory as unsigned long: 0x454C4946
When compared as multi-char literal: 'ELIF'
```

The C++ code checks `Magic == 'ELIF'` because C++ multi-character literals are stored with the first character in the least significant byte on little-endian systems.

### 2.3 Update Sequence Array (USA) Fixup

NTFS uses USA to detect torn writes. The last 2 bytes of each 512-byte sector are replaced with a check value, and the original bytes are stored in the USA.

```cpp
bool unfixup(size_t max_size) {
    unsigned short* usa = reinterpret_cast<unsigned short*>(
        reinterpret_cast<unsigned char*>(this) + this->USAOffset);
    unsigned short const usa0 = usa[0];  // Check value
    bool result = true;
    
    for (unsigned short i = 1; i < this->USACount; i++) {
        size_t const offset = i * 512 - sizeof(unsigned short);
        unsigned short* const check = reinterpret_cast<unsigned short*>(
            reinterpret_cast<unsigned char*>(this) + offset);
        
        if (offset < max_size) {
            result &= (*check == usa0);  // Verify check value
            *check = usa[i];             // Restore original bytes
        } else {
            break;
        }
    }
    return result;  // false if any sector failed verification
}
```

**Handling Fixup Failures:**
```cpp
if (frsh->MultiSectorHeader.Magic == 'ELIF') {
    if (frsh->MultiSectorHeader.unfixup(mft_record_size)) {
        // Record is valid, proceed with parsing
    } else {
        frsh->MultiSectorHeader.Magic = 'DAAB';  // Mark as 'BAAD'
        // Skip this record
    }
}
```

