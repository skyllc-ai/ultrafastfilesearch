# Daemon

The UFFS daemon is a long-running background process that holds MFT
indices in memory and serves search queries over a local IPC socket.
Searches that would normally take 60+ seconds to load data complete in
**~200 ms** end-to-end because the daemon keeps everything hot.

> **See also:** [Getting Started](getting-started.md) ·
> [CLI Overview](cli-overview.md) ·
> [Cache & Data Sources](cache-and-data.md) ·
> [Advanced Diagnostics](advanced-diagnostics.md)

---

## 1  Architecture

```
┌─────────┐                          ┌─────────────┐
│ uffs CLI ├──── JSON-RPC over ──────┤ uffs-daemon  │
│ uffs_tui │     local IPC socket    │  (in-memory  │
│ uffs --mcp │                         │   MFT index) │
└─────────┘                          └─────────────┘
```

The daemon loads MFT data once at startup, then serves any number of
search queries without re-reading disk.  Multiple CLI, TUI, and MCP
clients share the same daemon instance.

| Transport | Platform |
|-----------|----------|
| Unix domain socket | macOS / Linux |
| Named pipe | Windows |

---

## 2  Quick Start

### macOS / Linux (Offline MFT Files)

On non-Windows platforms, the daemon works with MFT capture files (`.iocp`,
`.bin`, `.mft`) exported from Windows NTFS volumes.

```bash
# Start the daemon with a data directory
uffs --daemon start --data-dir ~/uffs_data

# Or with individual MFT files
uffs --daemon start --mft-file /path/to/C_mft.iocp --mft-file /path/to/D_mft.iocp

# Search (daemon is already running — instant results)
uffs "*.rs" --data-dir ~/uffs_data

# Auto-start: if no daemon is running, search starts one automatically
uffs "*.dll" --data-dir ~/uffs_data
```

The `--data-dir` flag points to a directory with `drive_c/`, `drive_d/`, etc.
subdirectories, each containing an MFT capture file.

### Windows (Live NTFS Drives)

On Windows, the daemon auto-discovers all NTFS drives and reads their MFT
directly.  No `--data-dir` or `--mft-file` needed.

```powershell
# Start the daemon (auto-discovers C:, D:, E:, ...)
uffs --daemon start

# Search — daemon auto-starts if not running
uffs "*.exe"

# Force specific drives only
uffs --daemon start --drive C --drive D
```

> **Note:** Live MFT access requires **Administrator privileges**.

---

## 3  Auto-Start

You rarely need to start the daemon manually.  When you run `uffs` (or
any client), the auto-start mechanism handles everything:

1. CLI checks if a daemon is already running (reads PID file, probes
   socket).
2. If no daemon is found, the CLI **spawns one in the background**,
   passing along `--data-dir`, `--mft-file`, and drive flags from
   the current command.
3. The CLI waits for the daemon to become "Ready" (MFT loaded, index
   built).
4. The CLI sends the search query over IPC.

This means your first `uffs *.txt --data-dir ~/uffs_data` on a clean
machine does everything: spawn daemon, load MFT, build index, search,
return results.  The next search is instant.

---

## 4  Idle Retirement

The daemon retires automatically after being idle for **2 hours**
(7200 seconds).  No cleanup needed — the PID file and socket are
removed on exit.

| Setting | Flag | Default |
|---------|------|---------|
| Idle timeout | `--idle-timeout <SECS>` | `7200` (2 hours) |
| Disable retirement | `--no-retire` | Off |

These flags are passed by the auto-start mechanism.  You can also set
them on `uffs --daemon start`:

```bash
# Never retire (run indefinitely)
uffs --daemon start --data-dir ~/uffs_data --idle-timeout 0

# Retire after 30 minutes
uffs --daemon start --data-dir ~/uffs_data --idle-timeout 1800
```

---

## 5  Management Commands

| Command | Description |
|---------|-------------|
| `uffs --daemon start` | Start the daemon (with data sources) |
| `uffs --daemon status` | Show PID, uptime, loaded drives, record counts |
| `uffs --daemon stats` | Show performance metrics (queries, timing, startup) |
| `uffs --daemon stop` | Graceful shutdown via RPC |
| `uffs --daemon kill` | Hard kill + remove PID/socket files |
| `uffs --daemon restart` | Stop → re-start with same data sources |

### `uffs --daemon status`

```
$ uffs --daemon status
Daemon PID:    72558
Uptime:        145s
Status:        Ready
Connections:   1
  C: —  3,428,455 records (file:/Users/rnio/uffs_data/drive_c/C_mft.iocp)
  D: —  7,065,539 records (file:/Users/rnio/uffs_data/drive_d/D_mft.iocp)
  E: —  2,929,519 records (file:/Users/rnio/uffs_data/drive_e/E_mft.iocp)
  ...
```

