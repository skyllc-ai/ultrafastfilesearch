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
//!
//! ## API hygiene policy (Phase 3b §3.4 / §3.6 / §3.7)
//!
//! - **`protocol::*` wire DTOs / wire enums** — `pub` fields **are** the
//!   contract (`serde` JSON keys 1:1; §3.4).  Kept exhaustive (§3.6): monorepo
//!   deploys daemon + client together (no skew scenario), hundreds of
//!   struct-literal construction sites, and the wire enums
//!   (`SearchPredicateOp`, `SearchPayload`, `DaemonStatus`, …) are dispatch
//!   enums where exhaustive `match` is the compile-time safety net.  Revisit
//!   when this crate publishes externally.
//! - **`schema::*` field-metadata DTOs / enums** — same field-discipline
//!   reasoning as `protocol`; closed type-code enums by definition.
//! - **`UffsClient` / `UffsClientSync`** — fields private, smart constructors
//!   protect reader/writer pairing and `next_id` monotonicity invariants; sync
//!   sibling adds the Windows `deadline_guard` watchdog-already-spawned
//!   invariant.  See the focused decision record on `connect::UffsClient`.
//! - **`ClientError`** — `#[non_exhaustive]` applied (the lone API attribute
//!   change in Phase 3b); safe under both external usage patterns in
//!   `uffs-cli`.
//! - **§3.7** N/A — no `pub trait` declarations in this crate.

// On docs.rs only: enable the `doc_cfg` rustdoc feature so cfg-gated items
// (`#[cfg(feature = "async")]`, `#[cfg(windows)]`, etc.) render with their
// cfg badge.  Gated behind `cfg(docsrs)` so local `cargo doc` never
// exercises the nightly-only feature.  Post-Rust-1.92 the `doc_auto_cfg`
// feature was merged into `doc_cfg` (rust-lang/rust#138907).
#![cfg_attr(docsrs, feature(doc_cfg))]

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
/// Auto-start daemon helpers (`auto_start_daemon`, `is_process_alive`,
/// `is_daemon_process`) — split off `connect_sync` to keep that file
/// under the 800-LOC policy ceiling.
pub(crate) mod connect_sync_autostart;
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
