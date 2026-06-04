#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! ```
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// =============================================================================
// scripts/windows/record-demo-prep.rs — put a box into a known, HONEST state
// before recording a UFFS demo clip (see scripts/dev/demo/README.md).
// =============================================================================
//
// The launch demo GIFs must show real, reproducible behaviour. This tool warms
// (or, on request, cools) the UFFS daemon and prints the exact recorder
// settings so every capture is consistent and the on-screen latency matches
// docs/benchmarks/.
//
// **Why Rust, not PowerShell.** This is the same device-orchestration shape as
// the sibling validation/benchmark tools (api-validation.rs, cli-validation.rs,
// tier-load-compare.rs, ...). It reuses their `default_binary()` discovery so
// it resolves `uffs.exe` BY NAME — sidestepping the Windows `PATHEXT` trap
// where a bare `uffs` resolves to the legacy C++ `uffs.com` (which has no
// `daemon` subcommand and uses `--drives=`/`--columns=` syntax). It also gates
// on a real semver parsed from `uffs --version`, so an old build — or the C++
// reference tool — is refused up front with an actionable message.
//
// It only calls DOCUMENTED `uffs` commands. It is non-destructive by default;
// the cold path (which deletes on-disk caches) is gated behind
// `--confirm-destructive`.
//
// Usage:
//   rust-script scripts/windows/record-demo-prep.rs
//   rust-script scripts/windows/record-demo-prep.rs --mode hot --drives C,D,E,F,G,M,S
//   rust-script scripts/windows/record-demo-prep.rs --mode cold --drives D --confirm-destructive
//   rust-script scripts/windows/record-demo-prep.rs --bin C:\Users\me\bin\uffs.exe

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use colored::Colorize;

/// Minimum `uffs` version this tool will drive. Anything below — or the
/// legacy C++ `uffs.com`, which emits no `uffs X.Y.Z` clap line at all —
/// is refused before we touch the daemon.
const MIN_VERSION: (u64, u64, u64) = (0, 5, 0);

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Restart the daemon and warm it so targeted queries answer from memory.
    /// This is the "instant" story — caption your clip "hot daemon, <N> records".
    Hot,
    /// Show the cold MFT-build path. DESTRUCTIVE: evicts caches for the target
    /// drives so the next search rebuilds from raw MFT (tens of seconds).
    /// Requires `--confirm-destructive`.
    Cold,
}

#[derive(Parser)]
#[command(
    name = "record-demo-prep",
    about = "Put a box into a known, HONEST state before recording a UFFS demo clip",
    after_help = "EXAMPLES:\n  \
        rust-script scripts/windows/record-demo-prep.rs --mode hot --drives C,D\n  \
        rust-script scripts/windows/record-demo-prep.rs --mode cold --drives D --confirm-destructive"
)]
struct Cli {
    /// hot (default): warm the daemon. cold: show the cold build path (destructive).
    #[arg(long, value_enum, default_value = "hot")]
    mode: Mode,

    /// Drive letters to warm/preload (hot) or forget (cold), comma-separated.
    #[arg(long, value_delimiter = ',', default_value = "C")]
    drives: Vec<String>,

    /// Required to actually run `--mode cold` (which deletes on-disk caches).
    #[arg(long)]
    confirm_destructive: bool,

    /// Explicit path to the RUST uffs binary. Defaults to the standard
    /// discovery order (see `default_binary`).
    #[arg(long, alias = "binary")]
    bin: Option<String>,
}

fn step(msg: &str) {
    println!("{} {msg}", "==>".cyan().bold());
}

fn warn(msg: &str) {
    println!("{} {msg}", "!! ".yellow().bold());
}

/// Walk up from CWD to find the workspace root (has Cargo.toml + .cargo).
fn find_workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").exists() && dir.join(".cargo").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    cwd
}

/// Locate an existing uffs binary; do **not** auto-build.
///
/// Mirrors `scripts/windows/api-validation.rs::default_binary`. Resolves
/// `uffs.exe` (Windows) / `uffs` (Unix) BY NAME so we never inherit the
/// `PATHEXT` `.com`-before-`.exe` ordering that makes a bare `uffs` run the
/// legacy C++ reference tool.
///
/// Search order:
///   1. `$HOME/bin/uffs[.exe]`       — `just use` install location
///   2. `target/release/uffs[.exe]`  — `cargo build --release` output
///   3. Bare `uffs[.exe]`            — falls through to PATH lookup
fn default_binary() -> String {
    let bin_name = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(home_var).unwrap_or_else(|_| ".".to_string());
    let candidates = [
        PathBuf::from(&home).join("bin").join(bin_name),
        find_workspace_root()
            .join("target")
            .join("release")
            .join(bin_name),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    bin_name.to_string()
}

/// Parse the `uffs X.Y.Z` clap version line from `<bin> --version`.
///
/// Returns the `(major, minor, patch)` triple. The legacy C++ `uffs.com`
/// prints only the vanity banner (`UFFS version: 1.0.0`, capitalised, with a
/// colon) and no `uffs X.Y.Z` line, so it deterministically fails this parse
/// — which is exactly the wrong-binary guard we want.
fn parse_version(bin: &str) -> Result<(u64, u64, u64)> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run `{bin} --version` (is it installed / on PATH?)"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for line in text.lines() {
        // Match the clap line `uffs X.Y.Z` exactly; the banner line
        // `UFFS version: 1.0.0` is capitalised + colon-delimited and is
        // intentionally NOT matched.
        if let Some(rest) = line.trim().strip_prefix("uffs ") {
            if let Some(triple) = parse_semver(rest.split_whitespace().next().unwrap_or("")) {
                return Ok(triple);
            }
        }
    }
    bail!(
        "`{bin}` did not emit a `uffs X.Y.Z` version line.\n\
         This is almost certainly the legacy C++ reference tool (uffs.com), \
         not the Rust daemon client.\n\
         Pass --bin <path-to-uffs.exe>, or put the Rust uffs.exe ahead on PATH."
    )
}

