# uffs-mcp

**Model Context Protocol (MCP) server for UFFS — bridges AI agents to the daemon.**

[![Crates.io](https://img.shields.io/crates/v/uffs-mcp.svg)](https://crates.io/crates/uffs-mcp)
[![Documentation](https://docs.rs/uffs-mcp/badge.svg)](https://docs.rs/uffs-mcp)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](../../LICENSE)
[![Repository](https://img.shields.io/badge/repo-skyllc--ai%2FUltraFastFileSearch-blue)](https://github.com/skyllc-ai/UltraFastFileSearch)

`uffs-mcp` is the [Model Context Protocol][mcp] bridge between LLM
hosts (Claude Desktop, Cursor, Windsurf, custom agent frameworks) and
the [UFFS][uffs-repo] daemon.  It exposes UFFS's search, drive, and
index tools as MCP tool calls, resources, and prompts, so an AI agent
can issue filesystem queries on a running UFFS index with the same
correctness guarantees as the CLI.

[mcp]: https://modelcontextprotocol.io/
[uffs-repo]: https://github.com/skyllc-ai/UltraFastFileSearch

## What ships in this crate

The crate produces:

- **`uffsmcp`** — the canonical stdio-transport MCP server binary
  (`cargo install uffs-mcp`).  Wire-up in your LLM host's MCP config.
- **`uffs-mcp-http`** — an optional Streamable-HTTP gateway binary
  for hosts that prefer HTTP framing over stdio
  (feature-gated on `streamable-http`, enabled by default).
- **`uffs_mcp` library** — embed the server in your own process if
  you're building a custom AI integration on top of UFFS.

## Architecture

```text
LLM Host  ──stdio──▶  uffsmcp  ──UffsClient (JSON-RPC)──▶  uffsd
        \\
         \\─HTTP──▶  uffs-mcp-http  ──UffsClient──▶  uffsd
```

The MCP server is **not in the query data path** — it merely bridges
MCP framing (tool calls, resources, prompts) to the daemon's native
protocol.  All search work happens server-side in `uffsd`.

## Install

```bash
# Recommended: install the stdio server.
cargo install uffs-mcp

# Then wire it up in Claude Desktop / Cursor / Windsurf / etc.
# (LLM host's MCP-server config block; example for Claude Desktop):
{
  "mcpServers": {
    "uffs": {
      "command": "uffsmcp"
    }
  }
}
```

The `uffsmcp` binary auto-spawns the UFFS daemon on first request, so
the LLM host doesn't need to start `uffsd` separately.

## Library API

For host applications that want to embed the MCP server (rather than
spawning the binary), drop `uffs-mcp` in as a library dep:

```toml
[dependencies]
uffs-mcp = "0.5"
```

```rust,no_run
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Run the MCP server on stdio with default config.
    uffs_mcp::run_mcp_server().await
}
```

### Custom config

```rust,no_run
use uffs_mcp::{McpConfig, run_mcp_server_with_config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = McpConfig {
        // Args forwarded to `uffsd` when auto-spawning.
        daemon_spawn_args: vec!["--data-dir".into(), "/var/lib/uffs".into()],
        // Idle timeout in seconds; 0 = never idle out.
        idle_timeout_secs: 600,
        ..McpConfig::default()
    };
    run_mcp_server_with_config(&config).await
}
```

### Tracing

Optional tracing initialisation for embeddings that want structured
logs (returns a guard that must be held for the writer to flush):

```rust,no_run
let _guard = uffs_mcp::init_mcp_tracing("info", None);
```

## Features

| Feature | Default | Effect |
|---|---|---|
| `streamable-http` | ✅ | Enables the `uffs-mcp-http` Streamable-HTTP gateway binary (extra `axum` + `tower-service` deps). |

To get a minimal stdio-only build:

```toml
[dependencies]
uffs-mcp = { version = "0.5", default-features = false }
```

## Properties

- **Built on the [`rmcp`][rmcp] SDK** — the official MCP Rust SDK; no
  hand-rolled framing or wire parsers.
- **Auto-spawn of `uffsd`** — first request triggers a daemon spawn
  via `uffs_client::daemon_ctl::ensure_daemon_running` if needed.
- **Idle shutdown** — configurable `idle_timeout_secs` lets the server
  exit cleanly when no requests have arrived recently, useful for
  short-lived LLM-host sessions.
- **Public surface scoped to embedding hooks** — `run_mcp_server()`,
  `run_mcp_server_with_config()`, `McpConfig`, `init_mcp_tracing()`,
  and the public `handler::UffsMcpServer` for integration testing.
  Most internal modules (tools, resources, schemas, roots, stats,
  cookbook) are `pub(crate)` and not part of the SemVer contract.

[rmcp]: https://crates.io/crates/rmcp

## What this crate does *not* do

- **Implement search.** Search runs in `uffsd`; this crate is the MCP
  envelope layer.
- **Bundle `uffsd`.**  The daemon is a separate binary located on
  `PATH` (today shipped with `uffs-cli`'s release; eventually
  `cargo install uffsd` directly).

## Relationship to the UFFS workspace

`uffs-mcp` consumes `uffs-client` for the IPC pipe to `uffsd`:

```
LLM Host ──MCP──▶ uffs-mcp ──UffsClient (async)──▶ uffsd
```

Published independently so a third-party host can install the MCP
server without dragging the CLI or other UFFS surfaces along.

## License

Licensed under the [Mozilla Public License 2.0](../../LICENSE).
