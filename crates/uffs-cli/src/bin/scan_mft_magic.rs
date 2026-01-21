//! Compatibility stub for the old `scan_mft_magic` binary location.
//!
//! This crate used to host the real implementation, but the tool has been
//! moved to `crates/uffs-diag` to keep the main CLI dependency footprint
//! smaller. We keep this tiny shim so that existing workflows that call
//! `uffs scan_mft_magic` (or reference the old path) receive a clear and
//! actionable message instead of a hard failure.

#![allow(clippy::print_stderr)]

// Keep these dependencies wired up so that this compatibility stub reflects
// the same crate graph as the main CLI, satisfying `unused_crate_dependencies`
// while remaining a tiny binary.
use {
    anyhow as _, chrono as _, clap as _, dirs_next as _, indicatif as _, tokio as _, tracing as _,
    tracing_appender as _, tracing_subscriber as _, uffs_core as _, uffs_mft as _,
    uffs_polars as _,
};

fn main() {
    eprintln!(
        "scan_mft_magic has moved to the uffs-diag crate.\n\
	     Please run it via `cargo run -p uffs-diag --bin scan_mft_magic -- <args>`",
    );
}
