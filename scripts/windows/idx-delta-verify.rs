#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! ```
//!
//! idx-delta-verify.rs — measurement rig + baseline for the incremental-index-
//! maintenance work (design: `docs/architecture/incremental-index-maintenance.md`).
//!
//! Phase 0 goal: **before** any delta work, prove the rig works on the WIN box
//! and capture a timing BASELINE so later phases can detect a regression.  It
//! deliberately mirrors `scripts/windows/usn-verify.rs` (same `~/bin/uffs.exe`
//! resolution, `~/idxtest` scratch, `_run/` artifacts, daemon-restart-with-
//! logging) so the dev loop is identical: push -> pull on WIN -> run -> share
//! `_run/`.
//!
//! What it does:
//!   0. BIN SYNC — copies the freshly built `uffs`/`uffsd` (+ broker/mcp if
//!      present) from **the build dir cargo actually uses** (`cargo metadata`'s
//!      `target_directory`, honouring `CARGO_TARGET_DIR` / `.cargo/*.toml`;
//!      override with `UFFS_RELEASE_DIR`) into `~/bin`, so the rig can never run
//!      a stale daemon.  Build, then run — no manual copy step.
//!   1. BUILD CONFIRMATION — restarts the daemon with logging, then asserts the
//!      log contains `IDXDELTA build active`, prints the version + git SHA, and
//!      asserts that SHA equals repo HEAD (hard stale-daemon guard).
//!   2. CHURN + TIMING — creates files in escalating bursts so each apply fires
//!      the O(n) full rebuild, captures every `IDXDELTA-TIMING apply` line, and
//!      summarises the per-index rebuild cost (children / trigram / ext / total)
//!      at the drive's live record count.
//!   3. FRESHNESS — measures wall-clock from a create to the file being
//!      search-visible (sanity: no backlog at the pinned apply interval).
//!   4. BASELINE — writes `_run/baseline.txt` (the numbers to commit per the
//!      design doc §8) + `_run/idx-timing.log` (the raw IDXDELTA-TIMING lines).
//!
//! Usage:  rust-script scripts\windows\idx-delta-verify.rs
//!
//! All `IDXDELTA` markers are dev-only; the design doc §9 / Phase 5 removes them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Apply-cadence override (ms) for the test daemon.  Kept above the per-apply
/// rebuild cost (~600 ms on a multi-million-record drive) so apply ticks don't
/// outrun the rebuild and pile up — same rationale as the USN harness.
const APPLY_INTERVAL_MS: &str = "1500";
/// Let the full pipeline (poll -> buffer -> apply -> rebuild -> swap) settle
/// before measuring; generous on a busy multi-million-record volume.
const SETTLE: Duration = Duration::from_secs(6);
/// Settle after `--daemon stop` so the socket / PID file clear.
const KILL_SETTLE: Duration = Duration::from_secs(2);
/// Poll cadence while waiting for a burst's files to become search-visible.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// `info` enables the daemon build marker AND the `IDXDELTA-TIMING` apply line
/// (both logged at INFO); core trace adds per-change detail if needed.
const LOG_SPEC: &str = "info,uffs_core=info,uffs_daemon=info";
/// Escalating create-burst sizes — bigger bursts exercise bigger apply batches.
/// The 100k burst crosses `TRIGRAM_COMPACT_THRESHOLD` (50k) so it also measures
/// a delta compaction (full trigram refold) under load, while the smaller
/// bursts measure the steady-state delta-overlay apply (trigram_us ≈ 0).
const BURSTS: &[usize] = &[1_000, 10_000, 100_000];

/// `~/bin/uffs.exe` — the canonical user-installed **Rust** binary.  Pinned to
/// the explicit `.exe` so a bare `uffs` can't resolve the C++ `uffs.com` via
/// PATHEXT (see usn-verify.rs).  Copy your freshly built binaries into `~/bin`
/// first — the spawned `uffsd.exe` is the one next to this `uffs.exe`.
fn uffs_bin() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .expect("USERPROFILE or HOME must be set");
    let name = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    home.join("bin").join(name)
}

/// Display name for the cosmetic `$ ...` echoes — `uffs.exe`, never bare `uffs`.
fn uffs_display() -> &'static str {
    if cfg!(windows) { "uffs.exe" } else { "uffs" }
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .expect("USERPROFILE or HOME must be set")
}

