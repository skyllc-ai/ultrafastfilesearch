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

---

## 3. Attribute Parsing

### 3.1 Attribute Record Header

Every attribute starts with this header:

```cpp
enum AttributeTypeCode {
    AttributeStandardInformation = 0x10,
    AttributeAttributeList       = 0x20,
    AttributeFileName            = 0x30,
    AttributeObjectId            = 0x40,
    AttributeSecurityDescriptor  = 0x50,
    AttributeVolumeName          = 0x60,
    AttributeVolumeInformation   = 0x70,
    AttributeData                = 0x80,
    AttributeIndexRoot           = 0x90,
    AttributeIndexAllocation     = 0xA0,
    AttributeBitmap              = 0xB0,
    AttributeReparsePoint        = 0xC0,
    AttributeEAInformation       = 0xD0,
    AttributeEA                  = 0xE0,
    AttributePropertySet         = 0xF0,
    AttributeLoggedUtilityStream = 0x100,
    AttributeEnd                 = 0xFFFFFFFF,  // End marker
    AttributeNone                = 0x00,        // Invalid
};

struct ATTRIBUTE_RECORD_HEADER {
    AttributeTypeCode Type;
    unsigned long Length;
    unsigned char IsNonResident;
    unsigned char NameLength;
    unsigned short NameOffset;
    unsigned short Flags;
    unsigned short Instance;

    union {
        struct RESIDENT {
            unsigned long ValueLength;
            unsigned short ValueOffset;
            unsigned short Flags;

            void* GetValue() {
                return reinterpret_cast<char*>(
                    CONTAINING_RECORD(this, ATTRIBUTE_RECORD_HEADER, Resident)
                ) + this->ValueOffset;
            }
        } Resident;

        struct NONRESIDENT {
            long long LowestVCN;
            long long HighestVCN;
            unsigned short MappingPairsOffset;
            unsigned char CompressionUnit;
            unsigned char Reserved[5];
            long long AllocatedSize;
            long long DataSize;
            long long InitializedSize;
            long long CompressedSize;
        } NonResident;
    };

    ATTRIBUTE_RECORD_HEADER* next() {
        return reinterpret_cast<ATTRIBUTE_RECORD_HEADER*>(
            reinterpret_cast<unsigned char*>(this) + this->Length);
    }

    wchar_t* name() {
        return reinterpret_cast<wchar_t*>(
            reinterpret_cast<unsigned char*>(this) + this->NameOffset);
    }
};
```

### 3.2 Attribute Iteration Loop (Lines 525-527)

```cpp
for (ntfs::ATTRIBUTE_RECORD_HEADER const* ah = frsh->begin();
     ah < frsh_end &&
     ah->Type != ntfs::AttributeTypeCode::AttributeNone &&
     ah->Type != ntfs::AttributeTypeCode::AttributeEnd;
     ah = ah->next())
{
    // Sanity check: prevent infinite loop
    if (ah->Length == 0) break;

    switch (ah->Type) {
        case AttributeStandardInformation: ...
        case AttributeFileName: ...
        case AttributeData:
        case AttributeIndexRoot:
        case AttributeIndexAllocation:
        case AttributeBitmap:
        case AttributeReparsePoint:
        case AttributeEA:
        case AttributeEAInformation:
        case AttributeObjectId:
        case AttributePropertySet:
        default: ...
    }
}
```

### 3.3 $STANDARD_INFORMATION (0x10) Parsing

```cpp
struct STANDARD_INFORMATION {
    long long CreationTime;           // 0x00: File creation time (FILETIME)
    long long LastModificationTime;   // 0x08: Last content modification
    long long LastChangeTime;         // 0x10: Last MFT record change
    long long LastAccessTime;         // 0x18: Last access time
    unsigned long FileAttributes;     // 0x20: FILE_ATTRIBUTE_* flags
    // NTFS 3.0+ additional fields (optional):
    // unsigned long MaxVersions;
    // unsigned long VersionNumber;
    // unsigned long ClassId;
    // unsigned long OwnerId;
    // unsigned long SecurityId;
    // unsigned long long QuotaCharged;
    // unsigned long long USN;
};
```

**C++ Parsing (Lines 531-542):**
```cpp
case ntfs::AttributeTypeCode::AttributeStandardInformation:
    if (ntfs::STANDARD_INFORMATION const* const fn =
        static_cast<ntfs::STANDARD_INFORMATION const*>(ah->Resident.GetValue()))
    {
        base_record->stdinfo.created  = fn->CreationTime;
        base_record->stdinfo.written  = fn->LastModificationTime;
        base_record->stdinfo.accessed = fn->LastAccessTime;
        base_record->stdinfo.attributes(fn->FileAttributes |
            ((frsh->Flags & ntfs::FRH_DIRECTORY) ? FILE_ATTRIBUTE_DIRECTORY : 0));
    }
    break;
```

**Key Detail**: The `FRH_DIRECTORY` flag from the record header is ORed into the file attributes.

### 3.4 $FILE_NAME (0x30) Parsing

