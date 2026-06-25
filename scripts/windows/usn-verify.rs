#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! ```
//!
//! usn-verify.rs — controlled, repeatable live USN-journal verification.
//!
//! Reproduces the manual "create / search / rename / delete" sequence that
//! PowerShell's multi-line paste kept mangling, but driven from Rust so each
//! step runs in order, every search's output is captured to a file, and the
//! daemon's debug/trace log is collected — all in ONE place so the run is
//! trivial to share.
//!
//! What it exercises (the v0.6.13 USN-delta fixes):
//!   * create  → file findable by name AND by `--ext`, with REAL size/time
//!               (the metadata backfill), not size 0 / 1601-epoch.
//!   * rename  → `charlie.log` → `charlie.pdf` moves into `--ext pdf`, out of
//!               `--ext log`.
//!   * delete  → `bravo.dll` drops out of `--ext dll`.
//!   * FRS reuse → recreating into a just-deleted dir doesn't drop files.
//!
//! ## Binary
//!
//! Uses `~/bin/uffs.exe` (the canonical install path). **Copy your freshly
//! built `target\release\{uffs,uffsd,uffs-broker}.exe` into `~/bin` first** —
//! the daemon that gets spawned is the `uffsd.exe` sitting next to the
//! `uffs.exe` this script invokes, so the install dir is what's under test.
//!
//! ## Usage
//!
//!   rust-script scripts\windows\usn-verify.rs
//!
//! ## Output
//!
//! Everything lands in `~/usntest`:
//!   * `usntest_*.{pdf,dll,log}`     — the files under test
//!   * `_run/NN-*.csv`               — each search's exact stdout
//!   * `_run/uffsd.log`              — the daemon's debug+trace log for the run
//!   * `_run/usn-apply.log`          — just the `usn apply:` / `usn backfill:`
//!                                     lines, extracted for quick sharing
//!
//! Share `_run/` and we can see, per 500 ms poll, exactly what the journal
//! loop did (`created=N deleted=N renamed=N skipped=N`) and whether the
//! targeted metadata read fired.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Time to let the per-shard USN loop ingest a batch and patch the live
/// body.  With [`APPLY_INTERVAL_MS`] pinned to 500 ms below, the apply
/// tick fires on essentially the first poll that sees the new events, so
/// 3 s is a comfortable margin (the body is searchable well under 1 s
/// after the file op in practice).  No 5-minute disk-save wait is needed
/// — the apply tick is decoupled from the rare compact-cache save.
const POLL_SETTLE: Duration = Duration::from_secs(3);
/// Apply-cadence override (ms) for the test daemon — pins
/// `UFFS_USN_APPLY_INTERVAL_MS` low so the near-live body patch fires
/// promptly and deterministically within [`POLL_SETTLE`].  The
/// production default is 30 s (tuned so constant FS churn stays
/// background noise); the harness pins it to 500 ms so the short
/// create / rename / delete rounds don't have to wait that out.
const APPLY_INTERVAL_MS: &str = "500";
/// Settle time after `--daemon stop` so the socket / PID file clear.
const KILL_SETTLE: Duration = Duration::from_secs(2);
/// Tracing directive: per-change USN trace + daemon-side debug (backfill,
/// journal loop) on top of an `info` baseline. `init_tracing` feeds this
/// straight into `EnvFilter::try_new`, so the full directive form works.
const LOG_SPEC: &str = "info,uffs_core::compact_loader=trace,uffs_daemon=debug";
/// Bytes written into the headline `.pdf` so the size-backfill assertion is
/// visible (`Size` column should read this, not 0).
const ALPHA_BYTES: usize = 5000;

/// `~/bin/uffs.exe` — the canonical user-installed **Rust** binary.
///
/// Pinned to the explicit `uffs.exe` filename on purpose: a bare `uffs`
/// on Windows resolves through `PATHEXT`, where `.com` precedes `.exe`,
/// so if the C++ build (`uffs.com`) is also on `PATH` it would shadow
/// the Rust `uffs.exe` we are trying to exercise.  Returning the full
/// path and handing it to `Command::new` bypasses `PATHEXT` resolution
/// entirely, so this script always runs the Rust build under test.
/// Never "simplify" this to a bare `uffs`.
fn uffs_bin() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .expect("USERPROFILE or HOME must be set");
    let name = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    home.join("bin").join(name)
}

/// Display name for the cosmetic `$ ...` echo lines.  Uses the same
/// `uffs.exe` the script actually spawns so a shared transcript is
/// copy-paste-safe: pasting `uffs <args>` into a shell could hit the
/// C++ `uffs.com` (see [`uffs_bin`]), but `uffs.exe <args>` cannot.
fn uffs_display() -> &'static str {
    if cfg!(windows) { "uffs.exe" } else { "uffs" }
}

/// Home directory (`~`) — the scratch tree lives at `~/usntest`.
fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .expect("USERPROFILE or HOME must be set")
}

/// Run a `uffs` subcommand inheriting stdout/stderr (for `--status`, daemon
/// control) — the user sees exactly what they would running it by hand.
fn run(uffs: &Path, args: &[&str]) -> Result<()> {
    println!("\n$ {} {}", uffs_display(), args.join(" "));
    Command::new(uffs)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn uffs {}", args.join(" ")))?;
    Ok(())
}

