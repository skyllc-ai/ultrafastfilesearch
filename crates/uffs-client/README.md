# uffs-client

**Thin client library for the UFFS daemon — connect, query, lifecycle.**

[![Crates.io](https://img.shields.io/crates/v/uffs-client.svg)](https://crates.io/crates/uffs-client)
[![Documentation](https://docs.rs/uffs-client/badge.svg)](https://docs.rs/uffs-client)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](../../LICENSE)
[![Repository](https://img.shields.io/badge/repo-skyllc--ai%2FUltraFastFileSearch-blue)](https://github.com/skyllc-ai/UltraFastFileSearch)

`uffs-client` is the **single client surface** that every UFFS frontend
— the CLI (`uffs`), the MCP server (`uffsmcp`), the TUI / GUI on the
roadmap, and any third-party integration — uses to talk to the
[UFFS][uffs-repo] daemon (`uffsd`).  It handles transport (Unix domain
socket on Linux / macOS, named pipe on Windows), JSON-RPC framing,
keepalive, auto-spawn of the daemon when it's not already running, and
the canonical typed result shapes shared between every consumer.

[uffs-repo]: https://github.com/skyllc-ai/UltraFastFileSearch

## Why a dedicated client crate?

Every UFFS frontend would otherwise duplicate:

- Connect / reconnect logic (Unix socket vs Windows named pipe path
  selection, retry envelope, deadline handling).
- JSON-RPC request id management + response routing.
- Daemon discovery + auto-spawn (the daemon is itself a UFFS binary
  with its own lifecycle and PID file).
- The exact wire-format types (`SearchPayload`, `DaemonStatus`,
  `LoadDriveResponse`, …) that the daemon emits.

Centralising these in one crate means the CLI and the MCP server hit
**the same byte-identical wire path** and the same typed result types.
A breaking change to a wire shape is a compile error in every consumer,
not a runtime mismatch.

## Add it

```toml
[dependencies]
# Async API (default) — uses tokio, suitable for daemons / servers.
uffs-client = "0.5"

# Sync-only API — drops tokio entirely (and `ws2_32.dll` on Windows).
# Suitable for hot-path CLI binaries where launch cost matters.
uffs-client = { version = "0.5", default-features = false }
```

## Usage

### Async (default — `tokio` runtime)

```rust,no_run
use uffs_client::connect::UffsClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to a running daemon, or auto-spawn one if absent.
    let mut client = UffsClient::connect().await?;

    // List the drives the daemon has loaded.
    let drives = client.drives().await?;
    for d in drives {
        println!("{} — {} records", d.letter, d.record_count);
    }

    // Run a query.
    let results = client.search("*.rs").await?;
    println!("{} hits", results.rows.len());

    Ok(())
}
```

### Sync (CLI hot path — no `tokio`)

```rust,no_run
use uffs_client::connect_sync::UffsClientSync;

fn main() -> anyhow::Result<()> {
    let mut client = UffsClientSync::connect()?;
    let drives = client.drives()?;
    for d in drives {
        println!("{} — {} records", d.letter, d.record_count);
    }
    Ok(())
}
```

The sync client mirrors the async API call-by-call.  It is the path
the `uffs` CLI binary uses to keep its cold-start floor minimal
(every microsecond of `ws2_32.dll` initialisation matters when the
CLI is invoked from a shell loop).

### Daemon lifecycle (`daemon_ctl`)

Spawn / shutdown / status helpers for higher-level orchestration
(used by the CLI's `uffs daemon start | stop | status` subcommands):

```rust,no_run
use uffs_client::daemon_ctl;

// Spawn the daemon if it isn't running.  Returns the resolved PID.
let pid = daemon_ctl::ensure_daemon_running()?;

// Graceful shutdown.
daemon_ctl::shutdown_daemon()?;
# Ok::<(), anyhow::Error>(())
```

### Output formatting (`format`)

The crate also re-exports the canonical CSV / parity / legacy-footer
writer (`uffs_format::write_rows`) so the CLI's stdout path is
byte-identical with the daemon's `--out=file` path.  See the
[`format`](https://docs.rs/uffs-client/latest/uffs_client/format/index.html)
module docs.

## Features

| Feature | Default | Effect |
|---|---|---|
| `async` | ✅ | Enables `UffsClient` + `tokio` dependency. |

Sync-only consumers should set `default-features = false`.

## Properties

- **`#[non_exhaustive]` on `ClientError`** — adding new error variants
  doesn't break consumer `match` blocks.
- **Wire DTOs are stable structs** — `protocol::*` field names are the
  `serde` JSON keys 1-for-1.  Renaming a wire field is therefore an
  observable contract change and shows up in `cargo-semver-checks`.
- **Cross-platform IPC transport** — Unix domain socket on Linux /
  macOS, `\\.\pipe\uffsd-<user>` on Windows; the platform split is
  transparent to consumers.
- **Auto-spawn opt-in** — `UffsClient::connect()` auto-spawns the
  daemon if absent; `UffsClient::connect_no_autostart()` errors instead
  if you want strict "must already be running" semantics.

## What this crate does *not* do

- **Index data plane.** All MFT reading, indexing, and query execution
  happens server-side in `uffsd`.  This crate is purely the IPC client.
- **Bundle the daemon binary.** `daemon_ctl::ensure_daemon_running`
  uses `which::which` to locate `uffsd` on `PATH`.  The daemon ships
  separately (today as part of `uffs-cli`'s release; eventually as
  its own `cargo install uffsd`).

## Relationship to the UFFS workspace

`uffs-client` is the shared dependency that every consumer brings in:

```
uffs-cli ─┐
uffs-mcp ─┼─▶ uffs-client ─▶ uffsd (daemon over IPC)
TUI/GUI ──┘
```

It is published independently so a third-party integration (e.g. a
custom shell, an editor plugin, a build-tool scanner) can talk to a
running UFFS daemon without pulling the CLI or MCP stack along.

## License

Licensed under the [Mozilla Public License 2.0](../../LICENSE).
