# Windows Volume I/O APIs

## Introduction

This document describes the Windows APIs used for direct NTFS volume access, MFT metadata retrieval, and high-performance asynchronous I/O. All information is based on publicly available Microsoft SDK documentation.

---

## Direct Volume Access

### Opening a Volume

To read the MFT directly, the volume must be opened as a raw device:

```
Path format: \\.\X:    (where X is the drive letter)

Required access:  GENERIC_READ
Share mode:       FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
Creation:         OPEN_EXISTING
Flags:            FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED
```

**`FILE_FLAG_NO_BUFFERING`**: Bypasses the file system cache. Requires:
- Buffer addresses aligned to sector size (512 bytes)
- Read sizes aligned to sector size
- File offsets aligned to sector size

**`FILE_FLAG_OVERLAPPED`**: Enables asynchronous I/O. Without this flag, all `ReadFile` calls block until completion.

**Privilege requirement**: Opening `\\.\X:` for `GENERIC_READ` requires **Administrator privileges** or the `SE_BACKUP_PRIVILEGE` token privilege.

### Reference

- [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
- [File Buffering](https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering)

---

## NTFS Volume Metadata

### FSCTL_GET_NTFS_VOLUME_DATA

Returns comprehensive NTFS-specific volume information.

```
DeviceIoControl(
    hVolume,                          // Volume handle
    FSCTL_GET_NTFS_VOLUME_DATA,       // Control code
    NULL, 0,                          // No input buffer
    &volumeData, sizeof(volumeData),  // Output buffer
    &bytesReturned, NULL
)
```

**Output: `NTFS_VOLUME_DATA_BUFFER`**

| Field | Type | Description |
|-------|------|-------------|
| `VolumeSerialNumber` | LARGE_INTEGER | Unique volume serial number |
| `NumberSectors` | LARGE_INTEGER | Total sectors on volume |
| `TotalClusters` | LARGE_INTEGER | Total clusters |
| `FreeClusters` | LARGE_INTEGER | Free clusters |
| `TotalReserved` | LARGE_INTEGER | Reserved clusters |
| `BytesPerSector` | DWORD | Sector size (usually 512) |
| `BytesPerCluster` | DWORD | Cluster size |
| `BytesPerFileRecordSegment` | DWORD | MFT record size (usually 1024) |
| `ClustersPerFileRecordSegment` | DWORD | Clusters per MFT record |
| `MftValidDataLength` | LARGE_INTEGER | Used size of $MFT |
| `MftStartLcn` | LARGE_INTEGER | Starting LCN of $MFT |
| `Mft2StartLcn` | LARGE_INTEGER | Starting LCN of $MFTMirr |
| `MftZoneStart` | LARGE_INTEGER | Start of MFT reserved zone |
| `MftZoneEnd` | LARGE_INTEGER | End of MFT reserved zone |

**Key derived values:**
```
mft_capacity = MftValidDataLength / BytesPerFileRecordSegment
records_per_cluster = BytesPerCluster / BytesPerFileRecordSegment
```

### Reference

- [FSCTL_GET_NTFS_VOLUME_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data)
- [NTFS_VOLUME_DATA_BUFFER](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-ntfs_volume_data_buffer)

---

## MFT Extent Retrieval

### FSCTL_GET_RETRIEVAL_POINTERS

Returns the physical disk locations (extents) of a file's data. Used to map the MFT's virtual cluster layout to physical disk offsets.

```
STARTING_VCN_INPUT_BUFFER input = { .StartingVcn = 0 };
RETRIEVAL_POINTERS_BUFFER output;

DeviceIoControl(
    hFile,                              // Handle to $MFT or $MFT::$BITMAP
    FSCTL_GET_RETRIEVAL_POINTERS,
    &input, sizeof(input),
    &output, outputBufferSize,
    &bytesReturned, NULL
)
```

**Input: `STARTING_VCN_INPUT_BUFFER`**

| Field | Type | Description |
|-------|------|-------------|
| `StartingVcn` | LARGE_INTEGER | VCN to start enumeration from |

**Output: `RETRIEVAL_POINTERS_BUFFER`**

| Field | Type | Description |
|-------|------|-------------|
| `ExtentCount` | DWORD | Number of extents returned |
| `StartingVcn` | LARGE_INTEGER | Actual starting VCN |
| `Extents[]` | Array | One entry per extent |

Each extent entry:

| Field | Type | Description |
|-------|------|-------------|
| `NextVcn` | LARGE_INTEGER | VCN just past the end of this extent |
| `Lcn` | LARGE_INTEGER | Physical cluster number (-1 = sparse/unallocated) |

**Computing extent details:**
```
extent[i].start_vcn = (i == 0) ? StartingVcn : extent[i-1].NextVcn
extent[i].cluster_count = extent[i].NextVcn - extent[i].start_vcn
extent[i].disk_offset = extent[i].Lcn * BytesPerCluster
```

### Opening $MFT for Retrieval Pointers

```
Path: "X:\$MFT"          — for MFT data extents
Path: "X:\$MFT::$BITMAP" — for MFT bitmap extents (use $BITMAP stream name)
```

These paths require `SE_BACKUP_PRIVILEGE` or Administrator access.

### Reference

- [FSCTL_GET_RETRIEVAL_POINTERS](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_retrieval_pointers)
- [RETRIEVAL_POINTERS_BUFFER](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-retrieval_pointers_buffer)

---

## I/O Completion Ports (IOCP)

I/O Completion Ports provide a high-performance mechanism for managing multiple asynchronous I/O operations. They are the recommended pattern for high-throughput disk I/O on Windows.

### Creating an IOCP

```
HANDLE hIocp = CreateIoCompletionPort(
    INVALID_HANDLE_VALUE,  // No file handle yet
    NULL,                  // Create new IOCP
    0,                     // Completion key
    0                      // Concurrent threads (0 = CPU count)
);
```

### Associating a File Handle

```
CreateIoCompletionPort(
    hVolume,   // Volume handle (opened with FILE_FLAG_OVERLAPPED)
    hIocp,     // Existing IOCP
    key,       // Completion key (user-defined, identifies the volume)
    0          // Ignored when associating
);
```

### Issuing Asynchronous Reads

```
OVERLAPPED overlapped = { 0 };
overlapped.Offset = (DWORD)(diskOffset);
overlapped.OffsetHigh = (DWORD)(diskOffset >> 32);

BOOL ok = ReadFile(
    hVolume,       // Volume handle
    buffer,        // Aligned buffer
    readSize,      // Must be sector-aligned
    NULL,          // Bytes read (ignored for async)
    &overlapped    // OVERLAPPED structure
);

// Returns FALSE with GetLastError() == ERROR_IO_PENDING → read is in progress
// Completion will be posted to the IOCP
```

### Receiving Completions

```
DWORD bytesTransferred;
ULONG_PTR completionKey;
OVERLAPPED* pOverlapped;

BOOL ok = GetQueuedCompletionStatus(
    hIocp,              // IOCP handle
    &bytesTransferred,  // Bytes read
    &completionKey,     // Identifies which volume
    &pOverlapped,       // The OVERLAPPED from ReadFile
    INFINITE            // Wait timeout (ms)
);
```

### Sliding Window Pattern

The high-performance pattern for sequential reads:

```
1. Issue N initial ReadFile operations (the "window")
2. Loop:
   a. GetQueuedCompletionStatus → wait for any read to complete
   b. Process the completed buffer (parse MFT records)
   c. Issue the next ReadFile (maintain N reads in flight)
3. Drain remaining completions after all reads issued
```

The window size (N) controls the trade-off between latency hiding and memory usage:
- **NVMe**: N=16–32 (deep queue to saturate device)
- **SSD**: N=4–8 (moderate parallelism)
- **HDD**: N=2–4 (too many concurrent reads cause seeks)

### OVERLAPPED Structure

```
typedef struct _OVERLAPPED {
    ULONG_PTR Internal;       // Status (set by system)
    ULONG_PTR InternalHigh;   // Bytes transferred (set by system)
    union {
        struct {
            DWORD Offset;     // Low 32 bits of file offset
            DWORD OffsetHigh; // High 32 bits of file offset
        };
        PVOID Pointer;
    };
    HANDLE hEvent;            // Optional event (NULL for IOCP)
} OVERLAPPED;
```

**Critical**: The `OVERLAPPED` structure must remain valid and **pinned in memory** until the I/O operation completes. Moving it would invalidate the kernel's pointer.

### Reference

- [I/O Completion Ports](https://learn.microsoft.com/en-us/windows/win32/fileio/i-o-completion-ports)
- [CreateIoCompletionPort](https://learn.microsoft.com/en-us/windows/win32/fileio/createiocompletionport)
- [GetQueuedCompletionStatus](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatus)
- [ReadFile (Overlapped)](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile)

---

## Privilege Management

### Checking Elevation

```
BOOL IsElevated() {
    HANDLE hToken;
    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &hToken);

    TOKEN_ELEVATION elevation;
    DWORD size;
    GetTokenInformation(hToken, TokenElevation, &elevation, sizeof(elevation), &size);

    CloseHandle(hToken);
    return elevation.TokenIsElevated;
}
```

### SE_BACKUP_PRIVILEGE

An alternative to running as full Administrator. Allows reading any file regardless of ACLs:

```
1. OpenProcessToken with TOKEN_ADJUST_PRIVILEGES
2. LookupPrivilegeValue for "SeBackupPrivilege"
3. AdjustTokenPrivileges to enable it
```

### Reference

- [Privilege Constants](https://learn.microsoft.com/en-us/windows/win32/secauthz/privilege-constants)

---

## Drive Type Detection

### WMI Query

Drive type (NVMe, SSD, HDD) can be detected via WMI:

```
Query: SELECT MediaType FROM MSFT_PhysicalDisk
Namespace: root\Microsoft\Windows\Storage
```

| MediaType | Meaning |
|-----------|---------|
| 0 | Unspecified |
| 3 | HDD |
| 4 | SSD |
| 5 | SCM (Storage Class Memory) |

NVMe detection requires additional checks on the bus type:

```
Query: SELECT BusType FROM MSFT_PhysicalDisk
```

| BusType | Meaning |
|---------|---------|
| 17 | NVMe |
| 11 | SATA |
| 10 | SAS |

### Reference

- [MSFT_PhysicalDisk](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/msft-physicaldisk)

---

## USN Journal APIs

### FSCTL_QUERY_USN_JOURNAL

Returns journal metadata:

```
USN_JOURNAL_DATA_V1 journalData;
DeviceIoControl(hVolume, FSCTL_QUERY_USN_JOURNAL, NULL, 0,
                &journalData, sizeof(journalData), &bytesReturned, NULL);
```

| Field | Description |
|-------|-------------|
| `UsnJournalID` | Unique journal identifier |
| `FirstUsn` | Earliest USN still available |
| `NextUsn` | Next USN to be assigned |
| `MaximumSize` | Maximum journal size |
| `AllocationDelta` | Growth increment |

### FSCTL_READ_USN_JOURNAL

Reads change records from the journal:

```
READ_USN_JOURNAL_DATA_V1 readData = {
    .StartUsn = lastKnownUsn,
    .ReasonMask = 0xFFFFFFFF,  // All change reasons
    .UsnJournalID = journalData.UsnJournalID,
};

DeviceIoControl(hVolume, FSCTL_READ_USN_JOURNAL, &readData, sizeof(readData),
                outputBuffer, bufferSize, &bytesReturned, NULL);
```

### USN_RECORD_V3 Fields

| Field | Description |
|-------|-------------|
| `FileReferenceNumber` | File reference (FRS + sequence) |
| `ParentFileReferenceNumber` | Parent directory reference |
| `Usn` | Update Sequence Number |
| `TimeStamp` | Time of change (FILETIME) |
| `Reason` | Change reason flags |
| `FileName` | Filename at time of change |
| `FileNameLength` | Length of filename |

### Change Reason Flags

| Flag | Value | Meaning |
|------|-------|---------|
| `USN_REASON_DATA_OVERWRITE` | 0x00000001 | File data modified |
| `USN_REASON_DATA_EXTEND` | 0x00000002 | File size increased |
| `USN_REASON_DATA_TRUNCATION` | 0x00000004 | File size decreased |
| `USN_REASON_NAMED_DATA_OVERWRITE` | 0x00000010 | ADS modified |
| `USN_REASON_NAMED_DATA_EXTEND` | 0x00000020 | ADS size increased |
| `USN_REASON_NAMED_DATA_TRUNCATION` | 0x00000040 | ADS size decreased |
| `USN_REASON_FILE_CREATE` | 0x00000100 | File created |
| `USN_REASON_FILE_DELETE` | 0x00000200 | File deleted |
| `USN_REASON_EA_CHANGE` | 0x00000400 | Extended attributes changed |
| `USN_REASON_SECURITY_CHANGE` | 0x00000800 | Security descriptor changed |
| `USN_REASON_RENAME_OLD_NAME` | 0x00001000 | Rename (old name) |
| `USN_REASON_RENAME_NEW_NAME` | 0x00002000 | Rename (new name) |
| `USN_REASON_INDEXABLE_CHANGE` | 0x00004000 | Indexing attribute changed |
| `USN_REASON_BASIC_INFO_CHANGE` | 0x00008000 | Timestamps/attributes changed |
| `USN_REASON_HARD_LINK_CHANGE` | 0x00010000 | Hard link created/deleted |
| `USN_REASON_COMPRESSION_CHANGE` | 0x00020000 | Compression state changed |
| `USN_REASON_ENCRYPTION_CHANGE` | 0x00040000 | Encryption state changed |
| `USN_REASON_OBJECT_ID_CHANGE` | 0x00080000 | Object ID changed |
| `USN_REASON_REPARSE_POINT_CHANGE` | 0x00100000 | Reparse point changed |
| `USN_REASON_STREAM_CHANGE` | 0x00200000 | Named stream created/deleted |
| `USN_REASON_CLOSE` | 0x80000000 | File handle closed (change committed) |

### Reference

- [Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals)
- [FSCTL_READ_USN_JOURNAL](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_read_usn_journal)
- [USN_RECORD_V3](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v3)

---

*Document Version: 1.0*
*Last Updated: 2026-03-23*
