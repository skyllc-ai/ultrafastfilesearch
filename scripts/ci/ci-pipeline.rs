#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! dirs-next = "2.0"
//! futures = "0.3"
//! tokio = { version = "1.0", features = ["full"] }
//! indicatif = "0.17"
//! serde = { version = "1.0", features = ["derive"] }
//! serde_json = "1.0"
//! chrono = { version = "0.4", features = ["serde"] }
//! uuid = { version = "1.0", features = ["v4"] }
//! num_cpus = "1.0"
//! tempfile = "3"
//! ```
// =============================================================================
// scripts/ci/ci-pipeline.rs - UFFS High-Performance CI Pipeline
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// UFFS - UltraFastFileSearch: High-Performance File Search Tool
// Contact: 50460704+githubrobbi@users.noreply.github.com for licensing inquiries
//
//! Nightly CI pipeline driver with Tokio async orchestration
//!
//! This script implements advanced CI pipeline optimizations using:
//! - Tokio async/await for true parallelism
//! - Resource-aware process management
//! - Dependency graph execution
//! - Smart error handling and recovery

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use colored::*;
use futures::future::try_join_all;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════
// Step Definitions for UFFS CI Pipeline
// ═══════════════════════════════════════════════════════════════════════════

const STEP_TOOLCHAIN_SYNC: &str = "00-toolchain-ensure"; // Ensure pinned nightly is installed
const STEP_UPDATE_POLARS: &str = "01-update-polars-git"; // Bump polars git lock to latest main
const STEP_CLEAN_ARTIFACTS: &str = "02-clean-artifacts";
const STEP_FORMAT_CODE: &str = "03-format-code";
const STEP_COVERAGE_TESTS: &str = "04-coverage-tests";
const STEP_PARALLEL_VALIDATION: &str = "05-parallel-validation";
const STEP_FORMAT_CHECK: &str = "06-format-check";
const STEP_VERSION_INCREMENT: &str = "07-version-increment";
// Steps 08 (build-release) and 09 (deploy-binary) were removed: `just ship`
// no longer produces binaries locally.  The release branch PR (step 11)
// lands the version bump on main; `auto-tag-release.yml` then tags the
// commit and invokes `release.yml`, which builds + publishes from GitHub
// Actions.  Step numbering is preserved to keep in-flight resumable-ship
// state files compatible with older pipeline runs.
const STEP_GIT_COMMIT: &str = "10-git-commit";
const STEP_GIT_PUSH: &str = "11-git-push";

const ALL_STEPS: &[&str] = &[
    STEP_UPDATE_POLARS,
    STEP_CLEAN_ARTIFACTS,
    STEP_FORMAT_CODE,
    STEP_COVERAGE_TESTS,
    STEP_PARALLEL_VALIDATION,
    STEP_FORMAT_CHECK,
    STEP_VERSION_INCREMENT,
    STEP_GIT_COMMIT,
    STEP_GIT_PUSH,
];

/// Get the cargo target directory, checking env var and config file
fn get_cargo_target_dir() -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return expand_tilde_path(&target_dir);
    }
    if let Some(target_dir) = parse_cargo_config_target_dir() {
        return target_dir;
    }
    PathBuf::from("./target")
}

/// Expand a leading `~` to the current user's home directory on Unix-like hosts.
fn expand_tilde_path(path_str: &str) -> PathBuf {
    if path_str == "~" || path_str.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            let rest = path_str.strip_prefix("~/").unwrap_or("");
            return PathBuf::from(home).join(rest);
        }
    }

    PathBuf::from(path_str)
}

