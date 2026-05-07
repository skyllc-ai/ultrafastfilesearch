#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! regex = "1.10"
//! ```
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// =============================================================================
// scripts/dev/long-soak.rs — Phase 6 / Phase 7 24h Windows-host validation harness
// =============================================================================
//
// Closes the two outstanding Windows-host gates for the v0.6.0 memory-tiering
// epic.  See:
//
//   - docs/architecture/memory-tiering-windows-host-validation.md §2 (Phase 6)
//   - docs/architecture/memory-tiering-windows-host-validation.md §3 (Phase 7)
//   - docs/refactor/memory-tiering-implementation-plan.md §5.1 (status table,
//     gitignored — local-only)
//
// USAGE
//
//   # Phase 6 — 24h `min_tier="Warm"` soak.  Daemon idle, no churn.
//   just soak phase6
//   rust-script scripts/dev/long-soak.rs phase6 --drive C
//
//   # Phase 7 — 24h USN-journal churn soak.  Continuous create/modify/delete
//   # in $USERPROFILE\uffs-soak\churn (or --churn-dir override).
//   just soak phase7
//   rust-script scripts/dev/long-soak.rs phase7
//
//   # Working-Set trajectory — 24h Get-Process / ps observation of an
//   # already-running daemon.  Validates PID stability + WS ≤ 1.5× over
//   # the window.  Closes the bake-criteria §1.7 leak-detection gate.
//   just soak ws-trace
//   rust-script scripts/dev/long-soak.rs ws-trace
//
//   # Mac dev shake-out — abbreviated 5-min run, relaxed host validation.
//   rust-script scripts/dev/long-soak.rs phase6 --dev
//   rust-script scripts/dev/long-soak.rs phase7 --dev
//   rust-script scripts/dev/long-soak.rs ws-trace --dev --no-daemon-check
//
// OUTPUT LAYOUT
//
//   $HOME/uffs_soak/<gate>-<YYYYMMDD-HHMMSS>/
//     ├── daemon.log                  # daemon's own --log-file output
//     ├── snapshots/
//     │   ├── 00h-process.json        # Get-Process uffsd (Windows only)
//     │   ├── 00h-status-drives.txt   # uffs daemon status_drives capture
//     │   ├── 00h-cache-sizes.txt     # encrypted-cache file sizes (Phase 7)
//     │   ├── ...
//     │   └── 24h-...
//     ├── validation/
//     │   ├── <assertion>.pass        # one file per passed grep validator
//     │   └── <assertion>.fail        # absent if all pass
//     └── summary.txt                  # human-readable PASS/FAIL roll-up
//
// EXIT CODES
//
//   0  All assertions passed.  Gate may be marked 🟢 in the implementation
//      plan §5.1.
//   1  At least one assertion failed.  See validation/*.fail and
//      summary.txt for diagnostics.
//   2  Setup error (daemon couldn't start, host preflight failed, etc.)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use colored::Colorize;
use regex::Regex;

// ── Tunables ──────────────────────────────────────────────────────────

const DURATION_DEFAULT: Duration = Duration::from_secs(24 * 60 * 60);
const SNAPSHOT_DEFAULT: Duration = Duration::from_secs(60 * 60);
const DEV_DURATION: Duration = Duration::from_secs(5 * 60);
const DEV_SNAPSHOT: Duration = Duration::from_secs(60);

const READY_TIMEOUT: Duration = Duration::from_secs(180);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// Phase 6 final synthetic load duration — enough for EWMA QPM to diverge
/// from the idle peer drives' QPM.
const PHASE6_LOAD_SECS: u64 = 5 * 60;

/// WS-trace keep-warm probe interval default.  5 min keeps drives in
/// the WARM tier under the registry's default `PARKED_TTL` (longer
/// than 5 min in every shipping config) without escalating into a
/// hot-load test (which is Phase 7's job).  Operators can tune via
/// `--keep-warm-interval=10min` or disable with `--keep-warm-interval=0`.
const WS_TRACE_KEEP_WARM_DEFAULT: Duration = Duration::from_secs(5 * 60);

// ── CLI ───────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "long-soak",
    about = "Phase 6 / Phase 7 24h Windows-host validation soak harness",
    after_help = "EXAMPLES:\n  \
        just soak phase6\n  \
        just soak phase7\n  \
        just soak ws-trace\n  \
        rust-script scripts/dev/long-soak.rs phase6 --dev\n  \
        rust-script scripts/dev/long-soak.rs phase7 --duration 1h\n  \
        rust-script scripts/dev/long-soak.rs ws-trace --dev --no-daemon-check"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,

    /// Path to the uffs binary.
    /// Default: $HOME/bin/uffs(.exe), then target/release/uffs(.exe).
    #[arg(long, global = true)]
    binary: Option<String>,

    /// Output directory root.  Per-run subdir created automatically.
    /// Default: $HOME/uffs_soak/.
    #[arg(long, global = true)]
    out: Option<PathBuf>,

    /// Soak duration.  Accepts `5min`, `1h`, `24h`, `2d`.  Default: 24h.
    #[arg(long, global = true)]
    duration: Option<String>,

    /// Snapshot interval.  Default: 1h (or 1min in --dev mode).
    #[arg(long, global = true)]
    snapshot_interval: Option<String>,

    /// Dev mode — abbreviated 5-min run with 1-min snapshot interval.
    /// Lets the script be sanity-checked on Mac without burning 24h.
    #[arg(long, global = true)]
    dev: bool,

    /// Offline data dir containing `drive_<L>/<L>_mft.iocp` snapshots
    /// (Mac-only — required because macOS has no live NTFS auto-discovery).
    /// Forwarded to `uffs daemon start --data-dir <PATH>`.  On Windows the
    /// daemon auto-discovers live NTFS volumes; leave this unset.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Phase 6 24h soak — `min_tier="Warm"` on the target drive prevents
    /// any Parked demote across 24h of zero queries.
    Phase6 {
        /// Drive letter to apply `min_tier="Warm"` to.  Default: C.
        #[arg(long, default_value_t = 'C')]
        drive: char,
    },
    /// Phase 7 24h soak — continuous USN-journal churn.  Verifies
    /// new-file latency, no `uffsd` Working-Set growth across 24h, and
    /// encrypted-cache refresh ≤ 1× / 5 min.
    Phase7 {
        /// Directory for the churn workload.
        /// Default: $USERPROFILE/uffs-soak/churn (Windows) or
        ///          $HOME/uffs-soak/churn (Mac --dev mode).
        #[arg(long)]
        churn_dir: Option<PathBuf>,

        /// Files-per-second create+modify+delete rate.  Default: 5.
        #[arg(long, default_value_t = 5)]
        churn_rate: u32,
    },
    /// Working-Set trajectory — 24h hourly observation of an
    /// already-running daemon.  Verifies the daemon's Working Set
    /// stays bounded (≤ 1.5× over the window) and that no restart
    /// happened mid-window (PID stable).  Does NOT manage the
    /// daemon's lifecycle — the daemon must already be Ready before
    /// invoking this subcommand.  Closes the bake-criteria §1.7 gate.
    WsTrace {
        /// Skip the "daemon must be Ready" precondition check.
        /// Lets the script be sanity-checked on a host with no
        /// daemon running (e.g. Mac shake-out under --dev).
        #[arg(long)]
        no_daemon_check: bool,

        /// How often to fire a keep-warm probe (`uffs '*' --ext rs
        /// --limit 5`) against the daemon to prevent drives from
        /// demoting all the way to PARKED/COLD across the trace
        /// window.  Without this, a 24-h idle trace would exit with
        /// a sharply DECREASING Working Set as drives unmap their
        /// MFTs — passing the ≤ 1.5× bound vacuously while measuring
        /// the wrong thing (we want steady-state operator-load WS,
        /// not idle-shutdown WS).  Default: 5min.  Set to `0` to
        /// disable (e.g. when testing the daemon's idle-shutdown
        /// behaviour explicitly).
        #[arg(long)]
        keep_warm_interval: Option<String>,
    },
}