```cpp
struct FILENAME_INFORMATION {
    unsigned long long ParentDirectory;  // 0x00: Parent FRS (48 bits) + sequence (16 bits)
    long long CreationTime;              // 0x08
    long long LastModificationTime;      // 0x10
    long long LastChangeTime;            // 0x18
    long long LastAccessTime;            // 0x20
    long long AllocatedLength;           // 0x28: Allocated size
    long long FileSize;                  // 0x30: Logical size
    unsigned long FileAttributes;        // 0x38: DOS attributes
    unsigned short PackedEaSize;         // 0x3C: Extended attributes size
    unsigned short Reserved;             // 0x3E
    unsigned char FileNameLength;        // 0x40: Filename length in characters
    unsigned char Flags;                 // 0x41: Namespace flags
    wchar_t FileName[1];                 // 0x42: Variable-length filename (Unicode)
};

// Filename namespace flags
enum {
    FILE_NAME_POSIX        = 0x00,  // Case-sensitive, any Unicode chars
    FILE_NAME_WIN32        = 0x01,  // Windows long filename
    FILE_NAME_DOS          = 0x02,  // 8.3 short filename only
    FILE_NAME_WIN32_AND_DOS = 0x03, // Both namespaces in one attribute
};
```

**Parent Directory Reference:**
```cpp
// Extract parent FRS from 64-bit value
unsigned long long parent_frs = fn->ParentDirectory & 0x0000FFFFFFFFFFFF;  // Lower 48 bits
unsigned short parent_seq = (fn->ParentDirectory >> 48) & 0xFFFF;          // Upper 16 bits
```

**C++ Parsing (Lines 543-587):**
```cpp
case ntfs::AttributeTypeCode::AttributeFileName:
    if (ntfs::FILENAME_INFORMATION const* const fn =
        static_cast<ntfs::FILENAME_INFORMATION const*>(ah->Resident.GetValue()))
    {
        unsigned int const frs_parent = static_cast<unsigned int>(fn->ParentDirectory);

        // Skip DOS-only names (we'll get the long name from another attribute)
        if (fn->Flags != 0x02 /* FILE_NAME_DOS */) {
            // If this record already has a name, push current to linked list
            if (LinkInfo* const si = this->nameinfo(&*base_record)) {
                size_t const link_index = this->nameinfos.size();
                this->nameinfos.push_back(base_record->first_name);
                base_record->first_name.next_entry = static_cast<LinkInfos::value_type::next_entry_type>(link_index);
            }

            // Store new name in first_name
            LinkInfo* const info = &base_record->first_name;
            info->name.offset(static_cast<unsigned int>(this->names.size()));
            info->name.length = static_cast<unsigned char>(fn->FileNameLength);
            bool const ascii = is_ascii(fn->FileName, fn->FileNameLength);
            info->name.ascii(ascii);
            info->parent = frs_parent;

            // Append filename to names buffer
            append_directional(this->names, fn->FileName, fn->FileNameLength, ascii ? 1 : 0);

            // Update parent's child list
            if (frs_parent != frs_base) {
                Records::iterator const parent = this->at(frs_parent, &base_record);
                size_t const child_index = this->childinfos.size();
                this->childinfos.push_back(empty_child_info);
                ChildInfo* const child_info = &this->childinfos.back();
                child_info->record_number = frs_base;
                child_info->name_index = base_record->name_count;  // BEFORE incrementing
                child_info->next_entry = parent->first_child;
                parent->first_child = static_cast<ChildInfos::value_type::next_entry_type>(child_index);
            }

            ++base_record->name_count;  // Increment AFTER setting name_index
        }
    }
    break;
```

**Key Details:**
1. **DOS names are skipped** (`fn->Flags == 0x02`) to avoid duplicate entries
2. **Hard links**: Each $FILE_NAME creates a new `LinkInfo` entry, linked via `next_entry`
3. **Parent-child relationship**: A `ChildInfo` is created linking parent directory to child
4. **name_index**: Set to `name_count` BEFORE incrementing (0-indexed)

### 3.5 Stream Attributes ($DATA, $INDEX_ROOT, etc.)

The C++ code handles multiple attribute types as "streams":