/// Parse .cargo/config.toml to find target-dir setting
fn parse_cargo_config_target_dir() -> Option<PathBuf> {
    let config_path = ".cargo/config.toml";
    if let Ok(content) = fs::read_to_string(config_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("target-dir") {
                if let Some(value) = trimmed.split('=').nth(1) {
                    let path_str = value.trim().trim_matches('"').trim_matches('\'');
                    return Some(expand_tilde_path(path_str));
                }
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Disk Space Monitoring
// ═══════════════════════════════════════════════════════════════════════════

/// Convert bytes to GiB (binary).
fn bytes_to_gib(bytes: u64) -> u64 {
    bytes / 1024 / 1024 / 1024
}

/// Best-effort free space lookup for the filesystem containing `path`.
/// Returns bytes free, or None if unavailable.
/// Uses `df -Pk` on unix-y systems.
async fn disk_free_bytes(path: &Path) -> Option<u64> {
    if cfg!(windows) {
        return None;
    }
    let path_str = path.to_str()?;
    let output = Command::new("df").arg("-Pk").arg(path_str).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    lines.next()?; // header
    let last = lines.last()?;
    let cols: Vec<&str> = last.split_whitespace().collect();
    if cols.len() < 4 {
        return None;
    }
    let avail_k = cols[3].parse::<u64>().ok()?;
    Some(avail_k * 1024)
}

/// Best-effort directory size for `path`, in bytes.
/// Uses `du -sk` on unix-y systems and is time-limited.
async fn dir_size_bytes(path: &Path, timeout_dur: Duration) -> Option<u64> {
    if cfg!(windows) {
        return None;
    }
    let path_str = path.to_str()?;

    let child = Command::new("du")
        .arg("-sk")
        .arg(path_str)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Use timeout wrapper
    let output = match timeout(timeout_dur, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(_)) | Err(_) => return None,
    };

    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let kb = stdout.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kb * 1024)
}

/// Check if a command exists in PATH
fn command_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ═══════════════════════════════════════════════════════════════════════════
// Workflow State Management
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkflowState {
    pub current_version: String,
    pub workflow_id: String,
    pub phase: WorkflowPhase,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_successful_version: String,
    pub failure_count: u32,
    pub last_error: Option<String>,
    pub step_tracker: StepTracker,
    pub version_incremented: bool,

    /// Per-step duration metrics (seconds), keyed by the step id (e.g. `03-coverage-tests`).
    /// Stored in the workflow-state file so you can compare runs over time.
    #[serde(default)]
    pub step_durations_secs: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum WorkflowPhase {
    Clean,
    VersionIncrementing,
    Testing,
    Building,
    Deploying,
    GitCommitting,
    GitPushing,
    Completed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StepTracker {
    pub completed_steps: BTreeSet<String>,
    pub failed_steps: BTreeSet<String>,
    pub current_step: Option<String>,
}

impl Default for StepTracker {
    fn default() -> Self {
        Self {
            completed_steps: BTreeSet::new(),
            failed_steps: BTreeSet::new(),
            current_step: None,
        }
    }
}

impl WorkflowState {
    const STATE_FILE: &'static str = "build/.uffs-workflow-state.json";

    pub fn load() -> Result<Self> {
        let path = Path::new(Self::STATE_FILE);
        if path.exists() {
            let content = fs::read_to_string(path).context("Failed to read workflow state file")?;
            serde_json::from_str(&content).context("Failed to parse workflow state file")
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(self).context("Failed to serialize workflow state")?;
        let path = Path::new(Self::STATE_FILE);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create state directory")?;
        }
        let temp_file = format!("{}.tmp", Self::STATE_FILE);
        fs::write(&temp_file, content).context("Failed to write temporary state file")?;
        fs::rename(&temp_file, Self::STATE_FILE).context("Failed to atomically update state file")?;
        Ok(())
    }

    pub fn advance_phase(&mut self, new_phase: WorkflowPhase) -> Result<()> {
        println!("🔄 Advancing workflow phase: {:?} → {:?}", self.phase, new_phase);
        self.phase = new_phase.clone();
        if new_phase == WorkflowPhase::Completed {
            self.completed_at = Some(Utc::now());
            self.last_successful_version = self.current_version.clone();
            println!("🎉 Workflow completed successfully! Version: {}", self.current_version);
        }
        self.save()
    }

    pub fn record_error(&mut self, error: &str) -> Result<()> {
        self.failure_count += 1;
        self.last_error = Some(error.to_string());
        self.save()
    }

    pub fn new_workflow(current_version: String) -> Self {
        Self {
            current_version,
            workflow_id: Uuid::new_v4().to_string(),
            phase: WorkflowPhase::Clean,
            started_at: Utc::now(),
            completed_at: None,
            last_successful_version: "unknown".to_string(),
            failure_count: 0,
            last_error: None,
            step_tracker: StepTracker::default(),
            version_incremented: false,
            step_durations_secs: BTreeMap::new(),
        }
    }

    pub fn is_resumable(&self) -> bool {
        matches!(
            self.phase,
            WorkflowPhase::VersionIncrementing
                | WorkflowPhase::Testing
                | WorkflowPhase::Building
                | WorkflowPhase::Deploying
                | WorkflowPhase::GitCommitting
                | WorkflowPhase::GitPushing
        )
    }

    pub fn is_step_completed(&self, step: &str) -> bool {
        self.step_tracker.completed_steps.contains(step)
    }

    pub fn mark_step_started(&mut self, step: &str) -> Result<()> {
        self.step_tracker.current_step = Some(step.to_string());
        self.step_tracker.failed_steps.remove(step);
        println!("🔄 Starting step: {}", step);
        self.save()
    }

    pub fn mark_step_completed(&mut self, step: &str) -> Result<()> {
        self.step_tracker.completed_steps.insert(step.to_string());
        self.step_tracker.failed_steps.remove(step);
        self.step_tracker.current_step = None;
        println!("✅ Completed step: {}", step);
        self.save()
    }

    pub fn mark_step_failed(&mut self, step: &str, error: &str) -> Result<()> {
        self.step_tracker.failed_steps.insert(step.to_string());
        self.step_tracker.completed_steps.remove(step);
        self.step_tracker.current_step = None;
        self.record_error(&format!("Step '{}' failed: {}", step, error))?;
        println!("❌ Failed step: {} - {}", step, error);
        self.save()
    }

    pub fn get_pending_steps(&self, all_steps: &[&str]) -> Vec<String> {
        all_steps
            .iter()
            .filter(|step| !self.is_step_completed(step))
            .map(|s| s.to_string())
            .collect()
    }
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            current_version: "unknown".to_string(),
            workflow_id: Uuid::new_v4().to_string(),
            phase: WorkflowPhase::Clean,
            started_at: Utc::now(),
            completed_at: None,
            last_successful_version: "unknown".to_string(),
            failure_count: 0,
            last_error: None,
            step_tracker: StepTracker::default(),
            version_incremented: false,
            step_durations_secs: BTreeMap::new(),
        }
    }
}


// ═══════════════════════════════════════════════════════════════════════════
// CLI Definition
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Parser)]
#[command(name = "ci-pipeline")]
#[command(about = "UFFS High-Performance CI Pipeline with Async Orchestration")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output (show all command details)
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Generate coverage report (slower, but comprehensive)
    #[arg(short, long, global = true)]
    coverage_report: bool,

    /// Force a full `cargo clean` at the start (slower, but can recover from stale artifacts)
    #[arg(long, global = true)]
    clean: bool,

    /// Force skipping cargo clean even when auto-clean would run (dangerous if disk is tight).
    #[arg(long, global = true)]
    no_clean: bool,

    /// Auto-clean if free disk space (GiB) is below this threshold.
    #[arg(long, global = true, default_value_t = 25)]
    min_free_gb: u64,

    /// Auto-clean if the cargo target directory exceeds this size (GiB). Best-effort; unix only.
    #[arg(long, global = true, default_value_t = 120)]
    max_target_gb: u64,

    /// Override Cargo build parallelism (rustc job count)
    /// If omitted, defaults to `min(num_cpus, 16)`.
    #[arg(long, global = true)]
    jobs: Option<usize>,

    /// Jobs per cargo process during the parallel validation stage (defaults to max(2, jobs/2)).
    #[arg(long, global = true)]
    parallel_jobs: Option<usize>,

    /// Disable sccache auto-detection/integration even if it is installed.
    #[arg(long, global = true)]
    no_sccache: bool,

    /// Force a fresh run, ignoring any previously completed steps.
    /// Use this to start the pipeline from scratch.
    #[arg(long, global = true)]
    fresh: bool,

    /// Skip the nightly toolchain bump even on `--fresh` runs.
    ///
    /// By default `ship --fresh` invokes `just toolchain-sync` (bumps
    /// `rust-toolchain.toml` to today's nightly).  Pass this flag when
    /// the latest nightly is known-broken and you want to keep the
    /// currently pinned one — the pipeline will fall back to
    /// `just toolchain-ensure` (install-the-pinned-one).  Non-fresh
    /// `ship` runs always use `toolchain-ensure` regardless of this
    /// flag; the sync only happens on `--fresh`.
    #[arg(long, global = true)]
    skip_toolchain_sync: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Safe-by-default validation workflow (no version bump, deploy, commit, or push)
    Go,
    /// Full ship pipeline: Phase 1 validation + Phase 2 deploy (resumable)
    /// Re-runs skip already-completed steps. Use --fresh to start from scratch.
    Ship,
    /// Comprehensive nightly-grade validation with parallel execution
    CheckAll,
    /// Phase 1 nightly validation gates with maximum parallelism
    Phase1,
    /// Explicit ship lane: version bump, build, deploy, commit, and push
    Phase2,
    /// Generate coverage report from existing data (or run tests if needed)
    CoverageReport,
    /// Multi-tool security audit with parallelism
    AuditComprehensive,
    /// Check current workflow status
    WorkflowStatus,
    /// Reset workflow state (force clean slate)
    WorkflowReset,
    /// Resume incomplete workflow
    WorkflowResume,
    /// Nightly cross-compilation validation
    CrossCheck,
}

/// Pipeline execution context with resource management
struct PipelineContext {
    start_time: Instant,
    max_parallel_jobs: usize,
    #[allow(dead_code)] // Reserved for future use with cargo -j flag
    parallel_jobs: usize,
    timeout_duration: Duration,
    verbose: bool,
    coverage_report: bool,
    force_clean: bool,
    force_no_clean: bool,
    min_free_gb: u64,
    max_target_gb: u64,
    /// Whether sccache was auto-detected and enabled.
    sccache_enabled: bool,
    /// Global environment variables to set for all cargo commands.
    global_env: Vec<(String, String)>,
    /// Log file for capturing output in non-verbose mode.
    log_file: Option<PathBuf>,
    /// Force a fresh run, ignoring any previously completed steps.
    fresh: bool,
    /// Skip `toolchain-sync` on `--fresh` runs (keep the currently pinned nightly).
    skip_toolchain_sync: bool,
}