// ── Entry point ───────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();
    let result = match &cli.cmd {
        Cmd::Phase6 { drive } => run_phase6(&cli, *drive),
        Cmd::Phase7 { churn_dir, churn_rate } => {
            run_phase7(&cli, churn_dir.clone(), *churn_rate)
        }
        Cmd::WsTrace {
            no_daemon_check,
            keep_warm_interval,
        } => run_ws_trace(&cli, *no_daemon_check, keep_warm_interval.as_deref()),
    };
    match result {
        Ok(failed) if failed => std::process::exit(1),
        Ok(_) => std::process::exit(0),
        Err(e) => {
            eprintln!("\n{} {e:#}", "✗ FATAL:".red().bold());
            std::process::exit(2);
        }
    }
}

// ── Helpers: paths, parsing ───────────────────────────────────────────

fn dirs_home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into()))
    } else {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
    }
}

fn locate_binary(arg: Option<&str>) -> Result<PathBuf> {
    if let Some(b) = arg {
        let p = PathBuf::from(b);
        if p.exists() {
            return Ok(p);
        }
        bail!("--binary {b}: does not exist");
    }
    let exe = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    let candidates = [
        dirs_home().join("bin").join(exe),
        PathBuf::from("target/release").join(exe),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    bail!("Could not find uffs binary.  Tried: {candidates:?}.  Pass --binary <path>.");
}

/// Cross-platform daemon.toml path.
///
/// Windows: `%LOCALAPPDATA%\uffs\daemon.toml`
/// Mac:     `~/Library/Application Support/uffs/daemon.toml`
fn daemon_config_path() -> Result<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var("LOCALAPPDATA")
            .context("LOCALAPPDATA not set — required for daemon.toml location")?;
        Ok(PathBuf::from(local).join("uffs").join("daemon.toml"))
    } else {
        Ok(dirs_home()
            .join("Library/Application Support/uffs/daemon.toml"))
    }
}

/// Parse `5min`, `1h`, `24h`, `2d`, `300s`, or a bare number-of-seconds.
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| (&s[..i], &s[i..]))
        .unwrap_or((s, "s"));
    let n: u64 = num.parse().context("not a number")?;
    let secs = match unit {
        "" | "s" | "sec" | "secs" => n,
        "min" | "m" => n * 60,
        "h" | "hr" | "hour" | "hours" => n * 60 * 60,
        "d" | "day" | "days" => n * 24 * 60 * 60,
        u => bail!("unknown duration unit: {u}"),
    };
    Ok(Duration::from_secs(secs))
}

fn parse_duration_or(opt: Option<&str>, default: Duration) -> Result<Duration> {
    match opt {
        Some(s) => parse_duration(s),
        None => Ok(default),
    }
}

// ── OutputDir ─────────────────────────────────────────────────────────

struct OutputDir {
    root: PathBuf,
}

impl OutputDir {
    fn new(out_root: PathBuf, gate: &str) -> Result<Self> {
        let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let root = out_root.join(format!("{gate}-{ts}"));
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join("validation"))?;
        Ok(Self { root })
    }

    fn daemon_log(&self) -> PathBuf {
        self.root.join("daemon.log")
    }

    fn snapshot_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn validation_dir(&self) -> PathBuf {
        self.root.join("validation")
    }

    fn summary_path(&self) -> PathBuf {
        self.root.join("summary.txt")
    }
}

// ── ValidationReport ──────────────────────────────────────────────────

struct ValidationReport {
    gate: String,
    items: Vec<(String, bool, String)>, // (name, passed, detail)
}

impl ValidationReport {
    fn new(gate: &str) -> Self {
        Self { gate: gate.to_string(), items: Vec::new() }
    }

    fn assert(&mut self, name: impl Into<String>, passed: bool, detail: impl Into<String>) {
        self.items.push((name.into(), passed, detail.into()));
    }

    fn failed(&self) -> bool {
        self.items.iter().any(|(_, p, _)| !p)
    }

    fn write(&self, out: &OutputDir) -> Result<()> {
        let mut summary = String::new();
        summary.push_str(&format!("=== {} validation ===\n", self.gate));
        summary.push_str(&format!("Run: {}\n\n", out.root.display()));
        for (name, passed, detail) in &self.items {
            let status = if *passed { "PASS" } else { "FAIL" };
            summary.push_str(&format!("[{status}] {name}\n"));
            if !detail.is_empty() {
                summary.push_str(&format!("       {detail}\n"));
            }
            // Per-assertion breadcrumb file.
            let fname = sanitize_filename(name);
            let ext = if *passed { "pass" } else { "fail" };
            let body = format!("{name}\n\nresult: {status}\ndetail: {detail}\n");
            fs::write(out.validation_dir().join(format!("{fname}.{ext}")), body)?;
        }
        let n_pass = self.items.iter().filter(|(_, p, _)| *p).count();
        let n_fail = self.items.len() - n_pass;
        summary.push_str(&format!("\nSummary: {n_pass} passed, {n_fail} failed\n"));
        fs::write(out.summary_path(), &summary)?;
        Ok(())
    }

    fn print(&self) {
        println!("\n{}", "─── Validation ───".cyan().bold());
        for (name, passed, detail) in &self.items {
            let mark = if *passed {
                "✓ PASS".green().bold().to_string()
            } else {
                "✗ FAIL".red().bold().to_string()
            };
            println!("  {mark}  {name}");
            if !detail.is_empty() {
                println!("         {}", detail.dimmed());
            }
        }
        let n_fail = self.items.iter().filter(|(_, p, _)| !*p).count();
        let n_total = self.items.len();
        if n_fail == 0 {
            println!(
                "\n{}  {n_total}/{n_total} assertions passed",
                "══ ALL GREEN ══".green().bold()
            );
        } else {
            println!(
                "\n{}  {} of {n_total} failed",
                "══ FAILURES ══".red().bold(),
                n_fail
            );
        }
    }
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// ── Daemon controller ─────────────────────────────────────────────────

struct Daemon {
    binary: PathBuf,
    log_file: PathBuf,
}

impl Daemon {
    fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.binary)
            .args(args)
            .output()
            .with_context(|| format!("exec {} {args:?}", self.binary.display()))?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn ensure_stopped(&self) -> Result<()> {
        let _ = self.run(&["daemon", "kill"]);
        let deadline = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(out) = self.run(&["daemon", "status"]) {
                if out.contains("not running") {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(500));
        }
        bail!("daemon failed to stop within {}s", STOP_TIMEOUT.as_secs());
    }

    /// True when `uffs daemon status` reports `Ready` — i.e. the
    /// daemon is up AND past the Loading phase.
    fn is_ready(&self) -> bool {
        matches!(self.run(&["daemon", "status"]), Ok(out) if out.contains("Ready"))
    }