/// Binaries the rig depends on, copied fresh from the build dir into `~/bin`.
/// `uffs` + `uffsd` are required (the daemon under test); the broker is
/// optional (only present once `uffs-broker` has been built) and copied
/// best-effort so a non-elevated box still re-syncs the two it needs.
const REQUIRED_BINS: &[&str] = &["uffs", "uffsd"];
const OPTIONAL_BINS: &[&str] = &["uffs-broker", "uffsmcp"];

/// Add the platform executable suffix (`.exe` on Windows).
fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

/// Resolve the `release/` dir of **the build cargo actually uses** — honouring
/// `CARGO_TARGET_DIR`, `.cargo/*.toml` `build.target-dir`, etc. — so the rig
/// copies the binary that was just built, not a stale `~/bin` copy (the
/// stale-binary trap that has bitten this dev loop repeatedly).
///
/// Order: explicit `UFFS_RELEASE_DIR` override → `cargo metadata`'s
/// `target_directory` + `release`.
fn release_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("UFFS_RELEASE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("failed to run `cargo metadata` to locate the build dir")?;
    if !out.status.success() {
        bail!(
            "`cargo metadata` failed ({}). Run the rig from inside the repo, or set \
             UFFS_RELEASE_DIR to your build's release dir.",
            out.status
        );
    }
    let json = String::from_utf8_lossy(&out.stdout);
    let target = parse_target_directory(&json).context(
        "could not find target_directory in `cargo metadata` output; \
         set UFFS_RELEASE_DIR explicitly",
    )?;
    Ok(PathBuf::from(target).join("release"))
}

/// Extract the JSON string value of `"target_directory"` from one-line
/// `cargo metadata` output, unescaping `\\`/`\"`/`\/` (Windows paths arrive as
/// `C:\\rust-target\\ttapi`).  No serde dependency — a focused hand-scan.
fn parse_target_directory(json: &str) -> Option<String> {
    let key = "\"target_directory\":\"";
    let start = json.find(key)? + key.len();
    let mut out = String::new();
    let mut chars = json[start..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                other => out.push(other), // \\ -> \, \" -> ", \/ -> /
            },
            other => out.push(other),
        }
    }
    None
}

/// Short HEAD SHA of the repo (`git rev-parse --short HEAD`), for the
/// build-id match guard.  `None` if git is unavailable.
fn git_head_short() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|sha| !sha.is_empty())
}

/// Copy freshly built binaries from the cargo build dir into `~/bin` so the rig
/// always exercises the just-built daemon.  Required bins missing → bail with a
/// "build first" hint; optional bins are copied only if present.
fn sync_bins(bin_dir: &Path) -> Result<()> {
    let src_dir = release_dir()?;
    println!("\n== Bin sync ==");
    println!("  build dir: {}", src_dir.display());
    println!("  dest:      {}", bin_dir.display());
    fs::create_dir_all(bin_dir).with_context(|| format!("create {}", bin_dir.display()))?;

    for name in REQUIRED_BINS {
        let src = src_dir.join(exe(name));
        if !src.exists() {
            bail!(
                "required binary {} not found — build first \
                 (e.g. `cargo build --release -p uffs-cli -p uffs-daemon`).",
                src.display()
            );
        }
        copy_bin(&src, &bin_dir.join(exe(name)))?;
    }
    for name in OPTIONAL_BINS {
        let src = src_dir.join(exe(name));
        if src.exists() {
            copy_bin(&src, &bin_dir.join(exe(name)))?;
        }
    }
    Ok(())
}

/// Copy one binary, reporting its source build mtime so a stale build is
/// visible at a glance.
fn copy_bin(src: &Path, dest: &Path) -> Result<()> {
    let built = src
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .map_or_else(|| "?".to_owned(), |age| format!("{}s ago", age.as_secs()));
    fs::copy(src, dest)
        .with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    println!("  copied {}  (built {built})", dest.display());
    Ok(())
}

/// Run a `uffs.exe` subcommand inheriting stdout/stderr.
fn run(uffs: &Path, args: &[&str]) -> Result<()> {
    println!("\n$ {} {}", uffs_display(), args.join(" "));
    Command::new(uffs)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn uffs {}", args.join(" ")))?;
    Ok(())
}

