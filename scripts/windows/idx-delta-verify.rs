#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! ```
//!
//! idx-delta-verify.rs — live WIN verification + perf guard for the incremental-
//! index-maintenance work (design: `docs/architecture/incremental-index-maintenance.md`).
//!
//! Goal: on the WIN box, prove the delta-overlay apply stays correct (creates,
//! renames, deletes become search-visible promptly) AND fast (per-apply cost
//! tracks the batch, not the drive size). It deliberately mirrors
//! `scripts/windows/usn-verify.rs` (same `~/bin/uffs.exe` resolution, `~/idxtest`
//! scratch, `_run/` artifacts, daemon-restart-with-logging) so the dev loop is
//! identical: push -> pull on WIN -> run -> share `_run/`.
//!
//! What it does:
//!   0. BIN SYNC — copies the freshly built `uffs`/`uffsd` (+ broker/mcp if
//!      present) from **the build dir cargo actually uses** (`cargo metadata`'s
//!      `target_directory`, honouring `CARGO_TARGET_DIR` / `.cargo/*.toml`;
//!      override with `UFFS_RELEASE_DIR`) into `~/bin`, so the rig can never run
//!      a stale daemon.  Build, then run — no manual copy step.
//!   1. BUILD CONFIRMATION — restarts the daemon with logging, then reads the
//!      `git=` stamp off the `uffsd starting` line and asserts it equals repo
//!      HEAD (hard stale-daemon guard).
//!   2. CHURN + TIMING — creates files in escalating bursts so each apply fires,
//!      captures every `usn apply: batch applied` DEBUG line, and summarises the
//!      per-apply wall-clock + compaction count at the drive's live record count.
//!   3. FRESHNESS — measures wall-clock from a create to the file being
//!      search-visible (sanity: no backlog at the pinned apply cadence).
//!   4. BASELINE — writes `_run/baseline.txt` (the per-apply numbers) +
//!      `_run/idx-timing.log` (the raw `usn apply` lines).
//!
//! Usage:  rust-script scripts\windows\idx-delta-verify.rs

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Phase-5 apply **max-wait** cap (ms) for the test daemon — the ceiling under
/// sustained churn. With apply now ~200 ms (Phases 1-4), the production default
/// is 2 s; the rig pins it explicitly so the apply cadence is deterministic
/// across runs.
const APPLY_INTERVAL_MS: &str = "2000";
/// Phase-5 apply **debounce / settle** window (ms) — the snappy half: a burst
/// that goes quiet for this long is applied at once, so an idle→active change
/// is searchable well under a second.
const APPLY_DEBOUNCE_MS: &str = "250";
/// Settle after `--daemon stop` so the socket / PID file clear.
const KILL_SETTLE: Duration = Duration::from_secs(2);
/// Poll cadence while waiting for a burst's files to become search-visible.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// `uffs_core=debug` surfaces the per-batch `usn apply: batch applied` summary
/// (logged at DEBUG); `info` everywhere else keeps the daemon `uffsd starting`
/// build stamp and the rest of the log readable.
const LOG_SPEC: &str = "info,uffs_core=debug,uffs_daemon=info";
/// Escalating create-burst sizes — bigger bursts exercise bigger apply batches.
/// The 100k burst crosses `TRIGRAM_COMPACT_THRESHOLD` (50k) so it also forces a
/// delta compaction (full base refold, `compacted=true`) under load, while the
/// smaller bursts stay on the steady-state delta-overlay apply.
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

