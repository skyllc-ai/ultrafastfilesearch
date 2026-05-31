# Installation

On Windows, the fastest path is [WinGet](https://learn.microsoft.com/windows/package-manager/)
(§1).  Pre-built binaries for all platforms are also available from
[GitHub Releases](https://github.com/skyllc-ai/UltraFastFileSearch/releases) (§2).
Most users do not need to build from source.

> **See also:** [Getting Started](getting-started.md) ·
> [CLI Overview](cli-overview.md) · [Daemon](daemon.md) ·
> [Cache & Data Sources](cache-and-data.md)

---

## 1  WinGet (Windows — Recommended)

If you have the [Windows Package Manager](https://learn.microsoft.com/windows/package-manager/)
(bundled with Windows 11 and modern Windows 10), install in one command:

```powershell
winget install SkyLLC.UFFS
```

This installs the `uffs` CLI (daemon + MCP + MFT tools) and puts it on
your PATH automatically.  Upgrade later with:

```powershell
winget upgrade SkyLLC.UFFS
```

Confirm the install:

```powershell
uffs --version
```

> Live NTFS search still requires an **Administrator** terminal — see
> [§3 Platform Requirements](#3--platform-requirements).

---

## 2  Pre-Built Binaries

Download the latest Windows x64 binaries from the
[GitHub Releases page](https://github.com/skyllc-ai/UltraFastFileSearch/releases/latest).

Each release includes:
- `uffs-windows-x64.exe` — main CLI (starts daemon + MCP)
- `uffs-mft-windows-x64.exe` — low-level MFT tool
- `CHECKSUMS.txt` — SHA-256 hashes for verification

### Quick Setup (Windows)

**Option A — Using `gh` CLI (recommended for developers):**

```powershell
# Install gh CLI if needed: winget install GitHub.cli
# Then from the repo directory:
just use
```

This downloads the latest release binaries and installs them to `~/bin`.

**Option B — Manual download:**

1. Download `uffs-windows-x64.exe` from the
   [latest release](https://github.com/skyllc-ai/UltraFastFileSearch/releases/latest).

2. Copy to a directory on your PATH and **rename to `uffs.exe`**.

3. Open a terminal **as Administrator** (required for live NTFS access).

4. Run your first search:

   ```powershell
   uffs "*.txt" --limit 5
   ```

   The daemon starts automatically.  Every search after this is instant.

5. To start the MCP server for AI agents:

   ```powershell
   uffs mcp start
   ```

   Run `uffs mcp --help` for instructions on how to configure
   Claude, Cursor, Windsurf, and other AI agents to connect to the
   MCP server.

### Verify Checksums

Each release includes `CHECKSUMS.txt` with SHA-256 hashes:

```powershell
# PowerShell
Get-FileHash uffs-windows-x64.exe -Algorithm SHA256
# Compare with CHECKSUMS.txt from the release
```

---

## 3  Platform Requirements

| Platform | Data source | Privileges |
|----------|------------|------------|
| **Windows** | Live NTFS MFT (auto-detected) | Administrator required |
| **macOS / Linux** | Offline MFT captures (`.iocp`, `.bin`, `.mft`) | None |

### Windows

The pre-built binary reads NTFS drives directly.  **Administrator
privileges are required** — the MFT is a protected system structure.

```powershell
# Option A: Run your terminal as Administrator
# Right-click Terminal → "Run as administrator"

# Option B: Use gsudo (recommended)
gsudo uffs "*.dll"
```

### macOS / Linux

macOS and Linux cannot read NTFS drives directly.  You need offline
MFT captures exported from a Windows machine, pointed at via
`--data-dir`:

```bash
uffs "*.txt" --data-dir ~/uffs_data --limit 5
```

See [Cache & Data Sources](cache-and-data.md) for how to set up the
`drive_c/`, `drive_d/` directory structure.

---

## 4  Add to PATH

### Windows (PowerShell)

```powershell
# Copy to a permanent location
$uffsDir = "$env:LOCALAPPDATA\uffs"
New-Item -ItemType Directory -Force -Path $uffsDir
Copy-Item dist\v0.4.105\uffs\uffs-windows-x64.exe "$uffsDir\uffs.exe"

# Add to user PATH (persists across sessions)
[Environment]::SetEnvironmentVariable("Path",
    "$uffsDir;$([Environment]::GetEnvironmentVariable('Path', 'User'))", "User")
```

After restarting your terminal, `uffs` is available everywhere:

```powershell
uffs --version
uffs "*.txt" --limit 5
```

### macOS / Linux (build from source — see §5)

```bash
ln -s "$(pwd)/target/release/uffs" /usr/local/bin/uffs
```

---

## 5  Build from Source

Building from source is needed for development, contributing, or
running on macOS/Linux.

> **⚠ Windows native build limitation:** Due to a Polars COFF archive
> size issue, `cargo build` does **not** currently work on Windows
> natively.  Windows binaries are cross-compiled from macOS or Linux.
> See [xwin workaround](../xwin-msvc-rlib-size-root-cause-and-workarounds.md)
> for technical details.

### Prerequisites

| Requirement | Version | Notes |
|-------------|---------|-------|
| **Rust** | Nightly (pinned in `rust-toolchain.toml`) | Polars requires `nightly` + `simd` |
| **just** | 1.0+ | Task runner — `cargo install just` |
| **cargo-nextest** | Latest | Test runner — `cargo install cargo-nextest` |
| **Git** | Any | To clone the repository |

### Build

```bash
git clone https://github.com/skyllc-ai/UltraFastFileSearch.git
cd UltraFastFileSearch

# Build release binaries (Rust nightly installs automatically)
cargo build --release
```

| Binary | Location | Purpose |
|--------|----------|---------|
| `uffs` | `target/release/uffs` | CLI search tool |
| `uffs_tui` | `target/release/uffs_tui` | Terminal UI |

### Using just (recommended)

```bash
just build       # Release build
just use         # Install binaries to ~/bin from the latest GitHub Release
just use-local   # Build workspace from source and install to ~/bin (dev)
just check       # Format + lint + build (no tests)
just go          # Full validation: format, lint, test, coverage
just test        # Run all tests with nextest
```

`just use` is the easiest way to get going on any OS — it downloads
the Release bundle that GitHub Actions built for your platform and
installs the binaries to `~/bin`.  Those bytes are identical to what
every other end user runs, so bug reports are reproducible.

`just use-local` is the dev-loop variant — it runs a full
`cargo build --release --workspace` and installs the just-built
binaries to `~/bin`.  Use this when you want to test local changes
before opening a PR.

If `~/bin` is not on your PATH, either recipe prints the line to add
to your shell profile.

Run `just` with no arguments to see all available tasks.

### Cross-Compiling Windows Binaries (from macOS / Linux)

This is how the release binaries are cross-compiled:

```bash
cargo install cargo-xwin
cargo xwin build --release --target x86_64-pc-windows-msvc
```

See [xwin workaround](../xwin-msvc-rlib-size-root-cause-and-workarounds.md)
for details on the `xwin-dev` profile and COFF archive size limits.

---

## 6  Verify Installation

```bash
# Check version
uffs --version

# Show help
uffs --help

# Run a test search (Windows — live NTFS)
uffs "*.txt" --limit 5

# Run a test search (macOS/Linux — offline MFT files)
uffs "*.txt" --data-dir ~/uffs_data --limit 5
```

If the search returns results, UFFS is working.  The daemon starts
automatically on first search — see [Daemon](daemon.md) for details.

---

## Next Steps

- [Getting Started](getting-started.md) — your first search in 5 minutes
- [CLI Overview](cli-overview.md) — all flags and subcommands at a glance
- [Cache & Data Sources](cache-and-data.md) — setting up offline MFT files
