#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! colored = "2"
//! ```
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//!
//! stream-stress.rs — Progressive shmem-stream stress harness.
//!
//! Escalates the blob size produced by `uffs "*" --limit N` across
//! doubling row-count tiers and each supported stdout sink (file,
//! pipe-to-null, inherited console), logging the first failure with
//! its stage, byte offset, and OS error code.  Built to pin down
//! Windows-specific `ShmemBlob` streaming regressions such as the
//! opaque `OS Error 16388 (FormatMessageW returned 317)` seen on
//! PowerShell with unlimited result sets.
//!
//! ## Why this exists
//!
//! The shmem fast path (`SearchPayload::ShmemBlob`) mmaps a
//! multi-hundred-MB file and hands the bytes to stdout.  Linux and
//! macOS tolerate arbitrary `write_all` sizes, but on Windows both
//! `WriteFile` on pipes and `WriteConsoleW` on interactive consoles
//! have per-call caps whose failure modes surface only under load.
//! This script hunts for the cliff by doubling the blob until the
//! first write error, then prints the `--limit`, byte size, sink,
//! stage, and raw OS error — enough context to attach to a bug
//! report or decide the next place to drop a `tracing::debug!`.
//!
//! ## Usage
//!
//! ```bash
//! rust-script scripts/windows/stream-stress.rs
//! rust-script scripts/windows/stream-stress.rs --pattern "*.dll"
//! rust-script scripts/windows/stream-stress.rs --bin target/release/uffs
//! rust-script scripts/windows/stream-stress.rs --sinks file,null
//! rust-script scripts/windows/stream-stress.rs --start-limit 100000 --stop-on-fail
//! ```
//!
//! ## Sinks
//!
//! - `file`    — redirect stdout to a temp file (`WriteFile` on a disk handle).
//! - `null`    — redirect stdout to `/dev/null` / `NUL` (`WriteFile` on a device handle).
//! - `pipe`    — pipe stdout into a byte-counting child process (`WriteFile` on a pipe).
//! - `console` — inherit parent stdout so the write hits `WriteConsoleW` directly.
//!               Runs every tier by default; pass `--console-limit N` to skip
//!               huge dumps in CI where an interactive terminal is not available.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use colored::Colorize;

// ── Config & CLI ────────────────────────────────────────────────────────────

/// Default limit doublings (rows).  Tuned so the lowest tier stays
/// under the blob shmem threshold (`InlineBlob` path) and the highest
/// tier always offloads to `ShmemBlob`, crossing the suspect cliff.
const DEFAULT_LIMITS: &[u64] = &[
    10,
    1_000,
    10_000,
    100_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
];

/// Default row-count ceiling above which the `console` sink is
/// skipped.
///
/// Set to [`u64::MAX`] so the sweep runs every configured tier
/// against the interactive terminal by default — the whole point
/// of the `console` sink is to reproduce the `uffs "*"` PowerShell
/// failure on the exact write path the user hits.  Callers who
/// *want* to short-circuit huge console dumps (e.g. CI, headless
/// regression jobs) still opt in explicitly via `--console-limit N`.
const DEFAULT_CONSOLE_LIMIT: u64 = u64::MAX;

/// Per-invocation timeout.  Streaming the full MFT on Windows can
/// take minutes; this cap stops a hung run from blocking the whole
/// matrix.  Increase with `--timeout-secs` when chasing a slow regression.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

#[derive(Clone, Debug)]
struct Args {
    bin: PathBuf,
    pattern: String,
    drive_filter: Option<String>,
    data_dir: Option<PathBuf>,
    limits: Vec<u64>,
    sinks: Vec<Sink>,
    console_limit: u64,
    timeout: Duration,
    stop_on_fail: bool,
    keep_artifacts: bool,
    out_dir: PathBuf,
}

