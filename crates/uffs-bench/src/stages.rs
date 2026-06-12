// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stages 1–3 — the live measurement wrappers (execution-plan §P5/§P6).
//!
//! Each stage follows the "no crumb left behind" cycle: it snapshots the host
//! resources the work will touch (R1 the UFFS daemon run-state, R2 the
//! per-drive cache files) and registers their restores on the [`RunGuard`]
//! *before* any mutation, then shells out through the [`Host`] seam.
//!
//! - **Stage 1 (cross-tool)** shells out to `cross-tool-benchmark.rs` with
//!   `--skip-cold`; the harness writes `bundle/cross-tool-summary.csv` itself.
//! - **Stage 2 (parity)** shells out to `cold-parity-per-drive.rs` via
//!   `rust-script`; the harness writes `bundle/parity.txt` (purging cache first
//!   when `--purge-cache` is set).
//! - **Stage 3 (full suite)** times native UFFS queries directly (N rounds per
//!   drive × pattern), reduces each cell with the unit-tested [`percentiles`]
//!   helper, and emits `bundle/full-suite.csv` + `bundle/full-suite.txt`.
//!
//! Because every side effect flows through [`Host`], all three stages — and
//! their snapshot/restore arming — are unit-testable under the `MockHost` on
//! any OS; the *live* runs require an elevated Windows box.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::error::{BenchError, Result};
use crate::gate::StepResult;
use crate::host::{Host, ProcOutput};
use crate::preflight::PatternProbe;
use crate::restore::RunGuard;

/// Repo-relative path to the cross-tool harness shelled out to by Stage 1.
pub const CROSS_TOOL_SCRIPT: &str = "scripts/windows/cross-tool-benchmark.rs";
/// Repo-relative path to the parity harness shelled out to by Stage 2.
pub const PARITY_SCRIPT: &str = "scripts/windows/cold-parity-per-drive.rs";

/// Bundle-relative name of the cross-tool summary CSV (written by the harness).
const CROSS_TOOL_OUT: &str = "cross-tool-summary.csv";
/// Bundle-relative name of the parity transcript (written by the script).
const PARITY_OUT: &str = "parity.txt";
/// Bundle-relative name of the Stage 3 full-suite CSV.
const FULL_SUITE_CSV: &str = "full-suite.csv";
/// Bundle-relative name of the Stage 3 full-suite text summary.
const FULL_SUITE_TXT: &str = "full-suite.txt";

/// Per-drive UFFS cache file suffixes snapshotted for R2 restore.
const CACHE_SUFFIXES: [&str; 2] = ["_index.uffs", "_compact.uffs"];

/// Common-convention header for the Stage 3 CSV (execution-plan §11).
const FULL_SUITE_HEADER: &str =
    "tool,version,phase,sink,drive,pattern,rows,p50_ms,p95_ms,stddev_ms,rounds,verdict,notes\n";

/// Reduced latency statistics for one measured cell.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Percentiles {
    /// Median (50th-percentile) round duration, in milliseconds.
    pub p50_ms: f64,
    /// 95th-percentile round duration, in milliseconds.
    pub p95_ms: f64,
    /// Population standard deviation of the round durations, in milliseconds.
    pub stddev_ms: f64,
    /// Number of samples the statistics were computed over.
    pub rounds: u32,
}

/// Nearest-rank percentile (`pct` in `0..=100`) of a pre-sorted slice.
///
/// Uses the integer ceiling rank `ceil(pct/100 · n)` clamped to `1..=n`, so it
/// never touches floating point and never indexes out of bounds.
fn nearest_rank(sorted: &[u64], pct: u32) -> u64 {
    let len = sorted.len();
    if len == 0 {
        return 0;
    }
    let pct_usize = usize::try_from(pct).unwrap_or(0);
    let rank = (pct_usize * len).div_ceil(100).clamp(1, len);
    sorted.get(rank - 1).copied().unwrap_or(0)
}

