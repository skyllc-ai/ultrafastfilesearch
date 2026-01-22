#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! futures = "0.3"
//! tokio = { version = "1.0", features = ["full"] }
//! indicatif = "0.17"
//! serde = { version = "1.0", features = ["derive"] }
//! serde_json = "1.0"
//! chrono = { version = "0.4", features = ["serde"] }
//! uuid = { version = "1.0", features = ["v4"] }
//! num_cpus = "1.0"
//! ```
// =============================================================================
// scripts/ci-pipeline.rs - UFFS High-Performance CI Pipeline
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 Robert Nio
//
// UFFS - UltraFastFileSearch: High-Performance File Search Tool
// Contact: 50460704+githubrobbi@users.noreply.github.com for licensing inquiries
//
//! High-Performance CI Pipeline with Tokio Async Orchestration
//!
//! This script implements advanced CI pipeline optimizations using:
//! - Tokio async/await for true parallelism
//! - Resource-aware process management
//! - Dependency graph execution
//! - Smart error handling and recovery

use std::collections::BTreeSet;
use std::fs;
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

const STEP_UPDATE_POLARS: &str = "00-update-polars-git"; // Bump polars git lock to latest main
const STEP_CLEAN_ARTIFACTS: &str = "01-clean-artifacts";
const STEP_FORMAT_CODE: &str = "02-format-code";
const STEP_COVERAGE_TESTS: &str = "03-coverage-tests";
const STEP_PARALLEL_VALIDATION: &str = "04-parallel-validation";
const STEP_FORMAT_CHECK: &str = "05-format-check";
const STEP_VERSION_INCREMENT: &str = "06-version-increment";
const STEP_BUILD_RELEASE: &str = "07-build-release";
const STEP_DEPLOY_BINARY: &str = "08-deploy-binary"; // Copy to dist/ and ~/bin
const STEP_GIT_COMMIT: &str = "09-git-commit";
const STEP_GIT_PUSH: &str = "10-git-push";

const ALL_STEPS: &[&str] = &[
    STEP_UPDATE_POLARS,
    STEP_CLEAN_ARTIFACTS,
    STEP_FORMAT_CODE,
    STEP_COVERAGE_TESTS,
    STEP_PARALLEL_VALIDATION,
    STEP_FORMAT_CHECK,
    STEP_VERSION_INCREMENT,
    STEP_BUILD_RELEASE,
    STEP_DEPLOY_BINARY,
    STEP_GIT_COMMIT,
    STEP_GIT_PUSH,
];

/// Get the cargo target directory, checking env var and config file
fn get_cargo_target_dir() -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir);
    }
    if let Some(target_dir) = parse_cargo_config_target_dir() {
        return target_dir;
    }
    PathBuf::from("./target")
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
                    if path_str.starts_with("~/") || path_str == "~" {
                        if let Ok(home) = std::env::var("HOME") {
                            let rest = path_str.strip_prefix("~/").unwrap_or("");
                            return Some(PathBuf::from(home).join(rest));
                        }
                    }
                    return Some(PathBuf::from(path_str));
                }
            }
        }
    }
    None
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
}

#[derive(Subcommand)]
enum Commands {
    /// Complete two-phase fast-fail workflow (optimized)
    Go,
    /// Comprehensive validation with parallel execution
    CheckAll,
    /// Phase 1: Testing with maximum parallelism
    Phase1,
    /// Phase 2: Build and deploy
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
    /// Cross-compilation validation
    CrossCheck,
}

/// Pipeline execution context with resource management
struct PipelineContext {
    start_time: Instant,
    max_parallel_jobs: usize,
    timeout_duration: Duration,
    verbose: bool,
    coverage_report: bool,
}

