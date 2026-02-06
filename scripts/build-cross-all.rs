#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! serde_json = "1.0"
//! dirs-next = "2.0"
//! sha2 = "0.10"
//! ```
// =============================================================================
// scripts/build-cross-all.rs - UFFS Cross-Platform Build (Windows Only)
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 Robert Nio
//
// UFFS - UltraFastFileSearch: High-Performance File Search Tool
//! Cross-compile UFFS binaries for Windows
//!
//! **IMPORTANT**: UFFS is a Windows-only tool. It reads the NTFS Master File
//! Table (MFT) directly using Windows kernel APIs. macOS and Linux binaries
//! would only return "PlatformNotSupported" errors.
//!
//! This script cross-compiles Windows binaries from macOS/Linux hosts:
//! - x86_64-pc-windows-msvc via cargo-xwin
//!
//! Prerequisites:
//! - macOS or Linux host (for cross-compilation)
//! - rustup target: rustup target add x86_64-pc-windows-msvc
//! - cargo-xwin: cargo install cargo-xwin
//! - LLVM/clang: brew install llvm (macOS) or apt install clang (Linux)

use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::{env, fs};

use sha2::{Digest, Sha256};

/// Host triple for macOS ARM64 (the expected cross-compilation host)
const HOST_TRIPLE: &str = "aarch64-apple-darwin";

/// UFFS binaries: (binary_name, package_name)
/// - uffs: Main CLI tool
/// - uffs_mft: Low-level MFT reading tool
/// - uffs_tui: Terminal UI (placeholder)
/// - uffs_gui: Graphical UI (placeholder)
/// - uffs-diag binaries: Excluded from workspace - build manually when needed
const BINARIES: &[(&str, &str)] = &[
    ("uffs", "uffs-cli"),
    ("uffs_mft", "uffs-mft"),
    ("uffs_tui", "uffs-tui"),
    ("uffs_gui", "uffs-gui"),
    // uffs-diag excluded from workspace - uncomment when needed:
    // ("analyze_mft_parents", "uffs-diag"),
    // ("dump_mft_records", "uffs-diag"),
    // ("scan_mft_magic", "uffs-diag"),
    // ("dump_mft_extents", "uffs-diag"),
    // ("cross_check_mft_reference", "uffs-diag"),
    // ("compare_raw_mft", "uffs-diag"),
    // ("inspect_mft_record_flow", "uffs-diag"),
    // ("analyze_diff", "uffs-diag"),
];

struct Target {
    triple: &'static str,
    platform_name: &'static str,
    use_xwin: bool,
    requires_linker: Option<&'static str>,
}

/// Build mode enum for clearer logic
#[derive(Debug, Clone, Copy, PartialEq)]
enum BuildMode {
    Release,
    Dev,
    Profiling,
}

/// Determine build mode from environment variables.
/// Priority: UFFS_PROFILING_BUILD > UFFS_RELEASE_BUILD > default (release)
fn get_build_mode() -> BuildMode {
    // Check for profiling mode first (highest priority)
    if env::var("UFFS_PROFILING_BUILD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return BuildMode::Profiling;
    }

    // Check for release mode (default is true)
    if env::var("UFFS_RELEASE_BUILD")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
    {
        BuildMode::Release
    } else {
        BuildMode::Dev
    }
}

/// Check if release build mode is enabled via UFFS_RELEASE_BUILD env var.
/// Default is DEV mode for faster iteration during development.
#[allow(dead_code)]
fn is_release_build() -> bool {
    get_build_mode() == BuildMode::Release
}

/// Check if profiling build mode is enabled via UFFS_PROFILING_BUILD env var.
#[allow(dead_code)]
fn is_profiling_build() -> bool {
    get_build_mode() == BuildMode::Profiling
}

/// Get the build profile name for display purposes
fn build_profile() -> &'static str {
    match get_build_mode() {
        BuildMode::Release => "release",
        BuildMode::Dev => "xwin-dev",
        BuildMode::Profiling => "profiling",
    }
}

/// Get the cargo output directory name (where binaries are placed)
/// Note: Custom profiles like "xwin-dev" and "profiling" output to their own directory
fn build_output_dir() -> &'static str {
    match get_build_mode() {
        BuildMode::Release => "release",
        BuildMode::Dev => "xwin-dev",
        BuildMode::Profiling => "profiling",
    }
}

