// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Thin client library for the UFFS daemon.
//!
//! All surfaces (CLI, TUI, GUI, MCP) use this crate to communicate with
//! the daemon. It handles auto-start, connection, keepalive, and reconnect.
//!
//! # Example
//!
//! ```rust,ignore
//! let client = UffsClient::connect().await?;
//! let results = client.search("*.rs").await?;
//! let drives = client.drives().await?;
//! ```

// Suppress unused crate warnings for deps used in sub-modules
use serde as _;
use uffs_security as _;

/// Async `UffsClient` over tokio — used by the MCP gateway and daemon.
///
/// Gated behind the `async` feature so the sync CLI can drop tokio (and
/// `ws2_32.dll`) from its binary.
#[cfg(feature = "async")]
pub mod connect;
/// Background keepalive task + `KeepaliveGuard` for long-lived clients.
///
/// `start_keepalive` is re-attached to `UffsClient` via a split `impl`;
/// external callers import `KeepaliveGuard` directly from this module
/// (no cascade through `connect`).
#[cfg(feature = "async")]
pub(crate) mod connect_keepalive;
/// Tracing helpers used only by [`connect`].  Private; sibling file
/// to keep `connect.rs` under the 800-LOC file-size policy after the
/// v0.5.36 UAC work expanded its public entry points.
#[cfg(feature = "async")]
mod connect_logging;
/// Platform-specific `platform_connect` impls for [`connect::UffsClient`].
///
/// Split `impl` blocks live on `UffsClient` via
/// [`connect::UffsClient::from_parts`]; callers see no change.
/// Extracted after the Run 10 Part B `cached_status` addition pushed
/// `connect.rs` over the 800-LOC policy ceiling.
#[cfg(feature = "async")]
mod connect_platform;
pub mod connect_sync;
/// Platform-specific `platform_connect` impls and the `rpc_deadline` helper.
///
/// Split `impl` blocks live on [`connect_sync::UffsClientSync`];
/// callers see no change.  Also hosts the env-override regression
/// tests for `rpc_deadline`.
pub(crate) mod connect_sync_platform;
/// Wire-protocol unit tests for [`connect_sync::UffsClientSync`].
///
/// Exercises the JSON-RPC request/response path via in-memory
/// reader/writer halves (no real socket).  `#[cfg(test)]` keeps it
/// out of release builds entirely.
#[cfg(test)]
mod connect_sync_tests;
/// Memory-tiering RPC helpers (`hibernate`, `preload`).
///
/// Phase 8-B / 8-C — split off `connect_sync` so the tiering cluster
/// stays under the 800-LOC policy ceiling without a file-size
/// exception.  Same precedent as the daemon-state types in
/// [`protocol::response_status`].
pub(crate) mod connect_sync_tiering;
/// Wire-protocol unit tests for [`connect::UffsClient`].
///
/// Exercises the JSON-RPC request/response path via in-memory tokio
/// `AsyncRead` / `AsyncWrite` halves (no real socket).  `#[cfg(test)]`
/// keeps it out of release builds entirely; gated on the `async`
/// feature so it compiles only when [`connect::UffsClient`] itself does.
#[cfg(test)]
#[cfg(feature = "async")]
mod connect_tests;
/// Child-process handle for spawned daemons.
///
/// Exposes `DaemonChildHandle`, `try_wait`, and the platform-specific
/// cleanup logic.  Canonical home — no cascade through `daemon_ctl`.
pub(crate) mod daemon_child;
pub mod daemon_ctl;
/// Daemon spawn implementation.
///
/// Exposes `spawn_daemon`, `ElevationPolicy`, the MSVCRT-compatible
/// arg quoter, and the Windows UAC helpers.  Canonical home — no
/// cascade through `daemon_ctl`.
pub(crate) mod daemon_spawn;
pub mod error;
pub mod format;
pub mod mcp_pid;
pub mod protocol;
pub mod shmem;
pub mod stdout_kind;
// Phase 3: types and verify have zero external module-path use.
pub(crate) mod types;
pub(crate) mod verify;
/// Windows-only per-RPC deadline enforcement.
///
/// Background watchdog thread that cancels synchronous I/O on the
/// owning thread when an armed deadline expires.  See the module
/// docs for the full design rationale.
#[cfg(windows)]
pub(crate) mod windows_deadline;

pub mod schema;