impl PipelineContext {
    #[allow(clippy::too_many_arguments)]
    fn new(
        verbose: bool,
        coverage_report: bool,
        force_clean: bool,
        force_no_clean: bool,
        min_free_gb: u64,
        max_target_gb: u64,
        jobs: Option<usize>,
        parallel_jobs: Option<usize>,
        no_sccache: bool,
        fresh: bool,
        skip_toolchain_sync: bool,
    ) -> Self {
        let max_jobs = jobs.unwrap_or_else(|| num_cpus::get().min(16));
        let par_jobs = parallel_jobs.unwrap_or_else(|| (max_jobs / 2).max(2));

        // Build global environment variables
        let mut global_env: Vec<(String, String)> = Vec::new();

        // Normalize Cargo's target dir so child cargo/nextest processes don't treat `~/...`
        // from .cargo/config.toml as a literal workspace-relative path segment.
        let cargo_target_dir = get_cargo_target_dir();
        global_env.push((
            "CARGO_TARGET_DIR".into(),
            cargo_target_dir.to_string_lossy().into_owned(),
        ));

        // Optional sccache integration (massive win in CI and on developer machines).
        //
        // sccache refuses to wrap rustc when CARGO_INCREMENTAL=1 because
        // incremental builds produce non-cacheable artifacts (sccache bails
        // out at `sccache rustc -vV` with
        // "incremental compilation is prohibited").  `just/shared.just`
        // exports CARGO_INCREMENTAL=1 for local dev UX, so whenever we enable
        // sccache for the pipeline we must pair it with CARGO_INCREMENTAL=0
        // at the SAME scope.  Previously the unset lived in a per-command
        // guard further down — which caught `cargo` and `rust-script` but
        // not `git` (whose pre-push hook shells out to cargo internally and
        // inherits this env).  Keeping the pair in the global env is the
        // only scope that covers every subprocess the pipeline spawns.
        let sccache_available = !no_sccache && command_exists("sccache");
        if sccache_available {
            global_env.push(("RUSTC_WRAPPER".into(), "sccache".into()));
            global_env.push(("CARGO_INCREMENTAL".into(), "0".into()));
            // Diagnostic marker — presence of this line in the pipeline
            // output proves the fresh binary (post-`fix(ci): pair
            // CARGO_INCREMENTAL=0 with RUSTC_WRAPPER=sccache`) is the
            // one being executed, not a stale rust-script cache entry.
            if verbose {
                eprintln!(
                    "[ci-pipeline][sccache-fix] global_env: RUSTC_WRAPPER=sccache + CARGO_INCREMENTAL=0 paired"
                );
            }
        }

        // Create log file for non-verbose mode
        let log_file = if !verbose {
            let log_dir = PathBuf::from("build/logs");
            let _ = fs::create_dir_all(&log_dir);
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            Some(log_dir.join(format!("ci-pipeline-{}.log", timestamp)))
        } else {
            None
        };

        Self {
            start_time: Instant::now(),
            max_parallel_jobs: max_jobs,
            parallel_jobs: par_jobs,
            timeout_duration: Duration::from_secs(3600), // 60 minutes max
            verbose,
            coverage_report,
            force_clean,
            force_no_clean,
            min_free_gb,
            max_target_gb,
            sccache_enabled: sccache_available,
            global_env,
            log_file,
            fresh,
            skip_toolchain_sync,
        }
    }
}

/// Create a fillup-style progress spinner
fn create_fillup_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    let fillup_frames = vec![
        "▱▱▱▱▱▱▱▱▱▱", "▰▱▱▱▱▱▱▱▱▱", "▰▰▱▱▱▱▱▱▱▱", "▰▰▰▱▱▱▱▱▱▱",
        "▰▰▰▰▱▱▱▱▱▱", "▰▰▰▰▰▱▱▱▱▱", "▰▰▰▰▰▰▱▱▱▱", "▰▰▰▰▰▰▰▱▱▱",
        "▰▰▰▰▰▰▰▰▱▱", "▰▰▰▰▰▰▰▰▰▱", "▰▰▰▰▰▰▰▰▰▰", "▱▰▰▰▰▰▰▰▰▰",
        "▱▱▰▰▰▰▰▰▰▰", "▱▱▱▰▰▰▰▰▰▰", "▱▱▱▱▰▰▰▰▰▰", "▱▱▱▱▱▰▰▰▰▰",
        "▱▱▱▱▱▱▰▰▰▰", "▱▱▱▱▱▱▱▰▰▰", "▱▱▱▱▱▱▱▱▰▰", "▱▱▱▱▱▱▱▱▱▰",
    ];
    let style = ProgressStyle::default_spinner()
        .tick_strings(&fillup_frames)
        .template(&format!("{{spinner}} {}", message.cyan()))
        .unwrap();
    pb.set_style(style);
    pb.enable_steady_tick(Duration::from_millis(150));
    pb
}

fn coverage_data_exists() -> bool {
    let target_dir = get_cargo_target_dir();
    target_dir.join("llvm-cov").exists()
}


// ═══════════════════════════════════════════════════════════════════════════
// Command Execution Functions
// ═══════════════════════════════════════════════════════════════════════════

async fn execute_command_with_env(
    name: &str,
    cmd: &str,
    args: &[&str],
    env_vars: &[(&str, &str)],
    ctx: &PipelineContext,
) -> Result<()> {
    let step_start = Instant::now();
    if ctx.verbose {
        println!("{} {} → {} {} (env: {:?})", "→".blue().bold(), name.cyan(), cmd.yellow(), args.join(" ").dimmed(), env_vars);
    } else {
        println!("{} {}", "→".blue().bold(), name.cyan());
    }

    let mut command = Command::new(cmd);
    command.args(args);

    // Apply global environment variables first
    for (key, value) in &ctx.global_env {
        command.env(key, value);
    }

    // Then apply step-specific environment variables (can override globals)
    for (key, value) in env_vars {
        command.env(key, value);
    }

    // NOTE: the sccache/`CARGO_INCREMENTAL=0` pairing is enforced in the
    // global env at context-construction time (see `ContextBuilder`),
    // which correctly covers every subprocess the pipeline spawns —
    // including `git`, whose pre-push hook shells out to cargo.  A
    // per-command guard here would only have caught `cargo` /
    // `rust-script`, leaving hook-invoked cargo processes to fail with
    // "incremental compilation is prohibited".

    // In verbose mode, inherit stdio; otherwise capture to log file
    if ctx.verbose {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }

    let child = command.spawn().with_context(|| format!("Failed to spawn command '{}' for step '{}'", cmd, name))?;
    let progress_bar = if !ctx.verbose { Some(create_fillup_spinner(name)) } else { None };

    let result = timeout(ctx.timeout_duration, child.wait_with_output())
        .await
        .with_context(|| format!("Command '{}' timed out after {}s", cmd, ctx.timeout_duration.as_secs()))?
        .with_context(|| format!("Failed to wait for command '{}' in step '{}'", cmd, name))?;

    if let Some(pb) = progress_bar { pb.finish_and_clear(); }
    let duration = step_start.elapsed();

    // Write output to log file if available
    if let Some(log_path) = &ctx.log_file {
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = writeln!(file, "\n=== {} ({}) ===", name, cmd);
            let _ = writeln!(file, "Command: {} {}", cmd, args.join(" "));
            let _ = writeln!(file, "Duration: {}s", duration.as_secs());
            if !result.stdout.is_empty() {
                let _ = writeln!(file, "--- stdout ---");
                let _ = file.write_all(&result.stdout);
            }
            if !result.stderr.is_empty() {
                let _ = writeln!(file, "--- stderr ---");
                let _ = file.write_all(&result.stderr);
            }
        }
    }

    if result.status.success() {
        println!("{} {} ({}s)", "✅".green(), name, duration.as_secs());
        Ok(())
    } else {
        let exit_code = result.status.code().map_or("unknown".to_string(), |c| c.to_string());
        println!("{} {} failed (exit code: {})", "❌".red(), name, exit_code);

        // Print stderr on failure even in non-verbose mode
        if !ctx.verbose && !result.stderr.is_empty() {
            eprintln!("{}", String::from_utf8_lossy(&result.stderr));
        }

        bail!("Step '{}' failed: command '{}' exited with code {} after {}s", name, cmd, exit_code, duration.as_secs());
    }
}