impl PipelineContext {
    fn new(verbose: bool, coverage_report: bool) -> Self {
        Self {
            start_time: Instant::now(),
            max_parallel_jobs: num_cpus::get().min(16),
            timeout_duration: Duration::from_secs(1800), // 30 minutes max
            verbose,
            coverage_report,
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
    for (key, value) in env_vars {
        command.env(key, value);
    }

    if ctx.verbose {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    let mut child = command.spawn().with_context(|| format!("Failed to spawn command '{}' for step '{}'", cmd, name))?;
    let progress_bar = if !ctx.verbose { Some(create_fillup_spinner(name)) } else { None };

    let result = timeout(ctx.timeout_duration, child.wait())
        .await
        .with_context(|| format!("Command '{}' timed out after {}s", cmd, ctx.timeout_duration.as_secs()))?
        .with_context(|| format!("Failed to wait for command '{}' in step '{}'", cmd, name))?;

    if let Some(pb) = progress_bar { pb.finish_and_clear(); }
    let duration = step_start.elapsed();

    if result.success() {
        println!("{} {} ({}s)", "✅".green(), name, duration.as_secs());
        Ok(())
    } else {
        let exit_code = result.code().map_or("unknown".to_string(), |c| c.to_string());
        println!("{} {} failed (exit code: {})", "❌".red(), name, exit_code);
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

/// Phase 1: Testing with maximum parallelism
async fn phase1_optimized(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🧪 PHASE 1: Optimized Testing Pipeline".blue().bold());

    // Step 1: Clean
    execute_command("Clean build artifacts", "cargo", &["clean"], ctx).await?;

    // Step 2: Format code
    execute_command("Format code", "cargo", &["fmt", "--all"], ctx).await?;

    // Step 3: Coverage tests
    // NOTE: Using --lib --bins --tests instead of --all-targets to exclude benchmarks.
    execute_command_with_env(
        "Coverage tests",
        "cargo",
        &["llvm-cov", "nextest", "--workspace", "--all-features", "--lib", "--bins", "--tests", "--jobs", "8", "--no-report"],
        &[],
        ctx,
    ).await?;

    // Step 4: Generate coverage report (optional)
    if ctx.coverage_report {
        execute_command("Coverage report", "cargo", &["llvm-cov", "report", "--html"], ctx).await?;
    } else if ctx.verbose {
        println!("{} Coverage report skipped (use --coverage-report to generate)", "⏭️".yellow());
    }

    // Step 5: Doc tests
    execute_command_with_env(
        "Documentation tests",
        "cargo",
        &["test", "--doc", "--workspace", "--all-features"],
        &[("RUSTDOCFLAGS", "-Dwarnings")],
        ctx,
    ).await?;

    // Step 6: Parallel linting
    // NOTE: --exclude uffs-legacy because it's frozen legacy code kept for reference only.
    // The modern crates (uffs-core, uffs-mft, uffs-cli, etc.) follow strict linting.
    let linting_commands = vec![
        ("Production linting", "cargo", vec![
            "clippy", "--workspace", "--exclude", "uffs-legacy",
            "--all-targets", "--all-features", "--no-deps", "--",
            "-D", "clippy::pedantic", "-D", "clippy::nursery", "-D", "clippy::cargo",
            "-A", "clippy::multiple_crate_versions", "-W", "clippy::panic",
            "-W", "clippy::todo", "-W", "clippy::unimplemented", "-D", "warnings",
            "-W", "clippy::unwrap_used", "-W", "clippy::expect_used",
        ]),
        ("Test linting", "cargo", vec![
            "clippy", "--workspace", "--exclude", "uffs-legacy",
            "--all-targets", "--all-features", "--tests", "--no-deps", "--",
            "-D", "clippy::pedantic", "-D", "clippy::nursery", "-D", "clippy::cargo",
            "-A", "clippy::multiple_crate_versions", "-W", "clippy::panic",
            "-W", "clippy::todo", "-W", "clippy::unimplemented", "-D", "warnings",
            "-A", "clippy::unwrap_used", "-A", "clippy::expect_used",
        ]),
        ("Dependency security", "cargo", vec!["deny", "check"]),
    ];
    execute_parallel(linting_commands, ctx).await?;

    // Step 7: Final format normalize
    execute_command("Format normalization", "cargo", &["fmt", "--all"], ctx).await?;

    println!("{}", "✅ PHASE 1 COMPLETE: All tests passed!".green().bold());
    Ok(())
}

/// Phase 2: Build and deploy
async fn phase2_optimized(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "🚀 PHASE 2: Build and Deploy".blue().bold());

    // Step 1: Version increment
    version_bump(ctx).await?;

    // Update workflow state with new version
    let mut state = WorkflowState::load().context("Failed to load workflow state")?;
    let new_version = get_current_version().context("Failed to get updated version")?;
    state.current_version = new_version;
    state.version_incremented = true;
    state.save().context("Failed to save workflow state")?;
    println!("✅ Workflow state updated with new version: {}", state.current_version);

    // Step 2: Build release binary
    build_release(ctx).await?;

    // Step 3: Git commit
    git_commit(ctx).await?;

    // Step 4: Git push
    git_push(ctx).await?;

    println!("{}", "✅ PHASE 2 COMPLETE: Build and deploy successful!".green().bold());
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

/// Check if release build mode is enabled via UFFS_RELEASE_BUILD env var.
/// Default is DEV mode for faster iteration during development.
fn is_release_build() -> bool {
    std::env::var("UFFS_RELEASE_BUILD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

async fn build_release(ctx: &PipelineContext) -> Result<()> {
    let release_build = is_release_build();
    let build_type = if release_build { "release" } else { "dev" };
    println!("{}", format!("🔨 Building {} binary...", build_type).blue());

    let args: Vec<&str> = if release_build {
        vec!["build", "--release", "--workspace"]
    } else {
        vec!["build", "--workspace"]
    };

    execute_command(
        &format!("Build {}", build_type),
        "cargo",
        &args,
        ctx,
    ).await?;
    println!("{} {} binary built successfully", "✅".green(), build_type);
    Ok(())
}

/// Deploy binary to dist/ directory and ~/bin
/// On macOS ARM64, runs cross-compilation for all platforms
/// On other platforms, runs local build only
async fn deploy_binary(ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📦 Deploying binary to dist/ and ~/bin...".blue());

    // Detect if we're on macOS ARM64 for cross-compilation
    let is_macos_arm64 = std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64";

    if is_macos_arm64 {
        // Cross-compile for Windows from macOS
        // Note: The xwin-dev profile handles COFF archive size limits for polars crates
        println!("{} Running cross-platform build...", "🌍".blue());
        execute_command(
            "Cross-platform build",
            "rust-script",
            &["scripts/build-cross-all.rs"],
            ctx,
        ).await?;
    } else {
        println!("{} Running local build...", "🖥️".blue());
        execute_command(
            "Local build & install",
            "rust-script",
            &["scripts/build-local.rs"],
            ctx,
        ).await?;
    }

    println!("{} Binary deployed successfully", "✅".green());
    Ok(())
}

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
    println!("{}", "🚀 Pushing to remote...".blue());
    execute_command("Git pull rebase", "git", &["pull", "origin", "main", "--rebase"], ctx).await?;
    execute_command("Git push", "git", &["push", "origin", "main"], ctx).await?;
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
        execute_command_with_env(
            "Coverage tests",
            "cargo",
            &["llvm-cov", "nextest", "--workspace", "--all-features", "--lib", "--bins", "--tests", "--jobs", "8", "--html"],
            &[],
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
    match step_fn().await {
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

/// Update Polars git dependencies to the latest commit on main
/// This keeps the uffs-polars facade fresh with the latest Polars features
async fn update_polars_git(_ctx: &PipelineContext) -> Result<()> {
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

    // Step 0: Update Polars git lock to latest commit on main
    execute_step_with_tracking(state, STEP_UPDATE_POLARS, || async {
        update_polars_git(ctx).await
    }).await?;

    println!("{}", "📋 Stage 1: Sequential Prerequisites".yellow().bold());

    // Step 1: Clean build artifacts
    execute_step_with_tracking(state, STEP_CLEAN_ARTIFACTS, || async {
        execute_command("Clean build artifacts", "cargo", &["clean"], ctx).await
    }).await?;

    // Step 2: Format code
    execute_step_with_tracking(state, STEP_FORMAT_CODE, || async {
        execute_command("Format code", "cargo", &["fmt", "--all"], ctx).await
    }).await?;

    // Step 3: Coverage tests
    // NOTE: Using --lib --bins --tests instead of --all-targets to exclude benchmarks.
    // Benchmarks create large DataFrames during initialization which causes SIGKILL
    // when nextest tries to enumerate tests.
    execute_step_with_tracking(state, STEP_COVERAGE_TESTS, || async {
        execute_command_with_env(
            "Coverage tests",
            "cargo",
            &["llvm-cov", "nextest", "--workspace", "--all-features", "--lib", "--bins", "--tests", "--jobs", "8", "--no-report"],
            &[],
            ctx,
        ).await
    }).await?;

    // Step 4: Parallel validation (doc tests + linting)
    // NOTE: --exclude uffs-legacy because it's frozen legacy code kept for reference only.
    // The modern crates (uffs-core, uffs-mft, uffs-cli, etc.) follow strict linting.
    execute_step_with_tracking(state, STEP_PARALLEL_VALIDATION, || async {
        let parallel_commands = vec![
            ("Documentation tests", "cargo", vec!["test", "--doc", "--workspace", "--all-features"]),
            ("Production linting", "cargo", vec![
                "clippy", "--workspace", "--exclude", "uffs-legacy",
                "--lib", "--bins", "--all-features", "--no-deps", "--",
                "-D", "warnings", "-D", "clippy::pedantic", "-D", "clippy::nursery", "-D", "clippy::cargo",
                "-A", "clippy::multiple_crate_versions", "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
            ]),
            ("Test linting", "cargo", vec![
                "clippy", "--workspace", "--exclude", "uffs-legacy",
                "--all-targets", "--all-features", "--tests", "--no-deps", "--",
                "-D", "clippy::pedantic", "-D", "clippy::nursery", "-D", "clippy::cargo",
                "-A", "clippy::multiple_crate_versions", "-W", "clippy::panic", "-W", "clippy::todo", "-W", "clippy::unimplemented",
                "-D", "warnings", "-A", "clippy::unwrap_used", "-A", "clippy::expect_used", "-A", "unused-crate-dependencies",
            ]),
            ("Dependency security", "cargo", vec!["deny", "check"]),
        ];
        let env_vars = vec![("RUSTDOCFLAGS", "-Dwarnings")];
        execute_parallel_with_env(parallel_commands, &env_vars, ctx).await
    }).await?;

    // Step 5: Final format check
    // NOTE: --exclude uffs-legacy for the same reason as above.
    execute_step_with_tracking(state, STEP_FORMAT_CHECK, || async {
        execute_command("Format normalization", "cargo", &["fmt", "--all"], ctx).await?;
        execute_command("CI lint gating", "cargo", &[
            "clippy", "--workspace", "--exclude", "uffs-legacy", "--all-features", "--", "-D", "warnings"
        ], ctx).await
    }).await?;

    println!("{}", "✅ PHASE 1 COMPLETE - All testing and validation passed!".green().bold());
    Ok(())
}

async fn run_enhanced_phase2(state: &mut WorkflowState, ctx: &PipelineContext) -> Result<()> {
    println!("{}", "📦 PHASE 2: Version Increment, Build & Deploy".blue().bold());

    // Step 6: Version increment
    execute_step_with_tracking(state, STEP_VERSION_INCREMENT, || async {
        increment_version().await
    }).await?;

    if !state.version_incremented {
        state.version_incremented = true;
        let new_version = get_current_version().context("Failed to get updated version")?;
        state.current_version = new_version;
        state.save()?;
    }

    // Step 7: Build release
    execute_step_with_tracking(state, STEP_BUILD_RELEASE, || async {
        build_release(ctx).await
    }).await?;

    // Step 8: Deploy binary (copy to dist/ and ~/bin)
    execute_step_with_tracking(state, STEP_DEPLOY_BINARY, || async {
        deploy_binary(ctx).await
    }).await?;

    // Step 9: Git commit
    execute_step_with_tracking(state, STEP_GIT_COMMIT, || async { git_commit(ctx).await }).await?;

    // Step 10: Git push
    execute_step_with_tracking(state, STEP_GIT_PUSH, || async { git_push(ctx).await }).await?;

    println!("{}", "✅ PHASE 2 COMPLETE - Build and deploy successful!".green().bold());
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
            println!("\n💡 Ready to start new workflow: rust-script scripts/ci-pipeline.rs go");
        }
        _ => {
            println!("\n💡 Workflow in progress - resume with: rust-script scripts/ci-pipeline.rs go");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Main Entry Point
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = PipelineContext::new(cli.verbose, cli.coverage_report);

    if ctx.verbose {
        println!("{} Verbose mode enabled", "🔍".blue());
        println!("{} Coverage report: {}", "📊".blue(), if ctx.coverage_report { "enabled" } else { "disabled" });
    }

    // Show build mode (DEV is default, set UFFS_RELEASE_BUILD=1 for release)
    let build_mode = if is_release_build() { "RELEASE (optimized)" } else { "DEV (fast, default)" };
    println!("{} Build mode: {}", "🔧".blue(), build_mode);

    match cli.command {
        Commands::Go => {
            println!("{}", "🚀 Complete Two-Phase Fast-Fail Workflow (OPTIMIZED)".blue().bold());

            let mut state = WorkflowState::load().context("Failed to load workflow state")?;

            match state.phase {
                WorkflowPhase::Completed | WorkflowPhase::Clean => {
                    let current_version = get_current_version().context("Failed to get current version")?;
                    state = WorkflowState::new_workflow(current_version);
                    state.save().context("Failed to save initial workflow state")?;
                    println!("🆕 Starting new workflow with ID: {}", state.workflow_id);
                }
                _ => {
                    println!("🔄 Resuming workflow from phase: {:?}", state.phase);
                    if state.failure_count > 0 {
                        println!("⚠️  Previous failures detected: {}", state.failure_count);
                    }
                }
            }

            // PHASE 1: Testing and Validation
            if matches!(state.phase, WorkflowPhase::Clean) {
                state.advance_phase(WorkflowPhase::Testing)?;
            }

            if matches!(state.phase, WorkflowPhase::Testing) {
                run_enhanced_phase1(&mut state, &ctx).await.context("PHASE 1 failed")?;
                state.advance_phase(WorkflowPhase::Building)?;
            }

            // PHASE 2: Build and Deploy
            if matches!(state.phase, WorkflowPhase::Building | WorkflowPhase::Deploying | WorkflowPhase::GitCommitting | WorkflowPhase::GitPushing) {
                run_enhanced_phase2(&mut state, &ctx).await.context("PHASE 2 failed")?;
                state.advance_phase(WorkflowPhase::Completed)?;
            }

            let total_time = ctx.start_time.elapsed();
            println!("{} Total pipeline time: {}s", "🎉".green(), total_time.as_secs());

            if !ctx.coverage_report {
                println!("{} Tip: Use --coverage-report to generate HTML coverage report", "💡".blue());
            }
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
                println!("💡 Run 'rust-script scripts/ci-pipeline.rs go' to continue");
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
        }
    }

    Ok(())
}