```cpp
case ntfs::AttributeTypeCode::AttributeObjectId:
case ntfs::AttributeTypeCode::AttributePropertySet:
case ntfs::AttributeTypeCode::AttributeBitmap:
case ntfs::AttributeTypeCode::AttributeIndexAllocation:
case ntfs::AttributeTypeCode::AttributeIndexRoot:
case ntfs::AttributeTypeCode::AttributeData:
case ntfs::AttributeTypeCode::AttributeReparsePoint:
case ntfs::AttributeTypeCode::AttributeEA:
case ntfs::AttributeTypeCode::AttributeEAInformation:
default:
{
    bool const is_primary_attribute = !(ah->IsNonResident && ah->NonResident.LowestVCN);
    if (is_primary_attribute) {
        // Check if this is a directory index ($I30)
        bool const isdir = (ah->Type == AttributeBitmap ||
                           ah->Type == AttributeIndexRoot ||
                           ah->Type == AttributeIndexAllocation) &&
                          ah->NameLength == 4 &&
                          memcmp(ah->name(), _T("$I30"), sizeof(*ah->name()) * 4) == 0;

        unsigned char const name_length = isdir ? 0 : ah->NameLength;
        unsigned char const type_name_id = isdir ? 0 : static_cast<int>(ah->Type) >> 4;

        // Create or find StreamInfo entry
        StreamInfo* info = nullptr;
        if (StreamInfo* const si = this->streaminfo(&*base_record)) {
            if (isdir) {
                // Merge with existing directory stream
                for (StreamInfo* k = si; k; k = this->streaminfo(k->next_entry)) {
                    if (k->type_name_id == type_name_id && k->name.length == name_length) {
                        info = k;
                        break;
                    }
                }
            }
            if (!info) {
                // Push existing to linked list
                size_t const stream_index = this->streaminfos.size();
                this->streaminfos.push_back(*si);
                si->next_entry = static_cast<small_t<size_t>::type>(stream_index);
            }
        }

        if (!info) {
            info = &base_record->first_stream;
            info->allocated = 0;
            info->length = 0;
            info->bulkiness = 0;
            info->treesize = 0;
            info->is_sparse = 0;
            info->type_name_id = type_name_id;
            info->name.length = name_length;

            if (isdir) {
                info->name.offset(0);  // Suppress name for $I30
            } else {
                info->name.offset(static_cast<unsigned int>(this->names.size()));
                bool const ascii = is_ascii(ah->name(), ah->NameLength);
                info->name.ascii(ascii);
                append_directional(this->names, ah->name(), ah->NameLength, ascii ? 1 : 0);
            }

            ++base_record->stream_count;
        }

        // Accumulate sizes
        bool const is_sparse = !!(ah->Flags & 0x8000);
        if (is_sparse) info->is_sparse |= 0x1;

        info->allocated += ah->IsNonResident ?
            (ah->NonResident.CompressionUnit ?
                static_cast<file_size_type>(ah->NonResident.CompressedSize) :
                static_cast<file_size_type>(ah->NonResident.AllocatedSize)) :
            0;
        info->length += ah->IsNonResident ?
            static_cast<file_size_type>(ah->NonResident.DataSize) :
            ah->Resident.ValueLength;
        info->bulkiness += info->allocated;
        info->treesize = isdir;
    }
    break;
}
```

**Key Details:**
1. **Primary vs Extension**: Only primary attributes (`LowestVCN == 0`) create new streams
2. **$I30 directories**: Index attributes named "$I30" are merged into a single directory stream
3. **type_name_id**: Attribute type shifted right by 4 bits (e.g., 0x80 → 0x08)
4. **Sparse files**: Detected via `Flags & 0x8000`
5. **Compressed files**: Use `CompressedSize` instead of `AllocatedSize`

---

## 4. C++ Data Structures

### 4.1 file_size_type (6-byte packed)

```cpp
// From packed_file_size.hpp
#pragma pack(push, 1)
struct file_size_type {
    unsigned int low;       // Lower 32 bits
    unsigned short high;    // Upper 16 bits

    file_size_type() : low(0), high(0) {}
    file_size_type(unsigned long long v) : low(static_cast<unsigned int>(v)),
                                           high(static_cast<unsigned short>(v >> 32)) {}

    operator unsigned long long() const {
        return static_cast<unsigned long long>(low) |
               (static_cast<unsigned long long>(high) << 32);
    }

    file_size_type& operator+=(file_size_type const& rhs) {
        unsigned long long sum = static_cast<unsigned long long>(*this) +
                                 static_cast<unsigned long long>(rhs);
        *this = file_size_type(sum);
        return *this;
    }
};
#pragma pack(pop)
```

**Purpose**: Saves 2 bytes per size field (6 bytes vs 8 bytes). Supports up to 256 TB.

### 4.2 SizeInfo

```cpp
#pragma pack(push, 1)
struct SizeInfo {
    file_size_type length;     // Logical file size
    file_size_type allocated;  // Allocated size on disk
    file_size_type bulkiness;  // Size including slack space
    file_size_type treesize;   // For directories: descendant count
};
#pragma pack(pop)
```

### 4.3 NameInfo

```cpp
#pragma pack(push, 1)
struct NameInfo {
    unsigned int _offset;      // Offset into names buffer (high bit = ascii flag)
    unsigned char length;      // Name length in characters

    void offset(unsigned int v) { _offset = (_offset & 0x80000000) | (v & 0x7FFFFFFF); }
    unsigned int offset() const { return _offset & 0x7FFFFFFF; }

    void ascii(bool v) { _offset = v ? (_offset | 0x80000000) : (_offset & 0x7FFFFFFF); }
    bool ascii() const { return !!(_offset & 0x80000000); }
};
#pragma pack(pop)
```

**Purpose**: Stores name offset (31 bits) and ASCII flag (1 bit) in a single 32-bit field.

### 4.4 LinkInfo