impl Args {
    fn parse() -> Result<Self> {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let mut bin: Option<PathBuf> = None;
        let mut pattern = "*".to_string();
        let mut drive_filter: Option<String> = None;
        let mut data_dir: Option<PathBuf> = None;
        let mut limits: Option<Vec<u64>> = None;
        let mut sinks: Option<Vec<Sink>> = None;
        let mut console_limit = DEFAULT_CONSOLE_LIMIT;
        let mut timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
        let mut stop_on_fail = false;
        let mut keep_artifacts = false;
        let mut start_limit: Option<u64> = None;
        let mut out_dir: Option<PathBuf> = None;

        let mut i = 0;
        while i < argv.len() {
            let arg = argv[i].as_str();
            let mut take_value = || -> Result<String> {
                i += 1;
                argv.get(i)
                    .cloned()
                    .ok_or_else(|| anyhow!("flag {arg} expects a value"))
            };
            match arg {
                "--bin" => bin = Some(PathBuf::from(take_value()?)),
                "--pattern" => pattern = take_value()?,
                "--drive" => drive_filter = Some(take_value()?),
                "--data-dir" => data_dir = Some(PathBuf::from(take_value()?)),
                "--limits" => {
                    let raw = take_value()?;
                    let parsed = raw
                        .split(',')
                        .map(|s| {
                            s.trim().parse::<u64>().with_context(|| {
                                format!("invalid --limits entry '{s}' (want a u64)")
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    limits = Some(parsed);
                }
                "--start-limit" => {
                    start_limit = Some(
                        take_value()?
                            .parse::<u64>()
                            .context("--start-limit must be a u64")?,
                    );
                }
                "--sinks" => {
                    let raw = take_value()?;
                    sinks = Some(
                        raw.split(',')
                            .map(|s| Sink::from_flag(s.trim()))
                            .collect::<Result<Vec<_>>>()?,
                    );
                }
                "--console-limit" => {
                    console_limit = take_value()?
                        .parse::<u64>()
                        .context("--console-limit must be a u64")?;
                }
                "--timeout-secs" => {
                    timeout = Duration::from_secs(
                        take_value()?
                            .parse::<u64>()
                            .context("--timeout-secs must be a u64")?,
                    );
                }
                "--stop-on-fail" => stop_on_fail = true,
                "--keep-artifacts" => keep_artifacts = true,
                "--out-dir" => out_dir = Some(PathBuf::from(take_value()?)),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => bail!("unknown flag: {other}  (try --help)"),
            }
            i += 1;
        }

        let resolved_bin = match bin {
            Some(explicit) => {
                if !explicit.exists() {
                    bail!(
                        "uffs binary not found at {} (from --bin)",
                        explicit.display()
                    );
                }
                explicit
            }
            None => resolve_default_bin()?,
        };

        let mut final_limits = limits.unwrap_or_else(|| DEFAULT_LIMITS.to_vec());
        if let Some(start) = start_limit {
            final_limits.retain(|n| *n >= start);
        }
        if final_limits.is_empty() {
            bail!("no --limits remain after --start-limit filtering");
        }

        let final_sinks = sinks.unwrap_or_else(Sink::defaults);
        // Explicit --out-dir wins; otherwise land under the shared bench tree's
        // `stream-stress/` namespace so artifacts sit beside the rest of the
        // bench output instead of a lone $TMPDIR leaf.
        let final_out_dir =
            out_dir.unwrap_or_else(|| shared_bench_root().join("stream-stress"));
        std::fs::create_dir_all(&final_out_dir).with_context(|| {
            format!("creating artifact dir {}", final_out_dir.display())
        })?;

        Ok(Self {
            bin: resolved_bin,
            pattern,
            drive_filter,
            data_dir,
            limits: final_limits,
            sinks: final_sinks,
            console_limit,
            timeout,
            stop_on_fail,
            keep_artifacts,
            out_dir: final_out_dir,
        })
    }
}

/// Resolve the consolidated bench-artifact root, mirroring the `_bench-dir`
/// helper in `just/bench_uffs.just` and the other bench scripts so every tool
/// writes under ONE tree.  Precedence: `$UFFS_BENCH_DIR` >
/// `%LOCALAPPDATA%\uffs-bench` > `$XDG_CACHE_HOME|~/.cache` + `/uffs-bench`.
fn shared_bench_root() -> PathBuf {
    if let Ok(v) = std::env::var("UFFS_BENCH_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(v) = std::env::var("LOCALAPPDATA") {
        if !v.is_empty() {
            return PathBuf::from(v).join("uffs-bench");
        }
    }
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache")
        });
    base.join("uffs-bench")
}

/// Locate the `uffs` binary used for the stress matrix.
///
/// Probes, in order:
///
/// 1. `target/release/uffs[.exe]` relative to the current workspace
///    (the cargo-built artefact; preferred because it always reflects
///    the local source tree).
/// 2. `$HOME/bin/uffs[.exe]` on Unix, `%USERPROFILE%\bin\uffs.exe`
///    on Windows — the install location used by the project's own
///    `install` recipe and by `just phase2-ship`.
/// 3. `PATH` lookup via `which`-style iteration over `PATH` entries.
///
/// The candidate list matches [`api-validation::default_binary`] so
/// both scripts behave identically on a fresh Windows box.  An
/// explicit `--bin` always wins and bypasses this probe entirely.
fn resolve_default_bin() -> Result<PathBuf> {
    let name = if cfg!(windows) { "uffs.exe" } else { "uffs" };

    let mut candidates: Vec<PathBuf> = Vec::new();

    candidates.push(PathBuf::from("target").join("release").join(name));

    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Ok(home) = std::env::var(home_var) {
        candidates.push(PathBuf::from(&home).join("bin").join(name));
    }

    if let Ok(path_var) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep).filter(|s| !s.is_empty()) {
            candidates.push(PathBuf::from(dir).join(name));
        }
    }

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    let preview: Vec<String> = candidates
        .iter()
        .take(4)
        .map(|p| p.display().to_string())
        .collect();
    bail!(
        "uffs binary not found. Searched (first 4 of {}):\n  - {}\n\
         Pass --bin <path> or build it with `cargo build --release`.",
        candidates.len(),
        preview.join("\n  - ")
    )
}