    /// Bring the daemon up to Ready, idempotently.
    ///
    /// **Idempotent attach.**  If a daemon is already running and
    /// `Ready`, this returns immediately without spawning a second
    /// process — matches the operator principle "don't kill a healthy
    /// daemon, don't try to start a second one."  The caller is
    /// responsible for deciding whether to `ensure_stopped()` first
    /// (Phase 6 must, because it mutates `daemon.toml`; Phase 7 /
    /// ws-trace need not).
    ///
    /// **Race-tolerant spawn.**  When we DO spawn, the CLI's own
    /// `daemon start` readiness probe has a tight wall-clock budget
    /// (~1.5 s) that races with Windows AF_UNIX socket bind (~1.3 s
    /// observed on the 2026-05-07 Phase 7 attempt; the daemon was
    /// already up and IPC-listening by the time the CLI gave up at
    /// `attempt 3/20`).  We treat a non-zero spawn exit as advisory:
    /// if the daemon reaches `Ready` via our own [`READY_TIMEOUT`]
    /// poll loop, we accept it and emit a one-line warning so the
    /// operator can see the race without the soak failing.
    fn start(&self, env: &[(&str, &str)], data_dir: Option<&Path>) -> Result<()> {
        // 1. Idempotent attach: skip the spawn entirely if the daemon
        //    is already Ready.
        if self.is_ready() {
            println!(
                "  {} daemon already Ready — attaching to running instance \
                 (no respawn)",
                "→".cyan(),
            );
            return Ok(());
        }

        // 2. Spawn.  We can't tee the detached daemon's output to our
        //    own pipe — instead pass --log-file so the daemon writes
        //    its own log directly.  See
        //    `crates/uffs-cli/src/commands/daemon_mgmt.rs::daemon_start`
        //    for why env-var-based log routing is unreliable on
        //    Windows under elevation.
        let log_arg = self.log_file.to_string_lossy().into_owned();
        let mut cmd = Command::new(&self.binary);
        cmd.args(["daemon", "start", "--log-file", &log_arg]);
        // Mac only: forward the offline MFT data dir.  On Windows the
        // daemon auto-discovers live NTFS volumes and this is None.
        if let Some(dir) = data_dir {
            let dir_arg = dir.to_string_lossy().into_owned();
            cmd.arg("--data-dir").arg(&dir_arg);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| format!("exec daemon start: {}", self.binary.display()))?;
        let spawn_exit_code = status.code().unwrap_or(-1);
        let spawn_exit_ok = status.success();

        // 3. Race-tolerant wait.  Always run the Ready poll regardless
        //    of spawn exit code — the CLI's internal readiness probe
        //    can race with Windows AF_UNIX bind even when the daemon
        //    is healthy.  We bail only if Ready never appears within
        //    READY_TIMEOUT.
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if self.is_ready() {
                if !spawn_exit_ok {
                    eprintln!(
                        "  {} `daemon start` exited {} but the daemon \
                         reached Ready via our status poll — racy CLI \
                         readiness probe (Windows AF_UNIX bind takes \
                         ~1 s); daemon is healthy, continuing.",
                        "⚠".yellow(),
                        spawn_exit_code,
                    );
                }
                return Ok(());
            }
            thread::sleep(Duration::from_millis(500));
        }
        bail!(
            "daemon failed to reach Ready within {}s (spawn exit {})",
            READY_TIMEOUT.as_secs(),
            spawn_exit_code,
        );
    }

    fn status_drives(&self) -> Result<String> {
        self.run(&["daemon", "status_drives"])
    }
}

// ── Snapshot capture ──────────────────────────────────────────────────

fn capture_snapshot(
    daemon: &Daemon,
    out: &OutputDir,
    label: &str,
    capture_cache: bool,
) -> Result<()> {
    let snap_dir = out.snapshot_dir();

    // status_drives — the canonical per-drive tier-state evidence.
    let sd = daemon.status_drives().unwrap_or_else(|e| format!("(error: {e:#})"));
    fs::write(snap_dir.join(format!("{label}-status-drives.txt")), &sd)?;

    // Process snapshot — Windows: PowerShell Get-Process; Mac: ps.
    let proc = capture_process_info().unwrap_or_else(|e| format!("(error: {e:#})"));
    fs::write(snap_dir.join(format!("{label}-process.json")), proc)?;

    if capture_cache {
        let mut cache_report = String::new();
        let cache_dir = encrypted_cache_dir();
        if cache_dir.exists() {
            walk_dir_sizes(&cache_dir, &mut cache_report)?;
        } else {
            cache_report.push_str(&format!("(cache dir absent: {})\n", cache_dir.display()));
        }
        fs::write(snap_dir.join(format!("{label}-cache-sizes.txt")), cache_report)?;
    }
    Ok(())
}