/// UFFS only runs on Windows (requires NTFS MFT access via Windows APIs).
/// We only build Windows binaries - macOS/Linux are just cross-compilation
/// hosts.
const TARGETS: &[Target] = &[Target {
    triple: "x86_64-pc-windows-msvc",
    platform_name: "windows-x64",
    use_xwin: true,
    requires_linker: None,
}];

fn main() {
    let args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let native_only = args.iter().any(|a| a == "--native-only");

    // Disk safety:
    // By default we prune (delete) cross-target build artifacts after copying
    // binaries into dist/. Each target triple gets its own (potentially huge)
    // subtree under the cargo target-dir. Keeping all of them quickly explodes
    // disk usage (e.g. 4 targets => ~4× polars build).
    //
    // Use --keep-target-artifacts to opt out.
    // Use --prune-host to also delete host release artifacts after copying.
    let prune_cross_targets = !args
        .iter()
        .any(|a| a == "--keep-target-artifacts" || a == "--no-prune");
    let prune_host = args.iter().any(|a| a == "--prune-host");

    println!("🚀 UFFS Cross-Platform Build (Windows Only)");
    println!("ℹ️  UFFS is Windows-only (requires NTFS MFT access)");
    if verbose {
        println!("🔍 Verbose mode enabled");
    }

    // Show build mode
    let build_mode = get_build_mode();
    let build_mode_str = match build_mode {
        BuildMode::Release => "RELEASE (optimized)",
        BuildMode::Dev => "DEV (fast, default)",
        BuildMode::Profiling => "PROFILING (optimized + debug symbols for samply/PerfView)",
    };
    println!("🔧 Build mode: {}", build_mode_str);
    if build_mode == BuildMode::Profiling {
        println!("📊 Profiling build: binaries will include PDB symbols for analysis");
    }

    let (host_os, host_arch) = (env::consts::OS, env::consts::ARCH);
    println!(
        "🖥️  Host: {} {} (cross-compilation host)",
        host_os, host_arch
    );

    // On Windows, just build natively
    if native_only || host_os == "windows" {
        if native_only {
            println!("ℹ️  --native-only flag set. Using native-only build.");
        } else {
            println!("🎯 Running on Windows - building natively...");
        }
        build_native_only();
        return;
    }

    // On macOS/Linux, cross-compile for Windows
    if host_os != "macos" && host_os != "linux" {
        eprintln!(
            "⚠️  Unsupported host OS: {}. Use Windows, macOS, or Linux.",
            host_os
        );
        exit(1);
    }

    let version = read_current_version();
    let target_dir = get_cargo_target_dir();

    // Preflight: remove any leftover cross-target build trees from previous runs.
    // This prevents starting a new release with disk already consumed.
    if prune_cross_targets {
        prune_previous_cross_target_artifacts(&target_dir);
    }

    println!(
        "📦 Version: {}\n📂 Target: {}",
        version,
        target_dir.display()
    );

    // Ensure required tools are present before we start building
    ensure_required_tools_or_exit();

    let available = check_available_targets();
    println!("\n📋 Available targets:");
    for t in &available {
        println!("   ✅ {} ({})", t.triple, t.platform_name);
    }

    // Build order: non-host targets first, host last.
    // This lowers peak disk usage because the host build can be relatively large, and
    // we don't want it resident while building every other target triple.
    let mut build_order: Vec<&Target> = available
        .iter()
        .filter(|t| t.triple != HOST_TRIPLE)
        .copied()
        .collect();
    if let Some(host) = available.iter().find(|t| t.triple == HOST_TRIPLE) {
        build_order.push(host);
    }

    for target in &build_order {
        println!(
            "\n{}\n🎯 Building {} ({})\n{}",
            "═".repeat(60),
            target.triple,
            target.platform_name,
            "═".repeat(60)
        );

        print_free_disk(&target_dir, "before build");

        if !build_for_target(target, &target_dir, verbose) {
            eprintln!("\n❌ Build failed for {} - aborting!", target.triple);
            exit(1);
        }

        // Only copy to dist/ for release builds (not profiling or dev)
        if build_mode == BuildMode::Release {
            if !copy_binaries_to_dist(&version, target, &target_dir) {
                eprintln!("\n❌ Binary copy failed for {} - aborting!", target.triple);
                eprintln!("   This usually means the build succeeded but binaries were not placed in the expected location.");
                eprintln!(
                    "   Check the target directory: {:?}",
                    target_dir.join(target.triple)
                );
                exit(1);
            }
        }

        // Critical disk optimization: delete the *target-specific* build tree after
        // we've copied the final binaries into dist/.
        if prune_cross_targets && target.triple != HOST_TRIPLE {
            prune_target_artifacts_for_triple(&target_dir, target.triple);
            print_free_disk(&target_dir, "after prune");
        }

        // Optional: also prune host release artifacts after building it.
        if prune_host && target.triple == HOST_TRIPLE {
            prune_host_release_artifacts(&target_dir);
            print_free_disk(&target_dir, "after host prune");
        }
    }

    // Only update checksums/symlinks/git for release builds
    if build_mode == BuildMode::Release {
        update_all_checksums(&version, &build_order);
        update_latest_symlink(&version);

        // Add binaries to git for sharing
        add_binaries_to_git(&version);

        println!(
            "\n✅ Windows build complete!\n📦 Binaries in dist/{}/*/",
            version
        );
    } else if build_mode == BuildMode::Profiling {
        println!(
            "\n✅ Profiling build complete!\n📦 Binaries in {:?}/{}/profiling/",
            target_dir, TARGETS[0].triple
        );
        println!("📋 Run 'just profile-usb' to copy to USB for Windows profiling");
    } else {
        println!("\n✅ Dev build complete!");
    }
    println!("ℹ️  Note: UFFS only runs on Windows (requires NTFS MFT access)");
}

