#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! ```
// =============================================================================
// scripts/dev/tier-load-compare.rs — UFFS Cold-vs-Parked tier-load A/B
// =============================================================================
//
// Empirically validates the architectural claim that worst-case
// re-promote wall-clock is **identical** between the `Cold` and
// `Parked` source tiers *when every drive's bloom hits the query*
// (the broadest possible filter).  The bloom optimisation in
// `crate::index::dispatch::ensure_warm_for_dispatch::bloom_pre_check_should_promote`
// only helps when some blooms *miss* — under a broad filter every
// drive must body-load regardless of source tier, and the per-drive
// decrypt cost is shared by the same `load_or_join_in_flight`
// primitive on both code paths.
//
// **A/B design.**  N iterations of each phase; the daemon is killed
// and restarted between iterations so RAM state doesn't leak.
//
//   Phase A — All-Cold worst-case
//     1. start daemon            (clean RAM)
//     2. seed all drives → Warm  (one cheap search)
//     3. `daemon hibernate`      (cascade walk → all Cold)
//     4. verify all tier=cold    (status_drives parse)
//     5. **TIMED**: search       (Cold → Warm in parallel for all drives)
//
//   Phase B — All-Parked worst-case
//     1. start daemon            (clean RAM)
//     2. seed all drives → Warm  (one cheap search)
//     3. wait `--park-wait-secs` (TTL-driven Warm → Parked controller tick)
//     4. verify all tier=parked  (status_drives parse)
//     5. **TIMED**: search       (Parked → Warm in parallel for all drives)
//
// Each iteration's timed search is recorded; final summary reports
// min / median / max per phase plus the absolute and relative delta.
// The script exits non-zero if `|median(Cold) - median(Parked)| /
// max(median)` exceeds `--max-delta-percent` (default 15%).
//
// **Env vars.**  The script sets the demote-controller TTLs on
// every `daemon start` it spawns:
//   * `UFFS_WARM_TO_PARKED_IDLE_SECS=5` — controller fires within
//     the `--park-wait-secs` window.
//   * `UFFS_PARKED_TO_COLD_IDLE_SECS=900` — drives stay Parked for
//     15 min, well past any reasonable test wall-clock so we don't
//     accidentally slip into Cold mid-measurement.
//
// **Why a separate script** (vs. just `--pattern '*.txt'` to
// daemon-readiness): readiness's R6 vs S9 comparison is "good
// enough for a one-shot check" but not apples-to-apples (R6 has 7
// Cold drives; S9 has 6 Parked + 1 Hot from the scenario S
// preload).  This script does clean N-vs-N with explicit tier
// verification before each timed sample, plus N iterations for a
// proper distribution rather than a single point estimate.
//
// Usage:
//   rust-script scripts/dev/tier-load-compare.rs ~/uffs_data
//   rust-script scripts/dev/tier-load-compare.rs ~/uffs_data --pattern '*.dll'
//   rust-script scripts/dev/tier-load-compare.rs --binary target/release/uffs --iterations 5
//   rust-script scripts/dev/tier-load-compare.rs ~/uffs_data --park-wait-secs 60 --max-delta-percent 20

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use colored::Colorize;

/// Per-subprocess hard timeout.  A search against 7 cold-state
/// drives on a slow box can legitimately take ~5 s; the 180 s ceiling
/// is generous enough for any realistic load while still catching
/// genuine hangs.
const STEP_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Parser)]
#[command(
    name = "tier-load-compare",
    about = "UFFS Cold-vs-Parked tier-load wall-clock A/B",
    after_help = "EXAMPLES:\n  \
        rust-script scripts/dev/tier-load-compare.rs ~/uffs_data\n  \
        rust-script scripts/dev/tier-load-compare.rs ~/uffs_data --pattern '*.dll' --iterations 5\n  \
        rust-script scripts/dev/tier-load-compare.rs                           # Windows: auto-discover NTFS drives"
)]
struct Cli {
    /// Path to an MFT file or a data directory containing drive_*
    /// subdirs.  Auto-detected: file → `--mft-file`, dir →
    /// `--data-dir`.  On Windows, omit to auto-discover live NTFS
    /// drives.
    #[arg(value_name = "PATH")]
    path: Option<String>,