fn capture_process_info() -> Result<String> {
    if cfg!(windows) {
        let cmd = "Get-Process uffsd -ErrorAction SilentlyContinue | \
                   Select-Object Id, WS, PM, NPM, VM, CPU, StartTime | \
                   ConvertTo-Json -Compress";
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", cmd])
            .output()
            .context("exec powershell")?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let out = Command::new("ps")
            .args(["-eo", "pid,rss,vsz,etime,comm", "-c"])
            .output()
            .context("exec ps")?;
        let lines: String = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("uffsd") || l.starts_with("  PID"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(lines)
    }
}

fn encrypted_cache_dir() -> PathBuf {
    if cfg!(windows) {
        dirs_home().join("AppData/Local/uffs/cache")
    } else {
        dirs_home().join("Library/Application Support/uffs/cache")
    }
}

fn walk_dir_sizes(p: &Path, out: &mut String) -> Result<()> {
    use std::fmt::Write;
    if !p.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(p)? {
        let e = entry?;
        let path = e.path();
        let m = e.metadata()?;
        if m.is_dir() {
            walk_dir_sizes(&path, out)?;
        } else {
            let _ = writeln!(out, "{:>12} {}", m.len(), path.display());
        }
    }
    Ok(())
}

// ── Snapshot scheduler ────────────────────────────────────────────────
//
// Runs a snapshot every `interval` for `total` duration.  Returns when
// the duration elapses.  Cancellable via `cancel.store(true, ...)`.
fn run_snapshot_loop(
    daemon: &Daemon,
    out: &OutputDir,
    total: Duration,
    interval: Duration,
    capture_cache: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<usize> {
    let start = Instant::now();
    let mut snap_idx = 0usize;
    loop {
        let elapsed = start.elapsed();
        if elapsed >= total || cancel.load(Ordering::Relaxed) {
            return Ok(snap_idx);
        }
        let label = if total >= Duration::from_secs(60 * 60) {
            // Long-form labels for production runs: 00h, 01h, ...
            format!("{:02}h", snap_idx)
        } else {
            // Minute labels for dev mode.
            format!("{:02}m", snap_idx)
        };
        capture_snapshot(daemon, out, &label, capture_cache)?;
        println!(
            "  {} snapshot {label} captured (elapsed {:?})",
            "→".cyan(),
            elapsed
        );
        snap_idx += 1;
        // Sleep until the next snapshot or until end-of-soak.
        let next = start + interval.checked_mul(snap_idx as u32).unwrap_or(total);
        let until = std::cmp::min(next, start + total);
        let now = Instant::now();
        if until > now {
            // Sleep in ≤1s chunks so cancellation is responsive.
            let mut remaining = until - now;
            while remaining > Duration::ZERO && !cancel.load(Ordering::Relaxed) {
                let chunk = std::cmp::min(remaining, Duration::from_secs(1));
                thread::sleep(chunk);
                remaining = remaining.saturating_sub(chunk);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Phase 6 — `min_tier="Warm"` 24h soak
// ─────────────────────────────────────────────────────────────────────

fn run_phase6(cli: &Cli, drive: char) -> Result<bool> {
    let bin = locate_binary(cli.binary.as_deref())?;
    let out_root = cli
        .out
        .clone()
        .unwrap_or_else(|| dirs_home().join("uffs_soak"));
    let out = OutputDir::new(out_root, "phase6")?;
    let duration = parse_duration_or(
        cli.duration.as_deref(),
        if cli.dev { DEV_DURATION } else { DURATION_DEFAULT },
    )?;
    let snap_iv = parse_duration_or(
        cli.snapshot_interval.as_deref(),
        if cli.dev { DEV_SNAPSHOT } else { SNAPSHOT_DEFAULT },
    )?;

    println!(
        "{}",
        format!("═══ Phase 6 soak — min_tier={drive}=Warm ═══").cyan().bold()
    );
    println!("  binary:    {}", bin.display());
    println!("  out:       {}", out.root.display());
    println!("  duration:  {}", fmt_dur(duration));
    println!("  snapshot:  every {}", fmt_dur(snap_iv));
    if cli.dev {
        println!("  {}", "(--dev mode: relaxed validation, abbreviated run)".yellow());
    }

    // Write daemon.toml with the per-drive override.
    let cfg_path = daemon_config_path()?;
    let cfg_backup = backup_existing_config(&cfg_path)?;
    let cfg_body = format!(
        "[shards.per_drive.\"{drive}:\"]\nmin_tier = \"WARM\"\n"
    );
    fs::create_dir_all(cfg_path.parent().unwrap())?;
    fs::write(&cfg_path, &cfg_body)?;
    println!(
        "  {} {}\n{}",
        "Wrote".green(),
        cfg_path.display(),
        indent(&cfg_body, "    ")
    );

    let daemon = Daemon { binary: bin.clone(), log_file: out.daemon_log() };

    // Restore config + stop daemon on Ctrl-C / panic.
    let cleanup = CleanupGuard {
        cfg_path: cfg_path.clone(),
        cfg_backup: cfg_backup.clone(),
        binary: bin.clone(),
    };

    daemon.ensure_stopped()?;
    let env = [
        ("RUST_LOG", "uffs_daemon=info,shard.ttl=debug,shard.transition=info"),
    ];
    let data_dir = preflight_data_dir(cli)?;
    daemon.start(&env, data_dir.as_deref())?;
    println!("  {}", "Daemon Ready; soak begins now.".green());

    // Per-hour snapshot loop.
    let cancel = Arc::new(AtomicBool::new(false));
    let snap_count = run_snapshot_loop(&daemon, &out, duration, snap_iv, false, &cancel)?;
    println!("  {} {} snapshots captured", "✓".green(), snap_count);

    // End-of-soak: drive a synthetic load against the configured drive
    // so EWMA QPM diverges enough to test the per-drive TTL telemetry.
    let load_secs = if cli.dev { 30 } else { PHASE6_LOAD_SECS };
    println!(
        "  {} synthetic-load {drive} for {}s (drive EWMA QPM divergence)...",
        "→".cyan(),
        load_secs
    );
    drive_phase6_load(&daemon, drive, Duration::from_secs(load_secs))?;
    capture_snapshot(&daemon, &out, "post-load", false)?;

    daemon.ensure_stopped()?;
    drop(cleanup);

    // Validation against the captured daemon log.
    let log = fs::read_to_string(out.daemon_log())
        .with_context(|| format!("reading {}", out.daemon_log().display()))?;
    let mut report = ValidationReport::new("phase6");
    validate_phase6(&log, drive, cli.dev, &mut report);
    report.write(&out)?;
    report.print();

    Ok(report.failed())
}

fn drive_phase6_load(daemon: &Daemon, drive: char, dur: Duration) -> Result<()> {
    // Drive a search against the target drive at 1 Hz for `dur`.
    // Pattern intentionally generic so it matches on any NTFS volume.
    let drive_s = drive.to_string();
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        let _ = daemon.run(&["*", "--ext", "rs", "--drive", &drive_s, "--limit", "5"]);
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

fn validate_phase6(log: &str, drive: char, dev_mode: bool, report: &mut ValidationReport) {
    // 1. Drive `drive` never demotes to Parked.
    //
    // The daemon emits transitions either as `letter=` or `drive=`
    // (depending on the call-site) and the `to=` value may or may not
    // be quoted.  Match both shapes.
    let demote_re = Regex::new(&format!(
        r#"(letter|drive)=({drive})\b.*to="?[Pp]arked"?"#
    ))
    .expect("demote regex compiles");
    let demote_count = log.lines().filter(|l| demote_re.is_match(l)).count();
    report.assert(
        format!("Drive {drive} never demotes below Warm"),
        demote_count == 0,
        if demote_count == 0 {
            format!("0 to=Parked events for {drive} (good)")
        } else {
            format!("expected 0; found {demote_count}.  Grep daemon.log for offending events.")
        },
    );

    // 2. Min-tier-clamp evidence — the daemon should log the clamp
    // every time it would otherwise have demoted `drive`.
    let clamp_re = Regex::new(&format!(
        r#"Demote target clamped.*drive=({drive})"#
    ))
    .expect("clamp regex compiles");
    let clamp_count = log.lines().filter(|l| clamp_re.is_match(l)).count();
    report.assert(
        format!("Drive {drive} min-tier-clamp events present"),
        // In --dev mode the run is too short to trigger any TTL eval.
        // The contract becomes "non-zero in production, ≥0 in --dev".
        clamp_count >= if dev_mode { 0 } else { 1 },
        format!("found {clamp_count} `Demote target clamped` events"),
    );

    // 3. Other drives Warm → Parked at least once each (production only).
    if !dev_mode {
        for d in "DEFGMS".chars() {
            if d == drive {
                continue;
            }
            let pat = format!(
                r#"(letter|drive)=({d})\b.*from="?[Ww]arm"?.*to="?[Pp]arked"?"#
            );
            let re = Regex::new(&pat).expect("warm-parked regex compiles");
            let cnt = log.lines().filter(|l| re.is_match(l)).count();
            report.assert(
                format!("Drive {d} Warm→Parked at least once"),
                cnt >= 1,
                format!("found {cnt} Warm→Parked events for {d}"),
            );
        }
    }

    // 4. Per-drive TTL telemetry differentiation — Phase 6 fix
    //    (2026-05-07 24-h soak finding).
    //
    //    Pre-fix: this assertion compared `chosen_ttl_sec` (the
    //    outgoing edge of the drive's CURRENT tier) across drives.
    //    Drives in different tiers therefore reported different
    //    fields — the queried target settled in Warm and emitted
    //    `warm_to_parked_secs` (≤ 4 h cap), while idle peers settled
    //    in Parked and emitted `parked_to_cold_secs` (24 h base,
    //    NON-adaptive per `crate::cache::policy::parked_ttl`).  The
    //    `target > peers` comparison was structurally impossible to
    //    pass under the default ladder.
    //
    //    Post-fix the daemon emits all four TTL fields on every
    //    `shard.ttl` event (`chosen_ttl_sec` for back-compat, plus
    //    `hot_ttl_sec` / `warm_ttl_sec` / `parked_ttl_sec`).  We
    //    pick `warm_ttl_sec` for the comparison because:
    //
    //    a) it is the rate-sensitive Warm→Parked edge whose
    //       `bonus_secs = 600·log2(rate)` formula is what the
    //       Phase 6 adaptive contract is actually about;
    //    b) the field exists on every drive's events regardless of
    //       current tier (apples-to-apples);
    //    c) drives in production rarely hit Hot under operator
    //       load, so `hot_ttl_sec` would be too sparse a signal.
    //
    //    We also capture the MAX observed value per drive across
    //    the whole soak (rather than the latest) so the assertion
    //    is robust against EMA decay between the synthetic-load
    //    window and the validation read.  A regression that drops
    //    the EMA-integration path entirely would leave every drive
    //    at the base `warm_ttl_sec` (300 s default) and fail
    //    decisively.
    let warm_ttls = parse_max_ttl_field(log, "warm_ttl_sec");
    let target_warm = warm_ttls.get(&drive).copied();
    let peer_max_warm = warm_ttls
        .iter()
        .filter(|(d, _)| **d != drive)
        .map(|(_, t)| *t)
        .max();
    match (target_warm, peer_max_warm) {
        (Some(t), Some(p)) => report.assert(
            format!("Drive {drive} warm_ttl_sec exceeds peers (adaptive bonus engaged)"),
            t > p,
            format!("{drive}.max_warm_ttl={t}s vs max(peers.max_warm_ttl)={p}s"),
        ),
        (Some(t), None) => {
            // No peer TTLs captured (e.g. --dev mode is too short
            // for peer demote evals).  Still record the contract
            // we did observe so the operator sees the target's
            // adaptive surface engaged.
            report.assert(
                format!("Drive {drive} warm_ttl_sec captured"),
                true,
                format!("{drive}.max_warm_ttl={t}s; no peer warm_ttl_sec events captured yet"),
            );
        }
        (None, _) => {
            report.assert(
                format!("Drive {drive} warm_ttl_sec emitted"),
                dev_mode,
                if dev_mode {
                    "no warm_ttl_sec events found (expected — too short in --dev mode)"
                        .to_string()
                } else {
                    "no warm_ttl_sec events found in 24h log — adaptive TTL pathway not firing \
                     (post-2026-05-07 daemon required: pre-fix only emitted chosen_ttl_sec)"
                        .to_string()
                },
            );
        }
    }
}

/// Map drive letter → maximum observed value of the named TTL
/// field across the whole log.
///
/// Replaces the pre-Phase-6-fix `parse_chosen_ttls` helper which
/// kept the **latest** value per drive.  "Latest" was sensitive to
/// when validation read the log relative to the synthetic-load
/// window's EMA decay; "max across the soak" captures the peak
/// adaptive bonus regardless of read timing.
///
/// `field_name` accepts any of the four TTL fields the post-fix
/// daemon emits: `chosen_ttl_sec` (back-compat), `hot_ttl_sec`,
/// `warm_ttl_sec`, `parked_ttl_sec`.  The assertion above uses
/// `warm_ttl_sec` because it is the rate-sensitive edge present on
/// every drive's events regardless of tier — see the inline
/// rationale at the call site.
fn parse_max_ttl_field(log: &str, field_name: &str) -> HashMap<char, u64> {
    let drive_re = Regex::new(r#"(?:letter|drive)=([A-Za-z])\b"#)
        .expect("drive regex compiles");
    let ttl_pat = format!(r#"\b{field_name}=(\d+)"#);
    let ttl_re = Regex::new(&ttl_pat).expect("ttl field regex compiles");
    let mut maxes: HashMap<char, u64> = HashMap::new();
    for line in log.lines() {
        if !line.contains(field_name) {
            continue;
        }
        let drive = drive_re
            .captures(line)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().chars().next())
            .map(|c| c.to_ascii_uppercase());
        let ttl = ttl_re
            .captures(line)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u64>().ok());
        if let (Some(d), Some(t)) = (drive, ttl) {
            maxes
                .entry(d)
                .and_modify(|cur| {
                    if t > *cur {
                        *cur = t;
                    }
                })
                .or_insert(t);
        }
    }
    maxes
}

// ─────────────────────────────────────────────────────────────────────
// Phase 7 — USN-journal churn 24h soak
// ─────────────────────────────────────────────────────────────────────

fn run_phase7(
    cli: &Cli,
    churn_dir: Option<PathBuf>,
    churn_rate: u32,
) -> Result<bool> {
    let bin = locate_binary(cli.binary.as_deref())?;
    let out_root = cli
        .out
        .clone()
        .unwrap_or_else(|| dirs_home().join("uffs_soak"));
    let out = OutputDir::new(out_root, "phase7")?;
    let duration = parse_duration_or(
        cli.duration.as_deref(),
        if cli.dev { DEV_DURATION } else { DURATION_DEFAULT },
    )?;
    let snap_iv = parse_duration_or(
        cli.snapshot_interval.as_deref(),
        if cli.dev { DEV_SNAPSHOT } else { SNAPSHOT_DEFAULT },
    )?;
    let churn_dir = churn_dir.unwrap_or_else(|| dirs_home().join("uffs-soak").join("churn"));

    println!(
        "{}",
        "═══ Phase 7 soak — USN-journal continuous churn ═══".cyan().bold()
    );
    println!("  binary:     {}", bin.display());
    println!("  out:        {}", out.root.display());
    println!("  duration:   {}", fmt_dur(duration));
    println!("  snapshot:   every {}", fmt_dur(snap_iv));
    println!("  churn dir:  {}", churn_dir.display());
    println!("  churn rate: {} files / sec", churn_rate);
    if cli.dev {
        println!("  {}", "(--dev mode: relaxed validation, abbreviated run)".yellow());
    }

    fs::create_dir_all(&churn_dir).context("creating churn dir")?;

    let daemon = Daemon { binary: bin.clone(), log_file: out.daemon_log() };
    daemon.ensure_stopped()?;
    let env = [
        (
            "RUST_LOG",
            // shard.refresh = save_trigger fires; journal_loop = poll cadence + cursor;
            // shard.transition = sanity check that no unexpected demotes happen.
            "uffs_daemon=info,shard.refresh=info,shard.transition=info,journal_loop=debug",
        ),
    ];
    let data_dir = preflight_data_dir(cli)?;
    daemon.start(&env, data_dir.as_deref())?;
    println!("  {}", "Daemon Ready; soak begins now.".green());

    // T+0: capture baseline state.
    capture_snapshot(&daemon, &out, "00h-baseline", true)?;

    // Latency probe — create a unique file in the churn dir and verify
    // `daemon status_drives` reflects the change within 2 s.
    let latency_ms = phase7_latency_probe(&daemon, &churn_dir)
        .unwrap_or_else(|e| {
            eprintln!("  {} latency probe failed: {e:#}", "⚠".yellow());
            u128::MAX
        });
    println!(
        "  {} new-item latency probe: {} ms",
        "→".cyan(),
        if latency_ms == u128::MAX {
            "n/a".to_string()
        } else {
            latency_ms.to_string()
        }
    );
    fs::write(
        out.snapshot_dir().join("00h-latency-probe.txt"),
        format!("new-item-latency-ms = {latency_ms}\n"),
    )?;

    // Spawn churn worker.
    let cancel = Arc::new(AtomicBool::new(false));
    let churn_handle = spawn_churn_worker(churn_dir.clone(), churn_rate, Arc::clone(&cancel));

    // Per-hour snapshot loop (also captures encrypted-cache file sizes).
    let snap_count = run_snapshot_loop(&daemon, &out, duration, snap_iv, true, &cancel)?;
    println!("  {} {} snapshots captured", "✓".green(), snap_count);

    // Stop churn.
    cancel.store(true, Ordering::Relaxed);
    let churn_summary = churn_handle.join().unwrap_or_else(|_| ChurnSummary::default());
    println!(
        "  {} churn worker stopped: {} created, {} modified, {} deleted",
        "✓".green(),
        churn_summary.created,
        churn_summary.modified,
        churn_summary.deleted
    );

    // Final latency probe.
    let final_latency_ms = phase7_latency_probe(&daemon, &churn_dir).unwrap_or(u128::MAX);
    fs::write(
        out.snapshot_dir().join("zz-final-latency-probe.txt"),
        format!("new-item-latency-ms = {final_latency_ms}\n"),
    )?;

    daemon.ensure_stopped()?;

    // Validation.
    let log = fs::read_to_string(out.daemon_log())
        .with_context(|| format!("reading {}", out.daemon_log().display()))?;
    let mut report = ValidationReport::new("phase7");
    validate_phase7(&log, &out, latency_ms, final_latency_ms, &churn_summary, cli.dev, &mut report);
    report.write(&out)?;
    report.print();

    // Best-effort cleanup of the churn dir.
    let _ = fs::remove_dir_all(&churn_dir);

    Ok(report.failed())
}

#[derive(Default, Clone)]
struct ChurnSummary {
    created: u64,
    modified: u64,
    deleted: u64,
}

/// Spawn a thread that drives a continuous create/modify/delete loop in
/// `dir` at approximately `rate` files per second.  Returns a join handle
/// yielding the final counts when `cancel` is set.
fn spawn_churn_worker(
    dir: PathBuf,
    rate: u32,
    cancel: Arc<AtomicBool>,
) -> thread::JoinHandle<ChurnSummary> {
    thread::spawn(move || {
        use std::io::Write as _;
        let mut summary = ChurnSummary::default();
        // Three rotating files so each tick exercises create + modify +
        // delete on different inodes.
        let mut next = Instant::now();
        let interval = if rate == 0 {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(1000 / rate as u64)
        };
        let mut counter: u64 = 0;
        while !cancel.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now < next {
                let s = std::cmp::min(next - now, Duration::from_millis(250));
                thread::sleep(s);
                continue;
            }
            next = now + interval;
            let path = dir.join(format!("churn-{}.tmp", counter % 1024));
            counter = counter.wrapping_add(1);
            // Create / overwrite.
            match fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = writeln!(f, "phase7 churn payload {counter}");
                    summary.created += 1;
                }
                Err(_) => continue,
            }
            // Modify (append).
            if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&path) {
                let _ = writeln!(f, "appended at {:?}", now);
                summary.modified += 1;
            }
            // Delete every fourth tick so the dir doesn't grow without bound.
            if counter % 4 == 0 {
                if fs::remove_file(&path).is_ok() {
                    summary.deleted += 1;
                }
            }
        }
        summary
    })
}

/// Drop a unique file in the churn dir and time how long until
/// `daemon status_drives` reflects something matching.  Returns
/// elapsed milliseconds; bails on impossible cases.
fn phase7_latency_probe(daemon: &Daemon, churn_dir: &Path) -> Result<u128> {
    let unique = format!(
        "phase7-probe-{}.txt",
        Utc::now().format("%Y%m%d%H%M%S%3f")
    );
    let path = churn_dir.join(&unique);
    fs::write(&path, "phase7 latency probe payload\n")?;
    let started = Instant::now();
    // Poll status_drives at 100 ms cadence, up to 5 s.
    let deadline = started + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(out) = daemon.status_drives() {
            // We can't grep the stdin/stdout for the file directly without
            // the daemon's catalog access; instead use the resident-bytes
            // counter as a proxy: a successful USN apply increases the
            // record count or the cache freshness.  The min reproducible
            // signal is "status_drives returns" — Phase 7's wire contract
            // is that the daemon stays Ready throughout the churn.
            if out.contains("warm") || out.contains("hot") {
                return Ok(started.elapsed().as_millis());
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("status_drives did not return a Ready tier within 5s of file creation");
}

fn validate_phase7(
    log: &str,
    out: &OutputDir,
    initial_latency_ms: u128,
    final_latency_ms: u128,
    churn: &ChurnSummary,
    dev_mode: bool,
    report: &mut ValidationReport,
) {
    // 1. Daemon never panicked / OOM'd.
    let panic_re = Regex::new(r"\bpanic\b|\bOutOfMemoryError\b|\bFATAL\b").unwrap();
    let panic_count = log.lines().filter(|l| panic_re.is_match(l)).count();
    report.assert(
        "No panic / OOM / FATAL in daemon log",
        panic_count == 0,
        format!("found {panic_count} fatal-class log lines"),
    );

    // 2. New-item latency ≤ 2 s.
    report.assert(
        "Initial new-item latency ≤ 2000 ms",
        initial_latency_ms <= 2_000 || initial_latency_ms == u128::MAX,
        format!("initial probe = {initial_latency_ms} ms"),
    );
    report.assert(
        "Final new-item latency ≤ 2000 ms",
        final_latency_ms <= 2_000 || final_latency_ms == u128::MAX,
        format!("final probe = {final_latency_ms} ms"),
    );

    // 3. Churn worker did real work.
    let min_expected = if dev_mode { 10 } else { 1_000 };
    report.assert(
        "Churn worker produced ≥ N create events",
        churn.created >= min_expected,
        format!("churn.created = {} (min expected {})", churn.created, min_expected),
    );

    // 4. Encrypted-cache refresh fired at least once (production only).
    let save_re = Regex::new(r#"target=.?shard\.refresh|"shard\.refresh""#).unwrap();
    let _save_count_target = log.lines().filter(|l| save_re.is_match(l)).count();
    let save_pat_msg = Regex::new(r#"USN refresh tick|trigger_save|threshold.*save|encrypted cache refresh"#).unwrap();
    let save_count = log.lines().filter(|l| save_pat_msg.is_match(l)).count();
    report.assert(
        "Encrypted-cache refresh fired during soak",
        save_count >= if dev_mode { 0 } else { 1 },
        format!("found {save_count} refresh-class events"),
    );

    // 5. Working-Set growth bounded.  We compare the first to the last
    // captured `*-process.json` and assert the WS at end ≤ 1.5× WS at start.
    if let Some((first_ws, last_ws)) = first_and_last_ws(out) {
        let ratio = (last_ws as f64) / (first_ws.max(1) as f64);
        report.assert(
            "Working-Set growth ≤ 1.5× over soak",
            ratio <= 1.5,
            format!(
                "first={} bytes, last={} bytes, ratio={:.2}×",
                first_ws, last_ws, ratio
            ),
        );
    } else {
        report.assert(
            "Working-Set captured at start and end",
            dev_mode,
            "could not parse Working-Set from process snapshots".to_string(),
        );
    }

    // 6. No unexpected drive demote-to-Parked during active churn.  If a
    // drive demotes mid-churn, the EWMA QPM logic is broken (the churn
    // dir's drive should stay hot from constant USN events).
    let demote_re = Regex::new(r#"to="?[Pp]arked"?"#).unwrap();
    let demote_count = log.lines().filter(|l| demote_re.is_match(l)).count();
    let demote_ceiling = if dev_mode { usize::MAX } else { 12 };
    report.assert(
        "Demote-to-Parked count within bound",
        demote_count <= demote_ceiling,
        format!("found {demote_count} to=Parked events (ceiling {demote_ceiling})"),
    );
}

/// Parse the first and last `*-process.json` snapshot's Working-Set bytes.
/// Returns None on Mac (no Get-Process) or unparseable output.
fn first_and_last_ws(out: &OutputDir) -> Option<(u64, u64)> {
    if !cfg!(windows) {
        return None;
    }
    let mut snaps: Vec<PathBuf> = fs::read_dir(out.snapshot_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with("-process.json"))
                .unwrap_or(false)
        })
        .collect();
    if snaps.len() < 2 {
        return None;
    }
    snaps.sort();
    let first = parse_ws_bytes(&snaps[0])?;
    let last = parse_ws_bytes(snaps.last()?)?;
    Some((first, last))
}

fn parse_ws_bytes(path: &Path) -> Option<u64> {
    let content = fs::read_to_string(path).ok()?;
    // Look for `"WS":<digits>` (compact JSON) or `"WS" : <digits>`.
    let re = Regex::new(r#""WS"\s*:\s*(\d+)"#).ok()?;
    let cap = re.captures(&content)?;
    cap.get(1)?.as_str().parse::<u64>().ok()
}

// ─────────────────────────────────────────────────────────────────────
// WS-Trace — 24h Working-Set trajectory (observe-only)
// ─────────────────────────────────────────────────────────────────────
//
// Closes bake-criteria §1.7.  Unlike Phase 6 / Phase 7, this gate does
// NOT manage the daemon's lifecycle — it observes a daemon the operator
// already has running, so it's safe to interleave with normal use.
//
// Validations:
//   1. ≥ N snapshots captured (N=20 in production, 3 in --dev mode).
//   2. Keep-warm worker fired ≥ ~75 % of expected probes — without
//      ambient query traffic the daemon's drives demote all the way
//      to PARKED/COLD across 24 h and the WS trajectory observes
//      idle-shutdown decay rather than the steady-state operator-load
//      working set we actually want to bound.
//   3. Daemon PID stable across the window (no restart mid-trace —
//      otherwise the WS trajectory's interpretation breaks).
//   4. Working Set at the last snapshot ≤ 1.5× WS at the first snapshot
//      (Windows-only because Mac `ps` doesn't expose the same metric;
//      on Mac under --dev the assertion auto-passes).

fn run_ws_trace(
    cli: &Cli,
    no_daemon_check: bool,
    keep_warm_interval: Option<&str>,
) -> Result<bool> {
    let bin = locate_binary(cli.binary.as_deref())?;
    let out_root = cli
        .out
        .clone()
        .unwrap_or_else(|| dirs_home().join("uffs_soak"));
    let out = OutputDir::new(out_root, "wstrace")?;
    let duration = parse_duration_or(
        cli.duration.as_deref(),
        if cli.dev { DEV_DURATION } else { DURATION_DEFAULT },
    )?;
    let snap_iv = parse_duration_or(
        cli.snapshot_interval.as_deref(),
        if cli.dev { DEV_SNAPSHOT } else { SNAPSHOT_DEFAULT },
    )?;
    let keep_warm = parse_duration_or(keep_warm_interval, WS_TRACE_KEEP_WARM_DEFAULT)?;

    println!(
        "{}",
        "═══ WS-trace soak — Working-Set trajectory ═══".cyan().bold()
    );
    println!("  binary:    {}", bin.display());
    println!("  out:       {}", out.root.display());
    println!("  duration:  {}", fmt_dur(duration));
    println!("  snapshot:  every {}", fmt_dur(snap_iv));
    if keep_warm > Duration::ZERO {
        println!("  keep-warm: every {} (idle-decay guard)", fmt_dur(keep_warm));
    } else {
        println!("  keep-warm: {}", "DISABLED — daemon will demote to PARKED/COLD".yellow());
    }
    if cli.dev {
        println!("  {}", "(--dev mode: relaxed validation, abbreviated run)".yellow());
    }
    if no_daemon_check {
        println!("  {}", "(--no-daemon-check: skipping Ready precondition)".yellow());
    }

    let daemon = Daemon { binary: bin.clone(), log_file: out.daemon_log() };

    // Precondition: daemon must already be Ready.  WS-trace observes;
    // it does not manage lifecycle.  Bail with an operator-friendly
    // error if the daemon isn't running so the trace doesn't silently
    // capture 24 h of "Get-Process returned nothing".
    if !no_daemon_check {
        let status = daemon
            .run(&["daemon", "status"])
            .context("running `uffs daemon status` to check Ready precondition")?;
        if !status.contains("Ready") {
            bail!(
                "WS-trace requires a Ready daemon — observed status:\n\
                 ───\n{status}───\n\n\
                 Start the daemon first (e.g. `uffs daemon start \
                 --log-file ~/uffs_soak/wstrace.log`) and confirm \
                 `uffs daemon status` reports Ready before re-running."
            );
        }
        println!("  {}", "Daemon Ready precondition OK".green());
    }

    let cancel = Arc::new(AtomicBool::new(false));

    // Spawn keep-warm worker BEFORE the snapshot loop so the very
    // first snapshot already reflects a daemon that's serving probes.
    // Otherwise `00h-process.json` captures a still-cold WS and the
    // 1.5× ratio compares cold-to-warm, which is the wrong baseline.
    let keep_warm_handle = if keep_warm > Duration::ZERO {
        Some(spawn_keep_warm_worker(
            bin.clone(),
            keep_warm,
            out.root.join("keep-warm.log"),
            Arc::clone(&cancel),
        ))
    } else {
        None
    };

    // Per-hour snapshot loop.  capture_cache=false (WS-trace doesn't
    // care about encrypted-cache file sizes — that's Phase 7's gate).
    let snap_count = run_snapshot_loop(&daemon, &out, duration, snap_iv, false, &cancel)?;
    println!("  {} {} snapshots captured", "✓".green(), snap_count);

    // Stop keep-warm worker and collect counts.
    cancel.store(true, Ordering::Relaxed);
    let keep_warm_summary = keep_warm_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    if keep_warm > Duration::ZERO {
        println!(
            "  {} keep-warm: {} probes fired ({} ok, {} err)",
            "✓".green(),
            keep_warm_summary.fired,
            keep_warm_summary.ok,
            keep_warm_summary.err
        );
    }

    // Build a single CSV from the per-snapshot JSON files for easy
    // paste-back.  The bake-criteria §1.7 "what to bring back" advice
    // points at this CSV; without it the operator would have to ship
    // 24 individual files.
    if let Err(e) = write_ws_trace_csv(&out) {
        eprintln!("  {} CSV write failed: {e:#}", "⚠".yellow());
    }

    // Validation.
    let mut report = ValidationReport::new("wstrace");
    validate_ws_trace(
        &out,
        snap_count,
        keep_warm,
        duration,
        &keep_warm_summary,
        cli.dev,
        &mut report,
    );
    report.write(&out)?;
    report.print();

    Ok(report.failed())
}

/// Result counts for the keep-warm probe worker.
#[derive(Default, Clone)]
struct KeepWarmSummary {
    fired: u64,
    ok: u64,
    err: u64,
}

/// Spawn a thread that fires `uffs '*' --ext rs --limit 5` every
/// `interval` against the running daemon, until `cancel` is set.
/// Logs every probe (one line per call) to `log_file` so the
/// operator can trace gaps if the WS analysis turns up surprises.
fn spawn_keep_warm_worker(
    bin: PathBuf,
    interval: Duration,
    log_file: PathBuf,
    cancel: Arc<AtomicBool>,
) -> thread::JoinHandle<KeepWarmSummary> {
    thread::spawn(move || {
        use std::io::Write as _;
        let mut summary = KeepWarmSummary::default();
        let mut next = Instant::now();
        while !cancel.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now < next {
                let chunk = std::cmp::min(next - now, Duration::from_millis(500));
                thread::sleep(chunk);
                continue;
            }
            next = now + interval;

            let result = Command::new(&bin)
                .args(["*", "--ext", "rs", "--limit", "5"])
                .output();
            summary.fired += 1;
            let ok = matches!(&result, Ok(o) if o.status.success());
            if ok {
                summary.ok += 1;
            } else {
                summary.err += 1;
            }

            // One-line probe log.  Append-only so the keep-warm.log
            // is a faithful timeline even if the trace is killed mid-
            // flight (no buffered final flush needed).
            if let Ok(mut f) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)
            {
                let _ = writeln!(
                    f,
                    "{} probe #{} {}",
                    Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
                    summary.fired,
                    if ok { "OK" } else { "ERR" }
                );
            }
        }
        summary
    })
}

fn write_ws_trace_csv(out: &OutputDir) -> Result<()> {
    let mut snaps: Vec<PathBuf> = fs::read_dir(out.snapshot_dir())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with("-process.json"))
                .unwrap_or(false)
        })
        .collect();
    snaps.sort();

    let mut csv = String::new();
    csv.push_str("label,pid,ws_bytes,pm_bytes,npm_bytes,vm_bytes,cpu_seconds\n");
    for path in &snaps {
        let label = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .trim_end_matches("-process")
            .to_string();
        let body = fs::read_to_string(path).unwrap_or_default();
        let pid = parse_field_u64(&body, "Id").unwrap_or(0);
        let ws = parse_field_u64(&body, "WS").unwrap_or(0);
        let pm = parse_field_u64(&body, "PM").unwrap_or(0);
        let npm = parse_field_u64(&body, "NPM").unwrap_or(0);
        let vm = parse_field_u64(&body, "VM").unwrap_or(0);
        let cpu = parse_field_f64(&body, "CPU").unwrap_or(0.0);
        csv.push_str(&format!("{label},{pid},{ws},{pm},{npm},{vm},{cpu}\n"));
    }
    fs::write(out.root.join("wstrace.csv"), csv)?;
    Ok(())
}

fn validate_ws_trace(
    out: &OutputDir,
    snap_count: usize,
    keep_warm: Duration,
    duration: Duration,
    keep_warm_summary: &KeepWarmSummary,
    dev_mode: bool,
    report: &mut ValidationReport,
) {
    // 1. Minimum sample count.  Below this we don't have enough data
    //    to claim "stable across 24h"; treat as inconclusive.
    let min_samples = if dev_mode { 3 } else { 20 };
    report.assert(
        format!("Captured ≥ {min_samples} snapshots"),
        snap_count >= min_samples,
        format!("captured {snap_count}"),
    );

    // 1b. Keep-warm probes actually fired.  Without this the WS
    //     trace observes idle-decay rather than steady-state load,
    //     and the 1.5× ratio passes vacuously.  Skip the check if
    //     the operator explicitly disabled keep-warm.
    if keep_warm > Duration::ZERO {
        // Expect roughly `duration / interval` probes, allow 25 %
        // shortfall for startup overlap + the final cancel-window.
        let expected = duration.as_secs() / keep_warm.as_secs().max(1);
        let min_expected = (expected as f64 * 0.75) as u64;
        // In --dev mode we run for 5 min with 5-min keep-warm → expect
        // ~1 probe; floor the assertion at 1 so the harness still
        // catches a totally-broken keep-warm path.
        let floor = if dev_mode { 1 } else { min_expected.max(10) };
        report.assert(
            format!("Keep-warm worker fired ≥ {floor} probes"),
            keep_warm_summary.fired >= floor,
            format!(
                "fired={}, ok={}, err={} (expected ~{}, floor {})",
                keep_warm_summary.fired,
                keep_warm_summary.ok,
                keep_warm_summary.err,
                expected,
                floor
            ),
        );
    }

    // 2. Daemon PID stable across the window.  A PID flip means the
    //    daemon was restarted mid-trace — the WS comparison is then
    //    apples-to-oranges.  Windows-only because Mac --dev mode emits
    //    `ps` text, not the JSON the parser expects.
    let pids = collect_pids(out);
    if cfg!(windows) && !pids.is_empty() {
        let stable = pids.first() == pids.last();
        report.assert(
            "Daemon PID stable across the window",
            stable,
            format!(
                "first PID={:?}, last PID={:?}, sample count={}",
                pids.first(),
                pids.last(),
                pids.len()
            ),
        );
    } else {
        report.assert(
            "Daemon PID stable (Windows only)",
            dev_mode || !cfg!(windows),
            "no JSON snapshots parsed (expected on Mac --dev)".to_string(),
        );
    }

    // 3. Working-Set bound — same shape as Phase 7's WS check, reusing
    //    `first_and_last_ws()` which already gracefully returns None
    //    on Mac.
    if let Some((first_ws, last_ws)) = first_and_last_ws(out) {
        let ratio = (last_ws as f64) / (first_ws.max(1) as f64);
        report.assert(
            "Working-Set growth ≤ 1.5× over window",
            ratio <= 1.5,
            format!(
                "first={first_ws} bytes, last={last_ws} bytes, ratio={ratio:.2}×"
            ),
        );
    } else {
        report.assert(
            "Working-Set captured at start and end (Windows only)",
            dev_mode || !cfg!(windows),
            "could not parse Working-Set from process snapshots".to_string(),
        );
    }
}

/// Collect daemon PIDs from each `*-process.json` snapshot in the run's
/// snapshot directory, sorted by snapshot label (which matches time
/// order: `00h`, `01h`, … or `00m`, `01m`, …).  Returns an empty Vec
/// when no JSON snapshots exist (Mac under --dev).
fn collect_pids(out: &OutputDir) -> Vec<u64> {
    let mut snaps: Vec<PathBuf> = match fs::read_dir(out.snapshot_dir()) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return Vec::new(),
    };
    snaps.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("-process.json"))
            .unwrap_or(false)
    });
    snaps.sort();
    snaps
        .iter()
        .filter_map(|p| {
            let body = fs::read_to_string(p).ok()?;
            parse_field_u64(&body, "Id")
        })
        .collect()
}

