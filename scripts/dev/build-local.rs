#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! serde_json = "1.0"
//! dirs-next = "2.0"
//! sha2 = "0.10"
//! ```
// =============================================================================
// scripts/dev/build-local.rs - UFFS Local Build & Install (Windows Only)
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// UFFS - UltraFastFileSearch: High-Performance File Search Tool
//
//! **IMPORTANT**: UFFS is a Windows-only tool. It reads the NTFS Master File
//! Table (MFT) directly using Windows kernel APIs.
//!
//! On Windows: Builds and installs the native binary to ~/bin
//! On macOS/Linux: Use build-cross-all.rs to cross-compile for Windows

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use sha2::{Sha256, Digest};

/// Check if release build mode is enabled via UFFS_RELEASE_BUILD env var.
/// Default is DEV mode for faster iteration during development.
fn is_release_build() -> bool {
    env::var("UFFS_RELEASE_BUILD")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Get the build profile name ("debug" or "release")
fn build_profile() -> &'static str {
    if is_release_build() { "release" } else { "debug" }
}

/// UFFS binaries: (binary_name, package_name)
/// - uffs: Main CLI tool (thin client)
/// - uffsd: Background daemon (holds MFT index, serves queries via IPC)
/// - uffsmcp: MCP HTTP/stdio server (bridges AI agents to daemon)
/// - uffs_mft: Low-level MFT reading tool
/// - uffs-update: self-update helper (`uffs --update` spawns it as a sibling)
/// - uffs-broker: Windows elevated handle broker (real service on Windows;
///   a no-op stub off Windows)
///
/// NOTE: uffs_tui and uffs_gui have moved to the private uffs-products repo.
/// `uffs-broker` is **Windows-only** (the elevated handle service); it is
/// not built/installed on macOS/Linux, where it would only be a no-op stub.
#[cfg(windows)]
const BINARIES: &[(&str, &str)] = &[
    ("uffs", "uffs-cli"),
    ("uffsd", "uffs-daemon"),
    ("uffsmcp", "uffs-mcp"),
    ("uffs-mft", "uffs-mft"),
    ("uffs-update", "uffs-update"),
    ("uffs-broker", "uffs-broker"),
];

/// Non-Windows host: the same set minus the Windows-only broker.
#[cfg(not(windows))]
const BINARIES: &[(&str, &str)] = &[
    ("uffs", "uffs-cli"),
    ("uffsd", "uffs-daemon"),
    ("uffsmcp", "uffs-mcp"),
    ("uffs-mft", "uffs-mft"),
    ("uffs-update", "uffs-update"),
];

fn main() {
    let args: Vec<String> = env::args().collect();
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");

    println!("🚀 UFFS Local Build & Install");
    println!("ℹ️  UFFS is Windows-only (requires NTFS MFT access)");
    if verbose {
        println!("🔍 Verbose mode enabled");
    }

    // Show build mode (DEV is default, set UFFS_RELEASE_BUILD=1 for release)
    let build_mode = if is_release_build() { "RELEASE (optimized)" } else { "DEV (fast, default)" };
    println!("🔧 Build mode: {}", build_mode);

    // Detect platform
    let platform = detect_platform();
    println!("🖥️  Platform: {}", platform);

    // Warn if not on Windows
    let host_os = env::consts::OS;
    if host_os != "windows" {
        println!("\n⚠️  WARNING: UFFS only runs on Windows!");
        println!("   The binary will be built but will return 'PlatformNotSupported' errors.");
        println!("   Use 'rust-script scripts/ci/build-cross-all.rs' to cross-compile for Windows.\n");
    }

    // Detect target directory
    let target_dir = get_cargo_target_dir();
    println!("📂 Target dir: {}", target_dir.display());

    // Read current version
    let version = read_current_version();
    println!("📦 Version: {}", version);

    // Build all binaries
    let mut any_built = false;
    for (binary, package) in BINARIES {
        let needs_rebuild = check_dist_binary_needs_rebuild(&version, &platform, &target_dir, binary);

        if !needs_rebuild {
            println!("  ✅ {} is up-to-date (SHA matches)", binary);
        } else {
            let profile = build_profile();
            println!("\n🔨 Building {} for {} ({})...", binary, platform, profile);

            let mut args = vec!["build", "-p", *package, "--bin", *binary];
            if is_release_build() {
                args.push("--release");
            }

            let mut cmd = Command::new("cargo");
            cmd.args(&args);

            // In verbose mode, inherit stdio; otherwise capture output
            if verbose {
                cmd.stdout(std::process::Stdio::inherit());
                cmd.stderr(std::process::Stdio::inherit());
            } else {
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::piped());
            }

            let output = cmd.output().expect("Failed to execute cargo build");

            if !output.status.success() {
                eprintln!("❌ Failed to build {}", binary);
                if !verbose {
                    // Print stderr on failure even in non-verbose mode
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.is_empty() {
                        eprintln!("{}", stderr);
                    }
                }
                exit(1);
            }

            // Copy to dist directory
            copy_binary_to_dist(&version, &platform, &target_dir, binary);
            any_built = true;
        }
    }

    // Update stored hashes in checksums.txt (if any were built)
    if any_built {
        update_dist_checksums(&version, &platform);
        prune_old_dist_versions(&version, 2);
    }

    // Copy binaries to ~/bin
    println!("\n📁 Installing binaries to ~/bin...");
    install_binaries(&version, &platform);

    // Check and update PATH
    check_and_update_path();

    // Ensure all dist files are tracked by git
    ensure_dist_files_tracked(&version);

    println!("\n✅ Build and install complete!");
    println!("🎉 UFFS binaries are now available in your PATH");
}