fn print_usage() {
    println!(
        "stream-stress — progressively stream shmem blobs of increasing size to each stdout sink.\n\
         \n\
         Usage:\n\
             rust-script scripts/windows/stream-stress.rs [flags]\n\
         \n\
         Flags:\n\
           --bin PATH            uffs binary (default: target/release/uffs[.exe])\n\
           --pattern STR         search pattern (default: '*')\n\
           --drive LETTER        restrict to a single drive letter (e.g. C)\n\
           --data-dir PATH       forward `--data-dir` to uffs\n\
           --limits N,N,...      explicit row-count tiers (overrides defaults)\n\
           --start-limit N       drop tiers below N (keeps defaults otherwise)\n\
           --sinks file,null,pipe,console   sinks to exercise (default: all)\n\
           --console-limit N     skip console sink above N rows (default: no cap)\n\
           --timeout-secs N      per-run timeout (default {})\n\
           --stop-on-fail        abort the matrix on the first failing (tier, sink)\n\
           --keep-artifacts      retain per-run stdout files under --out-dir\n\
           --out-dir PATH        artifact directory.  Default: <bench-root>/stream-stress,\n\
           \x20                    where bench-root = $UFFS_BENCH_DIR >\n\
           \x20                    %LOCALAPPDATA%\\uffs-bench > ~/.cache/uffs-bench\n",
        DEFAULT_TIMEOUT_SECS
    );
}

// ── Sinks ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sink {
    File,
    Null,
    Pipe,
    Console,
}

impl Sink {
    fn defaults() -> Vec<Self> {
        vec![Self::File, Self::Null, Self::Pipe, Self::Console]
    }