```cpp
#pragma pack(push, 1)
struct LinkInfo {
    typedef small_t<size_t>::type next_entry_type;
    next_entry_type next_entry;  // Index of next LinkInfo (for hard links)
    unsigned int parent;         // Parent directory FRS
    NameInfo name;               // Filename
};
#pragma pack(pop)
```

### 4.5 StreamInfo

```cpp
#pragma pack(push, 1)
struct StreamInfo : SizeInfo {
    typedef small_t<size_t>::type next_entry_type;
    next_entry_type next_entry;  // Index of next StreamInfo
    NameInfo name;               // Stream name (empty for default $DATA)
    unsigned char is_sparse : 1;
    unsigned char is_allocated_size_accounted_for_in_main_stream : 1;
    unsigned char type_name_id : 6;  // Attribute type >> 4 (0 for $I30)
};
#pragma pack(pop)
```

### 4.6 ChildInfo

```cpp
#pragma pack(push, 1)
struct ChildInfo {
    typedef small_t<size_t>::type next_entry_type;
    next_entry_type next_entry;                    // Next child in linked list
    small_t<size_t>::type record_number;           // FRS of child
    unsigned short name_index;                     // Which hardlink (0-indexed)

    ChildInfo() : next_entry(negative_one), record_number(negative_one), name_index(negative_one) {}
};
#pragma pack(pop)
```

### 4.7 Record

```cpp
#pragma pack(push, 1)
struct Record {
    StandardInfo stdinfo;                    // Timestamps and attributes
    unsigned short name_count;               // Number of hard links (≤1024)
    unsigned short stream_count;             // Number of data streams
    ChildInfo::next_entry_type first_child;  // Index of first child (directories)
    LinkInfo first_name;                     // First/primary filename
    StreamInfo first_stream;                 // First/primary data stream

    Record() : stdinfo(), name_count(0), stream_count(0), first_child(negative_one),
               first_name(), first_stream() {
        this->first_stream.name.offset(negative_one);
        this->first_stream.next_entry = negative_one;
    }
};
#pragma pack(pop)
```

### 4.8 StandardInfo (Bitfield Attributes)

```cpp
#pragma pack(push, 1)
struct StandardInfo {
    unsigned long long created;
    unsigned long long written;
    unsigned long long accessed           : 58;  // 0x40 - 6 = 58 bits
    unsigned long long is_readonly        : 1;
    unsigned long long is_archive         : 1;
    unsigned long long is_system          : 1;
    unsigned long long is_hidden          : 1;
    unsigned long long is_offline         : 1;
    unsigned long long is_notcontentidx   : 1;
    unsigned long long is_noscrubdata     : 1;
    unsigned long long is_integretystream : 1;
    unsigned long long is_pinned          : 1;
    unsigned long long is_unpinned        : 1;
    unsigned long long is_directory       : 1;
    unsigned long long is_compressed      : 1;
    unsigned long long is_encrypted       : 1;
    unsigned long long is_sparsefile      : 1;
    unsigned long long is_reparsepoint    : 1;

    unsigned long attributes() const noexcept {
        return (is_readonly     ? FILE_ATTRIBUTE_READONLY            : 0U) |
               (is_archive      ? FILE_ATTRIBUTE_ARCHIVE             : 0U) |
               (is_system       ? FILE_ATTRIBUTE_SYSTEM              : 0U) |
               (is_hidden       ? FILE_ATTRIBUTE_HIDDEN              : 0U) |
               // ... etc
               (is_directory    ? FILE_ATTRIBUTE_DIRECTORY           : 0U);
    }

    void attributes(unsigned long value) noexcept {
        is_readonly   = !!(value & FILE_ATTRIBUTE_READONLY);
        is_archive    = !!(value & FILE_ATTRIBUTE_ARCHIVE);
        // ... etc
    }
};
#pragma pack(pop)
```

---

## 5. Complete Parsing Flow

### 5.1 Main Parsing Loop (Lines 513-728)

```cpp
void NtfsIndex::load(void* buffer, size_t size, unsigned long long virtual_offset,
                     unsigned long long skipped_begin, unsigned long long skipped_end) {
    size_t const mft_record_size = 1U << mft_record_size_log2;  // Usually 1024

    for (size_t i = 0; i + mft_record_size <= size; i += mft_record_size) {
        unsigned int const frs = static_cast<unsigned int>((virtual_offset + i) >> mft_record_size_log2);
        ntfs::FILE_RECORD_SEGMENT_HEADER* const frsh =
            reinterpret_cast<ntfs::FILE_RECORD_SEGMENT_HEADER*>(&static_cast<unsigned char*>(buffer)[i]);

        // Step 1: Check magic number and in-use flag
        if (frsh->MultiSectorHeader.Magic == 'ELIF' && !!(frsh->Flags & ntfs::FRH_IN_USE)) {

            // Step 2: Determine base record (for extension records)
            unsigned int const frs_base = frsh->BaseFileRecordSegment ?
                static_cast<unsigned int>(frsh->BaseFileRecordSegment) : frs;
            auto base_record = this->at(frs_base);

            // Step 3: Get record boundaries
            void const* const frsh_end = frsh->end(mft_record_size);

            // Step 4: Iterate all attributes
            for (ntfs::ATTRIBUTE_RECORD_HEADER const* ah = frsh->begin();
                 ah < frsh_end &&
                 ah->Type != ntfs::AttributeTypeCode::AttributeNone &&
                 ah->Type != ntfs::AttributeTypeCode::AttributeEnd;
                 ah = ah->next())
            {
                switch (ah->Type) {
                    case AttributeStandardInformation: /* ... */ break;
                    case AttributeFileName: /* ... */ break;
                    case AttributeData:
                    case AttributeIndexRoot:
                    case AttributeIndexAllocation:
                    case AttributeBitmap:
                    case AttributeReparsePoint:
                    case AttributeEA:
                    case AttributeEAInformation:
                    case AttributeObjectId:
                    case AttributePropertySet:
                    default: /* stream handling */ break;
                }
            }
        }
    }
}
```