fn detect_platform() -> String {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;

    match (os, arch) {
        ("macos", "aarch64") => "macos-arm64".to_string(),
        ("macos", "x86_64") => "macos-intel".to_string(),
        ("linux", "x86_64") => "linux-x64".to_string(),
        ("linux", "aarch64") => "linux-arm64".to_string(),
        ("windows", "x86_64") => "windows-x64".to_string(),
        _ => format!("{}-{}", os, arch),
    }
}

/// Get the cargo target directory, checking multiple sources in order:
/// 1. CARGO_TARGET_DIR environment variable
/// 2. .cargo/config.toml target-dir setting
/// 3. Default ./target
fn get_cargo_target_dir() -> PathBuf {
    // 1. Check CARGO_TARGET_DIR env var (works on all platforms)
    if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir);
    }

    // 2. Parse .cargo/config.toml for target-dir setting
    if let Some(target_dir) = parse_cargo_config_target_dir() {
        return target_dir;
    }

    // 3. Default to ./target
    PathBuf::from("./target")
}

/// Parse .cargo/config.toml to find target-dir setting
fn parse_cargo_config_target_dir() -> Option<PathBuf> {
    let config_path = ".cargo/config.toml";

    if let Ok(content) = fs::read_to_string(config_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("target-dir") {
                // Parse: target-dir = "/tmp/rust-target" or target-dir = "~/.rust-target"
                if let Some(value) = trimmed.split('=').nth(1) {
                    let path_str = value.trim().trim_matches('"').trim_matches('\'');

                    // Expand ~ to home directory
                    if path_str.starts_with("~/") || path_str == "~" {
                        if let Some(home) = dirs_next::home_dir() {
                            let rest = path_str.strip_prefix("~/").unwrap_or("");
                            return Some(home.join(rest));
                        }
                    }

                    return Some(PathBuf::from(path_str));
                }
            }
        }
    }

    None
}

/// Get the path to a binary in the target directory (debug or release based on UFFS_RELEASE_BUILD)
fn get_binary_path(target_dir: &Path, binary_name: &str) -> PathBuf {
    let binary_name_with_ext = if cfg!(windows) {
        format!("{}.exe", binary_name)
    } else {
        binary_name.to_string()
    };

    target_dir.join(build_profile()).join(binary_name_with_ext)
}

fn read_current_version() -> String {
    // Try UFFS workflow state first
    if let Ok(content) = fs::read_to_string("build/.uffs-workflow-state.json") {
        if let Ok(state) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(version) = state.get("current_version").and_then(|v| v.as_str()) {
                if version != "unknown" {
                    return format!("v{}", version);
                }
            }
        }
    }

    // Fallback to Cargo.toml [workspace.package] version
    if let Ok(content) = fs::read_to_string("Cargo.toml") {
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
                    if let Some(version) = trimmed.split('"').nth(1) {
                        return format!("v{}", version);
                    }
                }
            }
        }
    }

    "v0.1.0".to_string()
}