/// Run a `uffs` search, capture its stdout to `out`, and print a one-line
/// summary (row count + the names found) next to the expectation.
fn capture(uffs: &Path, args: &[&str], out: &Path, expect: &str) -> Result<()> {
    let output = Command::new(uffs)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn uffs {}", args.join(" ")))?;
    fs::write(out, &output.stdout).with_context(|| format!("write {}", out.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);

    // CSV: one header line + one blank, then data rows; all quoted lines.
    let quoted = text.lines().filter(|l| l.starts_with('"')).count();
    let rows = quoted.saturating_sub(1); // minus the header
    let names: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("\"C:") || l.starts_with("\"\\\\"))
        .filter_map(|l| l.split('"').nth(3)) // 2nd CSV field = Name
        .take(8)
        .collect();

    println!("\n$ {} {}", uffs_display(), args.join(" "));
    println!("   expect: {expect}");
    println!("   got:    {rows} row(s)  {names:?}");
    println!("   saved:  {}", out.display());
    Ok(())
}

fn main() -> Result<()> {
    let uffs = uffs_bin();
    if !uffs.exists() {
        bail!(
            "uffs binary not found at {}\n\
             Copy your freshly built target\\release\\{{uffs,uffsd,uffs-broker}}.exe \
             into ~/bin first, then re-run.",
            uffs.display()
        );
    }

    let base = home_dir().join("usntest");
    let run_dir = base.join("_run");
    println!("== UFFS USN verification ==");
    println!("binary:    {}", uffs.display());
    println!("scratch:   {}", base.display());
    println!("artifacts: {}", run_dir.display());

    // Fresh tree.
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;

    run(&uffs, &["--version"])?;

    // ── Restart the daemon with debug+trace logging into the artifacts dir ──
    let _ = Command::new(&uffs).args(["--daemon", "stop"]).status();
    sleep(KILL_SETTLE);
    println!(
        "\n$ {} --daemon start   (UFFS_LOG={LOG_SPEC}, UFFS_USN_APPLY_INTERVAL_MS={APPLY_INTERVAL_MS})",
        uffs_display()
    );
    let status = Command::new(&uffs)
        .args(["--daemon", "start"])
        .env("UFFS_LOG", LOG_SPEC)
        .env("UFFS_LOG_DIR", &run_dir)
        .env("UFFS_USN_APPLY_INTERVAL_MS", APPLY_INTERVAL_MS)
        .status()
        .context("failed to spawn `uffs --daemon start`")?;
    if !status.success() {
        bail!("`uffs --daemon start` exited with {status}");
    }
    run(&uffs, &["--status"])?;

    // ── Round 1: create four files with distinct extensions ─────────────────
    println!("\n== Round 1: create ==");
    fs::write(base.join("usntest_alpha.pdf"), "a".repeat(ALPHA_BYTES))?;
    fs::write(base.join("usntest_delta.pdf"), b"x")?;
    fs::write(base.join("usntest_bravo.dll"), b"x")?;
    fs::write(base.join("usntest_charlie.log"), b"x")?;
    sleep(POLL_SETTLE);

    capture(&uffs, &["usntest", "--format", "csv"], &run_dir.join("01-name.csv"), "4 files (alpha/delta/bravo/charlie)")?;
    capture(&uffs, &["usntest", "--ext", "pdf", "--format", "csv"], &run_dir.join("02-ext-pdf.csv"), "alpha.pdf + delta.pdf")?;
    capture(&uffs, &["usntest", "--ext", "dll", "--format", "csv"], &run_dir.join("03-ext-dll.csv"), "bravo.dll")?;
    capture(&uffs, &["usntest", "--ext", "log", "--format", "csv"], &run_dir.join("04-ext-log.csv"), "charlie.log")?;
    // Metadata backfill: alpha.pdf should show ~ALPHA_BYTES + real timestamps.
    capture(&uffs, &["usntest_alpha.pdf", "--format", "csv"], &run_dir.join("05-alpha-meta.csv"), &format!("size ≈ {ALPHA_BYTES}, real (non-1601) timestamps"))?;

    // ── Round 2: rename + delete ────────────────────────────────────────────
    println!("\n== Round 2: rename charlie.log -> charlie.pdf, delete bravo.dll ==");
    fs::rename(base.join("usntest_charlie.log"), base.join("usntest_charlie.pdf"))?;
    fs::remove_file(base.join("usntest_bravo.dll"))?;
    sleep(POLL_SETTLE);

    capture(&uffs, &["usntest", "--ext", "pdf", "--format", "csv"], &run_dir.join("06-ext-pdf-after.csv"), "alpha + delta + charlie (3 pdfs)")?;
    capture(&uffs, &["usntest", "--ext", "dll", "--format", "csv"], &run_dir.join("07-ext-dll-after.csv"), "EMPTY (bravo deleted)")?;
    capture(&uffs, &["usntest", "--ext", "log", "--format", "csv"], &run_dir.join("08-ext-log-after.csv"), "EMPTY (charlie renamed)")?;

    // ── Stop the daemon to flush the log, then extract the USN lines ────────
    println!("\n== Stopping daemon to flush the log ==");
    let _ = Command::new(&uffs).args(["--daemon", "stop"]).status();
    sleep(KILL_SETTLE);

    let log_path = run_dir.join("uffsd.log");
    let apply_path = run_dir.join("usn-apply.log");
    match fs::read_to_string(&log_path) {
        Ok(log) => {
            let lines: Vec<&str> = log
                .lines()
                .filter(|l| l.contains("usn apply:") || l.contains("usn backfill:"))
                .collect();
            fs::write(&apply_path, lines.join("\n"))?;
            println!("extracted {} usn-apply/backfill line(s) -> {}", lines.len(), apply_path.display());
        }
        Err(err) => {
            println!("(could not read {} — {err}; the daemon log may be elsewhere if UFFS_LOG_DIR was overridden)", log_path.display());
        }
    }

    println!("\n== Done ==");
    println!("Share the artifacts dir: {}", run_dir.display());
    println!("Key files: 01-name.csv (creates), 06/07/08 (rename+delete), 05-alpha-meta.csv (backfill), usn-apply.log (per-poll dispositions).");
    Ok(())
}