    fn from_flag(raw: &str) -> Result<Self> {
        match raw {
            "file" => Ok(Self::File),
            "null" => Ok(Self::Null),
            "pipe" => Ok(Self::Pipe),
            "console" => Ok(Self::Console),
            other => bail!("unknown sink '{other}' (want file|null|pipe|console)"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Null => "null",
            Self::Pipe => "pipe",
            Self::Console => "console",
        }
    }
}

// ── Runs ──────────────────────────────────────────────────────────────────

/// Outcome of a single (tier, sink) invocation.
struct RunResult {
    limit: u64,
    sink: Sink,
    success: bool,
    duration: Duration,
    bytes: u64,
    exit_code: Option<i32>,
    stderr: String,
    note: String,
}

impl RunResult {
    fn print_row(&self) {
        let status = if self.success {
            "OK".green().bold()
        } else {
            "FAIL".red().bold()
        };
        let bytes = format_bytes(self.bytes);
        let ms = self.duration.as_millis();
        println!(
            "  {status:>8}  limit {:>10}  sink {:<8}  {:>10} bytes  {:>6} ms  {}",
            self.limit,
            self.sink.label(),
            bytes,
            ms,
            self.note
        );
        if !self.success {
            for line in self.stderr.lines().take(4) {
                println!("           {}", line.dimmed());
            }
        }
    }
}

fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n_f = n as f64;
    if n_f >= GB {
        format!("{:.2} GiB", n_f / GB)
    } else if n_f >= MB {
        format!("{:.1} MiB", n_f / MB)
    } else if n_f >= KB {
        format!("{:.1} KiB", n_f / KB)
    } else {
        format!("{n} B")
    }
}

/// Build the `uffs` CLI argv used for every sink variant.
fn build_uffs_args(args: &Args, limit: u64) -> Vec<String> {
    let mut out = vec![args.pattern.clone(), "--limit".to_string(), limit.to_string()];
    if let Some(letter) = &args.drive_filter {
        out.push("--drive".to_string());
        out.push(letter.clone());
    }
    if let Some(dir) = &args.data_dir {
        out.push("--data-dir".to_string());
        out.push(dir.display().to_string());
    }
    out
}

fn run_with_sink(args: &Args, limit: u64, sink: Sink) -> Result<RunResult> {
    let uffs_args = build_uffs_args(args, limit);
    match sink {
        Sink::File => run_capture_to_file(args, limit, sink, &uffs_args),
        Sink::Null => run_capture_to_null(args, limit, sink, &uffs_args),
        Sink::Pipe => run_pipe_counter(args, limit, sink, &uffs_args),
        Sink::Console => run_inherit_console(args, limit, sink, &uffs_args),
    }
}

