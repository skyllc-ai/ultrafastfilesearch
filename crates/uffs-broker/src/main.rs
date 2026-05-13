// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS Access Broker — Windows service for elevated MFT handle brokering.
//!
//! A tiny Windows service that runs elevated and provides read-only NTFS
//! volume handles to the daemon process (which runs as a normal user).
//!
//! # Usage
//!
//! ```bash
//! uffs-broker --install     # Install as Windows Service
//! uffs-broker --uninstall   # Remove Windows Service
//! uffs-broker --start       # Start the service
//! uffs-broker --stop        # Stop the service
//! uffs-broker --run         # Run in foreground (for debugging)
//! ```
//!
//! On non-Windows platforms, this binary prints an error and exits.

// Windows-only modules: `broker` consumes `anyhow`, `tracing-subscriber`,
// `windows` — all of which are scoped to `[target.'cfg(windows)'.dependencies]`
// in `Cargo.toml`, so they don't even exist as `extern crate`s on
// non-Windows targets.  The cross-platform `[lib]` (see `lib.rs`)
// exposes the wire-protocol types regardless of host platform.

#[cfg(windows)]
mod broker;

// `thiserror` is a cross-platform `[dependencies]` entry consumed by
// `protocol::ProtocolError` in the `[lib]` target.  This binary doesn't
// touch it directly — silence `unused_crate_dependencies` with the
// rustc-documented marker (NOT a blanket allow; the dep IS used by the
// package, just not by this compilation unit).
use thiserror as _;
// `uffs_broker` is this package's own `[lib]`.  The binary uses
// `uffs_broker::protocol::*` only inside `#[cfg(windows)]` code paths
// in `broker.rs`, so on non-Windows compilations the bin sees the lib
// in scope but doesn't observably link any symbol from it — silence
// `unused_crate_dependencies` accordingly.
#[cfg(not(windows))]
use uffs_broker as _;

fn main() {
    #[cfg(windows)]
    {
        if let Err(run_err) = broker::run() {
            tracing::error!(%run_err, "uffs-broker fatal error");
            std::process::exit(1);
        }
    }

    #[cfg(not(windows))]
    {
        tracing::error!("uffs-broker is a Windows-only component");
        std::process::exit(1);
    }
}