/// Run a search, return (row_count, captured_stdout).  A row is a quoted CSV
/// data line (minus the header).
fn search(uffs: &Path, term: &str) -> Result<(usize, String)> {
    let output = Command::new(uffs)
        .args([term, "--format", "csv"])
        .output()
        .with_context(|| format!("failed to spawn uffs {term}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let rows = text
        .lines()
        .filter(|line| line.starts_with('"'))
        .count()
        .saturating_sub(1);
    Ok((rows, text))
}

/// Poll `search(term)` until at least `expected` rows are visible or `max_wait`
/// elapses. Returns `(rows_seen, latency, timed_out)` — the wall-clock from the
/// first poll to visibility is the true apply-to-searchable latency (vs. the old
/// fixed-sleep probe which only measured the settle constant).
fn poll_until_visible(
    uffs: &Path,
    term: &str,
    expected: usize,
    max_wait: Duration,
) -> Result<(usize, Duration, bool)> {
    let start = Instant::now();
    loop {
        let (rows, _) = search(uffs, term)?;
        if rows >= expected {
            return Ok((rows, start.elapsed(), false));
        }
        if start.elapsed() >= max_wait {
            return Ok((rows, start.elapsed(), true));
        }
        sleep(POLL_INTERVAL);
    }
}

fn main() -> Result<()> {
    let uffs = uffs_bin();

    // Sync freshly built bins from the actual cargo build dir into ~/bin so the
    // rig never runs a stale daemon.  Capture HEAD so the build-confirmation
    // step can assert the running uffsd is THIS commit.
    let bin_dir = home_dir().join("bin");
    sync_bins(&bin_dir)?;
    let head_sha = git_head_short();

    if !uffs.exists() {
        bail!(
            "uffs binary not found at {} even after bin sync — check the build dir.",
            uffs.display()
        );
    }

    let base = home_dir().join("idxtest");
    let run_dir = base.join("_run");
    println!("== UFFS incremental-index baseline rig ==");
    println!("binary:    {}", uffs.display());
    println!("scratch:   {}", base.display());
    println!("artifacts: {}", run_dir.display());

    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;

    run(&uffs, &["--version"])?;

    // ── Restart the daemon with logging into the artifacts dir ──────────────
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

    let log_path = run_dir.join("uffsd.log");

    // ── 1. BUILD CONFIRMATION — fail fast on a stale binary ─────────────────
    println!("\n== Build confirmation ==");
    let build_line = read_log(&log_path)
        .lines()
        .find(|line| line.contains("IDXDELTA build active"))
        .map(str::to_owned);
    let build_line = match build_line {
        Some(line) => {
            println!("  OK — {}", line.trim());
            line
        }
        None => bail!(
            "no `IDXDELTA build active` line in {} — the running uffsd.exe is NOT an \
             IDXDELTA build. Rebuild then re-run (the rig re-syncs ~/bin for you).",
            log_path.display()
        ),
    };

    // Build-id match guard: the running daemon's git SHA must equal repo HEAD,
    // else a stale uffsd is being exercised (the trap that has cost several
    // 30-min WIN cycles).  `git="<sha>"` is emitted by the IDXDELTA marker.
    if let Some(head) = &head_sha {
        let logged = build_line
            .split("git=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("");
        if logged != head {
            bail!(
                "STALE DAEMON: running uffsd is git={logged:?} but repo HEAD is {head:?}.\n\
                 The bin sync copied HEAD's build, so a daemon from an earlier build is \
                 still resident. Stop it (`uffs --daemon stop`) and re-run.",
            );
        }
        println!("  build-id match: uffsd git={logged} == HEAD {head}");
    }

    // ── 2 + 3. CHURN, TIMING, FRESHNESS ─────────────────────────────────────
    // Each burst is measured independently via a per-round filename prefix so
    // the poll target is exactly that burst's `count` (not the running total),
    // and creation throughput is reported apart from apply-to-visible latency.
    let mut total_created = 0usize;
    for (round, &count) in BURSTS.iter().enumerate() {
        println!("\n== Burst {}: create {count} files ==", round + 1);
        let create_start = Instant::now();
        for i in 0..count {
            fs::write(base.join(format!("idx_{round}_{i}.tmp")), b"x")
                .with_context(|| format!("write idx_{round}_{i}.tmp"))?;
        }
        let create_elapsed = create_start.elapsed();
        total_created += count;

        // Visibility budget scales with batch size: file-creation IO + USN poll
        // + apply + (for the 100k burst) a delta compaction. ~20 s floor plus
        // ~1 s per 5k files → 100k allows ~40 s before flagging a backlog.
        let max_wait = Duration::from_secs(20 + (count as u64) / 5_000);
        let term = format!("idx_{round}_");
        let (rows, latency, timed_out) = poll_until_visible(&uffs, &term, count, max_wait)?;
        let rate = (count as f64) / create_elapsed.as_secs_f64().max(0.001);
        println!(
            "   created {count} in {:.1}s ({:.0} files/s); '{term}' -> {rows}/{count} \
             visible after {:.1}s{}",
            create_elapsed.as_secs_f64(),
            rate,
            latency.as_secs_f64(),
            if timed_out { "  <<< TIMED OUT (apply backlog)" } else { "" },
        );
    }

    // ── Round with a rename + delete (correctness smoke, like the USN flow) ──
    println!("\n== Mutate: rename idx_0_0.tmp -> idx_renamed.log, delete idx_0_1.tmp ==");
    let _ = fs::rename(base.join("idx_0_0.tmp"), base.join("idx_renamed.log"));
    let _ = fs::remove_file(base.join("idx_0_1.tmp"));
    sleep(SETTLE);
    let (renamed_rows, _) = search(&uffs, "idx_renamed")?;
    let (deleted_rows, _) = search(&uffs, "idx_0_1")?;
    println!("   search 'idx_renamed' -> {renamed_rows} (expect >=1)");
    println!("   search 'idx_0_1'     -> {deleted_rows} (expect 0 for the deleted live file)");

    // ── Stop the daemon to flush, then extract + summarise the timing ───────
    println!("\n== Stopping daemon to flush the log ==");
    let _ = Command::new(&uffs).args(["--daemon", "stop"]).status();
    sleep(KILL_SETTLE);

    let log = read_log(&log_path);
    let timing_lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("IDXDELTA-TIMING apply"))
        .collect();
    fs::write(run_dir.join("idx-timing.log"), timing_lines.join("\n"))?;

    let baseline = summarise(&timing_lines);
    println!("\n== BASELINE (per-apply cost breakdown) ==");
    println!("{baseline}");
    fs::write(run_dir.join("baseline.txt"), &baseline)?;

    println!("\n== Done ==");
    println!("Share: {}", run_dir.display());
    println!(
        "Key: baseline.txt (commit per design §8), idx-timing.log (raw IDXDELTA-TIMING), uffsd.log."
    );
    Ok(())
}

/// Mean (and sample count) of a numeric `key=value` tracing field across all
/// lines that carry it.  Field-generic so new IDXDELTA-TIMING fields in later
/// phases need no parser change.
fn field_mean(lines: &[&str], key: &str) -> Option<(f64, usize)> {
    let prefix = format!("{key}=");
    let vals: Vec<f64> = lines
        .iter()
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix(&prefix))
                .and_then(|raw| raw.parse::<f64>().ok())
        })
        .collect();
    if vals.is_empty() {
        None
    } else {
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        Some((mean, vals.len()))
    }
}