fn add_binaries_to_git(version: &str) {
    println!("\n📦 Adding binaries to git...");
    let dist_path = format!("dist/{}", version);

    // Check if dist directory exists
    if !Path::new(&dist_path).exists() {
        eprintln!("  ⚠️  {} does not exist, skipping git add", dist_path);
        return;
    }

    // Add the dist directory to git
    let status = Command::new("git").args(["add", &dist_path]).status();

    match status {
        Ok(s) if s.success() => {
            println!("  ✅ Added {} to git staging", dist_path);

            // Show what was added
            let output = Command::new("git")
                .args(["status", "--porcelain", &dist_path])
                .output();

            if let Ok(o) = output {
                let files = String::from_utf8_lossy(&o.stdout);
                let count = files.lines().count();
                if count > 0 {
                    println!("  📋 {} files staged for commit", count);
                }
            }
        }
        Ok(_) => eprintln!("  ⚠️  git add failed"),
        Err(e) => eprintln!("  ⚠️  git add error: {}", e),
    }
}

fn build_native_only() {
    let s = Command::new("rust-script")
        .arg("scripts/build-local.rs")
        .status()
        .expect("Failed");
    if !s.success() {
        exit(1);
    }
}

fn read_current_version() -> String {
    // Try UFFS workflow state first
    if let Ok(c) = fs::read_to_string("build/.uffs-workflow-state.json") {
        if let Ok(s) = serde_json::from_str::<serde_json::Value>(&c) {
            if let Some(v) = s.get("current_version").and_then(|v| v.as_str()) {
                if v != "unknown" {
                    return format!("v{}", v);
                }
            }
        }
    }
    // Fallback to Cargo.toml [workspace.package] version
    if let Ok(c) = fs::read_to_string("Cargo.toml") {
        let mut in_workspace_package = false;
        for l in c.lines() {
            let trimmed = l.trim();
            if trimmed == "[workspace.package]" {
                in_workspace_package = true;
                continue;
            }
            if in_workspace_package {
                if trimmed.starts_with('[') && trimmed != "[workspace.package]" {
                    break;
                }
                if trimmed.starts_with("version") {
                    if let Some(v) = trimmed.split('"').nth(1) {
                        return format!("v{}", v);
                    }
                }
            }
        }
    }
    "v0.1.0".to_owned()
}

fn get_cargo_target_dir() -> PathBuf {
    if let Ok(d) = env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = parse_cargo_config_target_dir() {
        return d;
    }
    PathBuf::from("./target")
}