async fn execute_command(name: &str, cmd: &str, args: &[&str], ctx: &PipelineContext) -> Result<()> {
    execute_command_with_env(name, cmd, args, &[], ctx).await
}

async fn execute_parallel(commands: Vec<(&str, &str, Vec<&str>)>, ctx: &PipelineContext) -> Result<()> {
    let parallel_start = Instant::now();
    let command_count = commands.len();
    println!("{} Running {} commands in parallel...", "🔄".yellow(), command_count);

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(ctx.max_parallel_jobs));
    let tasks: Vec<_> = commands.into_iter().map(|(name, cmd, args)| {
        let semaphore = semaphore.clone();
        async move {
            let _permit = semaphore.acquire().await.context("Failed to acquire semaphore")?;
            execute_command(name, cmd, &args, ctx).await.with_context(|| format!("Parallel execution failed for '{}'", name))
        }
    }).collect();

    try_join_all(tasks).await.with_context(|| format!("Parallel execution failed - one or more of {} commands failed", command_count))?;
    println!("{} Parallel execution completed ({}s)", "✅".green(), parallel_start.elapsed().as_secs());
    Ok(())
}

async fn execute_parallel_with_env(commands: Vec<(&str, &str, Vec<&str>)>, env_vars: &[(&str, &str)], ctx: &PipelineContext) -> Result<()> {
    let parallel_start = Instant::now();
    let command_count = commands.len();
    println!("{} Running {} commands in parallel with env vars...", "⚡".yellow(), command_count);

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(ctx.max_parallel_jobs));
    let tasks: Vec<_> = commands.into_iter().map(|(name, cmd, args)| {
        let semaphore = semaphore.clone();
        let env_vars = env_vars.to_vec();
        async move {
            let _permit = semaphore.acquire().await.context("Failed to acquire semaphore")?;
            execute_command_with_env(name, cmd, &args, &env_vars, ctx).await.with_context(|| format!("Parallel execution failed for '{}'", name))
        }
    }).collect();

    try_join_all(tasks).await.with_context(|| format!("Parallel execution failed - one or more of {} commands failed", command_count))?;
    println!("{} Parallel execution completed ({}s)", "✅".green(), parallel_start.elapsed().as_secs());
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Phase 1: Safe-by-default validation with maximum parallelism
async fn phase1_optimized(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🧪 PHASE 1: Safe-by-Default Validation Pipeline".blue().bold());
    println!("ℹ️  No version bump, deploy, commit, or push in this lane");

    // Step 0: Ensure pinned nightly toolchain is installed
    execute_command("Toolchain ensure", "just", &["toolchain-ensure"], ctx).await?;

    // Step 0b: File size policy (fast — catches structural violations before expensive compilation)
    execute_command(
        "File size policy",
        "bash",
        &["scripts/ci/check_file_size_policy.sh"],
        ctx,
    ).await?;

    // Step 1: Workspace tests (nextest compiles everything — no separate `cargo check` needed)
    execute_command(
        "Workspace tests",
        "cargo",
        &["nextest", "run", "--workspace", "--all-features", "--lib", "--tests", "--profile", "ci"],
        ctx,
    ).await?;

    // Step 2: Generate coverage report (optional)
    if ctx.coverage_report {
        coverage_report_command(ctx).await?;
    } else if ctx.verbose {
        println!("{} Coverage report skipped (use --coverage-report to generate)", "⏭️".yellow());
    }

    // Step 3: Parallel — doc tests + linting + dependency security
    let parallel_commands = vec![
        ("Documentation tests", "cargo", vec!["test", "--doc", "--workspace", "--all-features"]),
        // pedantic/nursery/cargo/multiple_crate_versions levels are set in
        // workspace Cargo.toml — only per-target overrides needed here.
        ("Production linting", "cargo", vec![
            "clippy", "--workspace",
            "--all-targets", "--all-features", "--no-deps", "--",
            "-D", "warnings",
            "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
            "-W", "clippy::unwrap_used", "-W", "clippy::expect_used",
        ]),
        ("Test linting", "cargo", vec![
            "clippy", "--workspace",
            "--all-targets", "--all-features", "--tests", "--no-deps", "--",
            "-D", "warnings",
            "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
            "-A", "clippy::unwrap_used", "-A", "clippy::expect_used",
        ]),
        ("Dependency security", "cargo", vec!["deny", "check"]),
        ("Rustdoc link validation", "cargo", vec![
            "doc", "--workspace", "--all-features", "--no-deps",
        ]),
    ];
    execute_parallel_with_env(parallel_commands, &[("RUSTDOCFLAGS", "-Dwarnings")], ctx).await?;

    println!("{}", "✅ PHASE 1 COMPLETE: Validation passed without release-side effects!".green().bold());
    Ok(())
}