fn install_binaries(version: &str, platform: &str) {
    let home_dir = dirs_next::home_dir().expect("Could not find home directory");
    let bin_dir = home_dir.join("bin");

    // Create ~/bin if it doesn't exist
    if !bin_dir.exists() {
        fs::create_dir_all(&bin_dir).expect("Failed to create ~/bin directory");
        println!("📁 Created ~/bin directory");
    }

    let ext = if platform.contains("windows") { ".exe" } else { "" };

    for (binary, _) in BINARIES {
        let source_path = format!("dist/{}/{}/{}-{}{}", version, binary, binary, platform, ext);
        let dest_path = bin_dir.join(format!("{}{}", binary, ext));

        if Path::new(&source_path).exists() {
            if let Err(e) = fs::copy(&source_path, &dest_path) {
                eprintln!("⚠️  Failed to copy {}: {}", binary, e);
            } else {
                // Make executable on Unix systems
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = fs::metadata(&dest_path) {
                        let mut perms = metadata.permissions();
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(&dest_path, perms);
                    }
                }
                println!("  ✅ Installed: ~/bin/{}", binary);
            }
        } else {
            println!("  ⚠️  Binary not found: {}", source_path);
        }
    }
}

fn check_and_update_path() {
    let home_dir = dirs_next::home_dir().expect("Could not find home directory");
    let bin_dir = home_dir.join("bin");
    let bin_path = bin_dir.to_string_lossy();

    if let Ok(current_path) = env::var("PATH") {
        if current_path.contains(&*bin_path) {
            println!("✅ ~/bin is already in PATH");
            return;
        }
    }

    println!("📝 ~/bin is not in PATH. Add this to your shell profile:");
    println!("   export PATH=\"$HOME/bin:$PATH\"");
    
    // Try to detect shell and provide specific instructions
    if let Ok(shell) = env::var("SHELL") {
        let profile_file = if shell.contains("zsh") {
            "~/.zshrc"
        } else if shell.contains("bash") {
            "~/.bashrc or ~/.bash_profile"
        } else if shell.contains("fish") {
            "~/.config/fish/config.fish"
        } else {
            "your shell profile"
        };
        
        println!("   Add to {}: export PATH=\"$HOME/bin:$PATH\"", profile_file);
    }
}

fn ensure_dist_files_tracked(version: &str) {
    println!("\n📋 Ensuring all dist files are tracked by git...");

    // Force add all dist files (binaries and checksums)
    let commands = vec![
        vec!["add", "dist/"],
        vec!["add", "-f", "dist/**/*"], // Force add all nested files
    ];

    for cmd_args in &commands {
        let status = Command::new("git")
            .args(cmd_args)
            .status()
            .expect("Failed to execute git add");

        if !status.success() {
            println!("⚠️  Warning: git add {:?} failed", cmd_args);
        }
    }

    println!("✅ All dist files added to git tracking");

    // Show what was added
    let output = Command::new("git")
        .args(["status", "--porcelain", "dist/"])
        .output()
        .expect("Failed to execute git status");

    if !output.stdout.is_empty() {
        println!("📁 Files ready for commit:");
        let status_output = String::from_utf8_lossy(&output.stdout);
        for line in status_output.lines() {
            if !line.trim().is_empty() {
                println!("   {}", line);
            }
        }
    } else {
        println!("📁 All dist files already tracked and up-to-date");
    }

    // Create .gitkeep files for empty directories to ensure they're tracked
    create_gitkeep_files(version);
}