fn parse_cargo_config_target_dir() -> Option<PathBuf> {
    if let Ok(c) = fs::read_to_string(".cargo/config.toml") {
        for l in c.lines() {
            let t = l.trim();
            if t.starts_with("target-dir") {
                if let Some(v) = t.split('=').nth(1) {
                    let p = v.trim().trim_matches('"').trim_matches('\'');
                    if p.starts_with("~/") {
                        if let Some(h) = dirs_next::home_dir() {
                            return Some(h.join(p.strip_prefix("~/").unwrap_or("")));
                        }
                    }
                    return Some(PathBuf::from(p));
                }
            }
        }
    }
    None
}

fn check_available_targets() -> Vec<&'static Target> {
    let installed = get_installed_targets();
    TARGETS
        .iter()
        .filter(|t| {
            if !installed.contains(&t.triple.to_string()) {
                println!("   ⚠️  {} not installed", t.triple);
                return false;
            }
            if let Some(l) = t.requires_linker {
                if !cmd_exists(l) {
                    println!("   ⚠️  {} skipped (no {})", t.triple, l);
                    return false;
                }
            }
            if t.use_xwin && !cmd_exists("cargo-xwin") {
                println!("   ⚠️  {} skipped (no cargo-xwin)", t.triple);
                return false;
            }
            true
        })
        .collect()
}

fn get_installed_targets() -> Vec<String> {
    let o = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .expect("rustup");
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(String::from)
        .collect()
}