    /// Path to the uffs binary.  When omitted on macOS/Linux, the
    /// script does a fresh `cargo build --release --workspace` and
    /// uses `target/release/uffs` — this guarantees `uffs` and the
    /// sibling `uffsd` daemon binary are co-versioned (the daemon is
    /// resolved by sibling-of-uffs lookup; a stale `uffsd` from
    /// `~/bin/` would silently swallow the run).  On Windows,
    /// defaults to `~/bin/uffs.exe` first, then
    /// `target\release\uffs.exe`.  Pass an explicit path here to
    /// skip the auto-build (e.g. when iterating on the script alone).
    #[arg(long)]
    binary: Option<String>,

    /// Search pattern for the timed query.  `*.txt` is the default
    /// because it hits virtually every drive's bloom on a typical
    /// macOS dev machine — the worst-case scenario where the bloom
    /// optimisation provides no benefit and Cold/Parked must be
    /// equally expensive.  Use a more selective pattern (e.g.
    /// `xyzzy.log`) to *break* the parity assertion intentionally
    /// and demonstrate Parked's bloom-skip win.
    #[arg(long, default_value = "*.txt")]
    pattern: String,

    /// Iterations per phase.  Median + min/max reported across this
    /// many samples.  3 is the default for a quick run (~3 min total
    /// wall-clock); raise to 5 or 10 for tighter distribution
    /// estimates.
    #[arg(long, default_value_t = 3)]
    iterations: u32,

    /// Seconds to wait inside Phase B for the demote controller to
    /// move every drive Warm → Parked.  Must be longer than the
    /// `UFFS_WARM_TO_PARKED_IDLE_SECS` env var the script sets (5 s)
    /// AND longer than the controller's 30 s tick floor (the first
    /// tick is skipped so freshly-loaded shards aren't immediately
    /// demoted, putting the first real evaluation at +30 s after
    /// daemon start).  35 s is the practical minimum.
    #[arg(long, default_value_t = 35)]
    park_wait_secs: u64,

    /// Result-row limit on the timed search.  1 keeps stdout tiny
    /// (irrelevant to the wall-clock measurement, which is
    /// dominated by the body load + decrypt of the 7 drives).
    #[arg(long, default_value_t = 1)]
    limit: u32,

    /// Pass/fail threshold for `|median(Cold) - median(Parked)| /
    /// max(median)`.  Default 15% accommodates measurement noise on
    /// a busy laptop; a dedicated bench machine would tighten this
    /// to 5%.  Below threshold → exit 0; above → exit 1.
    #[arg(long, default_value_t = 15.0)]
    max_delta_percent: f64,
}

// ─────────────────────────────────────────────────────────────────────
// Test harness — modeled on `daemon-readiness.rs::Runner` but trimmed
// to just the helpers this script needs (no Phase 8 RPC timing
// instrumentation, no scenario book-keeping).
// ─────────────────────────────────────────────────────────────────────

struct Runner {
    binary: String,
    source_flag: Option<&'static str>,
    source_path: String,
    pattern: String,
    limit: u32,
}

impl Runner {
    /// Build the source args (e.g. `["--data-dir", "/path"]`) or
    /// empty for live drives on Windows.
    fn source_args(&self) -> Vec<&str> {
        match self.source_flag {
            Some(flag) => vec![flag, &self.source_path],
            None => vec![],
        }
    }

