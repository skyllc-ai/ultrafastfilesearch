// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs-update` — the self-update **acquire** helper binary.
//!
//! A separate binary from `uffs` so the HTTP/TLS stack (reqwest + rustls)
//! never bloats the lean CLI. `uffs update` spawns this for the
//! download/verify step only; detect + snapshot stay in `uffs-cli`.
//!
//! ```text
//! uffs-update acquire --repo <owner/name> --stage <dir>
//!                     [--version <tag>] [--bundle <asset>] [--sums <asset>]
//! ```

mod acquire;
mod github;
mod verify;

use anyhow::{Result, bail};

use crate::acquire::AcquirePlan;

/// Entry point. Returns a non-zero exit on any failure.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("acquire") => run_acquire(args.get(1..).unwrap_or_default()),
        Some("--help" | "-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => bail!("unknown subcommand `{other}` (try `acquire`)"),
    }
}

/// Parse the `acquire` flags and run it.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn run_acquire(args: &[String]) -> Result<()> {
    let repo = required(flag(args, "--repo"), "--repo <owner/name>")?;
    let stage = required(flag(args, "--stage"), "--stage <dir>")?;
    let plan = AcquirePlan {
        repo,
        tag: flag(args, "--version"),
        stage: std::path::PathBuf::from(stage),
        bundle: flag(args, "--bundle").unwrap_or_else(|| AcquirePlan::default_bundle().to_owned()),
        sums: flag(args, "--sums").unwrap_or_else(|| "SHA256SUMS".to_owned()),
    };
    let staged = acquire::run(&plan)?;
    println!("Acquired + verified: {}", staged.display());
    Ok(())
}

/// Return the value following `name` in `args` (`--name value`).
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}

/// Turn a missing required flag into a clear error.
fn required(value: Option<String>, what: &str) -> Result<String> {
    value.ok_or_else(|| anyhow::anyhow!("missing required {what}"))
}

/// Print usage to stdout.
#[expect(clippy::print_stdout, reason = "intentional help output")]
fn print_usage() {
    println!(
        "uffs-update — self-update acquire helper\n\n\
         USAGE:\n  uffs-update acquire --repo <owner/name> --stage <dir> \\\n\
         \x20                     [--version <tag>] [--bundle <asset>] [--sums <asset>]\n\n\
         Fetches the release, downloads the platform bundle + SHA256SUMS,\n\
         verifies the bundle's SHA-256, and leaves it staged. It does not\n\
         extract, verify Authenticode, or replace anything (apply phase).\n"
    );
}