/// Whether the diff between the running daemon's build SHA and HEAD touches any
/// **build-affecting** path (crate source or a Cargo manifest), i.e. the binary
/// is genuinely stale. A HEAD that advanced only through `scripts/` or `docs/`
/// (e.g. a verify-rig tweak) leaves the daemon binary current, so it is NOT
/// stale. Defaults to `true` (assume stale) if git can't answer — fail safe.
fn build_is_stale(daemon_sha: &str, head_sha: &str) -> bool {
    let Ok(out) = Command::new("git")
        .args(["diff", "--name-only", daemon_sha, head_sha])
        .output()
    else {
        return true;
    };
    if !out.status.success() {
        return true;
    }
    String::from_utf8_lossy(&out.stdout).lines().any(|path| {
        path.starts_with("crates/")
            || path == "Cargo.toml"
            || path == "Cargo.lock"
            || path.starts_with("rust-toolchain")
    })
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
            // Best-effort: the broker is a running LocalSystem service, so its
            // exe is legitimately locked (os error 32). The rig only needs a
            // fresh uffs + uffsd, so a locked/failed optional copy just warns.
            if let Err(err) = copy_bin(&src, &bin_dir.join(exe(name))) {
                println!("  skip {} ({err})", exe(name));
            }
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

/// Poll `search(term)` until **zero** rows match (the deleted / renamed-away
/// file has left the index) or `max_wait` elapses. Returns
/// `(rows_remaining, latency, timed_out)`.
fn poll_until_absent(uffs: &Path, term: &str, max_wait: Duration) -> Result<(usize, Duration, bool)> {
    let start = Instant::now();
    loop {
        let (rows, _) = search(uffs, term)?;
        if rows == 0 {
            return Ok((0, start.elapsed(), false));
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
        "\n$ {} --daemon start   (UFFS_LOG={LOG_SPEC}, UFFS_USN_APPLY_INTERVAL_MS={APPLY_INTERVAL_MS}, UFFS_USN_APPLY_DEBOUNCE_MS={APPLY_DEBOUNCE_MS})",
        uffs_display()
    );
    let status = Command::new(&uffs)
        .args(["--daemon", "start"])
        .env("UFFS_LOG", LOG_SPEC)
        .env("UFFS_LOG_DIR", &run_dir)
        .env("UFFS_USN_APPLY_INTERVAL_MS", APPLY_INTERVAL_MS)
        .env("UFFS_USN_APPLY_DEBOUNCE_MS", APPLY_DEBOUNCE_MS)
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
        .find(|line| line.contains("uffsd starting"))
        .map(str::to_owned);
    let build_line = match build_line {
        Some(line) => {
            println!("  OK — {}", line.trim());
            line
        }
        None => bail!(
            "no `uffsd starting` line in {} — the daemon did not log a startup banner. \
             Rebuild then re-run (the rig re-syncs ~/bin for you).",
            log_path.display()
        ),
    };

    // Build-id match guard: the running daemon's git SHA must equal repo HEAD,
    // else a stale uffsd is being exercised (the trap that has cost several
    // 30-min WIN cycles).  `git="<sha>"` is stamped on the `uffsd starting` line.
    if let Some(head) = &head_sha {
        let logged = build_line
            .split("git=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("");
        if logged == head {
            println!("  build-id match: uffsd git={logged} == HEAD {head}");
        } else if build_is_stale(logged, head) {
            bail!(
                "STALE DAEMON: running uffsd is git={logged:?} but HEAD is {head:?} and \
                 crate source / Cargo manifests differ between them — rebuild + re-run \
                 (the rig re-syncs ~/bin, but you must `cargo build --release` first).",
            );
        } else {
            // HEAD advanced only through scripts/docs (e.g. this rig itself);
            // the daemon binary is still current with the crate source.
            println!(
                "  build-id OK: uffsd git={logged}, HEAD={head} differ only in \
                 non-source files — binary is current."
            );
        }
    }

    // ── 2 + 3. CHURN, TIMING, FRESHNESS ─────────────────────────────────────
    // Each burst is measured independently via a per-round filename prefix so
    // the poll target is exactly that burst's `count` (not the running total),
    // and creation throughput is reported apart from apply-to-visible latency.
    for (round, &count) in BURSTS.iter().enumerate() {
        println!("\n== Burst {}: create {count} files ==", round + 1);
        let create_start = Instant::now();
        for i in 0..count {
            fs::write(base.join(format!("idx_{round}_{i}.tmp")), b"x")
                .with_context(|| format!("write idx_{round}_{i}.tmp"))?;
        }
        let create_elapsed = create_start.elapsed();

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

    // ── Rename + delete correctness smoke, on UNIQUE sentinel names ─────────
    // `idxmutate*` shares no trigram with the bulk `idx_<round>_<i>` files, so
    // each search is unambiguous (the old `idx_0_1` probe matched 111 bulk
    // files by substring — a false signal). Poll-until-applied, not a sleep.
    println!("\n== Mutate smoke (unique sentinels) ==");
    let src = base.join("idxmutate_src.tmp");
    let del = base.join("idxmutate_del.tmp");
    fs::write(&src, b"x").context("write idxmutate_src.tmp")?;
    fs::write(&del, b"x").context("write idxmutate_del.tmp")?;
    let (staged, _, stage_to) =
        poll_until_visible(&uffs, "idxmutate", 2, Duration::from_secs(20))?;
    println!(
        "   staged 2 sentinels; 'idxmutate' -> {staged}/2 visible{}",
        if stage_to { "  <<< TIMED OUT" } else { "" }
    );

    fs::rename(&src, base.join("idxmutate_renamed.tmp")).context("rename sentinel")?;
    fs::remove_file(&del).context("delete sentinel")?;

    let mutate_wait = Duration::from_secs(20);
    let (ren_rows, ren_lat, ren_to) =
        poll_until_visible(&uffs, "idxmutate_renamed", 1, mutate_wait)?;
    let (del_rows, del_lat, del_to) = poll_until_absent(&uffs, "idxmutate_del", mutate_wait)?;
    let (old_rows, _, _) = poll_until_absent(&uffs, "idxmutate_src", Duration::from_secs(6))?;
    println!(
        "   rename : 'idxmutate_renamed' -> {ren_rows} after {:.1}s (expect >=1){}",
        ren_lat.as_secs_f64(),
        if ren_to { "  <<< FAIL/TIMED OUT" } else { "" }
    );
    println!(
        "   delete : 'idxmutate_del'      -> {del_rows} after {:.1}s (expect 0){}",
        del_lat.as_secs_f64(),
        if del_to { "  <<< FAIL/TIMED OUT" } else { "" }
    );
    println!("   oldname: 'idxmutate_src'      -> {old_rows} (expect 0, renamed away)");

    // ── Stop the daemon to flush, then extract + summarise the timing ───────
    println!("\n== Stopping daemon to flush the log ==");
    let _ = Command::new(&uffs).args(["--daemon", "stop"]).status();
    sleep(KILL_SETTLE);

    let log = read_log(&log_path);
    let timing_lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("usn apply: batch applied"))
        .collect();
    fs::write(run_dir.join("idx-timing.log"), timing_lines.join("\n"))?;

    let baseline = summarise(&timing_lines);
    println!("\n== BASELINE (per-apply cost) ==");
    println!("{baseline}");
    fs::write(run_dir.join("baseline.txt"), &baseline)?;

    println!("\n== Done ==");
    println!("Share: {}", run_dir.display());
    println!("Key: baseline.txt (per-apply numbers), idx-timing.log (raw apply lines), uffsd.log.");
    Ok(())
}

/// All numeric values of a `key=value` tracing field across the lines that
/// carry it.  Field-generic so a new apply-summary field needs no parser change.
fn field_values(lines: &[&str], key: &str) -> Vec<f64> {
    let prefix = format!("{key}=");
    lines
        .iter()
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix(&prefix))
                .and_then(|raw| raw.parse::<f64>().ok())
        })
        .collect()
}