    /// Spawn the binary with optional env-var overrides, drain
    /// stdout/stderr on background threads (so a child writing more
    /// than the OS pipe buffer doesn't deadlock), and wait with a
    /// hard `STEP_TIMEOUT` ceiling.
    fn run_raw_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Output> {
        let mut cmd = Command::new(&self.binary);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to exec: {} {}", self.binary, args.join(" ")))?;

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = stdout_pipe {
                let _ = std::io::Read::read_to_end(&mut r, &mut buf);
            }
            buf
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = stderr_pipe {
                let _ = std::io::Read::read_to_end(&mut r, &mut buf);
            }
            buf
        });

        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stdout = stdout_thread.join().unwrap_or_default();
                    let stderr = stderr_thread.join().unwrap_or_default();
                    return Ok(Output { status, stdout, stderr });
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!(
                            "TIMEOUT after {}s: {} {}",
                            STEP_TIMEOUT.as_secs(),
                            self.binary,
                            args.join(" ")
                        );
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => bail!("wait error for {} {}: {e}", self.binary, args.join(" ")),
            }
        }
    }

    fn run_ok(&self, args: &[&str]) -> Result<String> {
        let out = self.run_raw_with_env(args, &[])?;
        if !out.status.success() {
            bail!(
                "exit {}: stdout={} stderr={}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Stop the daemon (if running) and wait up to 10 s for it to
    /// actually exit.  Idempotent on a stopped daemon.
    fn ensure_stopped(&self) {
        let _ = self.run_ok(&["daemon", "kill"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
            if let Ok(out) = self.run_ok(&["daemon", "status"]) {
                if out.contains("not running") {
                    return;
                }
            }
        }
    }

    /// Start the daemon with short Warm→Parked / long Parked→Cold
    /// TTLs so Phase B's wait window deterministically demotes
    /// every drive into Parked without slipping past it into Cold.
    /// `daemon start` blocks until Ready (or errors), so callers
    /// don't need their own poll loop.
    fn start_daemon(&self) -> Result<()> {
        let mut args: Vec<&str> = vec!["daemon", "start"];
        args.extend(self.source_args());
        let out = self.run_raw_with_env(
            &args,
            &[
                ("UFFS_WARM_TO_PARKED_IDLE_SECS", "5"),
                ("UFFS_PARKED_TO_COLD_IDLE_SECS", "900"),
            ],
        )?;
        if !out.status.success() {
            bail!(
                "daemon start failed: exit {}, stderr={}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// Sanity-check that the daemon is Ready and report drive count.
    fn assert_ready(&self) -> Result<usize> {
        let out = self.run_ok(&["daemon", "status"])?;
        if !out.contains("Ready") {
            bail!("expected 'Ready' in `daemon status`, got:\n{out}");
        }
        Ok(out.lines().filter(|l| l.contains("records")).count())
    }

    fn hibernate_all(&self) -> Result<()> {
        self.run_ok(&["daemon", "hibernate"])?;
        Ok(())
    }

    fn status_drives_table(&self) -> Result<String> {
        self.run_ok(&["daemon", "status_drives"])
    }

    /// Run the configured search, return wall-clock millis.  Stdout
    /// is captured + discarded so pipe-buffer drainage doesn't
    /// influence the measurement.
    fn search_timed(&self) -> Result<u128> {
        let lim = self.limit.to_string();
        let mut args: Vec<&str> = vec![&self.pattern];
        args.extend(self.source_args());
        args.extend(["--limit", &lim]);
        let t = Instant::now();
        self.run_ok(&args)?;
        Ok(t.elapsed().as_millis())
    }
}

/// Walk the `status_drives` table and count how many drives are at
/// the requested tier.  Used by the per-iteration verify step before
/// each timed sample, so a TTL miss (Phase B) or a stale-Hot leftover
/// (Phase A) can fail loudly instead of polluting the measurement.
///
/// Row layout (post-Phase 9):
///   `letter tier resident_value resident_unit qpm last_query pin_until promotions`
fn count_at_tier(table: &str, target_tier: &str) -> (usize, usize) {
    let mut matched = 0_usize;
    let mut total = 0_usize;
    for line in table.lines() {
        let trimmed = line.trim_start();
        let mut parts = trimmed.split_whitespace();
        let Some(letter) = parts.next() else {
            continue;
        };
        // Drive rows have a single ASCII-letter token in column 1
        // (`C`, `D`, …).  The header (`DRIVE`) is filtered by the
        // length check; box-drawing rules and blank lines are
        // filtered by `is_ascii_alphabetic`.
        if letter.len() != 1 || !letter.chars().next().unwrap().is_ascii_alphabetic() {
            continue;
        }
        let Some(tier) = parts.next() else {
            continue;
        };
        total += 1;
        if tier == target_tier {
            matched += 1;
        }
    }
    (matched, total)
}

/// Run N iterations of Phase A (All-Cold worst-case) and return the
/// per-iteration timed-search wall-clock samples.
fn measure_phase_cold(r: &Runner, n: u32) -> Result<Vec<u128>> {
    let mut samples = Vec::with_capacity(n as usize);
    for i in 1..=n {
        println!("  Iteration {}/{}", i, n);
        r.ensure_stopped();
        r.start_daemon()?;
        let drives = r.assert_ready()?;
        // Seed warm so every drive is loaded (a cold-boot daemon
        // start lands drives at Warm already, but doing one search
        // here also primes the OS page cache for the encrypted
        // compact files — without this, Phase A's iteration 1 sees
        // cold-page-cache I/O while iterations 2/3 don't).
        let _ = r.search_timed()?;
        // Hibernate cascades all drives to Cold (RAM-only release;
        // on-disk caches preserved).
        r.hibernate_all()?;
        let table = r.status_drives_table()?;
        let (matched, total) = count_at_tier(&table, "cold");
        if total == 0 {
            bail!("no drives loaded — daemon discovered nothing under {:?}", r.source_args());
        }
        if matched != total {
            bail!(
                "expected all {total} drives at tier=cold post-hibernate; only {matched} are cold"
            );
        }
        println!("    setup OK: {drives} drives loaded, {total} all cold");
        // Timed measurement: search forces ensure_warm_for_dispatch
        // to drive `load_or_join_in_flight` for every Cold drive in
        // parallel via `FuturesUnordered`.
        let ms = r.search_timed()?;
        println!(
            "    timed search: {} ({} drives Cold→Warm in parallel)",
            format!("{ms} ms").bold(),
            total
        );
        samples.push(ms);
    }
    r.ensure_stopped();
    Ok(samples)
}

/// Run N iterations of Phase B (All-Parked worst-case) and return
/// the per-iteration timed-search wall-clock samples.
fn measure_phase_parked(r: &Runner, n: u32, park_wait_secs: u64) -> Result<Vec<u128>> {
    let mut samples = Vec::with_capacity(n as usize);
    for i in 1..=n {
        println!("  Iteration {}/{}", i, n);
        r.ensure_stopped();
        r.start_daemon()?;
        let drives = r.assert_ready()?;
        let _ = r.search_timed()?; // seed warm + prime OS page cache
        // Wait for the demote controller (firing every 30 s) to
        // observe each drive idle past `UFFS_WARM_TO_PARKED_IDLE_SECS`
        // (5 s above) and walk every Warm shard to Parked.
        println!("    waiting {park_wait_secs}s for warm→parked TTL …");
        std::thread::sleep(Duration::from_secs(park_wait_secs));
        let table = r.status_drives_table()?;
        let (matched, total) = count_at_tier(&table, "parked");
        if total == 0 {
            bail!("no drives loaded — daemon discovered nothing under {:?}", r.source_args());
        }
        if matched != total {
            bail!(
                "expected all {total} drives at tier=parked after {park_wait_secs}s wait; \
                 only {matched} are parked.  Try raising --park-wait-secs."
            );
        }
        println!("    setup OK: {drives} drives loaded, {total} all parked");
        // Timed measurement: same code path, different source tier.
        let ms = r.search_timed()?;
        println!(
            "    timed search: {} ({} drives Parked→Warm in parallel)",
            format!("{ms} ms").bold(),
            total
        );
        samples.push(ms);
    }
    r.ensure_stopped();
    Ok(samples)
}

/// Compute (min, median, max) over a non-empty sample set.  Median
/// for even-length sets uses integer-division midpoint average
/// (rounds toward zero) — fine for ms-scale measurements where
/// fractional millis are noise.
fn stats(samples: &[u128]) -> (u128, u128, u128) {
    let mut sorted: Vec<u128> = samples.to_vec();
    sorted.sort_unstable();
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2
    };
    (min, median, max)
}

fn detect_data_source(path: &str) -> Result<(&'static str, String)> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        bail!("Path does not exist: {path}");
    }
    if p.is_file() {
        Ok(("--mft-file", path.to_owned()))
    } else if p.is_dir() {
        Ok(("--data-dir", path.to_owned()))
    } else {
        bail!("Path is neither a file nor a directory: {path}");
    }
}

fn default_data_dir() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::PathBuf::from(home).join("uffs_data");
    if dir.is_dir() {
        Some(dir.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Find the workspace root by walking up from cwd looking for
/// `Cargo.toml` + `.cargo` (the same heuristic used by
/// `daemon-readiness.rs::find_workspace_root`).
fn find_workspace_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
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

/// Build a fresh release **workspace** and return the path to the
/// `uffs` CLI binary (macOS/Linux).
///
/// Builds every workspace package — not just `uffs-cli` — so the
/// daemon (`uffsd`) is rebuilt in lock-step with the CLI.  Without
/// this, the script would silently pick up a stale `~/bin/uffs`
/// shipped from a prior `quick-deploy`, and the timing numbers
/// would reflect old code (the exact failure mode the user
/// reported on 2026-05-05 — `binary: /Users/rnio/bin/uffs`
/// instead of the freshly-built one).  Mirrors
/// `daemon-readiness.rs::ensure_fresh_release_build` so both
/// dev scripts behave identically.
fn ensure_fresh_release_build() -> String {
    let workspace = find_workspace_root();
    let binary_path = workspace.join("target").join("release").join("uffs");

    eprintln!("╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  Building fresh release workspace (uffs + uffsd + uffsmcp …)...  ║");
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("  Workspace: {}", workspace.display());

    let start = Instant::now();
    let status = Command::new("cargo")
        .args(["build", "--release", "--workspace"])
        .current_dir(&workspace)
        .status();

    match status {
        Ok(s) if s.success() => {
            eprintln!(
                "  ✅ Build completed in {:.1}s",
                start.elapsed().as_secs_f64()
            );
            eprintln!("  CLI binary: {}", binary_path.display());
            // Surface the daemon binary's mtime alongside the CLI
            // so a stale `uffsd` is loud at the top of the run rather
            // than silently producing wrong tier-load numbers.
            let uffsd = workspace.join("target").join("release").join("uffsd");
            if let Ok(meta) = std::fs::metadata(&uffsd) {
                if let Ok(modified) = meta.modified() {
                    eprintln!(
                        "  uffsd:      {} ({:.0}s ago)",
                        uffsd.display(),
                        modified.elapsed().map_or(0.0, |d| d.as_secs_f64())
                    );
                }
            }
            eprintln!();
        }
        Ok(s) => {
            eprintln!("  ❌ cargo build --release --workspace failed (exit {s})");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("  ❌ Failed to run cargo: {e}");
            std::process::exit(1);
        }
    }

    binary_path.to_string_lossy().into_owned()
}

fn default_binary() -> String {
    if cfg!(windows) {
        // Windows: prefer the deployed binary under ~/bin/, fall back
        // to the local `target\release\uffs.exe`.  We do NOT auto-
        // rebuild on Windows because cross-compilation realities mean
        // most operators iterate via `quick-deploy` rather than
        // building locally.
        if let Ok(home) = std::env::var("USERPROFILE") {
            let p = std::path::PathBuf::from(&home).join("bin").join("uffs.exe");
            if p.exists() {
                return p.to_string_lossy().into_owned();
            }
        }
        "target\\release\\uffs.exe".to_string()
    } else {
        // macOS/Linux: always fresh-build so we measure the latest
        // tiering code, not whatever was last shipped to ~/bin.
        ensure_fresh_release_build()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let binary = cli.binary.unwrap_or_else(default_binary);

    let (source_flag, source_path): (Option<&'static str>, String) = match &cli.path {
        Some(path) => {
            let (flag, val) = detect_data_source(path)?;
            (Some(flag), val)
        }
        None if cfg!(windows) => (None, String::new()),
        None => match default_data_dir() {
            Some(dir) => (Some("--data-dir"), dir),
            None => bail!(
                "PATH is required on non-Windows platforms.\n\n\
                 On macOS/Linux, provide a data directory or MFT file:\n  \
                 rust-script scripts/dev/tier-load-compare.rs ~/uffs_data\n\n\
                 On Windows, omit PATH to auto-discover live NTFS drives."
            ),
        },
    };

    println!("{}", "═══ UFFS Tier-Load Comparison (Cold vs Parked) ═══".bold());
    println!("  binary:           {binary}");
    match source_flag {
        Some(flag) => println!("  source:           {flag} {source_path}"),
        None => println!("  source:           live NTFS drives (auto-discover)"),
    }
    println!("  pattern:          {}", cli.pattern);
    println!("  iterations:       {} per phase", cli.iterations);
    println!("  park wait:        {}s", cli.park_wait_secs);
    println!("  result limit:     {}", cli.limit);
    println!("  max delta:        {:.1}%", cli.max_delta_percent);
    println!();

    let r = Runner {
        binary,
        source_flag,
        source_path,
        pattern: cli.pattern,
        limit: cli.limit,
    };

    println!("{}", "── Phase A: All-Cold worst-case (hibernate → search) ──".bold());
    let cold = measure_phase_cold(&r, cli.iterations)?;
    println!();

    println!("{}", "── Phase B: All-Parked worst-case (TTL wait → search) ──".bold());
    let parked = measure_phase_parked(&r, cli.iterations, cli.park_wait_secs)?;
    println!();

    let (cold_min, cold_med, cold_max) = stats(&cold);
    let (parked_min, parked_med, parked_max) = stats(&parked);

    println!("{}", "── Summary ──".bold());
    println!("  ┌─────────────────┬─────────┬─────────┬─────────┬──────────────────────┐");
    println!("  │ Source tier     │     min │  median │     max │ samples (ms)         │");
    println!("  ├─────────────────┼─────────┼─────────┼─────────┼──────────────────────┤");
    println!(
        "  │ Cold→Warm       │ {:>5}ms │ {:>5}ms │ {:>5}ms │ {:?}",
        cold_min, cold_med, cold_max, cold,
    );
    println!(
        "  │ Parked→Warm     │ {:>5}ms │ {:>5}ms │ {:>5}ms │ {:?}",
        parked_min, parked_med, parked_max, parked,
    );
    println!("  └─────────────────┴─────────┴─────────┴─────────┴──────────────────────┘");

    let larger = cold_med.max(parked_med) as f64;
    let smaller = cold_med.min(parked_med) as f64;
    let abs_delta_ms = cold_med.max(parked_med) - cold_med.min(parked_med);
    let delta_pct = if larger > 0.0 { ((larger - smaller) / larger) * 100.0 } else { 0.0 };

    println!();
    println!("  delta(median):     {abs_delta_ms} ms ({delta_pct:.2}%)");
    println!("  threshold:         ≤ {:.2}%", cli.max_delta_percent);
    println!();

    if delta_pct <= cli.max_delta_percent {
        println!(
            "{}",
            format!(
                "══ PASS ══  Cold ≈ Parked under a broad query ({delta_pct:.2}% ≤ {:.2}%)",
                cli.max_delta_percent
            )
            .green()
            .bold()
        );
        Ok(())
    } else {
        println!(
            "{}",
            format!(
                "══ FAIL ══  Cold and Parked diverge ({delta_pct:.2}% > {:.2}%)",
                cli.max_delta_percent
            )
            .red()
            .bold()
        );
        bail!(
            "tier-load divergence exceeds threshold ({delta_pct:.2}% > {:.2}%)",
            cli.max_delta_percent
        );
    }
}