### 5.2 Extension Record Handling

```cpp
// When BaseFileRecordSegment is non-zero, this is an extension record
unsigned int const frs_base = frsh->BaseFileRecordSegment ?
    static_cast<unsigned int>(frsh->BaseFileRecordSegment) : frs;

// All attributes from extension records are added to the base record
auto base_record = this->at(frs_base);
```

**When Extension Records Are Needed:**
- Files with many hard links (each link = one $FILE_NAME)
- Files with many alternate data streams
- Highly fragmented files (long data run lists)

### 5.3 ASCII Optimization

```cpp
bool is_ascii(wchar_t const* str, size_t len) {
    for (size_t i = 0; i < len; i++) {
        if (str[i] > 127) return false;
    }
    return true;
}

void append_directional(std::vector<unsigned char>& names,
                        wchar_t const* str, size_t len, int direction) {
    if (direction == 1) {
        // ASCII: store as single bytes (50% memory savings)
        for (size_t i = 0; i < len; i++) {
            names.push_back(static_cast<unsigned char>(str[i]));
        }
    } else {
        // Unicode: store as wchar_t (2 bytes per character)
        unsigned char const* bytes = reinterpret_cast<unsigned char const*>(str);
        names.insert(names.end(), bytes, bytes + len * sizeof(wchar_t));
    }
}
```

---

## 6. Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         MFT Parsing Data Flow                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Raw MFT Bytes (from disk)                                                   │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │ 46 49 4C 45 ... (1024 bytes per record × N records)                    │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                    │                                         │
│                                    ▼                                         │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │ For each record:                                                        │ │
│  │   1. Check Magic == 'ELIF' && Flags & FRH_IN_USE                       │ │
│  │   2. Apply USA fixup (verify sector boundaries)                         │ │
│  │   3. Determine base record (handle extension records)                   │ │
│  │   4. Iterate attributes                                                 │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                    │                                         │
│                    ┌───────────────┼───────────────┐                        │
│                    ▼               ▼               ▼                        │
│  ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐            │
│  │ $STANDARD_INFO   │ │ $FILE_NAME       │ │ $DATA/streams    │            │
│  │ (0x10)           │ │ (0x30)           │ │ (0x80, etc.)     │            │
│  ├──────────────────┤ ├──────────────────┤ ├──────────────────┤            │
│  │ → stdinfo.created│ │ → LinkInfo       │ │ → StreamInfo     │            │
│  │ → stdinfo.written│ │ → ChildInfo      │ │ → names buffer   │            │
│  │ → stdinfo.access │ │ → names buffer   │ │ → stream_count   │            │
│  │ → attributes     │ │ → name_count     │ │                  │            │
│  └──────────────────┘ └──────────────────┘ └──────────────────┘            │
│                                    │                                         │
│                                    ▼                                         │
│  ┌────────────────────────────────────────────────────────────────────────┐ │
│  │ In-Memory Index Structures:                                             │ │
│  │   • records: Vec<Record>        - One per FRS                          │ │
│  │   • nameinfos: Vec<LinkInfo>    - Additional hard links                │ │
│  │   • streaminfos: Vec<StreamInfo> - Additional streams                  │ │
│  │   • childinfos: Vec<ChildInfo>  - Parent-child relationships           │ │
│  │   • names: Vec<u8>              - All filenames and stream names       │ │
│  └────────────────────────────────────────────────────────────────────────┘ │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Implementation Plan

### 7.1 Transformer Approach

Similar to the tree algorithm port, we use a **transformer** approach:

1. **Transform**: Convert existing Rust parsing output to C++ port structures
2. **Run**: Execute C++ parsing algorithm on raw MFT bytes
3. **Write Back**: Store results in existing Rust structures

This allows A/B testing between algorithms without modifying existing code.

### 7.2 Phase 1: Core Structures (cpp_parse.rs)

Create Rust equivalents of C++ structures:

```rust
// Sentinel value for end of linked list
pub const CPP_NO_ENTRY: u32 = u32::MAX;

/// C++ file_size_type equivalent (6-byte packed)
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CppFileSize {
    pub low: u32,
    pub high: u16,
}

/// C++ NameInfo equivalent
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CppNameInfo {
    offset_and_ascii: u32,  // High bit = ascii flag
    pub length: u8,
}

/// C++ SizeInfo equivalent
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CppSizeInfo {
    pub length: CppFileSize,
    pub allocated: CppFileSize,
    pub bulkiness: CppFileSize,
    pub treesize: CppFileSize,
}

/// C++ LinkInfo equivalent
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CppLinkInfo {
    pub next_entry: u32,
    pub parent: u32,
    pub name: CppNameInfo,
}

/// C++ StreamInfo equivalent
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct CppStreamInfo {
    pub size: CppSizeInfo,
    pub next_entry: u32,
    pub name: CppNameInfo,
    pub flags: u8,  // is_sparse:1, is_allocated_accounted:1, type_name_id:6
}

/// C++ ChildInfo equivalent
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct CppChildInfo {
    pub next_entry: u32,
    pub record_number: u32,
    pub name_index: u16,
}

/// C++ Record equivalent
pub struct CppRecord {
    pub stdinfo: CppStandardInfo,
    pub name_count: u16,
    pub stream_count: u16,
    pub first_child: u32,
    pub first_name: CppLinkInfo,
    pub first_stream: CppStreamInfo,
}
```

### 7.3 Phase 2: NTFS Structures

Create Rust equivalents of NTFS on-disk structures:

```rust
/// FILE_RECORD_SEGMENT_HEADER
#[repr(C, packed)]
pub struct FileRecordHeader {
    pub magic: u32,                    // 'FILE' = 0x454C4946
    pub usa_offset: u16,
    pub usa_count: u16,
    pub lsn: u64,
    pub sequence_number: u16,
    pub link_count: u16,
    pub first_attribute_offset: u16,
    pub flags: u16,                    // FRH_IN_USE | FRH_DIRECTORY
    pub bytes_in_use: u32,
    pub bytes_allocated: u32,
    pub base_record_segment: u64,
    pub next_attribute_number: u16,
    pub segment_number_upper: u16,
    pub segment_number_lower: u32,
}

/// ATTRIBUTE_RECORD_HEADER
#[repr(C, packed)]
pub struct AttributeHeader {
    pub type_code: u32,
    pub length: u32,
    pub is_non_resident: u8,
    pub name_length: u8,
    pub name_offset: u16,
    pub flags: u16,
    pub instance: u16,
    // Union follows (resident or non-resident data)
}

/// STANDARD_INFORMATION
#[repr(C, packed)]
pub struct StandardInformation {
    pub creation_time: i64,
    pub modification_time: i64,
    pub mft_change_time: i64,
    pub access_time: i64,
    pub file_attributes: u32,
}

/// FILENAME_INFORMATION
#[repr(C, packed)]
pub struct FilenameInformation {
    pub parent_directory: u64,
    pub creation_time: i64,
    pub modification_time: i64,
    pub mft_change_time: i64,
    pub access_time: i64,
    pub allocated_length: i64,
    pub file_size: i64,
    pub file_attributes: u32,
    pub packed_ea_size: u16,
    pub reserved: u16,
    pub filename_length: u8,
    pub flags: u8,
    // filename: [u16; filename_length] follows
}
```

### 7.4 Phase 3: Parsing Functions

```rust
/// Apply USA fixup to a record
pub fn unfixup(data: &mut [u8], usa_offset: usize, usa_count: usize) -> bool {
    // Implementation matching C++ unfixup()
}

/// Parse a single MFT record
pub fn parse_record_cpp(
    data: &[u8],
    frs: u32,
    records: &mut Vec<CppRecord>,
    nameinfos: &mut Vec<CppLinkInfo>,
    streaminfos: &mut Vec<CppStreamInfo>,
    childinfos: &mut Vec<CppChildInfo>,
    names: &mut Vec<u8>,
) -> Result<(), ParseError> {
    // Implementation matching C++ load()
}

/// Parse $STANDARD_INFORMATION attribute
fn parse_standard_info_cpp(
    attr_data: &[u8],
    record: &mut CppRecord,
    is_directory: bool,
) {
    // Implementation matching C++ case AttributeStandardInformation
}

/// Parse $FILE_NAME attribute
fn parse_filename_cpp(
    attr_data: &[u8],
    frs: u32,
    frs_base: u32,
    record: &mut CppRecord,
    nameinfos: &mut Vec<CppLinkInfo>,
    childinfos: &mut Vec<CppChildInfo>,
    names: &mut Vec<u8>,
    parent_records: &mut Vec<CppRecord>,
) {
    // Implementation matching C++ case AttributeFileName
}

/// Parse stream attributes ($DATA, $INDEX_ROOT, etc.)
fn parse_stream_cpp(
    attr_data: &[u8],
    attr_type: u32,
    record: &mut CppRecord,
    streaminfos: &mut Vec<CppStreamInfo>,
    names: &mut Vec<u8>,
) {
    // Implementation matching C++ default case
}
```

### 7.5 Phase 4: Integration