/// Phase 2: Explicit ship lane
async fn phase2_optimized(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🚀 PHASE 2: Explicit Ship Lane".blue().bold());

    // Step 1: Version increment
    version_bump(ctx).await?;

    // Update workflow state with new version
    let mut state = WorkflowState::load().context("Failed to load workflow state")?;
    let new_version = get_current_version().context("Failed to get updated version")?;
    state.current_version = new_version;
    state.version_incremented = true;
    state.save().context("Failed to save workflow state")?;
    println!("✅ Workflow state updated with new version: {}", state.current_version);

    // Step 2: Git commit (signed version-bump commit on the working branch).
    git_commit(ctx).await?;

    // Step 3: Git push -- opens release/vX.Y.Z PR with auto-merge queued.
    //
    // Binaries are NOT built here.  Once the PR merges to main,
    // `auto-tag-release.yml` tags the commit and invokes `release.yml`,
    // which produces the reproducible cross-platform binaries on
    // GitHub-hosted runners.
    git_push(ctx).await?;

    println!("{}", "✅ PHASE 2 COMPLETE: Versioned, committed, and release PR opened!".green().bold());
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

async fn version_bump(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📈 Incrementing version...".blue());
    let script_path = Path::new("./build/update_all_versions.rs");
    if script_path.exists() {
        execute_command("Version increment", "./build/update_all_versions.rs", &["patch"], ctx).await?;
    } else {
        println!("{}", "⚠️  Version script not found".yellow());
        bail!("Version bump failed - ./build/update_all_versions.rs not found");
    }
    Ok(())
}

// `build_release` + `deploy_binary` + `is_release_build` were removed in the
// "GitHub builds the binaries" hardening pass.  Binaries are now produced
// exclusively by `.github/workflows/release.yml` on GitHub-hosted runners
// (triggered by `auto-tag-release.yml` on version-bump merge to main), so
// the developer's laptop is no longer the trust root for shipped bytes.
//
// For a local dev build, run `just use-local` (renamed from `just use` in
// the same refactor).  For release-parity smoke testing run
// `gh workflow run release.yml --ref main -f version=vX.Y.Z` against a
// throwaway version and clean up with `gh release delete vX.Y.Z --cleanup-tag --yes`.

async fn git_commit(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📝 Creating auto-generated commit...".blue());
    execute_command("Git add", "git", &["add", "."], ctx).await?;

    let cargo_toml = fs::read_to_string("Cargo.toml").context("Failed to read Cargo.toml")?;
    let version = extract_version_from_cargo_toml(&cargo_toml)?;
    let commit_message = format!("chore: development v{} - comprehensive testing complete [auto-commit]", version);
    execute_command("Git commit", "git", &["commit", "-m", &commit_message], ctx).await?;
    Ok(())
}

async fn git_push(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🚀 Opening release PR (branch-protection-compatible)...".blue());

    // Get current branch name dynamically
    let branch_output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get current branch")?;
    let current_branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();

    if current_branch.is_empty() || current_branch == "HEAD" {
        bail!("Could not determine current branch (detached HEAD?)");
    }

    println!("📌 Current branch: {}", current_branch.cyan());

    // Stay current with upstream before opening the release PR.  Rebase keeps
    // the auto-commit on top of any intervening mainline changes.
    execute_command(
        "Git pull rebase",
        "git",
        &["pull", "origin", &current_branch, "--rebase"],
        ctx,
    )
    .await?;

    // Derive the release branch name from the workspace version that Phase 2
    // step 07 bumped.  Example: `release/v0.5.68`.
    let version = get_current_version()?;
    let release_branch = format!("release/v{}", version);
    let push_ref = format!("HEAD:refs/heads/{}", release_branch);

    // Push HEAD to the release branch.  No-op / fast-forward on re-runs of a
    // resumable ship that has already pushed the same commit.
    println!("📤 Pushing HEAD to {}", release_branch.cyan());
    execute_command(
        "Git push (release branch)",
        "git",
        &["push", "origin", &push_ref],
        ctx,
    )
    .await?;

    // Idempotent PR creation: reuse an existing open PR for the same release
    // branch if the pipeline is resuming from a previously-failed step 11.
    let existing_pr_output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            &release_branch,
            "--state",
            "open",
            "--json",
            "number",
            "-q",
            ".[0].number",
        ])
        .output()
        .context("Failed to query existing release PR via gh")?;
    let existing_pr = String::from_utf8_lossy(&existing_pr_output.stdout)
        .trim()
        .to_string();

    if existing_pr.is_empty() {
        let pr_title = format!("chore: release v{version} — ship pipeline auto-commit");
        let pr_body = format!(
            "## Summary\n\n\
             `just ship` Phase 2 auto-commit for **v{version}**.  Binaries + \
             GitHub Release v{version} are already live (step 09).  This PR \
             routes the corresponding commit through branch-protection rules.\n\n\
             ## Auto-merge\n\n\
             `--auto --squash` is queued — GitHub will merge as soon as the \
             required status checks pass.  Squash is required because \
             `main-protection` mandates signed commits, and GitHub's \
             rebase-auto-merge cannot sign the rebased commit; the \
             squash-merge commit is signed by GitHub's own key, which \
             satisfies `required_signatures: true`.  The original author's \
             signed commit remains verifiable in the PR branch history.\n\n\
             ## After merge\n\n\
             Local `{current_branch}` had this commit with a different SHA \
             before squash rewrote it onto main; recover with \
             `git fetch origin && git reset --hard origin/{current_branch}`."
        );

        println!("📬 Opening release PR");
        execute_command(
            "Open release PR",
            "gh",
            &[
                "pr",
                "create",
                "--base",
                &current_branch,
                "--head",
                &release_branch,
                "--title",
                &pr_title,
                "--body",
                &pr_body,
            ],
            ctx,
        )
        .await?;
    } else {
        println!(
            "ℹ️  Reusing existing release PR #{}",
            existing_pr.as_str().cyan()
        );
    }

    // Enable auto-merge (squash).  Squash is mandatory on this repo because:
    //
    //   1. `main-protection` ruleset requires `required_signatures: true`
    //      (every commit on main must be signed).
    //   2. GitHub's rebase-auto-merge cannot sign the rebased commit; it
    //      fails with `GraphQL: Base branch requires signed commits.
    //      Rebase merges cannot be automatically signed by GitHub`
    //      (observed on PR #36, the first real `just ship` for v0.5.69).
    //   3. GitHub signs the squash-merge commit with its own key, which
    //      satisfies `required_signatures: true` on main.
    //
    // Trust trade-off: the author's GPG signature is lost on the commit
    // that lands on main (it becomes a GitHub-signed squash).  The
    // original signed commit remains verifiable in the PR branch history,
    // and every prior merged PR on this repo uses the same pattern.
    println!("⚡ Ensuring auto-merge is enabled (squash strategy)");
    execute_command(
        "Enable auto-merge",
        "gh",
        &[
            "pr",
            "merge",
            &release_branch,
            "--auto",
            "--squash",
        ],
        ctx,
    )
    .await?;

    println!(
        "{} Release PR for v{} opened with auto-merge queued",
        "✅".green(),
        version
    );
    println!(
        "   💡 Watch checks: {}",
        format!("gh pr checks {} --watch", release_branch).cyan()
    );
    println!(
        "   💡 After merge:  {}",
        format!(
            "git fetch origin && git reset --hard origin/{} (squash rewrites commit SHA)",
            current_branch
        )
        .cyan()
    );

    Ok(())
}

fn get_current_version() -> Result<String> {
    let cargo_toml = fs::read_to_string("Cargo.toml").context("Failed to read Cargo.toml")?;
    for line in cargo_toml.lines() {
        if line.trim().starts_with("version = ") {
            if let Some(version) = line.split('"').nth(1) {
                return Ok(version.to_string());
            }
        }
    }
    bail!("Could not find version in Cargo.toml")
}

fn extract_version_from_cargo_toml(content: &str) -> Result<String> {
    let mut in_workspace_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if in_workspace_package {
            if trimmed.starts_with('[') && trimmed != "[workspace.package]" {
                break;
            }
            if trimmed.starts_with("version") {
                if let Some(equals_pos) = trimmed.find('=') {
                    let version_part = &trimmed[equals_pos + 1..].trim();
                    let version = version_part.trim_matches('"').trim_matches('\'');
                    return Ok(version.to_string());
                }
            }
        }
    }
    bail!("Version extraction failed - no version found in [workspace.package]")
}

async fn coverage_report_command(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📊 Coverage Report Generation".blue().bold());
    if coverage_data_exists() {
        println!("{} Found existing coverage data, generating report...", "🔍".green());
        execute_command("Coverage report", "cargo", &["llvm-cov", "report", "--html"], ctx).await?;
    } else {
        println!("{} No coverage data found, running tests first...", "⚠️".yellow());
        execute_command(
            "Coverage tests",
            "cargo",
            &["llvm-cov", "nextest", "--workspace", "--all-features", "--lib", "--bins", "--tests", "--profile", "ci", "--html"],
            ctx,
        ).await?;
    }
    println!("{} Coverage report: target/llvm-cov/html/index.html", "📁".green());
    Ok(())
}

async fn execute_step_with_tracking<F, Fut>(state: &mut WorkflowState, step_name: &str, step_fn: F) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    if state.is_step_completed(step_name) {
        println!("⏭️  Skipping completed step: {}", step_name);
        return Ok(());
    }
    state.mark_step_started(step_name)?;

    let step_start = Instant::now();
    let result = step_fn().await;
    let duration_secs = step_start.elapsed().as_secs();

    // Record step duration regardless of success/failure
    state.step_durations_secs.insert(step_name.to_string(), duration_secs);

    match result {
        Ok(()) => { state.mark_step_completed(step_name)?; Ok(()) }
        Err(e) => { state.mark_step_failed(step_name, &e.to_string())?; Err(e) }
    }
}

