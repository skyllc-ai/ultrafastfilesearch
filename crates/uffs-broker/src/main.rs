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

// `broker` is gated to `#[cfg(windows)]` because the entire module
// (Win32 named-pipe service, OpenProcess, DuplicateHandle, audit log,
// Authenticode) is meaningful only on Windows.  All of its supporting
// crates (`anyhow`, `tracing-subscriber`, `windows`,
// `uffs-broker-protocol`) live in `[target.'cfg(windows)'.dependencies]`
// in `Cargo.toml`, so they don't even exist as `extern crate`s on
// non-Windows targets — the bin's non-Windows compilation produces no
// `unused_crate_dependencies` warnings without any markers.
#[cfg(windows)]
mod broker;

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