```rust
/// Main entry point for C++ parsing algorithm
pub fn parse_mft_cpp_port(
    mft_data: &[u8],
    record_size: usize,
) -> CppParseResult {
    let num_records = mft_data.len() / record_size;

    let mut records = Vec::with_capacity(num_records);
    let mut nameinfos = Vec::new();
    let mut streaminfos = Vec::new();
    let mut childinfos = Vec::new();
    let mut names = Vec::new();

    for frs in 0..num_records {
        let offset = frs * record_size;
        let record_data = &mft_data[offset..offset + record_size];

        parse_record_cpp(
            record_data,
            frs as u32,
            &mut records,
            &mut nameinfos,
            &mut streaminfos,
            &mut childinfos,
            &mut names,
        )?;
    }

    Ok(CppParseResult {
        records,
        nameinfos,
        streaminfos,
        childinfos,
        names,
    })
}
```

---

## 8. Unit Tests

### 8.1 USA Fixup Tests

```rust
#[test]
fn test_unfixup_valid_record() {
    // Create a mock 1024-byte record with valid USA
    let mut data = vec![0u8; 1024];
    // Set up USA at offset 0x30
    // usa[0] = check value, usa[1] = original sector 1, usa[2] = original sector 2
    // Place check value at offsets 510 and 1022
    assert!(unfixup(&mut data, 0x30, 3));
}

#[test]
fn test_unfixup_torn_write() {
    // Create a record with mismatched check values (simulates torn write)
    let mut data = vec![0u8; 1024];
    // Set up USA with wrong check value at sector boundary
    assert!(!unfixup(&mut data, 0x30, 3));
}
```

### 8.2 Attribute Parsing Tests

```rust
#[test]
fn test_parse_standard_info() {
    // Create mock $STANDARD_INFORMATION attribute
    let attr_data = create_standard_info_attribute(
        0x01D1234567890000,  // created
        0x01D1234567890001,  // modified
        0x01D1234567890002,  // accessed
        FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_READONLY,
    );

    let mut record = CppRecord::default();
    parse_standard_info_cpp(&attr_data, &mut record, false);

    assert_eq!(record.stdinfo.created, 0x01D1234567890000);
    assert!(record.stdinfo.is_readonly);
    assert!(record.stdinfo.is_archive);
}

#[test]
fn test_parse_filename_skip_dos() {
    // Create mock $FILE_NAME with DOS namespace (should be skipped)
    let attr_data = create_filename_attribute(
        5,                    // parent FRS
        "MYDOCU~1.TXT",       // DOS name
        FILE_NAME_DOS,        // namespace = 0x02
    );

    let mut record = CppRecord::default();
    let initial_name_count = record.name_count;

    parse_filename_cpp(&attr_data, 100, 100, &mut record, ...);

    // Name count should not increase (DOS name skipped)
    assert_eq!(record.name_count, initial_name_count);
}

#[test]
fn test_parse_filename_hardlink() {
    // Create two $FILE_NAME attributes for same file (hard link)
    let mut record = CppRecord::default();
    let mut nameinfos = Vec::new();

    // First name
    parse_filename_cpp(&create_filename_attribute(5, "file.txt", FILE_NAME_WIN32), ...);
    assert_eq!(record.name_count, 1);

    // Second name (hard link in different directory)
    parse_filename_cpp(&create_filename_attribute(10, "link.txt", FILE_NAME_WIN32), ...);
    assert_eq!(record.name_count, 2);
    assert_eq!(nameinfos.len(), 1);  // First name pushed to nameinfos
}
```

### 8.3 Stream Parsing Tests

```rust
#[test]
fn test_parse_data_stream_resident() {
    // Create mock resident $DATA attribute
    let attr_data = create_data_attribute_resident(b"Hello, World!");

    let mut record = CppRecord::default();
    parse_stream_cpp(&attr_data, 0x80, &mut record, ...);

    assert_eq!(record.stream_count, 1);
    assert_eq!(u64::from(record.first_stream.size.length), 13);
}

#[test]
fn test_parse_data_stream_nonresident() {
    // Create mock non-resident $DATA attribute
    let attr_data = create_data_attribute_nonresident(
        1_000_000,    // data size
        1_048_576,    // allocated size (1 MB)
    );

    let mut record = CppRecord::default();
    parse_stream_cpp(&attr_data, 0x80, &mut record, ...);

    assert_eq!(record.stream_count, 1);
    assert_eq!(u64::from(record.first_stream.size.length), 1_000_000);
    assert_eq!(u64::from(record.first_stream.size.allocated), 1_048_576);
}

#[test]
fn test_parse_directory_index_merge() {
    // $INDEX_ROOT and $INDEX_ALLOCATION for same $I30 should merge
    let mut record = CppRecord::default();

    parse_stream_cpp(&create_index_root_attribute("$I30"), 0x90, &mut record, ...);
    assert_eq!(record.stream_count, 1);

    parse_stream_cpp(&create_index_allocation_attribute("$I30"), 0xA0, &mut record, ...);
    assert_eq!(record.stream_count, 1);  // Still 1 (merged)
}

#[test]
fn test_parse_alternate_data_stream() {
    // Named $DATA attribute (ADS)
    let attr_data = create_named_data_attribute("Zone.Identifier", b"[ZoneTransfer]\r\nZoneId=3");

    let mut record = CppRecord::default();
    let mut streaminfos = Vec::new();
    let mut names = Vec::new();

    // First: default stream
    parse_stream_cpp(&create_data_attribute_resident(b"content"), 0x80, &mut record, ...);

    // Second: ADS
    parse_stream_cpp(&attr_data, 0x80, &mut record, &mut streaminfos, &mut names);

    assert_eq!(record.stream_count, 2);
    assert_eq!(streaminfos.len(), 1);  // First stream pushed to streaminfos
}
```