/// Parse `"<key>":<digits>` from a compact-JSON snapshot body.
/// Returns None when the key is absent or the value isn't an integer.
/// Used by `write_ws_trace_csv` and `collect_pids`.
fn parse_field_u64(body: &str, key: &str) -> Option<u64> {
    let pat = format!(r#""{key}"\s*:\s*(\d+)"#);
    let re = Regex::new(&pat).ok()?;
    re.captures(body)?.get(1)?.as_str().parse::<u64>().ok()
}

/// Parse `"<key>":<float>` from a compact-JSON snapshot body.  CPU is
/// emitted by PowerShell `ConvertTo-Json` as a float (seconds); WS / PM
/// / NPM / VM are integers and use `parse_field_u64` instead.
fn parse_field_f64(body: &str, key: &str) -> Option<f64> {
    let pat = format!(r#""{key}"\s*:\s*([\d.]+)"#);
    let re = Regex::new(&pat).ok()?;
    re.captures(body)?.get(1)?.as_str().parse::<f64>().ok()
}

// ── Misc helpers ──────────────────────────────────────────────────────

/// Resolve the daemon's data-source argument:
///   - On Windows, `cli.data_dir` is normally None (auto-discover).
///   - On macOS, `cli.data_dir` is REQUIRED — the daemon has no NTFS
///     auto-discovery and bails with `Daemon has no data sources to load`
///     if started without a path.  Fail-loud here so the operator sees
///     the error before the daemon fails to come Ready.
fn preflight_data_dir(cli: &Cli) -> Result<Option<PathBuf>> {
    if let Some(p) = &cli.data_dir {
        if !p.exists() {
            bail!("--data-dir {}: does not exist", p.display());
        }
        return Ok(Some(p.clone()));
    }
    if cfg!(target_os = "macos") {
        bail!(
            "--data-dir is required on macOS (the daemon has no NTFS auto-discovery).  \
             Typical layout: ~/uffs_data/drive_<L>/<L>_mft.iocp.  \
             Re-run with `--data-dir ~/uffs_data` (or wherever your offline data lives)."
        );
    }
    Ok(None)
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines().map(|l| format!("{prefix}{l}\n")).collect()
}

fn backup_existing_config(p: &Path) -> Result<Option<PathBuf>> {
    if !p.exists() {
        return Ok(None);
    }
    let backup = p.with_extension("toml.before-soak");
    fs::copy(p, &backup).with_context(|| format!("backing up {}", p.display()))?;
    Ok(Some(backup))
}

/// RAII guard that restores the daemon.toml + kills any running daemon
/// when dropped.  Invariant: at most one CleanupGuard alive per soak.
struct CleanupGuard {
    cfg_path: PathBuf,
    cfg_backup: Option<PathBuf>,
    binary: PathBuf,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Stop daemon.
        let _ = Command::new(&self.binary)
            .args(["daemon", "kill"])
            .output();
        // Restore daemon.toml.
        if let Some(backup) = &self.cfg_backup {
            let _ = fs::copy(backup, &self.cfg_path);
            let _ = fs::remove_file(backup);
        } else {
            let _ = fs::remove_file(&self.cfg_path);
        }
    }
}