fn cmd_exists(c: &str) -> bool {
    Command::new("which")
        .arg(c)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Disk space monitoring and artifact pruning
// ─────────────────────────────────────────────────────────────────────────────

/// Returns free disk space in GiB for the volume containing `path`.
fn disk_free_gib(path: &Path) -> f64 {
    // Use `df` to get free space - works on macOS and Linux
    let output = Command::new("df")
        .arg("-k") // 1K blocks
        .arg(path)
        .output();

    if let Ok(o) = output {
        let stdout = String::from_utf8_lossy(&o.stdout);
        // Parse the second line (first is header)
        if let Some(line) = stdout.lines().nth(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // df -k output: Filesystem 1K-blocks Used Available Use% Mounted
            if parts.len() >= 4 {
                if let Ok(avail_kb) = parts[3].parse::<u64>() {
                    return avail_kb as f64 / (1024.0 * 1024.0); // KB to GiB
                }
            }
        }
    }
    0.0
}

/// Print free disk space for the volume containing `path`.
fn print_free_disk(path: &Path, label: &str) {
    let free = disk_free_gib(path);
    println!("  💾 Free disk space ({}): {:.1} GiB", label, free);
}

/// Remove a directory tree, ignoring errors (best-effort cleanup).
fn remove_dir_best_effort(path: &Path) {
    if path.exists() {
        if let Err(e) = fs::remove_dir_all(path) {
            eprintln!("  ⚠️  Could not remove {}: {}", path.display(), e);
        }
    }
}

/// Preflight cleanup: remove leftover cross-target build trees from previous runs.
/// This prevents starting a new release with disk already consumed by stale artifacts.
fn prune_previous_cross_target_artifacts(target_dir: &Path) {
    println!("\n🧹 Preflight cleanup: removing leftover cross-target artifacts...");
    let mut removed_any = false;

    for target in TARGETS {
        // Skip host target - we don't want to remove our own build artifacts
        if target.triple == HOST_TRIPLE {
            continue;
        }

        let triple_dir = target_dir.join(target.triple);
        if triple_dir.exists() {
            println!("  🗑️  Removing {}", triple_dir.display());
            remove_dir_best_effort(&triple_dir);
            removed_any = true;
        }
    }

    if !removed_any {
        println!("  ✅ No leftover cross-target artifacts found");
    } else {
        print_free_disk(target_dir, "after preflight cleanup");
    }
}

/// Prune build artifacts for a specific target triple after building.
fn prune_target_artifacts_for_triple(target_dir: &Path, triple: &str) {
    let triple_dir = target_dir.join(triple);
    if triple_dir.exists() {
        println!("  🗑️  Pruning artifacts for {}", triple);
        remove_dir_best_effort(&triple_dir);
    }
}

/// Prune host release artifacts (optional, for extreme disk savings).
fn prune_host_release_artifacts(target_dir: &Path) {
    let release_dir = target_dir.join("release");
    if release_dir.exists() {
        println!("  🗑️  Pruning host release artifacts");
        remove_dir_best_effort(&release_dir);
    }
}

/// Ensure required tools are installed before starting the build.
fn ensure_required_tools_or_exit() {
    let mut missing = Vec::new();

    // Check for cargo-xwin (required for Windows cross-compilation)
    if !cmd_exists("cargo-xwin") {
        missing.push("cargo-xwin (install with: cargo install cargo-xwin)");
    }

    // Check for LLVM/clang-cl (required for Windows cross-compilation on macOS)
    if !Path::new("/opt/homebrew/opt/llvm/bin/clang-cl").exists() {
        missing.push("LLVM clang-cl (install with: brew install llvm)");
    }

    if !missing.is_empty() {
        eprintln!("\n❌ Missing required tools:");
        for tool in &missing {
            eprintln!("   • {}", tool);
        }
        exit(1);
    }
}

fn build_for_target(target: &Target, target_dir: &Path, verbose: bool) -> bool {
    let build_mode = get_build_mode();
    let profile = build_profile();

    for (binary, package) in BINARIES {
        let mut args: Vec<&str> = if target.use_xwin {
            vec!["xwin", "build"]
        } else {
            vec!["build"]
        };

        // Add profile based on build mode
        match build_mode {
            BuildMode::Release => {
                args.push("--release");
            }
            BuildMode::Profiling => {
                // Use profiling profile for performance analysis with debug symbols
                args.extend_from_slice(&["--profile", "profiling"]);
            }
            BuildMode::Dev => {
                if target.use_xwin {
                    // Use xwin-dev profile for xwin dev builds to avoid COFF archive size limits
                    // See: docs/xwin-msvc-rlib-size-root-cause-and-workarounds.md
                    args.extend_from_slice(&["--profile", "xwin-dev"]);
                }
            }
        }

        // Add target and package args
        args.extend_from_slice(&["--target", target.triple, "--bin", binary, "-p", package]);

        // Print verbose command info (similar to CI pipeline format)
        println!(
            "  → {} ({}) → cargo {} (target: {})",
            binary,
            profile,
            args.join(" "),
            target.triple
        );

        let mut cmd = Command::new("cargo");
        cmd.args(&args);

        // Set CARGO_TARGET_DIR to the expanded path (cargo xwin doesn't expand ~ in config)
        cmd.env("CARGO_TARGET_DIR", target_dir);

        // For Windows cross-compilation, add LLVM to PATH for clang-cl
        if target.use_xwin {
            let llvm_bin = "/opt/homebrew/opt/llvm/bin";
            let current_path = env::var("PATH").unwrap_or_default();
            let new_path = format!("{}:{}", llvm_bin, current_path);
            cmd.env("PATH", new_path);
            cmd.env(
                "CC_x86_64_pc_windows_msvc",
                format!("{}/clang-cl", llvm_bin),
            );
            cmd.env(
                "CXX_x86_64_pc_windows_msvc",
                format!("{}/clang-cl", llvm_bin),
            );
            cmd.env(
                "AR_x86_64_pc_windows_msvc",
                format!("{}/llvm-lib", llvm_bin),
            );
        }

        // For Linux musl cross-compilation, set bindgen to use musl headers
        if target.triple == "x86_64-unknown-linux-musl" {
            let musl_sysroot = "/opt/homebrew/opt/musl-cross/libexec/x86_64-linux-musl";
            cmd.env(
                "BINDGEN_EXTRA_CLANG_ARGS",
                format!(
                    "--sysroot={} -isystem {}/include",
                    musl_sysroot, musl_sysroot
                ),
            );
        }

        // In verbose mode, inherit stdio; otherwise capture output
        if verbose {
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        } else {
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::piped());
        }

        let output = cmd.output().expect("cargo failed to start");
        if !output.status.success() {
            eprintln!("  ❌ Failed to build {} for {}", binary, target.triple);
            if !verbose {
                // Print stderr on failure even in non-verbose mode
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    eprintln!("{}", stderr);
                }
            }
            return false;
        }
        println!("  ✅ {}", binary);
    }
    true
}