### `uffs --daemon stats`

```
$ uffs --daemon stats
═══ Daemon Performance Stats ═══
Uptime:            591s
Startup duration:  10871ms
Total records:     25,846,853
Queries served:    2
Avg query time:    1190.5µs (1.19ms)
Total query time:  2381µs (2.38ms)
Queries/second:    0.00
```

---

## 6  Logging

The daemon runs detached — its stdout is `/dev/null`.  To capture logs,
use `--log-file` and `--log-level`:

```bash
uffs --daemon start --data-dir ~/uffs_data \
    --log-level debug \
    --log-file ~/uffs_daemon.log
```

| Flag | Default | Description |
|------|---------|-------------|
| `--log-level <LEVEL>` | `info` | Tracing level: `error`, `warn`, `info`, `debug`, `trace` |
| `--log-file <PATH>` | *(none)* | Write daemon logs to this file |

The `RUST_LOG` and `UFFS_LOG_DIR` environment variables also control
logging — see [Advanced Diagnostics](advanced-diagnostics.md) for details.

---

## 7  Platform Differences

| Aspect | Windows | macOS / Linux |
|--------|---------|---------------|
| Data source | Live NTFS MFT (auto-detected) | Offline captures (`.iocp`, `.bin`, `.mft`) |
| Privileges | Administrator required | None (reads regular files) |
| IPC transport | Named pipe | Unix domain socket |
| Auto-discovery | All NTFS drives | Requires `--data-dir` or `--mft-file` |

### IPC Socket Locations

| Platform | Default path |
|----------|-------------|
| macOS | `~/Library/Application Support/uffs/uffs-daemon.sock` |
| Linux | `$XDG_RUNTIME_DIR/uffs/uffs-daemon.sock` or `/tmp/uffs/uffs-daemon.sock` |
| Windows | `\\.\pipe\uffs-daemon` |

PID files are stored alongside the socket.  `uffs --daemon kill` removes
both if a graceful stop fails.

---

## 8  Performance

### Windows — Live NTFS, 7 Drives, 25.9M Records

Measured on AMD Ryzen 9 3900XT (12c/24t, 64 GB DDR4), 7 NTFS volumes
(NVMe + SATA SSD + SATA HDD), 25,929,744 total records:

| Operation                   | Time       |
|-----------------------------|------------|
| Daemon startup (cold, all drives) | ~66 s |
| Daemon startup (warm cache)      | ~7 s  |
| Search end-to-end (HOT, CLI)     | ~200–380 ms |
| Daemon-side search (HOT)         | ~151 ms |
| Graceful stop               | ~15 ms     |
| Hard kill                   | ~25 ms     |

Cold startup is dominated by raw MFT reading.  Warm cache startup
deserializes `.iocp` files (~7 s for 25.9M records).  Once loaded,
the daemon-side search takes ~151 ms for all 25.9M records; the
~200–380 ms CLI time includes process spawn, IPC round-trip, and
stdout formatting.

> 📖 **Full data:** [Performance](performance.md) — per-drive
> cold/warm/hot tables, profile internals, query pattern comparison.

---

## 9  Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| "Connection refused" on search | Daemon not running | Let auto-start handle it, or `uffs --daemon start` |
| Stale PID file | Previous daemon crashed | `uffs --daemon kill` removes PID + socket |
| First search slow after restart | MFT being loaded | Normal — ~7 s warm cache (or ~66 s cold), sub-second after |
| "Permission denied" (Windows) | Not running as Admin | Right-click terminal → "Run as administrator" |
| Multiple daemons running | Rare race condition | `uffs --daemon kill` + `uffs --daemon start` |

> **More troubleshooting:** [Troubleshooting](troubleshooting.md)

---

## 10  Readiness Verification

A comprehensive test script exercises all daemon lifecycle combinations
(10 scenarios, 68 steps):

```bash
# macOS/Linux: with a data directory
rust-script scripts/dev/daemon-readiness.rs ~/uffs_data

# macOS/Linux: with a single MFT file
rust-script scripts/dev/daemon-readiness.rs /path/to/C_mft.iocp

# macOS/Linux: with custom search pattern
rust-script scripts/dev/daemon-readiness.rs ~/uffs_data --pattern "*.dll"

# Windows: auto-discovers live NTFS drives (no path needed)
rust-script scripts/dev/daemon-readiness.rs

# Windows: with custom pattern
rust-script scripts/dev/daemon-readiness.rs --pattern "*.exe"
```

Scenarios tested: clean lifecycle, idempotent ops on stopped daemon, double
start, hard kill recovery, graceful stop→start cycle, restart data
preservation, double restart, stats accumulation, kill→status, and search
auto-start.