/// Build the human-readable baseline: the mean of each per-apply cost field.
/// The apply emits two lines (whole-body clone; per-change loop + rebuild), so
/// `clone_ms` and the rebuild fields come from different lines — we report each
/// mean and the implied full per-apply cost = clone + loop + rebuild.
fn summarise(lines: &[&str]) -> String {
    if lines.is_empty() {
        return "  (no `IDXDELTA-TIMING apply` lines captured — did any apply fire? \
                check uffsd.log / the apply interval)"
            .to_owned();
    }
    let records = lines
        .iter()
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix("records="))
                .and_then(|raw| raw.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0);

    // The daemon logs whole-microsecond fields (`*_us`) — integer, to respect
    // uffs-core's no-float policy; render them as ms here (1 us = 0.001 ms).
    let row = |label: &str, key: &str| -> String {
        match field_mean(lines, key) {
            Some((mean_us, count)) => {
                format!("  mean {label:<10} {:>8.3} ms   (n={count})\n", mean_us / 1000.0)
            }
            None => format!("  mean {label:<10}      -- (no samples)\n"),
        }
    };

    let mean_us = |key: &str| field_mean(lines, key).map_or(0.0, |(mean, _)| mean);
    let implied_ms = (mean_us("clone_us") + mean_us("loop_us") + mean_us("rebuild_us")) / 1000.0;

    let mut out = format!(
        "  apply lines:   {}\n  drive records: {records}\n",
        lines.len()
    );
    out.push_str(&row("clone", "clone_us"));
    out.push_str(&row("loop", "loop_us"));
    out.push_str(&row("children", "children_us"));
    out.push_str(&row("paths", "paths_us"));
    out.push_str(&row("trigram", "trigram_us"));
    out.push_str(&row("ext", "ext_us"));
    out.push_str(&row("rebuild", "rebuild_us"));
    out.push_str(&format!(
        "  ─────────────────────────────────\n  \
         IMPLIED full apply ≈ clone+loop+rebuild = {implied_ms:>8.3} ms   \
         <- the per-apply cost to beat\n"
    ));
    out
}

/// Read the daemon log, tolerating a missing file (returns empty).
fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