fn run_capture_to_file(
    args: &Args,
    limit: u64,
    sink: Sink,
    uffs_args: &[String],
) -> Result<RunResult> {
    let out_path = args
        .out_dir
        .join(format!("uffs-stream-{limit}-{}.csv", sink.label()));
    let stdout_file = File::create(&out_path)
        .with_context(|| format!("creating sink file {}", out_path.display()))?;
    let child = Command::new(&args.bin)
        .args(uffs_args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {} for tier {limit}", args.bin.display()))?;

    let outcome = wait_with_timeout(child, args.timeout)?;
    let bytes = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    if !args.keep_artifacts {
        drop(std::fs::remove_file(&out_path));
    }
    Ok(RunResult {
        limit,
        sink,
        success: outcome.success,
        duration: outcome.duration,
        bytes,
        exit_code: outcome.exit_code,
        stderr: outcome.stderr,
        note: describe_exit(outcome.success, outcome.exit_code),
    })
}

fn run_capture_to_null(
    args: &Args,
    limit: u64,
    sink: Sink,
    uffs_args: &[String],
) -> Result<RunResult> {
    let null_path = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let null = std::fs::OpenOptions::new()
        .write(true)
        .open(null_path)
        .with_context(|| format!("opening {null_path}"))?;
    let child = Command::new(&args.bin)
        .args(uffs_args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(null))
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {} for tier {limit}", args.bin.display()))?;
    let outcome = wait_with_timeout(child, args.timeout)?;
    // Null sink swallows bytes — byte count is unobservable; report 0.
    Ok(RunResult {
        limit,
        sink,
        success: outcome.success,
        duration: outcome.duration,
        bytes: 0,
        exit_code: outcome.exit_code,
        stderr: outcome.stderr,
        note: describe_exit(outcome.success, outcome.exit_code),
    })
}

fn run_pipe_counter(
    args: &Args,
    limit: u64,
    sink: Sink,
    uffs_args: &[String],
) -> Result<RunResult> {
    let start = Instant::now();
    let mut child = Command::new(&args.bin)
        .args(uffs_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {} for tier {limit}", args.bin.display()))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("child stdout not captured"))?;
    let reader_bytes = std::thread::spawn(move || -> std::io::Result<u64> {
        let mut total: u64 = 0;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => return Ok(total),
                Ok(n) => total = total.saturating_add(n as u64),
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    });

    let outcome = wait_with_timeout_partial(child, args.timeout)?;
    let bytes = reader_bytes
        .join()
        .map_err(|_| anyhow!("stdout reader thread panicked"))??;

    Ok(RunResult {
        limit,
        sink,
        success: outcome.success,
        duration: start.elapsed(),
        bytes,
        exit_code: outcome.exit_code,
        stderr: outcome.stderr,
        note: describe_exit(outcome.success, outcome.exit_code),
    })
}

fn run_inherit_console(
    args: &Args,
    limit: u64,
    sink: Sink,
    uffs_args: &[String],
) -> Result<RunResult> {
    if limit > args.console_limit {
        return Ok(RunResult {
            limit,
            sink,
            success: true,
            duration: Duration::ZERO,
            bytes: 0,
            exit_code: None,
            stderr: String::new(),
            note: format!(
                "skipped — limit > --console-limit ({})",
                args.console_limit
            ),
        });
    }
    let child = Command::new(&args.bin)
        .args(uffs_args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {} for tier {limit}", args.bin.display()))?;
    let outcome = wait_with_timeout(child, args.timeout)?;
    Ok(RunResult {
        limit,
        sink,
        success: outcome.success,
        duration: outcome.duration,
        bytes: 0,
        exit_code: outcome.exit_code,
        stderr: outcome.stderr,
        note: describe_exit(outcome.success, outcome.exit_code),
    })
}

fn describe_exit(success: bool, exit_code: Option<i32>) -> String {
    if success {
        "ok".to_string()
    } else {
        match exit_code {
            Some(code) => format!("exit {code}"),
            None => "terminated by signal / timeout".to_string(),
        }
    }
}

// ── Timeouts ──────────────────────────────────────────────────────────────

struct WaitOutcome {
    success: bool,
    duration: Duration,
    exit_code: Option<i32>,
    stderr: String,
}

/// Drive a child to completion with a wall-clock `timeout`, capturing
/// stderr.  Kills the child on timeout and reports it as a failure.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<WaitOutcome> {
    let start = Instant::now();
    // Spawn a dedicated thread to drain stderr so a chatty child
    // cannot deadlock the pipe while we poll below.
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || -> std::io::Result<String> {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf)?;
            Ok(buf)
        })
    });

    loop {
        if let Some(status) = child.try_wait()? {
            let stderr = stderr_handle
                .map(|h| h.join().unwrap_or_else(|_| Ok(String::new())))
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            return Ok(WaitOutcome {
                success: status.success(),
                duration: start.elapsed(),
                exit_code: status.code(),
                stderr,
            });
        }
        if start.elapsed() >= timeout {
            drop(child.kill());
            let stderr = stderr_handle
                .map(|h| h.join().unwrap_or_else(|_| Ok(String::new())))
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            return Ok(WaitOutcome {
                success: false,
                duration: start.elapsed(),
                exit_code: None,
                stderr: format!("TIMEOUT after {:?}\n{}", timeout, stderr),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Like [`wait_with_timeout`] but does not touch stdout — used when a
/// reader thread already owns the child's stdout handle.
fn wait_with_timeout_partial(mut child: Child, timeout: Duration) -> Result<WaitOutcome> {
    let start = Instant::now();
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || -> std::io::Result<String> {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf)?;
            Ok(buf)
        })
    });

    loop {
        if let Some(status) = child.try_wait()? {
            let stderr = stderr_handle
                .map(|h| h.join().unwrap_or_else(|_| Ok(String::new())))
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            return Ok(WaitOutcome {
                success: status.success(),
                duration: start.elapsed(),
                exit_code: status.code(),
                stderr,
            });
        }
        if start.elapsed() >= timeout {
            drop(child.kill());
            let stderr = stderr_handle
                .map(|h| h.join().unwrap_or_else(|_| Ok(String::new())))
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            return Ok(WaitOutcome {
                success: false,
                duration: start.elapsed(),
                exit_code: None,
                stderr: format!("TIMEOUT after {:?}\n{}", timeout, stderr),
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── Daemon sanity ─────────────────────────────────────────────────────────

/// Emit a one-time `uffs daemon status` summary before the matrix so
/// a zero-rows run is obviously a "daemon has no drives" issue rather
/// than a streaming regression.
fn print_daemon_banner(bin: &Path) {
    let out = Command::new(bin).args(["daemon", "status"]).output();
    match out {
        Ok(o) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            for line in combined.lines().take(16) {
                println!("  {}", line.dimmed());
            }
        }
        Err(err) => {
            println!("  {} {err}", "daemon status failed:".red());
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse()?;
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".dimmed()
    );
    println!(
        "{} {}",
        "stream-stress:".bold(),
        "shmem blob stdout sweep".bold()
    );
    println!(
        "  bin={}  pattern='{}'  sinks={}",
        args.bin.display(),
        args.pattern,
        args.sinks
            .iter()
            .map(|s| s.label())
            .collect::<Vec<_>>()
            .join(",")
    );
    let console_limit_display = if args.console_limit == u64::MAX {
        "unlimited".to_string()
    } else {
        args.console_limit.to_string()
    };
    println!(
        "  limits=[{}]  timeout={}s  console_limit={}",
        args.limits
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        args.timeout.as_secs(),
        console_limit_display
    );
    if let Some(d) = &args.data_dir {
        println!("  data_dir={}", d.display());
    }
    if let Some(l) = &args.drive_filter {
        println!("  drive={}", l);
    }
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".dimmed()
    );

    println!();
    println!("  {}", "Daemon snapshot:".bold());
    print_daemon_banner(&args.bin);
    println!();
    println!("  {}", "Stress matrix:".bold());

    let mut failures: Vec<RunResult> = Vec::new();
    'outer: for &limit in &args.limits {
        for &sink in &args.sinks {
            let result = run_with_sink(&args, limit, sink)?;
            result.print_row();
            let failed = !result.success;
            if failed {
                failures.push(result);
                if args.stop_on_fail {
                    break 'outer;
                }
            }
        }
    }

    println!();
    if failures.is_empty() {
        println!(
            "  {} every (tier, sink) combination streamed cleanly.",
            "OK".green().bold()
        );
        Ok(())
    } else {
        println!(
            "  {} {} failing (tier, sink) combination(s):",
            "FAIL".red().bold(),
            failures.len()
        );
        for f in &failures {
            let code = f
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "    - limit {:>10}  sink {:<8}  exit={}  {}  stderr[:160]={}",
                f.limit,
                f.sink.label(),
                code,
                f.note,
                f.stderr
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            );
        }
        // Exit non-zero so CI / shell scripts can detect the regression.
        std::process::exit(1);
    }
}