/// Parse a bare `X.Y.Z` (ignoring any pre-release/build suffix) into a triple.
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Validate each drive token is a single ASCII letter and normalise to
/// uppercase.
fn normalise_drives(raw: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for d in raw {
        let t = d.trim().trim_end_matches(':');
        let mut chars = t.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => out.push(c.to_ascii_uppercase().to_string()),
            _ => bail!("invalid drive letter: '{d}' (expected a single A-Z letter)"),
        }
    }
    if out.is_empty() {
        bail!("no drives given");
    }
    Ok(out)
}

/// Run a uffs subcommand with its output shown (for restart/preload/status).
fn run_show(bin: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{bin} {}`", args.join(" ")))?;
    if !status.success() {
        bail!("`{bin} {}` exited with {status}", args.join(" "));
    }
    Ok(())
}

/// Run a uffs query with output discarded (warm-up priming, not part of the clip).
fn run_quiet(bin: &str, args: &[&str]) {
    let _ = Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn print_recorder_settings() {
    println!();
    step("Recommended recorder settings (keep every clip consistent):");
    println!(
        "{}",
        "  Terminal      : Windows Terminal, single tab, no split panes\n  \
         Window size   : 1200 x 640 (CLI)  /  1280 x 720 (TUI)\n  \
         Font          : Cascadia Mono / Cascadia Code, size 18-20\n  \
         Theme         : dark, high-contrast (e.g. One Half Dark / Catppuccin Mocha)\n  \
         FPS / width   : 12-15 fps, export at 1200px wide, target < 3 MB GIF\n  \
         Recorder      : ScreenToGif (reliable, Windows-native) or VHS + ttyd\n  \
                         VHS on Windows: winget install tsl0922.ttyd, then open a NEW terminal"
            .dimmed()
    );
}

fn run_hot(bin: &str, drives: &[String]) -> Result<()> {
    println!();
    step("Mode=hot — warming the daemon so targeted queries answer from memory.");
    step("Restarting daemon for a clean, known state...");
    run_show(bin, &["daemon", "restart"])?;

    for d in drives {
        step(&format!(
            "Preloading drive {d} and pinning it for the recording window..."
        ));
        run_show(bin, &["daemon", "preload", d, "--pin-minutes", "60"])?;
    }

    step("Priming the query path (these warm-up results are NOT part of the clip)...");
    run_quiet(bin, &["*.rs"]);
    run_quiet(bin, &["*.dll", "--drive", &drives[0]]);

    println!();
    step("Current tier / telemetry (this is the table the CLI clip shows):");
    run_show(bin, &["daemon", "status_drives"])?;

    println!();
    println!(
        "{}",
        "READY (hot). Caption the clip as a HOT daemon over your real record count."
            .green()
            .bold()
    );
    warn("Honesty: do not edit frames to alter latency; the numbers on screen must stand.");
    Ok(())
}

fn run_cold(bin: &str, drives: &[String], confirm: bool) -> Result<()> {
    println!();
    warn(&format!(
        "Mode=cold is DESTRUCTIVE: it evicts on-disk caches for: {}",
        drives.join(", ")
    ));
    warn("The next search rebuilds from raw MFT (tens of seconds). Indexes are re-buildable, but this is not instant.");
    if !confirm {
        bail!(
            "Refusing to run cold mode without --confirm-destructive. \
             Re-run with that flag if you really want the cold-build clip."
        );
    }
    for d in drives {
        step(&format!("Forgetting drive {d} (evict + delete on-disk caches)..."));
        run_show(bin, &["daemon", "forget", d, "--force"])?;
    }
    println!();
    println!(
        "{}",
        "READY (cold). Your next 'uffs' query will now show the COLD build path."
            .green()
            .bold()
    );
    warn("Label the clip COLD and show the full build time honestly.");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let bin = cli.bin.unwrap_or_else(default_binary);
    let drives = normalise_drives(&cli.drives)?;

    // --- locate + gate the binary ------------------------------------------
    if bin.to_lowercase().ends_with(".com") {
        bail!(
            "`{bin}` is the legacy C++ reference tool (uffs.com). \
             Pass --bin pointing at the Rust uffs.exe instead."
        );
    }
    let (maj, min, pat) = parse_version(&bin)?;
    if (maj, min, pat) < MIN_VERSION {
        bail!(
            "`{bin}` is uffs {maj}.{min}.{pat}, but this tool needs >= {}.{}.{}. \
             Update the binary (e.g. `just use`) or pass --bin to a newer build.",
            MIN_VERSION.0, MIN_VERSION.1, MIN_VERSION.2
        );
    }
    step(&format!("Using uffs: {bin}  (v{maj}.{min}.{pat})"));

    print_recorder_settings();

    match cli.mode {
        Mode::Hot => run_hot(&bin, &drives)?,
        Mode::Cold => run_cold(&bin, &drives, cli.confirm_destructive)?,
    }

    println!();
    step("Next: record BOTH clips with ScreenToGif OR VHS.");
    step("      ScreenToGif: Windows-native, no extra deps.");
    step("      VHS: `winget install tsl0922.ttyd` then open a NEW terminal and run:");
    step("           vhs scripts/dev/demo/cli-demo.tape");
    step("      See scripts/dev/demo/README.md for shot lists and wiring.");
    Ok(())
}