fn create_gitkeep_files(version: &str) {
    for (binary, _) in BINARIES {
        let gitkeep_dir = format!("dist/{}/{}", version, binary);
        let gitkeep_path = format!("{}/.gitkeep", gitkeep_dir);

        if !Path::new(&gitkeep_path).exists() {
            if let Ok(()) = fs::create_dir_all(&gitkeep_dir) {
                if fs::write(&gitkeep_path, "# Keep this directory in git\n").is_ok() {
                    println!("📁 Created: {}", gitkeep_path);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Smart Binary Checking System - Uses actual dist files as cache
// ═══════════════════════════════════════════════════════════════════════════════

fn calculate_file_hash(file_path: &str) -> Option<String> {
    if let Ok(content) = fs::read(file_path) {
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Some(format!("{:x}", hasher.finalize()))
    } else {
        None
    }
}

fn calculate_file_hash_path(path: &Path) -> Option<String> {
    if let Ok(content) = fs::read(path) {
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Some(format!("{:x}", hasher.finalize()))
    } else {
        None
    }
}

fn get_stored_hash_from_checksums(version: &str, binary_path: &str) -> Option<String> {
    let checksums_path = format!("dist/{}/checksums.txt", version);
    if let Ok(content) = fs::read_to_string(&checksums_path) {
        for line in content.lines() {
            if line.contains(binary_path) {
                // Extract hash from line like: "abc123def  dist/v0.1.1/uffs/uffs-macos-arm64 (1234 bytes)"
                if let Some(hash) = line.split_whitespace().next() {
                    return Some(hash.to_string());
                }
            }
        }
    }
    None
}

fn check_dist_binary_needs_rebuild(version: &str, platform: &str, target_dir: &Path, binary: &str) -> bool {
    let ext = if platform.contains("windows") { ".exe" } else { "" };
    let dist_path = format!("dist/{}/{}/{}-{}{}", version, binary, binary, platform, ext);
    let profile = build_profile();
    let binary_path = get_binary_path(target_dir, binary);

    // If binary doesn't exist, need to build
    if !binary_path.exists() {
        println!("   📦 {} - {} binary missing", binary, profile);
        return true;
    }

    // If dist binary doesn't exist, need to copy
    if !Path::new(&dist_path).exists() {
        println!("   📦 {} - dist binary missing", binary);
        return true;
    }

    // Compare hashes
    let build_hash = calculate_file_hash_path(&binary_path);
    let dist_hash = calculate_file_hash(&dist_path);

    match (build_hash, dist_hash) {
        (Some(b), Some(d)) if b == d => {
            println!("   ✅ {} - SHA matches ({}...)", binary, &b[..8]);
            false
        }
        (Some(b), Some(d)) => {
            println!("   🔄 {} - SHA changed ({}... -> {}...)", binary, &d[..8], &b[..8]);
            true
        }
        _ => {
            println!("   ⚠️  {} - could not compare hashes", binary);
            true
        }
    }
}

fn copy_binary_to_dist(version: &str, platform: &str, target_dir: &Path, binary: &str) {
    let dist_dir = format!("dist/{}/{}", version, binary);
    fs::create_dir_all(&dist_dir).expect("Failed to create dist directory");

    let source = get_binary_path(target_dir, binary);
    let ext = if platform.contains("windows") { ".exe" } else { "" };
    let dest = format!("{}/{}-{}{}", dist_dir, binary, platform, ext);

    if source.exists() {
        fs::copy(&source, &dest).expect("Failed to copy binary to dist");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(m) = fs::metadata(&dest) {
                let mut p = m.permissions();
                p.set_mode(0o755);
                let _ = fs::set_permissions(&dest, p);
            }
        }
        println!("  ✅ Copied {} to {}", binary, dest);
    } else {
        eprintln!("  ⚠️  Source binary not found: {:?}", source);
    }
}

/// Keep only the `keep` most recent versioned directories in `dist/`.
/// Also removes the legacy `dist/latest` symlink if present.
fn prune_old_dist_versions(current: &str, keep: usize) {
    let dist = Path::new("dist");

    // Remove legacy symlink
    let latest_link = dist.join("latest");
    if latest_link.exists() || latest_link.is_symlink() {
        let _ = fs::remove_file(&latest_link);
        println!("🗑️  Removed dist/latest symlink");
    }

    // Collect versioned directories (start with 'v')
    let mut versions: Vec<String> = fs::read_dir(dist)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('v') { Some(name) } else { None }
        })
        .collect();

    versions.sort();

    let to_remove: Vec<String> = if versions.len() > keep {
        versions[..versions.len() - keep]
            .iter()
            .filter(|v| v.as_str() != current)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    for v in &to_remove {
        let path = dist.join(v);
        if let Err(e) = fs::remove_dir_all(&path) {
            eprintln!("⚠️  Failed to remove dist/{}: {}", v, e);
        } else {
            println!("🗑️  Pruned old dist/{}", v);
        }
    }

    if to_remove.is_empty() {
        println!("✅ dist/ clean — {} version(s) retained", versions.len());
    } else {
        println!("✅ Pruned {} old version(s), kept {}", to_remove.len(), keep);
    }
}

fn update_dist_checksums(version: &str, platform: &str) {
    println!("\n📋 Updating checksums...");

    let checksums_path = format!("dist/{}/checksums.txt", version);
    let ext = if platform.contains("windows") { ".exe" } else { "" };
    let mut lines: Vec<String> = Vec::new();

    for (binary, _) in BINARIES {
        let binary_path = format!("dist/{}/{}/{}-{}{}", version, binary, binary, platform, ext);

        if Path::new(&binary_path).exists() {
            if let Some(hash) = calculate_file_hash(&binary_path) {
                if let Ok(metadata) = fs::metadata(&binary_path) {
                    lines.push(format!("{}  {} ({} bytes)", hash, binary_path, metadata.len()));
                }
            }
        }
    }

    if !lines.is_empty() {
        fs::write(&checksums_path, lines.join("\n") + "\n").expect("Failed to write checksums");
        println!("✅ Updated {}", checksums_path);
    }
}