/// Build the human-readable baseline from the `usn apply: batch applied` lines:
/// how many applies fired, the changes they coalesced, the per-apply wall-clock
/// (mean + worst case), and how many crossed the compaction threshold. The
/// per-apply `apply_us` is the number the perf guard watches — it must track the
/// batch size, not the drive's record count.
fn summarise(lines: &[&str]) -> String {
    if lines.is_empty() {
        return "  (no `usn apply: batch applied` lines captured — did any apply fire? \
                check uffsd.log, the apply cadence, and that UFFS_LOG enables uffs_core=debug)"
            .to_owned();
    }
    let records = field_values(lines, "records")
        .into_iter()
        .fold(0_f64, f64::max);
    let changes = field_values(lines, "changes");
    let total_changes: f64 = changes.iter().sum();
    let mean_changes = total_changes / changes.len().max(1) as f64;

    // `apply_us` is whole-microsecond (integer, per uffs-core's no-float policy);
    // render as ms here (1 us = 0.001 ms).
    let apply = field_values(lines, "apply_us");
    let mean_apply_ms = apply.iter().sum::<f64>() / apply.len().max(1) as f64 / 1000.0;
    let max_apply_ms = apply.iter().copied().fold(0_f64, f64::max) / 1000.0;

    // `compacted=true` counts the applies that crossed TRIGRAM_COMPACT_THRESHOLD
    // and refolded the bases (the O(total) path); the rest stayed O(changed).
    let compactions = lines
        .iter()
        .filter(|line| line.split_whitespace().any(|tok| tok == "compacted=true"))
        .count();

    format!(
        "  apply lines:       {}\n  \
         drive records:     {records:.0}\n  \
         total changes:     {total_changes:.0}\n  \
         mean changes/apply {mean_changes:>10.0}\n  \
         compactions:       {compactions}  (compacted=true, full base refold)\n  \
         ─────────────────────────────────\n  \
         mean apply         {mean_apply_ms:>10.3} ms\n  \
         max  apply         {max_apply_ms:>10.3} ms   <- worst per-apply cost\n",
        lines.len()
    )
}

/// Read the daemon log, tolerating a missing file (returns empty).
fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
