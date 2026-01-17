# UFFS Binary Distribution

This directory contains pre-built UFFS binaries.

## ⚠️ Windows Only

**UFFS is a Windows-only tool.** It reads the NTFS Master File Table (MFT)
directly using Windows kernel APIs for ultra-fast file searching.

macOS and Linux are only used as cross-compilation hosts - the resulting
binaries still only run on Windows.

## Binaries

| Binary | Description |
|--------|-------------|
| `uffs` | Main CLI tool for fast file searching |
| `uffs_mft` | Low-level MFT reading tool (read, info, drives) |
| `uffs_tui` | Terminal UI for interactive searching |
| `uffs_gui` | Graphical UI (placeholder) |

## Structure

- `latest/` - Symlink to the current version
- `v{version}/{binary}/` - Binaries for each version
  - `{binary}-windows-x64.exe` - Windows x64 (the only supported platform)

## Installation

### Windows

Copy the binary to a directory in your PATH:

```powershell
copy uffs-windows-x64.exe C:\Users\YourName\bin\uffs.exe
```

Or add the bin directory to your PATH:

```powershell
# Add C:\Users\YourName\bin to your PATH if not already there
$env:Path += ";$env:USERPROFILE\bin"
```

## Cross-Compilation

To build Windows binaries from macOS or Linux:

```bash
# Install prerequisites
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc

# Build
rust-script scripts/build-cross-all.rs
```

## Why Windows Only?

UFFS achieves its speed by reading the NTFS Master File Table (MFT) directly
from disk, bypassing the Windows file enumeration APIs. This requires:

- Direct volume access via `\\.\X:` paths
- Windows kernel ioctls like `FSCTL_GET_NTFS_VOLUME_DATA`
- Administrator privileges for raw disk access

These APIs are Windows-specific and cannot be replicated on other platforms.

