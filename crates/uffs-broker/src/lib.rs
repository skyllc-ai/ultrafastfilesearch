// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS Access Broker — library surface.
//!
//! `uffs-broker` ships as both a Windows-only `[[bin]]` (the elevated
//! handle broker service) and this thin `[lib]` (cross-platform
//! protocol types).
//!
//! # What this library exposes
//!
//! [`protocol`] — the wire-protocol types shared between the broker
//! service ([`crate::protocol::HandleResponse::encode`]) and the
//! daemon-side client (`uffs-daemon::broker_client`).  Specifically:
//!
//! - [`protocol::PIPE_NAME`] — the named-pipe path both sides connect to
//! - [`protocol::HandleRequest`] — the 1-byte request format (drive letter)
//! - [`protocol::HandleResponse`] — the 9-byte response format (status +
//!   handle)
//! - [`protocol::Status`] — the response status byte (`Ok` / `Error`)
//! - [`protocol::ProtocolError`] — structured parse-error type
//!
//! # Why this is its own module (not embedded in the binary)
//!
//! Before F5 (issue #205), `BROKER_PIPE_NAME` and the wire-format byte
//! layout were duplicated in `crates/uffs-broker/src/broker.rs` and
//! `crates/uffs-daemon/src/broker_client.rs`.  The daemon side carried
//! a textual `// must match uffs-broker/src/broker.rs` comment — i.e.
//! the only thing preventing protocol drift was reviewer discipline.
//!
//! Promoting the protocol to a shared module:
//!
//! 1. eliminates the textual "must match" coupling — the compiler now enforces
//!    a single source of truth
//! 2. lets us unit-test the protocol cross-platform (the binary's Windows-only
//!    FFI cannot be unit-tested on macOS/Linux CI lanes)
//! 3. keeps the broker crate self-contained — no new workspace member
//!
//! # What this library does NOT expose
//!
//! The Windows-only handle-brokering FFI (named pipes, `OpenProcess`,
//! `DuplicateHandle`, `CreateNamedPipeW`, audit log, Authenticode) stays
//! in the binary at `src/main.rs` / `src/broker.rs`.  Those 23
//! `unsafe { }` blocks cannot be unit-tested without a real Windows
//! kernel; they're audited via SAFETY paragraphs and exercised by
//! manual Windows runner smoke tests.

// ── Cross-platform extern-crate markers ─────────────────────────────
//
// `tracing` is a cross-platform `[dependencies]` entry that this crate's
// `[[bin]]` consumes (the non-Windows `main()` path emits `tracing::error!`
// before exiting).  The `[lib]` compilation sees it in scope but does not
// use it — declare the dependency intentional so rustc's
// `unused_crate_dependencies` lint stays silent.  This is the idiomatic
// rustc-documented response for `[lib] + [[bin]]` packages whose two
// targets have heterogeneous extern-crate needs.
// ── Windows-only extern-crate markers ───────────────────────────────
//
// `anyhow`, `tracing-subscriber`, and `windows` are scoped to
// `[target.'cfg(windows)'.dependencies]` in `Cargo.toml`.  On the
// Windows target they're consumed by the bin's `broker.rs`, but the
// lib doesn't use them — same `unused_crate_dependencies` situation
// as `tracing` above, gated to `cfg(windows)` because these crates
// don't exist as extern crates on other targets.
#[cfg(windows)]
use anyhow as _;
use tracing as _;
#[cfg(windows)]
use tracing_subscriber as _;
#[cfg(windows)]
use windows as _;

pub mod protocol;