### 8.4 Extension Record Tests

```rust
#[test]
fn test_extension_record_attributes_go_to_base() {
    // Extension record's attributes should be added to base record
    let mut records = vec![CppRecord::default(); 100];

    // Parse extension record (FRS 50) with BaseFileRecordSegment = 10
    let extension_data = create_extension_record(50, 10);
    parse_record_cpp(&extension_data, 50, &mut records, ...);

    // Attributes should be in base record (FRS 10), not extension (FRS 50)
    assert!(records[10].stream_count > 0 || records[10].name_count > 0);
}
```

---

## 9. Performance Benchmarking

### 9.1 Benchmark Setup

```rust
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_parse_mft(c: &mut Criterion) {
    let mft_data = load_test_mft();  // Load real MFT data
    let record_size = 1024;

    let mut group = c.benchmark_group("mft_parsing");

    group.bench_function("current_rust", |b| {
        b.iter(|| parse_mft_current(&mft_data, record_size))
    });

    group.bench_function("cpp_port", |b| {
        b.iter(|| parse_mft_cpp_port(&mft_data, record_size))
    });

    group.finish();
}
```

### 9.2 Metrics to Compare

| Metric | Description |
|--------|-------------|
| Parse time | Total time to parse all MFT records |
| Memory usage | Peak memory during parsing |
| Records/second | Throughput |
| Attribute parse time | Time per attribute type |

### 9.3 Expected Results

The C++ port should match or exceed current Rust performance because:
1. **Packed structures**: Reduced memory footprint
2. **Linked lists**: No vector reallocation during parsing
3. **ASCII optimization**: 50% memory savings for typical filenames
4. **Single-pass parsing**: No intermediate allocations

---

## 10. Verification

### 10.1 Parity Testing

Compare C++ port output against current Rust implementation:

```rust
#[test]
fn test_parity_with_current_implementation() {
    let mft_data = load_test_mft();

    let rust_result = parse_mft_current(&mft_data, 1024);
    let cpp_result = parse_mft_cpp_port(&mft_data, 1024);

    // Compare record counts
    assert_eq!(rust_result.records.len(), cpp_result.records.len());

    // Compare each record
    for (frs, (rust_rec, cpp_rec)) in rust_result.records.iter()
        .zip(cpp_result.records.iter())
        .enumerate()
    {
        assert_eq!(rust_rec.name_count, cpp_rec.name_count, "FRS {}: name_count mismatch", frs);
        assert_eq!(rust_rec.stream_count, cpp_rec.stream_count, "FRS {}: stream_count mismatch", frs);
        // ... more comparisons
    }
}
```

### 10.2 Edge Cases to Test

| Case | Description |
|------|-------------|
| Empty MFT | Zero records |
| Deleted records | `FRH_IN_USE` not set |
| Corrupted records | Bad magic number, USA fixup failure |
| Extension records | `BaseFileRecordSegment` non-zero |
| Maximum hard links | 1024 $FILE_NAME attributes |
| Maximum streams | Many ADS |
| Very long filenames | 255 characters |
| Unicode filenames | Non-ASCII characters |
| Sparse files | `Flags & 0x8000` |
| Compressed files | `CompressionUnit` non-zero |

---

## 11. Summary

### 11.1 Key Differences from Current Rust Implementation

| Aspect | Current Rust | C++ Port |
|--------|--------------|----------|
| Structures | Rust-native types | Packed C++ equivalents |
| Linked lists | `Vec` with indices | True linked list pattern |
| Name storage | `String` | Byte buffer with offset/length |
| Size types | `u64` | 6-byte packed `file_size_type` |
| Attributes | Bitflags crate | Bitfield struct |

### 11.2 Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/uffs-mft/src/cpp_parse.rs` | Create | C++ port structures and parsing functions |
| `crates/uffs-mft/src/lib.rs` | Modify | Export `cpp_parse` module |
| `crates/uffs-mft/src/index.rs` | Modify | Integrate `ParseAlgorithm::CppPort` |

### 11.3 Success Criteria

1. ✅ All unit tests pass
2. ✅ Parity with current Rust implementation (same output)
3. ✅ Performance equal or better than current implementation
4. ✅ No memory safety issues (no unsafe code without justification)
5. ✅ Clean clippy output