/// Reduce raw per-round millisecond samples to [`Percentiles`].
///
/// `p50`/`p95` are integer nearest-rank selections; the standard deviation is
/// the population formula over the same samples. Returns the all-zero default
/// for an empty slice.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::float_arithmetic,
    reason = "percentile statistics require an f64 mean/variance over the \
              millisecond samples; benchmark round counts are small so the \
              u64/usize -> f64 widening loses no meaningful precision"
)]
pub fn percentiles(samples_ms: &[u64]) -> Percentiles {
    let rounds = u32::try_from(samples_ms.len()).unwrap_or(u32::MAX);
    if samples_ms.is_empty() {
        return Percentiles::default();
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_unstable();

    let count = samples_ms.len();
    let sum: u64 = samples_ms.iter().sum();
    let mean = sum as f64 / count as f64;
    let variance = samples_ms
        .iter()
        .map(|&value| {
            let delta = value as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / count as f64;

    Percentiles {
        p50_ms: nearest_rank(&sorted, 50) as f64,
        p95_ms: nearest_rank(&sorted, 95) as f64,
        stddev_ms: variance.sqrt(),
        rounds,
    }
}

/// A single command, displayed verbatim on its card and run verbatim here.
struct Invocation {
    /// Executable to spawn.
    exe: String,
    /// Arguments passed to the executable.
    args: Vec<String>,
}

impl Invocation {
    /// Render the command as a single shell-style line for the card.
    fn display(&self) -> String {
        if self.args.is_empty() {
            self.exe.clone()
        } else {
            format!("{} {}", self.exe, self.args.join(" "))
        }
    }

    /// Spawn the command through the [`Host`] seam, capturing all output.
    fn run(&self, host: &dyn Host) -> io::Result<ProcOutput> {
        let refs: Vec<&str> = self.args.iter().map(String::as_str).collect();
        host.run(&self.exe, &refs)
    }

    /// Spawn the command, inheriting the parent's stdout/stderr so output
    /// flows live to the operator's terminal.  Returns a synthetic
    /// [`ProcOutput`] with the exit code and empty captured streams.
    fn run_streaming(&self, host: &dyn Host) -> io::Result<ProcOutput> {
        let refs: Vec<&str> = self.args.iter().map(String::as_str).collect();
        let code = host.run_streaming(&self.exe, &refs)?;
        Ok(ProcOutput {
            code,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// The `rust-script` launcher used to run the Stage 1 and Stage 2 harnesses.
const RUST_SCRIPT_EXE: &str = "rust-script";

/// Card-facing label for the daemon run-state resource (R1).
const DAEMON_RESOURCE: &str = "uffs daemon (run-state)";

/// Everything a measurement stage needs to plan and run.
///
/// Built once per stage from the CLI, the negotiated matrix (for the
/// cross-tool–capable drive subset), and the default pattern set. Holding the
/// resolved values here keeps [`plan`] (card rendering) and [`run_stage`]
/// (execution) in lock-step so the command shown equals the command run.
#[derive(Debug, Clone, Default)]
pub struct StageCfg {
    /// Bundle directory artifacts (and R2 backups) are written into.
    pub bundle_dir: PathBuf,
    /// Drives every required tool can serve (Stage 1 cross-tool head-to-head).
    pub capable_drives: Vec<char>,
    /// All candidate drives (Stage 2 parity and Stage 3 native).
    pub drives: Vec<char>,
    /// Tool ids participating in the cross-tool stage.
    pub tools: Vec<String>,
    /// Measurement rounds per cell.
    pub rounds: u32,
    /// Whether cold measurements (and the R2 cache purge) are requested.
    pub drop_cache: bool,
    /// Native Stage 3 patterns (UFFS argument templates with `{DRIVE}`).
    pub patterns: Vec<PatternProbe>,
    /// The UFFS executable invoked for Stage 3 native timing.
    pub uffs_exe: String,
    /// Named Everything instance to connect to via `es.exe -instance <name>`.
    /// `None` means the default IPC window (system-wide Everything instance).
    /// Set to `Some(INSTANCE_NAME)` when the bench tool launched a private
    /// `Everything.exe -instance uffs-bench` so the harness can find it.
    pub es_instance_name: Option<String>,
}

/// The card-facing plan for one measurement stage.
///
/// A pure projection of [`StageCfg`]: the exact commands the stage will run,
/// the resources it touches, the backups it takes before mutating, and a rough
/// time estimate. [`run_stage`] performs precisely these commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlan {
    /// Exact commands shown verbatim on the card and run verbatim.
    pub commands: Vec<String>,
    /// Blast-radius resource ids the stage touches.
    pub resources: Vec<String>,
    /// Human-readable description of the backups taken before mutating.
    pub backups: Vec<String>,
    /// Rough time estimate for the stage.
    pub est_time: String,
}

/// Join drive letters into the comma list the harness flags expect.
fn join_drives(drives: &[char]) -> String {
    drives
        .iter()
        .map(char::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Map a bench CLI tool name to the token the cross-tool harness accepts,
/// or `None` if the harness does not support that tool.
///
/// `everything_gui` (`Everything.exe`) has no CLI benchmarking interface
/// the harness can drive; it is benchmarked indirectly via `es.exe` (the
/// `everything` token).  All other names are passed through with `_` → `-`.
fn harness_tool(name: &str) -> Option<String> {
    match name {
        "everything_gui" | "everything-gui" => None,
        other => Some(other.replace('_', "-")),
    }
}

/// Card note describing the daemon restore taken before mutating.
fn daemon_backup_note() -> String {
    format!("{DAEMON_RESOURCE}: stop/restart to as-found state on teardown")
}

/// Card note describing the per-drive cache backup (R2).
fn cache_backup_note(drives: &[char]) -> String {
    format!(
        "uffs cache for {}: copied to bundle/backup",
        join_drives(drives)
    )
}

/// The UFFS cache directory (`%LOCALAPPDATA%\uffs\cache`), if discoverable.
fn cache_dir(host: &dyn Host) -> Option<PathBuf> {
    let base = host.env("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("uffs").join("cache"))
}

/// Enumerate the per-drive UFFS cache file paths that a measurement stage may
/// touch (R2 resource).
///
/// Used by [`crate::teardown::baseline`] to build the
/// [`crate::fingerprint::FingerprintSpec`] that
/// records the pre/post-run cache-file state as part of the "no crumb left
/// behind" policy.  Returns an empty `Vec` when `LOCALAPPDATA` is absent (non-
/// Windows or a stripped test environment).
#[must_use]
pub fn cache_files(host: &dyn Host, drives: &[char]) -> Vec<PathBuf> {
    let Some(dir) = cache_dir(host) else {
        return Vec::new();
    };
    drives
        .iter()
        .flat_map(|&drive| {
            let drive_dir = dir.clone();
            CACHE_SUFFIXES
                .iter()
                .map(move |suffix| drive_dir.join(format!("{drive}{suffix}")))
        })
        .collect()
}

/// Substitute `{DRIVE}` in a pattern's argument template for one drive.
fn resolve_pattern_args(drive: char, probe: &PatternProbe) -> Vec<String> {
    let letter = drive.to_string();
    probe
        .args
        .iter()
        .map(|arg| arg.replace("{DRIVE}", &letter))
        .collect()
}

/// Build the native UFFS invocation for one `(drive, pattern)` cell.
fn native_invocation(cfg: &StageCfg, drive: char, probe: &PatternProbe) -> Invocation {
    Invocation {
        exe: cfg.uffs_exe.clone(),
        args: resolve_pattern_args(drive, probe),
    }
}

/// Build the Stage 1 cross-tool harness invocation.
fn cross_tool_invocation(cfg: &StageCfg) -> Invocation {
    let out_path = cfg.bundle_dir.join(CROSS_TOOL_OUT);
    let mut args = vec![CROSS_TOOL_SCRIPT.to_owned()];
    if !cfg.capable_drives.is_empty() {
        args.push("--drives".to_owned());
        args.push(join_drives(&cfg.capable_drives));
    }
    args.push("--tools".to_owned());
    args.push(
        cfg.tools
            .iter()
            .filter_map(|tool| harness_tool(tool))
            .collect::<Vec<_>>()
            .join(","),
    );
    if let Some(inst) = &cfg.es_instance_name {
        args.push("--es-instance".to_owned());
        args.push(inst.clone());
    }
    args.push("--rounds".to_owned());
    args.push(cfg.rounds.to_string());
    args.push("--out".to_owned());
    args.push(out_path.display().to_string());
    // Cold/warm UFFS phases only run when the operator asked to drop caches.
    if !cfg.drop_cache {
        args.push("--skip-cold".to_owned());
    }
    Invocation {
        exe: RUST_SCRIPT_EXE.to_owned(),
        args,
    }
}

/// Build the Stage 2 parity harness invocation.
fn parity_invocation(cfg: &StageCfg) -> Invocation {
    let out_path = cfg.bundle_dir.join(PARITY_OUT);
    let mut args = vec![PARITY_SCRIPT.to_owned()];
    if !cfg.capable_drives.is_empty() {
        args.push("--drives".to_owned());
        args.push(join_drives(&cfg.capable_drives));
    }
    args.push("--rounds".to_owned());
    args.push(cfg.rounds.to_string());
    args.push("--output-file".to_owned());
    args.push(out_path.display().to_string());
    // The parity harness purges every drive cache before the cold warm-up.
    if cfg.drop_cache {
        args.push("--purge-cache".to_owned());
    }
    Invocation {
        exe: RUST_SCRIPT_EXE.to_owned(),
        args,
    }
}

// The daemon run-state snapshot/restore (R1) moved to `crate::run_state`,
// registered once in `crate::run` *before* the first daemon kill. Snapshotting
// it per-stage here was too late — `capture()` already restarts the daemon
// (scoped to the capable drives) before any stage runs, so the as-found drive
// set was already gone. The old probe also captured only a *bool* and shelled
// a bare `uffs` off `PATH`, so it never restored the operator's drive set.

/// Snapshot the per-drive UFFS cache files (R2) into `bundle/backup` and
/// register their restores *before* the stage purges them.
///
/// Best-effort: drives whose cache files are absent (or a host with no
/// `LOCALAPPDATA`) are skipped silently.
///
/// # Errors
/// Returns an error if the backup directory cannot be created or a cache file
/// cannot be copied into it.
fn snapshot_cache(host: &dyn Host, guard: &mut RunGuard<'_>, cfg: &StageCfg) -> Result<()> {
    let Some(dir) = cache_dir(host) else {
        return Ok(());
    };
    let backup_dir = cfg.bundle_dir.join("backup");
    host.create_dir_all(&backup_dir)
        .map_err(|err| BenchError::io(&backup_dir, err))?;
    for &drive in &cfg.capable_drives {
        for suffix in CACHE_SUFFIXES {
            let src = dir.join(format!("{drive}{suffix}"));
            if !host.path_exists(&src) {
                continue;
            }
            let backup = backup_dir.join(format!("{drive}{suffix}"));
            host.copy_file(&src, &backup)
                .map_err(|err| BenchError::io(&src, err))?;
            let restore_src = src.clone();
            let restore_backup = backup.clone();
            guard.register(format!("cache {drive}{suffix}"), move |restore_host| {
                restore_host
                    .copy_file(&restore_backup, &restore_src)
                    .map(|_bytes| ())
                    .map_err(|err| BenchError::io(&restore_src, err))
            });
        }
    }
    Ok(())
}

/// Convert chrono wall-clock endpoints to a non-negative millisecond duration.
fn elapsed_ms(start: DateTime<Utc>, end: DateTime<Utc>) -> u64 {
    u64::try_from((end - start).num_milliseconds()).unwrap_or(0)
}

/// Parse the first integer found in `stdout` (UFFS `--count` row total).
fn parse_rows(stdout: &str) -> u64 {
    for line in stdout.lines() {
        let digits: String = line.chars().filter(char::is_ascii_digit).collect();
        if let Ok(value) = digits.parse::<u64>() {
            return value;
        }
    }
    0
}

/// Probe `exe --version`, returning the first line or `"unknown"`.
fn probe_version(host: &dyn Host, exe: &str) -> String {
    match host.run(exe, &["--version"]) {
        Ok(out) if out.success() => {
            let line = out.stdout.lines().next().unwrap_or("").trim();
            if line.is_empty() {
                "unknown".to_owned()
            } else {
                line.to_owned()
            }
        }
        _ => "unknown".to_owned(),
    }
}

/// One measured `(drive, pattern)` native cell.
struct CellResult {
    /// Drive letter the cell was measured on.
    drive: char,
    /// Pattern name the cell was measured for.
    pattern: String,
    /// Row count reported by the last successful round.
    rows: u64,
    /// Reduced latency statistics over the rounds.
    stats: Percentiles,
    /// Whether every round of the cell exited successfully.
    ok: bool,
}

/// Time one native cell over `cfg.rounds`, reducing to a [`CellResult`].
fn measure_cell(host: &dyn Host, cfg: &StageCfg, drive: char, probe: &PatternProbe) -> CellResult {
    let capacity = usize::try_from(cfg.rounds).unwrap_or(usize::MAX);
    let mut samples = Vec::with_capacity(capacity);
    let invocation = native_invocation(cfg, drive, probe);
    let mut rows = 0_u64;
    let mut ok = true;
    for _round in 0..cfg.rounds {
        let start = host.now();
        match invocation.run(host) {
            Ok(out) => {
                samples.push(elapsed_ms(start, host.now()));
                if out.success() {
                    rows = parse_rows(&out.stdout);
                } else {
                    ok = false;
                }
            }
            Err(_) => ok = false,
        }
    }
    CellResult {
        drive,
        pattern: probe.name.clone(),
        rows,
        stats: percentiles(&samples),
        ok,
    }
}

/// Render Stage 3 cells to the common-convention CSV (execution-plan §11).
fn render_csv(version: &str, cells: &[CellResult]) -> String {
    let rows: Vec<String> = cells
        .iter()
        .map(|cell| {
            let verdict = if cell.ok { "ok" } else { "fail" };
            format!(
                "uffs,{version},hot,count,{drive},{pattern},{rows},\
                 {p50:.1},{p95:.1},{stddev:.1},{rounds},{verdict},",
                drive = cell.drive,
                pattern = cell.pattern,
                rows = cell.rows,
                p50 = cell.stats.p50_ms,
                p95 = cell.stats.p95_ms,
                stddev = cell.stats.stddev_ms,
                rounds = cell.stats.rounds,
            )
        })
        .collect();
    if rows.is_empty() {
        FULL_SUITE_HEADER.to_owned()
    } else {
        format!("{FULL_SUITE_HEADER}{}\n", rows.join("\n"))
    }
}

/// Render Stage 3 cells to a human-readable text summary.
fn render_txt(version: &str, cells: &[CellResult]) -> String {
    let mut lines = vec![
        format!("UFFS native full-suite — version {version}"),
        format!("cells: {}", cells.len()),
        String::new(),
    ];
    for cell in cells {
        let verdict = if cell.ok { "ok" } else { "FAIL" };
        lines.push(format!(
            "[{verdict}] {drive}: {pattern} — rows={rows} \
             p50={p50:.1}ms p95={p95:.1}ms stddev={stddev:.1}ms (n={rounds})",
            drive = cell.drive,
            pattern = cell.pattern,
            rows = cell.rows,
            p50 = cell.stats.p50_ms,
            p95 = cell.stats.p95_ms,
            stddev = cell.stats.stddev_ms,
            rounds = cell.stats.rounds,
        ));
    }
    // Trailing empty element makes `join` emit the final newline.
    lines.push(String::new());
    lines.join("\n")
}

/// Build a [`StepResult`] from a wrapped-harness process outcome.
fn step_from_output(out: &ProcOutput, output_path: &Path, label: &str) -> StepResult {
    let path = output_path.display().to_string();
    let summary = if out.success() {
        format!("{label} harness completed; results in {path}")
    } else {
        format!("{label} harness exited with {:?}", out.code)
    };
    StepResult {
        code: out.code,
        summary,
        output_path: Some(path),
    }
}

/// Stage 1 — cross-tool head-to-head (run the harness).
///
/// The daemon run-state restore (R1) is registered once, up front, in
/// [`crate::run`] — before the daemon is first killed — so it is not re-taken
/// per stage here (by stage time the as-found state is already gone).
fn run_cross_tool(
    host: &dyn Host,
    _guard: &mut RunGuard<'_>,
    cfg: &StageCfg,
) -> Result<StepResult> {
    let out = cross_tool_invocation(cfg)
        .run_streaming(host)
        .map_err(|err| BenchError::Command(format!("cross-tool harness: {err}")))?;
    let out_path = cfg.bundle_dir.join(CROSS_TOOL_OUT);
    Ok(step_from_output(&out, &out_path, "cross-tool"))
}

/// Stage 2 — per-drive parity (+R2 cache backup when purging, run the script).
///
/// R1 daemon run-state is restored once via [`crate::run`] (see
/// [`run_cross_tool`]); only the per-drive cache backup is stage-local.
fn run_parity(host: &dyn Host, guard: &mut RunGuard<'_>, cfg: &StageCfg) -> Result<StepResult> {
    if cfg.drop_cache {
        snapshot_cache(host, guard, cfg)?;
    }
    let out = parity_invocation(cfg)
        .run_streaming(host)
        .map_err(|err| BenchError::Command(format!("parity harness: {err}")))?;
    let out_path = cfg.bundle_dir.join(PARITY_OUT);
    Ok(step_from_output(&out, &out_path, "parity"))
}

/// Stage 3 — native UFFS full-suite timing (snapshot R1, measure, emit
/// CSV/TXT).
fn run_native(host: &dyn Host, _guard: &mut RunGuard<'_>, cfg: &StageCfg) -> Result<StepResult> {
    let version = probe_version(host, &cfg.uffs_exe);
    let mut cells = Vec::new();
    let mut all_ok = true;
    for &drive in &cfg.capable_drives {
        for probe in &cfg.patterns {
            let cell = measure_cell(host, cfg, drive, probe);
            all_ok = all_ok && cell.ok;
            cells.push(cell);
        }
    }
    let csv_path = cfg.bundle_dir.join(FULL_SUITE_CSV);
    host.write_file(&csv_path, render_csv(&version, &cells).as_bytes())
        .map_err(|err| BenchError::io(&csv_path, err))?;
    let txt_path = cfg.bundle_dir.join(FULL_SUITE_TXT);
    host.write_file(&txt_path, render_txt(&version, &cells).as_bytes())
        .map_err(|err| BenchError::io(&txt_path, err))?;
    Ok(StepResult {
        code: Some(i32::from(!all_ok)),
        summary: format!(
            "Timed {} native cell(s); results in {FULL_SUITE_CSV}",
            cells.len()
        ),
        output_path: Some(csv_path.display().to_string()),
    })
}

/// Project a stage's [`StageCfg`] into its card-facing [`StagePlan`].
///
/// Pure (no host access): the commands here are exactly what [`run_stage`]
/// executes for the same `stage` and `cfg`, upholding the transparency
/// guarantee. Stage numbers other than 1/2 render the native (Stage 3) plan.
#[must_use]
pub fn plan(stage: u32, cfg: &StageCfg) -> StagePlan {
    match stage {
        1 => StagePlan {
            commands: vec![cross_tool_invocation(cfg).display()],
            resources: vec![DAEMON_RESOURCE.to_owned()],
            backups: vec![daemon_backup_note()],
            est_time: "~2-8 min".to_owned(),
        },
        2 => {
            let mut resources = vec![DAEMON_RESOURCE.to_owned()];
            let mut backups = vec![daemon_backup_note()];
            if cfg.drop_cache {
                resources.push(format!("uffs cache: {}", join_drives(&cfg.capable_drives)));
                backups.push(cache_backup_note(&cfg.capable_drives));
            }
            StagePlan {
                commands: vec![parity_invocation(cfg).display()],
                resources,
                backups,
                est_time: "~1-6 min".to_owned(),
            }
        }
        _ => {
            let commands = cfg
                .capable_drives
                .iter()
                .flat_map(|&drive| {
                    cfg.patterns
                        .iter()
                        .map(move |probe| native_invocation(cfg, drive, probe).display())
                })
                .collect();
            StagePlan {
                commands,
                resources: vec![DAEMON_RESOURCE.to_owned()],
                backups: vec![daemon_backup_note()],
                est_time: "~1-5 min".to_owned(),
            }
        }
    }
}

/// Run measurement `stage` (1 cross-tool, 2 parity, else 3 native) over `cfg`.
///
/// Each stage registers its snapshot restores on `guard` *before* mutating, so
/// teardown (or a `Drop` on early return) leaves the host as found.
///
/// # Errors
/// Returns [`BenchError::Command`] if a wrapped harness cannot be spawned, or
/// [`BenchError::Io`] if a snapshot/backup or a Stage 3 artifact write fails. A
/// harness that *runs* but exits non-zero is reported in the [`StepResult`],
/// not as an error.
pub fn run_stage(
    host: &dyn Host,
    guard: &mut RunGuard<'_>,
    stage: u32,
    cfg: &StageCfg,
) -> Result<StepResult> {
    match stage {
        1 => run_cross_tool(host, guard, cfg),
        2 => run_parity(host, guard, cfg),
        _ => run_native(host, guard, cfg),
    }
}