async fn increment_version() -> Result<()> {
    println!("📈 Incrementing version...");
    let output = Command::new("./build/update_all_versions.rs")
        .arg("patch")
        .output()
        .await
        .context("Failed to execute version update script")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Version bump failed: {}", stderr);
    }
    println!("✅ Version incremented successfully");
    Ok(())
}

/// Update Polars git dependencies to the latest commit on main.
///
/// **Skipped** when `uffs-polars/Cargo.toml` uses `rev = "..."` pinning
/// (which prevents upstream breakage). In that case the pinned commit is
/// used as-is and `cargo update` is called with `--precise <pinned-rev>`.
async fn update_polars_git(_ctx: &PipelineContext) -> Result<()> {
    // Check if uffs-polars/Cargo.toml uses rev pinning
    let cargo_toml = std::fs::read_to_string("crates/uffs-polars/Cargo.toml")
        .context("Failed to read crates/uffs-polars/Cargo.toml")?;
    if let Some(rev_line) = cargo_toml.lines().find(|l| l.contains("polars") && l.contains("rev =")) {
        // Extract the rev hash
        if let Some(start) = rev_line.find("rev = \"") {
            let hash_start = start + 7;
            if let Some(end) = rev_line[hash_start..].find('"') {
                let pinned_rev = &rev_line[hash_start..hash_start + end];
                println!(
                    "{}",
                    format!("📌 Polars pinned to rev={} — skipping auto-update", &pinned_rev[..12]).blue()
                );
                // Still run cargo update to ensure lockfile matches the pinned rev
                let status = Command::new("cargo")
                    .args(["update", "-p", "polars", "--precise", pinned_rev])
                    .status()
                    .await
                    .context("Failed to run cargo update for pinned polars")?;
                if !status.success() {
                    println!("⚠️  cargo update --precise failed (lockfile may already be correct)");
                }
                return Ok(());
            }
        }
    }

    println!(
        "{}",
        "📦 Updating Polars (git, branch=main) to latest commit...".blue()
    );

    // 1) Discover latest commit on main
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("https://github.com/pola-rs/polars")
        .arg("refs/heads/main")
        .output()
        .await
        .context("Failed to run 'git ls-remote' for Polars")?;
    if !output.status.success() {
        bail!("git ls-remote failed for Polars main");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Unable to parse Polars main HEAD sha"))?;

    // 2) Pin workspace lockfile to that exact commit for the 'polars' package
    let status = Command::new("cargo")
        .arg("update")
        .arg("-w")
        .arg("-p")
        .arg("polars")
        .arg("--precise")
        .arg(sha)
        .status()
        .await
        .context("Failed to execute 'cargo update -w -p polars --precise <sha>'")?;

    if !status.success() {
        bail!("Polars update failed - 'cargo update -w -p polars --precise <sha>' exited with non-zero status");
    }

    println!("{} {}", "✅ Polars pinned to commit".green(), sha);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Enhanced Phase Functions with Step Tracking
// ═══════════════════════════════════════════════════════════════════════════

async fn run_enhanced_phase1(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🧪 PHASE 1: Optimized Testing & Validation Pipeline".blue().bold());
    println!("ℹ️  Running all validation with CURRENT version - version increment happens AFTER validation passes");
    println!("🚀 Using safe parallel optimization: format → compile → parallel(doc tests + linting)");

    // Step 0: Toolchain — on `--fresh` runs, bump to today's nightly via
    // `toolchain-sync` (unless `--skip-toolchain-sync` is set, used when
    // the latest nightly is known-broken).  On resumable runs we only
    // ensure the currently pinned one is installed, so repeat ship
    // invocations don't churn `rust-toolchain.toml`.
    let (step_label, step_recipe) = if ctx.fresh && !ctx.skip_toolchain_sync {
        ("Toolchain sync (fresh bump to latest nightly)", "toolchain-sync")
    } else {
        ("Toolchain ensure (pinned nightly)", "toolchain-ensure")
    };
    execute_step_with_tracking(state, STEP_TOOLCHAIN_SYNC, || async {
        execute_command(step_label, "just", &[step_recipe], ctx).await
    }).await?;

    // Step 1: Update Polars git lock to latest commit on main
    execute_step_with_tracking(state, STEP_UPDATE_POLARS, || async {
        update_polars_git(ctx).await
    }).await?;

    println!("{}", "📋 Stage 1: Sequential Prerequisites".yellow().bold());

    // Step 1: Clean build artifacts (with auto-clean logic)
    execute_step_with_tracking(state, STEP_CLEAN_ARTIFACTS, || async {
        let target_dir = get_cargo_target_dir();

        // Check disk space and target directory size
        let free_gb = disk_free_bytes(&target_dir).await.map(bytes_to_gib);
        let target_gb = dir_size_bytes(&target_dir, Duration::from_secs(30)).await.map(bytes_to_gib);

        if ctx.verbose {
            if let Some(free) = free_gb {
                println!("  💾 Free disk space: {} GiB", free);
            }
            if let Some(size) = target_gb {
                println!("  📁 Target directory size: {} GiB", size);
            }
        }

        // Determine if we should clean
        let should_clean = ctx.force_clean
            || (!ctx.force_no_clean
                && (free_gb.map(|g| g < ctx.min_free_gb).unwrap_or(false)
                    || target_gb.map(|g| g > ctx.max_target_gb).unwrap_or(false)));

        if should_clean {
            if ctx.force_clean {
                println!("  🧹 Forced clean (--clean flag)");
            } else {
                println!("  🧹 Auto-clean triggered (disk space low or target too large)");
            }
            execute_command("Clean build artifacts", "cargo", &["clean"], ctx).await
        } else {
            println!("  ⏭️  Skipping clean (disk space OK, target size OK)");
            Ok(())
        }
    }).await?;

    // Step 2: Format code
    execute_step_with_tracking(state, STEP_FORMAT_CODE, || async {
        execute_command("Format code", "cargo", &["fmt", "--all"], ctx).await
    }).await?;

    // Step 2b: File size policy (fast — catches structural violations before expensive tests)
    execute_command(
        "File size policy",
        "bash",
        &["scripts/ci/check_file_size_policy.sh"],
        ctx,
    ).await?;

    // Step 3: Coverage tests
    // NOTE: Using --lib --bins --tests instead of --all-targets to exclude benchmarks.
    // Benchmarks create large DataFrames during initialization which causes SIGKILL
    // when nextest tries to enumerate tests.
    execute_step_with_tracking(state, STEP_COVERAGE_TESTS, || async {
        execute_command(
            "Coverage tests",
            "cargo",
            &["llvm-cov", "nextest", "--workspace", "--all-features", "--lib", "--bins", "--tests", "--profile", "ci", "--no-report"],
            ctx,
        ).await
    }).await?;

    // Step 4: Parallel validation (doc tests + linting + security)
    execute_step_with_tracking(state, STEP_PARALLEL_VALIDATION, || async {
        // Doc tests need RUSTDOCFLAGS; clippy/deny don't but it's harmless
        let parallel_commands = vec![
            ("Documentation tests", "cargo", vec!["test", "--doc", "--workspace", "--all-features"]),
            // pedantic/nursery/cargo/multiple_crate_versions levels are set in
            // workspace Cargo.toml — only per-target overrides needed here.
            ("Production linting", "cargo", vec![
                "clippy", "--workspace",
                "--lib", "--bins", "--all-features", "--no-deps", "--",
                "-D", "warnings",
                "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
            ]),
            ("Test linting", "cargo", vec![
                "clippy", "--workspace",
                "--all-targets", "--all-features", "--tests", "--no-deps", "--",
                "-D", "warnings",
                "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
                "-A", "clippy::unwrap_used", "-A", "clippy::expect_used", "-A", "unused-crate-dependencies",
            ]),
            ("Dependency security", "cargo", vec!["deny", "check"]),
            ("Rustdoc link validation", "cargo", vec![
                "doc", "--workspace", "--all-features", "--no-deps",
            ]),
        ];
        execute_parallel_with_env(parallel_commands, &[("RUSTDOCFLAGS", "-Dwarnings")], ctx).await
    }).await?;

    // Step 5: Final format verification (check only — formatting was done in Step 2)
    execute_step_with_tracking(state, STEP_FORMAT_CHECK, || async {
        execute_command("Format check", "cargo", &["fmt", "--all", "--", "--check"], ctx).await
    }).await?;

    println!("{}", "✅ PHASE 1 COMPLETE - All testing and validation passed!".green().bold());
    Ok(())
}

async fn run_enhanced_phase2(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📦 PHASE 2: Version Increment + Release PR".blue().bold());

    // Step 07: Version increment
    execute_step_with_tracking(state, STEP_VERSION_INCREMENT, || async {
        increment_version().await
    }).await?;

    if !state.version_incremented {
        state.version_incremented = true;
        let new_version = get_current_version().context("Failed to get updated version")?;
        state.current_version = new_version;
        state.save()?;
    }

    // Step 10: Git commit (signed version-bump commit on the working branch).
    execute_step_with_tracking(state, STEP_GIT_COMMIT, || async { git_commit(ctx).await }).await?;

    // Step 11: Git push -- opens release/vX.Y.Z PR with auto-merge queued.
    //
    // Binaries are NOT built here.  Once the PR merges to main,
    // `auto-tag-release.yml` tags the commit and invokes `release.yml`,
    // which produces the reproducible cross-platform binaries on
    // GitHub-hosted runners.
    execute_step_with_tracking(state, STEP_GIT_PUSH, || async { git_push(ctx).await }).await?;

    println!("{}", "✅ PHASE 2 COMPLETE - Release PR opened; GitHub Actions will produce binaries.".green().bold());
    Ok(())
}

/// Combined ship pipeline: Phase 1 (validation) + Phase 2 (deploy)
/// Supports resumable execution - re-runs skip already-completed steps.
/// Use --fresh flag to reset state and start from scratch.
async fn run_ship_pipeline(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🚢 UFFS Ship Pipeline (Phase 1 + Phase 2, Resumable)".blue().bold());
    println!("═══════════════════════════════════════════════════════════════════");

    // Load or create workflow state
    let mut state = if ctx.fresh {
        println!("{} Fresh run requested - resetting workflow state", "🔄".yellow());
        let current_version = get_current_version().unwrap_or_else(|_| "unknown".to_string());
        let new_state = WorkflowState::new_workflow(current_version);
        new_state.save().context("Failed to save fresh workflow state")?;
        new_state
    } else {
        WorkflowState::load().unwrap_or_else(|_| {
            let current_version = get_current_version().unwrap_or_else(|_| "unknown".to_string());
            WorkflowState::new_workflow(current_version)
        })
    };

    // Show current progress
    let completed_count = state.step_tracker.completed_steps.len();
    let total_steps = ALL_STEPS.len();
    let pending_steps = state.get_pending_steps(ALL_STEPS);

    if completed_count > 0 && !ctx.fresh {
        println!("{} Resuming from previous run: {}/{} steps completed", "📋".cyan(), completed_count, total_steps);
        if !state.step_tracker.completed_steps.is_empty() {
            println!("   Already completed:");
            for step in &state.step_tracker.completed_steps {
                println!("     {} {}", "✓".green(), step);
            }
        }
        if !pending_steps.is_empty() {
            println!("   Remaining:");
            for step in &pending_steps {
                println!("     {} {}", "○".dimmed(), step);
            }
        }
        println!();
    }

    // Run Phase 1: Validation
    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".dimmed());
    run_enhanced_phase1(&mut state, ctx).await.context("Phase 1 (validation) failed")?;

    // Run Phase 2: Version bump + release PR open (GitHub Actions takes
    // over from there via `auto-tag-release.yml` -> `release.yml`).
    println!("{}", "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".dimmed());
    run_enhanced_phase2(&mut state, ctx).await.context("Phase 2 (release PR) failed")?;

    // Mark workflow as completed
    state.advance_phase(WorkflowPhase::Completed)?;

    // Print summary
    let total_time = ctx.start_time.elapsed();
    println!();
    println!("{}", "═══════════════════════════════════════════════════════════════════".green());
    println!("{} Ship Pipeline Complete!", "🎉".green().bold());
    println!("═══════════════════════════════════════════════════════════════════");
    println!("   Version:    {}", state.current_version.cyan());
    println!("   Total time: {}s", total_time.as_secs());
    println!("   Steps:      {}/{} completed", state.step_tracker.completed_steps.len(), total_steps);

    // Show step timing breakdown
    if !state.step_durations_secs.is_empty() {
        println!();
        println!("   Step Timings:");
        for (step, secs) in &state.step_durations_secs {
            let mins = secs / 60;
            let remaining_secs = secs % 60;
            if mins > 0 {
                println!("     {} {}m {}s", step, mins, remaining_secs);
            } else {
                println!("     {} {}s", step, secs);
            }
        }
    }

    // Show sccache stats if enabled
    if ctx.sccache_enabled {
        if let Ok(out) = Command::new("sccache").arg("-s").output().await {
            if ctx.verbose {
                println!();
                println!("{} sccache stats:\n{}", "⚡".green(), String::from_utf8_lossy(&out.stdout));
            }
        }
    }

    println!();
    println!("{} Release PR opened; auto-merge queued", "📤".green());
    println!("{} GitHub Actions will build + publish v{} on merge",
        "👷".green(), state.current_version);
    println!("   💡 Watch: {}",
        "gh run list --repo <owner>/<repo> --workflow=release.yml --limit 5".cyan());

    Ok(())
}

fn print_workflow_status(state: &WorkflowState) {
    println!("📊 UFFS Workflow Status");
    println!("═══════════════════════════════════════");
    println!("🔖 Current Version: {}", state.current_version);
    println!("🆔 Workflow ID: {}", state.workflow_id);
    println!("📍 Current Phase: {:?}", state.phase);
    println!("⏰ Started: {}", state.started_at.format("%Y-%m-%d %H:%M:%S UTC"));

    if let Some(completed) = state.completed_at {
        println!("✅ Completed: {}", completed.format("%Y-%m-%d %H:%M:%S UTC"));
        let duration = completed.signed_duration_since(state.started_at);
        println!("⏱️  Duration: {}s", duration.num_seconds());
    }

    println!("🏆 Last Successful: {}", state.last_successful_version);
    println!("📈 Version Incremented: {}", if state.version_incremented { "✅" } else { "❌" });

    let completed_count = state.step_tracker.completed_steps.len();
    let total_steps = ALL_STEPS.len();
    println!("📊 Step Progress: {}/{} completed", completed_count, total_steps);

    if !state.step_tracker.completed_steps.is_empty() {
        println!("✅ Completed Steps:");
        for step in &state.step_tracker.completed_steps {
            println!("   • {}", step);
        }
    }

    if !state.step_tracker.failed_steps.is_empty() {
        println!("❌ Failed Steps:");
        for step in &state.step_tracker.failed_steps {
            println!("   • {}", step);
        }
    }

    if let Some(current_step) = &state.step_tracker.current_step {
        println!("🔄 Current Step: {}", current_step);
    }

    let pending_steps = state.get_pending_steps(ALL_STEPS);
    if !pending_steps.is_empty() {
        println!("📋 Pending Steps:");
        for step in &pending_steps {
            println!("   • {}", step);
        }
    }

    if state.failure_count > 0 {
        println!("❌ Failure Count: {}", state.failure_count);
        if let Some(error) = &state.last_error {
            println!("🔍 Last Error: {}", error);
        }
    }

    match state.phase {
        WorkflowPhase::Clean | WorkflowPhase::Completed => {
            println!("\n💡 Validate only:    rust-script scripts/ci/ci-pipeline.rs go");
            println!("💡 Full ship lane:   rust-script scripts/ci/ci-pipeline.rs ship");
            println!("💡 Fresh ship run:   rust-script scripts/ci/ci-pipeline.rs ship --fresh");
        }
        _ => {
            println!("\n💡 Resume ship workflow: rust-script scripts/ci/ci-pipeline.rs ship");
            println!("💡 Start fresh:          rust-script scripts/ci/ci-pipeline.rs ship --fresh");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main Entry Point
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let validation_command = matches!(cli.command, Commands::Go | Commands::CheckAll | Commands::Phase1);
    let ctx = PipelineContext::new(
        cli.verbose,
        cli.coverage_report,
        cli.clean,
        cli.no_clean,
        cli.min_free_gb,
        cli.max_target_gb,
        cli.jobs,
        cli.parallel_jobs,
        cli.no_sccache || validation_command,
        cli.fresh,
        cli.skip_toolchain_sync,
    );

    if ctx.verbose {
        println!("{} Verbose mode enabled", "🔍".blue());
        println!("{} Coverage report: {}", "📊".blue(), if ctx.coverage_report { "enabled" } else { "disabled" });
    }

    // Show sccache status
    if ctx.sccache_enabled {
        println!("{} sccache: enabled (RUSTC_WRAPPER=sccache)", "⚡".green());
    } else if ctx.verbose {
        println!("{} sccache: disabled (install sccache for big CI wins)", "⚡".yellow());
    }

    // Show log file location if not verbose
    if let Some(log_path) = &ctx.log_file {
        println!("{} Log file: {}", "📝".blue(), log_path.display());
    }

    // Start sccache server early (no-op if already running). This is safe and fast.
    if ctx.sccache_enabled {
        let _ = Command::new("sccache").arg("--start-server").output().await;
    }

    match cli.command {
        Commands::Go => {
            println!("{}", "🚀 Safe-by-Default Validation Workflow (OPTIMIZED)".blue().bold());
            phase1_optimized(&ctx).await.context("Validation workflow failed")?;

            let total_time = ctx.start_time.elapsed();
            println!("{} Total pipeline time: {}s", "🎉".green(), total_time.as_secs());

            // Show sccache stats if enabled
            if ctx.sccache_enabled {
                if let Ok(out) = Command::new("sccache").arg("-s").output().await {
                    if ctx.verbose {
                        println!("{} sccache stats:\n{}", "⚡".green(), String::from_utf8_lossy(&out.stdout));
                    }
                }
            }

            if !ctx.coverage_report {
                println!("{} Tip: Use --coverage-report to generate HTML coverage report", "💡".blue());
            }
            println!("{} Run 'rust-script scripts/ci/ci-pipeline.rs phase2' or 'just phase2-ship' when ready to ship", "💡".blue());
        }
        Commands::Ship => {
            run_ship_pipeline(&ctx).await.context("Ship pipeline failed")?;
        }
        Commands::CheckAll => {
            println!("{}", "📋 Comprehensive Validation (PARALLEL)".blue().bold());
            phase1_optimized(&ctx).await.context("Comprehensive validation failed")?;
            println!("{} Comprehensive validation complete!", "✅".green());
        }
        Commands::Phase1 => {
            phase1_optimized(&ctx).await.context("PHASE 1 standalone execution failed")?;
        }
        Commands::Phase2 => {
            phase2_optimized(&ctx).await.context("PHASE 2 standalone execution failed")?;
        }
        Commands::CoverageReport => {
            coverage_report_command(&ctx).await.context("Coverage report generation failed")?;
        }
        Commands::AuditComprehensive => {
            println!("{}", "🔒 Multi-Tool Security Audit (PARALLEL)".blue().bold());
            let audit_commands = vec![
                ("Cargo audit", "cargo", vec!["audit"]),
                ("Cargo deny", "cargo", vec!["deny", "check"]),
                ("Security advisory", "cargo", vec!["audit", "--deny", "warnings"]),
            ];
            execute_parallel(audit_commands, &ctx).await.context("Security audit failed")?;
        }
        Commands::WorkflowStatus => {
            let state = WorkflowState::load().context("Failed to load workflow state")?;
            print_workflow_status(&state);
        }
        Commands::WorkflowReset => {
            let state = WorkflowState::default();
            state.save().context("Failed to save reset workflow state")?;
            println!("🧹 Workflow state reset to clean slate");
        }
        Commands::WorkflowResume => {
            let state = WorkflowState::load().context("Failed to load workflow state")?;
            if state.is_resumable() {
                println!("🔄 Resuming workflow from phase: {:?}", state.phase);
                println!("💡 Run 'rust-script scripts/ci/ci-pipeline.rs phase2' to continue the explicit ship lane");
            } else {
                println!("❌ No resumable workflow found");
                print_workflow_status(&state);
            }
        }
        Commands::CrossCheck => {
            println!("🔍 Cross-compilation syntax validation...");
            println!("⚠️  Note: This checks syntax only (no linking) to catch API compatibility issues");

            // Check if cross-compilation toolchain is available
            let has_cross_toolchain = std::process::Command::new("which")
                .arg("x86_64-linux-gnu-gcc")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);

            if has_cross_toolchain {
                execute_command(
                    "Cross-compile syntax check (Linux x86_64)",
                    "cargo",
                    &["check", "--workspace", "--all-features", "--target", "x86_64-unknown-linux-gnu", "--lib"],
                    &ctx,
                ).await.context("Cross-compilation syntax check failed")?;
                println!("✅ Cross-compilation syntax check passed");
            } else {
                println!("⚠️  Cross-compilation toolchain not available (x86_64-linux-gnu-gcc)");
                println!("   This check will run in CI with proper toolchain setup");
                println!("✅ Cross-compilation setup completed (skipped locally)");
            }

            // Windows cross-check — catches #[cfg(windows)] code errors from macOS.
            // Uses cargo-xwin which bundles MSVC headers/libs for C build scripts.
            let has_cargo_xwin = std::process::Command::new("cargo")
                .args(["xwin", "--version"])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);

            if has_cargo_xwin {
                execute_command(
                    "Cross-compile syntax check (Windows x86_64)",
                    "cargo",
                    &["xwin", "check", "--workspace", "--target", "x86_64-pc-windows-msvc"],
                    &ctx,
                ).await.context("Windows cross-compilation syntax check failed")?;
                println!("✅ Windows cross-compilation syntax check passed");
            } else {
                println!("⚠️  cargo-xwin not available — skipping Windows cross-check");
                println!("   Install with: cargo install cargo-xwin");
            }
        }
    }

    Ok(())
}