/// Copy built binaries to dist directory.
/// Returns true if ALL binaries were copied successfully, false if any are
/// missing or failed.
fn copy_binaries_to_dist(version: &str, target: &Target, target_dir: &Path) -> bool {
    let profile = build_profile();
    let output_dir = build_output_dir();
    println!("  📁 Copying {} binaries to dist/{}...", profile, version);

    let mut all_success = true;
    let mut missing_binaries: Vec<String> = Vec::new();
    let mut failed_copies: Vec<String> = Vec::new();

    for (binary, _) in BINARIES {
        let bin_name = if target.triple.contains("windows") {
            format!("{}.exe", binary)
        } else {
            (*binary).to_string()
        };
        let source = target_dir
            .join(target.triple)
            .join(output_dir)
            .join(&bin_name);
        let dest_dir = format!("dist/{}/{}", version, binary);
        let dest_name = if target.triple.contains("windows") {
            format!("{}-{}.exe", binary, target.platform_name)
        } else {
            format!("{}-{}", binary, target.platform_name)
        };
        let dest = format!("{}/{}", dest_dir, dest_name);

        if let Err(e) = fs::create_dir_all(&dest_dir) {
            eprintln!("  ❌ Failed to create {}: {}", dest_dir, e);
            all_success = false;
            failed_copies.push(format!("{} (dir creation failed: {})", binary, e));
            continue;
        }
        if source.exists() {
            if let Err(e) = fs::copy(&source, &dest) {
                eprintln!("  ❌ Failed to copy {}: {}", binary, e);
                all_success = false;
                failed_copies.push(format!("{} (copy failed: {})", binary, e));
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(m) = fs::metadata(&dest) {
                        let mut p = m.permissions();
                        p.set_mode(0o755);
                        let _ = fs::set_permissions(&dest, p);
                    }
                }
                println!("  ✅ {}", dest_name);
            }
        } else {
            eprintln!("  ❌ Binary not found: {:?}", source);
            all_success = false;
            missing_binaries.push(bin_name);
        }
    }

    // Report summary of failures
    if !missing_binaries.is_empty() {
        eprintln!(
            "\n❌ FATAL: {} binaries missing for {}: {:?}",
            missing_binaries.len(),
            target.triple,
            missing_binaries
        );
    }
    if !failed_copies.is_empty() {
        eprintln!(
            "\n❌ FATAL: {} binaries failed to copy for {}: {:?}",
            failed_copies.len(),
            target.triple,
            failed_copies
        );
    }

    all_success
}

fn update_latest_symlink(version: &str) {
    let latest_link = Path::new("dist/latest");
    if latest_link.exists() || latest_link.is_symlink() {
        let _ = fs::remove_file(latest_link);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        if symlink(version, latest_link).is_ok() {
            println!("✅ Updated dist/latest -> {}", version);
        }
    }
}

fn update_all_checksums(version: &str, targets: &[&Target]) {
    println!("\n📋 Updating checksums...");
    let path = format!("dist/{}/checksums.txt", version);
    let mut lines: Vec<String> = Vec::new();

    for target in targets {
        for (binary, _) in BINARIES {
            let bin_file = if target.triple.contains("windows") {
                format!("{}-{}.exe", binary, target.platform_name)
            } else {
                format!("{}-{}", binary, target.platform_name)
            };
            let p = format!("dist/{}/{}/{}", version, binary, bin_file);
            if let Some(hash) = calc_hash(&p) {
                if let Ok(m) = fs::metadata(&p) {
                    lines.push(format!("{}  {} ({} bytes)", hash, p, m.len()));
                }
            }
        }
    }

    if let Err(e) = fs::write(&path, lines.join("\n") + "\n") {
        eprintln!("⚠️  Failed to write checksums: {}", e);
    } else {
        println!("✅ Updated {}", path);
    }
}

fn calc_hash(path: &str) -> Option<String> {
    fs::read(path).ok().map(|c| {
        let mut h = Sha256::new();
        h.update(&c);
        format!("{:x}", h.finalize())
    })
}
