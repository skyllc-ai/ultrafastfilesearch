# UFFS Reference Documentation

**Last Updated**: 2026-03-23

This reference set documents the **external specifications** that UFFS operates on — NTFS on-disk formats and Windows I/O APIs. These are pure specification references based on publicly available Microsoft documentation, independent of any particular implementation.

---

## Documents

| # | Document | Scope |
|---|----------|-------|
| 01 | [NTFS Volume Layout](01-ntfs-volume-layout.md) | Boot sector, cluster geometry, MFT location, volume metadata |
| 02 | [MFT Record Format](02-ntfs-mft-record-format.md) | FILE record header, Update Sequence Array, attribute record layout |
| 03 | [NTFS Attributes](03-ntfs-attributes.md) | All attribute types (0x10–0x100), field layouts, resident vs non-resident |
| 04 | [NTFS System Metafiles](04-ntfs-metafiles.md) | Reserved FRS 0-15, `$Extend` contents, root directory |
| 05 | [Windows Volume I/O APIs](05-windows-io-apis.md) | FSCTL codes, IOCP, direct volume access, retrieval pointers |

---

## Scope & Sources

These documents describe **what the data structures and APIs are**, not how any tool reads or processes them. All information is derived from:

- [Microsoft NTFS Technical Reference](https://learn.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- [Microsoft Windows SDK Documentation](https://learn.microsoft.com/en-us/windows/win32/api/)
- [NTFS Documentation (forensics community)](https://flatcap.github.io/linux-ntfs/ntfs/)
- Published NTFS specifications in Windows Driver Kit (WDK) headers

---

## Relationship to Engine Documentation

The [Engine Architecture docs](../engine/README.md) describe how UFFS is built. These Reference docs describe the external specifications UFFS operates on. Together they form a complete picture:

```
Reference docs:  "What does an NTFS $FILE_NAME attribute contain?"
Engine docs:     "How does UFFS parse that attribute into its index?"
```
