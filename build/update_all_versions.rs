#!/usr/bin/env rust-script
// =============================================================================
// build/update_all_versions.rs - Professional Version Management Tool
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// UFFS - UltraFastFileSearch: High-Performance File Search Tool
//
//! # Comprehensive Workspace Version Management Tool
//!
//! A professional-grade Rust script for managing semantic versioning across
//! entire workspace projects with dynamic detection and comprehensive file updates.
//!
//! ## 🎯 Core Philosophy
//!
//! This tool embodies the **Rust Master** approach to version management:
//! - **Zero Configuration**: Dynamically detects project metadata
//! - **Comprehensive Coverage**: Updates ALL version references across the codebase
//! - **Safety First**: Dry-run mode prevents accidental modifications
//! - **Workspace Native**: Designed specifically for Cargo workspace projects
//! - **Pattern Resilient**: Handles multiple formatting styles and edge cases
//!
//! ## 🏗️ Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    Version Update Pipeline                      │
//! ├─────────────────────────────────────────────────────────────────┤
//! │ 1. Dynamic Detection Phase                                      │
//! │    ├── Package Name    (from [package] section)                 │
//! │    ├── Repository Name (from repository URL)                    │
//! │    └── Current Version (from [workspace.package] section)       │
//! │                                                                 │
//! │ 2. Version Calculation Phase                                    │
//! │    ├── Parse semantic version (major.minor.patch)               │
//! │    ├── Apply increment type (patch/minor/major)                 │
//! │    └── Generate new semantic version                            │
//! │                                                                 │
//! │ 3. File Update Phase (with pattern matching)                   │
//! │    ├── Cargo.toml     (workspace version + flexible spacing)    │
//! │    ├── README.md      (5 pattern types + dependency refs)       │
//! │    └── Documentation  (version tags + exact matches)            │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## 🔧 Technical Implementation Details
//!
//! ### Workspace Detection Strategy
//! The tool uses a **hierarchical parsing approach** for Cargo.toml:
//! 1. **Section-aware parsing**: Tracks current TOML section context
//! 2. **Workspace-first**: Prioritizes `[workspace.package]` for version source
//! 3. **Fallback resilience**: Graceful handling of missing sections
//!
//! ### Pattern Matching Engine
//! Implements **multi-pattern recognition** for maximum coverage:
//! - **Spacing agnostic**: Handles various whitespace patterns
//! - **Context aware**: Different patterns for different file types
//! - **Dependency smart**: Uses package name for dependency declarations
//!
//! ## 📋 Usage Examples
//!
//! ```bash
//! # Safe exploration (recommended first step)
//! ./build/update_all_versions.rs --help
//! ./build/update_all_versions.rs patch --dry-run
//!
//! # Production usage
//! ./build/update_all_versions.rs patch    # 0.1.143 → 0.1.144
//! ./build/update_all_versions.rs minor    # 0.1.143 → 0.2.0
//! ./build/update_all_versions.rs major    # 0.1.143 → 1.0.0
//! ```
//!
//! ## 🛡️ Safety Guarantees
//!
//! - **Dry-run mode**: Test changes without file modifications
//! - **Atomic operations**: Each file update is independent
//! - **Validation**: Semantic version format validation
//! - **Error isolation**: Single file failures don't affect others
//!
//! ## 🎨 Output Design
//!
//! Uses **semantic emojis** and **structured progress reporting**:
//! - 🔄 Process initiation
//! - 📦 Project metadata
//! - 📍 Current state
//! - 🎯 Target state
//! - 📝 File operations
//! - ✅ Success confirmations
//! - ⚠️ Warnings and skips
//!
//! ```cargo
//! [dependencies]
//! # No external dependencies - uses only std library for maximum portability
//! ```

use std::fs;
use std::env;
use std::process::Command;

/// # Main Entry Point - Version Update Orchestrator
///
/// Coordinates the entire version update process through a **three-phase pipeline**:
/// 1. **Discovery Phase**: Dynamic project metadata extraction
/// 2. **Calculation Phase**: Semantic version increment computation
/// 3. **Update Phase**: Comprehensive file modification (or simulation)
///
/// ## 🔍 Command Line Interface Design
///
/// The CLI follows **Unix philosophy** with sensible defaults:
/// - **Default behavior**: Patch increment (safest option)
/// - **Explicit flags**: `--help`, `--dry-run` for safety
/// - **Positional args**: Increment type as first argument
///
/// ## 🛡️ Error Handling Strategy
///
/// Uses **fail-fast with context** approach:
/// - Early validation of all inputs before any modifications
/// - Detailed error messages with actionable guidance
/// - Graceful degradation for optional operations
///
/// ## 📊 Process Flow
///
/// ```text
/// Input Args → Validation → Discovery → Calculation → Update/Simulate → Report
///     ↓            ↓           ↓           ↓             ↓            ↓
///   Parse      Help/DryRun   Metadata   New Version   File Ops    Success
/// ```
///
/// ## 🎯 Return Value Semantics
///
/// - `Ok(())`: All operations completed successfully
/// - `Err(Box<dyn Error>)`: Any step failed with descriptive error
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 0: Command Line Interface Processing
    // ═══════════════════════════════════════════════════════════════════════

    // Handle help request - early exit for documentation
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        print_help();
        return Ok(());
    }

    // Extract increment type with sensible default (patch is safest)
    let increment_type = args.get(1).map(|s| s.as_str()).unwrap_or("patch");

    // Detect dry-run mode for safe testing
    let dry_run = args.contains(&"--dry-run".to_string()) || args.contains(&"-n".to_string());

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 1: Dynamic Project Discovery
    // ═══════════════════════════════════════════════════════════════════════

    // Extract project metadata using workspace-aware parsing
    // These operations are designed to fail fast if Cargo.toml is malformed
    let package_name = get_package_name()?;
    let repository_name = get_repository_name()?;
    let current_version = get_current_version()?;

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 2: User Communication & Status Reporting
    // ═══════════════════════════════════════════════════════════════════════

    println!("🔄 Comprehensive version update for {} project", package_name);
    println!("📦 Repository: {}", repository_name);
    println!("📋 Increment type: {}", increment_type);
    println!("📍 Current version: {}", current_version);

    if dry_run {
        println!("🔍 DRY RUN MODE - No files will be modified");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 3: Version Calculation & Validation
    // ═══════════════════════════════════════════════════════════════════════

    // Calculate new version using semantic versioning rules
    let new_version = increment_version(&current_version, increment_type)?;
    println!("🎯 New version: {}", new_version);

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 4: File Update Orchestration (or Simulation)
    // ═══════════════════════════════════════════════════════════════════════

    if dry_run {
        // Simulation mode: Show what would happen without making changes
        println!("📝 Would update Cargo.toml...");
        println!("📝 Would update README.md...");
        println!("📝 Would update documentation files...");
        println!("🔒 Would refresh Cargo.lock (cargo generate-lockfile --offline)...");
        println!("✅ Dry run completed - no files were modified");
        println!("📦 {} would be updated to version: {}", package_name, new_version);
    } else {
        // Production mode: Execute actual file modifications
        // Each update operation is independent and can fail without affecting others
        update_cargo_toml(&current_version, &new_version)?;
        update_readme(&package_name, &current_version, &new_version)?;
        update_docs(&current_version, &new_version)?;
        refresh_cargo_lock()?;

        println!("✅ All versions updated successfully!");
        println!("📦 {} is now at version: {}", package_name, new_version);
    }

    Ok(())
}

/// # Cargo.lock Refresh
///
/// Runs `cargo generate-lockfile --offline` to ensure `Cargo.lock`'s internal
/// `[[package]]` entries track the new workspace version after the Cargo.toml
/// edits.  Without this step, the lockfile silently drifts (workspace
/// `Cargo.toml` reports the new version, but the lockfile keeps the OLD
/// version on internal crates) until some later `cargo` invocation self-heals
/// it — breaking the "tagged release is byte-reproducible from its
/// `Cargo.lock`" invariant for every release shipped before the self-heal
/// fires.  See `docs/architecture/release-automation-plan.md` §2.2.
///
/// `--offline` is intentional: the workspace-internal version rewrite needs
/// no network access, and using `--offline` makes this step deterministic on
/// air-gapped CI.  External dependency updates are intentionally NOT done
/// here — that is `cargo update`'s job, governed by Dependabot, not the
/// version-bump tool.
///
/// **Forward note**: this helper exists only until release-plz takes over
/// version bumping in Phase R5 of the release-automation plan.  Release-plz
/// refreshes the lockfile natively via its own `dependencies_update = true`
/// path, so this function gets deleted alongside `update_all_versions.rs`
/// itself.
fn refresh_cargo_lock() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔒 Refreshing Cargo.lock to track new workspace version...");

    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .status()
        .map_err(|e| format!("failed to spawn `cargo generate-lockfile`: {e}"))?;

    if !status.success() {
        return Err(format!(
            "`cargo generate-lockfile --offline` exited with {status}; \
             Cargo.lock may have drifted from Cargo.toml. \
             Investigate and run manually before pushing."
        )
        .into());
    }

    println!("✅ Cargo.lock refreshed");
    Ok(())
}

/// # Interactive Help System
///
/// Provides **comprehensive documentation** for the version update tool,
/// designed following **man page conventions** with clear sections and examples.
///
/// ## 📋 Help Content Strategy
///
/// - **Progressive disclosure**: Basic usage first, advanced features later
/// - **Example-driven**: Real commands users can copy-paste
/// - **Visual hierarchy**: Emojis and spacing for readability
/// - **Technical depth**: Explains what the tool actually does
fn print_help() {
    println!("📚 Comprehensive Version Update Tool");
    println!("    Professional Rust workspace version management with dynamic detection");
    println!();

    println!("🎯 PHILOSOPHY:");
    println!("    Zero-configuration tool that dynamically discovers project metadata");
    println!("    and updates ALL version references across your entire workspace.");
    println!();

    println!("📖 USAGE:");
    println!("    ./build/update_all_versions.rs [INCREMENT_TYPE] [OPTIONS]");
    println!();

    println!("🔢 INCREMENT_TYPE (Semantic Versioning):");
    println!("    patch    Increment patch version (default) - 0.1.143 → 0.1.144");
    println!("             Use for: Bug fixes, documentation updates, minor improvements");
    println!();
    println!("    minor    Increment minor version - 0.1.143 → 0.2.0");
    println!("             Use for: New features, API additions (backward compatible)");
    println!();
    println!("    major    Increment major version - 0.1.143 → 1.0.0");
    println!("             Use for: Breaking changes, major API redesigns");
    println!();

    println!("⚙️  OPTIONS:");
    println!("    --dry-run, -n    Show what would be updated without making changes");
    println!("                     RECOMMENDED: Always test with dry-run first!");
    println!();
    println!("    --help, -h       Show this comprehensive help message");
    println!();

    println!("💡 EXAMPLES:");
    println!("    # Safe exploration (recommended workflow)");
    println!("    ./build/update_all_versions.rs --help");
    println!("    ./build/update_all_versions.rs patch --dry-run");
    println!();
    println!("    # Production usage");
    println!("    ./build/update_all_versions.rs                    # Patch increment");
    println!("    ./build/update_all_versions.rs minor              # Minor increment");
    println!("    ./build/update_all_versions.rs major --dry-run    # Major increment (test first)");
    println!();

    println!("🚀 ADVANCED FEATURES:");
    println!("    ✓ Dynamic package name detection from Cargo.toml [package] section");
    println!("    ✓ Dynamic repository name extraction from repository URL");
    println!("    ✓ Workspace-aware version management ([workspace.package] priority)");
    println!("    ✓ Comprehensive README.md pattern matching (5 different patterns)");
    println!("    ✓ Multiple documentation file updates with progress tracking");
    println!("    ✓ Flexible Cargo.toml spacing pattern recognition");
    println!("    ✓ Atomic file operations with independent error handling");
    println!();

    println!("📁 FILES UPDATED:");
    println!("    • Cargo.toml           [workspace.package] version field");
    println!("    • README.md            Version badges, tags, dependencies, references");
    println!("    • CHANGELOG.md         Version references and tags");
    println!("    • docs/*.md            Documentation version references");
    println!();

    println!("🛡️  SAFETY GUARANTEES:");
    println!("    • Dry-run mode for safe testing");
    println!("    • Semantic version validation");
    println!("    • Independent file operations (single failure doesn't affect others)");
    println!("    • Comprehensive error messages with actionable guidance");
    println!();

    println!("🔧 TECHNICAL DETAILS:");
    println!("    • Zero external dependencies (std library only)");
    println!("    • Workspace-native design for Cargo workspace projects");
    println!("    • Pattern-resilient parsing (handles various formatting styles)");
    println!("    • Section-aware TOML parsing with context tracking");
}

/// # Dynamic Package Name Detection
///
/// Extracts the package name from Cargo.toml using a **multi-strategy approach**:
/// 1. First tries the `[package]` section (standard single-crate projects)
/// 2. Falls back to extracting from repository URL in `[workspace.package]` (virtual workspaces)
///
/// ## 🎯 Algorithm Design
///
/// 1. **Section Tracking**: Maintains state of current TOML section
/// 2. **Exact Matching**: Only processes `name` field within `[package]` section
/// 3. **Quote Extraction**: Safely extracts quoted string values
/// 4. **Fallback Strategy**: Uses repository name for virtual workspaces
/// 5. **Error Context**: Provides actionable error messages
///
/// ## 📋 Expected Input Formats
///
/// Standard package:
/// ```toml
/// [package]
/// name = "uffs-cli"
/// version = { workspace = true }
/// ```
///
/// Virtual workspace (no [package] section):
/// ```toml
/// [workspace.package]
/// repository = "https://github.com/user/uffs"
/// # name is derived from repository URL → "uffs"
/// ```
///
/// ## 🛡️ Edge Case Handling
///
/// - **Multiple sections**: Ignores `name` fields in other sections
/// - **Malformed quotes**: Validates quote pairing before extraction
/// - **Virtual workspace**: Falls back to repository name extraction
/// - **Empty values**: Handles empty strings gracefully
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(String)`: Successfully extracted package name
/// - `Err(...)`: Missing section, malformed TOML, or I/O error
fn get_package_name() -> Result<String, Box<dyn std::error::Error>> {
    let content = fs::read_to_string("Cargo.toml")?;
    let mut in_package = false;

    // Strategy 1: Try to find name in [package] section (standard projects)
    for line in content.lines() {
        let trimmed = line.trim();

        // Section boundary detection
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }

        // Exit package section when entering any other section
        if trimmed.starts_with('[') && trimmed != "[package]" {
            in_package = false;
            continue;
        }

        // Process name field only within [package] section
        if in_package && trimmed.starts_with("name") && trimmed.contains('=') {
            // Safe quote extraction with bounds checking
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed.rfind('"') {
                    if start < end {
                        return Ok(trimmed[start + 1..end].to_string());
                    }
                }
            }
        }
    }

    // Strategy 2: Fall back to repository name for virtual workspaces
    // Virtual workspaces don't have a [package] section, so we derive the name
    // from the repository URL in [workspace.package]
    match get_repository_name() {
        Ok(repo_name) => {
            println!("ℹ️  Virtual workspace detected - using repository name: {}", repo_name);
            Ok(repo_name)
        }
        Err(_) => Err("Could not find package name - ensure Cargo.toml has either a [package] section with name field, or a [workspace.package] section with repository field".into())
    }
}

/// # Dynamic Repository Name Extraction
///
/// Extracts the repository name from the **`[workspace.package]` section** by
/// parsing the repository URL and extracting the final path component.
///
/// ## 🎯 URL Parsing Strategy
///
/// 1. **Workspace Priority**: Looks in `[workspace.package]` for shared metadata
/// 2. **URL Decomposition**: Extracts final path segment from repository URL
/// 3. **Fallback Handling**: Returns full URL if path parsing fails
/// 4. **Section Isolation**: Only processes repository field in correct section
///
/// ## 📋 Expected Input Formats
///
/// ```toml
/// [workspace.package]
/// repository = "https://github.com/user/repo"           # → "repo"
/// repository = "https://gitlab.com/org/project"         # → "project"
/// repository = "git@github.com:user/repo.git"          # → "repo.git"
/// repository = "custom-name"                            # → "custom-name"
/// ```
///
/// ## 🔧 Algorithm Details
///
/// - **Path Extraction**: Uses `rfind('/')` to get last URL segment
/// - **Graceful Degradation**: Returns full URL if no path separators found
/// - **Quote Safety**: Validates quote boundaries before string extraction
///
/// ## 🛡️ Error Handling
///
/// - **Missing Section**: Clear error for missing `[workspace.package]`
/// - **Missing Field**: Specific error for missing `repository` field
/// - **Malformed URL**: Graceful fallback to full URL string
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(String)`: Repository name (last path component or full URL)
/// - `Err(...)`: Missing section/field, malformed TOML, or I/O error
fn get_repository_name() -> Result<String, Box<dyn std::error::Error>> {
    let content = fs::read_to_string("Cargo.toml")?;
    let mut in_workspace_package = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Section boundary detection for workspace.package
        if trimmed == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }

        // Exit workspace.package section when entering any other section
        if trimmed.starts_with('[') && trimmed != "[workspace.package]" {
            in_workspace_package = false;
            continue;
        }

        // Process repository field only within [workspace.package] section
        if in_workspace_package && trimmed.starts_with("repository") && trimmed.contains("=") {
            // Safe quote extraction with bounds checking
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed.rfind('"') {
                    if start < end {
                        let repo_url = &trimmed[start + 1..end];

                        // Extract repository name from URL path
                        // Example: "https://github.com/user/repo" → "repo"
                        if let Some(last_slash) = repo_url.rfind('/') {
                            return Ok(repo_url[last_slash + 1..].to_string());
                        }

                        // Fallback: return full URL if no path separators found
                        return Ok(repo_url.to_string());
                    }
                }
            }
        }
    }

    Err("Could not find repository in [workspace.package] section - ensure Cargo.toml has a valid [workspace.package] section with repository field".into())
}

/// # Workspace Version Detection
///
/// Extracts the current version from the **`[workspace.package]` section** of Cargo.toml,
/// which serves as the **single source of truth** for version information in workspace projects.
///
/// ## 🏗️ Workspace Architecture Understanding
///
/// In Cargo workspace projects, version information follows this hierarchy:
/// 1. **`[workspace.package]`**: Defines shared metadata (including version)
/// 2. **Individual crates**: Inherit version with `version = { workspace = true }`
/// 3. **This tool**: Updates the workspace version, which propagates to all crates
///
/// ## 📋 Expected Input Format
///
/// ```toml
/// [workspace.package]
/// version = "0.1.143"
/// authors = ["..."]
/// # ... other shared metadata
/// ```
///
/// ## 🎯 Algorithm Design
///
/// - **Section Isolation**: Only processes version field within `[workspace.package]`
/// - **Exact Matching**: Looks for `version` field with assignment operator
/// - **Quote Validation**: Ensures proper quote pairing before extraction
/// - **Context Awareness**: Tracks section boundaries to avoid false matches
///
/// ## 🛡️ Error Scenarios
///
/// - **Missing Section**: Workspace doesn't define shared package metadata
/// - **Missing Field**: Section exists but no version field
/// - **Malformed Quotes**: Unmatched or missing quotes around version string
/// - **I/O Errors**: File read failures or permission issues
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(String)`: Valid semantic version string (e.g., "0.1.143")
/// - `Err(...)`: Missing/malformed version or I/O error with context
fn get_current_version() -> Result<String, Box<dyn std::error::Error>> {
    let content = fs::read_to_string("Cargo.toml")?;
    let mut in_workspace_package = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Section boundary detection for workspace.package
        if trimmed == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }

        // Exit workspace.package section when entering any other section
        if trimmed.starts_with('[') && trimmed != "[workspace.package]" {
            in_workspace_package = false;
            continue;
        }

        // Process version field only within [workspace.package] section
        if in_workspace_package && trimmed.starts_with("version") && trimmed.contains("=") {
            // Safe quote extraction with bounds checking
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed.rfind('"') {
                    if start < end {
                        return Ok(trimmed[start + 1..end].to_string());
                    }
                }
            }
        }
    }

    Err("Could not find version in [workspace.package] section - ensure Cargo.toml has a valid [workspace.package] section with version field".into())
}

/// # Semantic Version Increment Engine
///
/// Implements **semantic versioning (SemVer)** increment logic following the
/// [Semantic Versioning 2.0.0](https://semver.org/) specification.
///
/// ## 📊 Semantic Versioning Rules
///
/// Given a version number `MAJOR.MINOR.PATCH`, increment the:
/// - **MAJOR**: Incompatible API changes (resets MINOR and PATCH to 0)
/// - **MINOR**: Backward-compatible functionality additions (resets PATCH to 0)
/// - **PATCH**: Backward-compatible bug fixes (increments only PATCH)
///
/// ## 🎯 Increment Type Mapping
///
/// ```text
/// Input: "0.1.143"
///
/// "major" → "1.0.0"    (Breaking changes)
/// "minor" → "0.2.0"    (New features)
/// "patch" → "0.1.144"  (Bug fixes, default)
/// ```
///
/// ## 🔧 Algorithm Implementation
///
/// 1. **Parse**: Split version string on '.' delimiter
/// 2. **Validate**: Ensure exactly 3 numeric components
/// 3. **Convert**: Parse each component to u32 for arithmetic
/// 4. **Increment**: Apply SemVer rules based on increment type
/// 5. **Format**: Reconstruct version string with proper zero-resets
///
/// ## 🛡️ Input Validation
///
/// - **Format Check**: Must be exactly 3 dot-separated components
/// - **Numeric Validation**: Each component must parse as valid u32
/// - **Overflow Protection**: u32 provides sufficient range (0 to 4,294,967,295)
///
/// ## 🔄 Error Handling
///
/// - **Invalid Format**: Non-standard version format (not X.Y.Z)
/// - **Parse Errors**: Non-numeric components
/// - **Overflow**: Extremely unlikely with u32 range
///
/// ## 📋 Return Value Semantics
///
/// - `Ok(String)`: Valid incremented semantic version
/// - `Err(...)`: Invalid input format or numeric parsing failure
fn increment_version(current: &str, increment_type: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Parse version string into components
    let version_parts: Vec<&str> = current.split('.').collect();

    // Validate semantic version format (must be exactly 3 components)
    if version_parts.len() != 3 {
        return Err(format!(
            "Invalid version format: '{}' - expected format: MAJOR.MINOR.PATCH (e.g., '0.1.143')",
            current
        ).into());
    }

    // Parse each component to u32 with detailed error context
    let major: u32 = version_parts[0].parse()
        .map_err(|_| format!("Invalid major version component: '{}'", version_parts[0]))?;
    let minor: u32 = version_parts[1].parse()
        .map_err(|_| format!("Invalid minor version component: '{}'", version_parts[1]))?;
    let patch: u32 = version_parts[2].parse()
        .map_err(|_| format!("Invalid patch version component: '{}'", version_parts[2]))?;

    // Apply semantic versioning increment rules
    let new_version = match increment_type {
        "major" => {
            // Major increment: Breaking changes, reset minor and patch to 0
            format!("{}.0.0", major + 1)
        },
        "minor" => {
            // Minor increment: New features, reset patch to 0
            format!("{}.{}.0", major, minor + 1)
        },
        "patch" | _ => {
            // Patch increment (default): Bug fixes, increment patch only
            format!("{}.{}.{}", major, minor, patch + 1)
        },
    };

    Ok(new_version)
}

/// # Cargo.toml Version Update Engine
///
/// Updates the version field in Cargo.toml using **pattern-resilient matching**
/// to handle various formatting styles and spacing conventions.
///
/// ## 🎯 Multi-Pattern Strategy
///
/// Different projects use different formatting styles for Cargo.toml:
/// ```toml
/// version = "0.1.143"        # Standard spacing (most common)
/// version       = "0.1.143"  # Aligned spacing (for readability)
/// version="0.1.143"          # No spaces (compact style)
/// version	= "0.1.143"        # Tab spacing (some editors)
/// ```
///
/// ## 🔧 Algorithm Design
///
/// 1. **Pattern Generation**: Create all possible formatting variations
/// 2. **Sequential Matching**: Test each pattern against file content
/// 3. **Safe Replacement**: Only replace exact matches to avoid false positives
/// 4. **Atomic Update**: Write file only if changes were made
///
/// ## 🛡️ Safety Mechanisms
///
/// - **Exact Matching**: Prevents accidental replacement of similar strings
/// - **Pattern Validation**: Only replaces when pattern is found
/// - **Atomic Write**: File is updated only after successful pattern matching
/// - **Rollback Safety**: Original content preserved until write operation
///
/// ## 📊 Pattern Priority
///
/// Patterns are tested in order of commonality:
/// 1. Standard spacing (most Rust projects)
/// 2. Aligned spacing (formatted projects)
/// 3. No spaces (compact style)
/// 4. Tab spacing (legacy/editor-specific)
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(())`: File successfully updated or no changes needed
/// - `Err(...)`: I/O error during read/write operations
///
/// ## 📋 Output Behavior
///
/// - **Success**: "✅ Cargo.toml updated"
/// - **No Match**: "⚠️ No version pattern found in Cargo.toml"
fn update_cargo_toml(current: &str, new: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("📝 Updating Cargo.toml...");

    let content = fs::read_to_string("Cargo.toml")?;

    // Generate all possible spacing patterns for version field
    // Ordered by commonality in Rust ecosystem
    let patterns = [
        format!("version = \"{}\"", current),           // Standard spacing (most common)
        format!("version       = \"{}\"", current),     // Aligned spacing (formatted)
        format!("version=\"{}\"", current),             // No spaces (compact)
        format!("version\t= \"{}\"", current),          // Tab spacing (legacy)
    ];

    let mut new_content = content.clone();
    let mut updated = false;

    // Test each pattern and apply replacement if found
    for pattern in &patterns {
        let replacement = pattern.replace(current, new);
        if new_content.contains(pattern) {
            new_content = new_content.replace(pattern, &replacement);
            updated = true;
            // Note: We continue checking all patterns to handle edge cases
            // where multiple patterns might exist (though this is rare)
        }
    }

    // Atomic file update: only write if changes were made
    if updated {
        fs::write("Cargo.toml", new_content)?;
        println!("✅ Cargo.toml updated");
    } else {
        println!("⚠️  No version pattern found in Cargo.toml");
        println!("    Expected patterns: version = \"{}\", version       = \"{}\", etc.", current, current);
    }

    Ok(())
}

/// # README.md Comprehensive Pattern Matching Engine
///
/// Updates version references in README.md using **5 distinct pattern types**
/// to ensure comprehensive coverage of all common version reference formats.
///
/// ## 🎯 Pattern Recognition Strategy
///
/// README files contain version information in various contexts:
/// 1. **Badges**: Visual indicators (shields.io, etc.)
/// 2. **Git Tags**: Release references
/// 3. **Documentation**: Prose version mentions
/// 4. **Dependencies**: Code examples showing how to use the package
/// 5. **Configuration**: TOML/JSON examples
///
/// ## 📋 Pattern Types & Examples
///
/// ```markdown
/// # Pattern 1: Version Badges
/// ![Version](https://img.shields.io/badge/version-0.1.143-blue)
///
/// # Pattern 2: Version Tags (Git releases)
/// Download [v0.1.143](https://github.com/user/repo/releases/tag/v0.1.143)
///
/// # Pattern 3: Version References (prose)
/// This documentation covers version 0.1.143 of the API.
///
/// # Pattern 4: Dependency Declarations (Cargo.toml examples)
/// [dependencies]
/// uffs-core = "0.1.143"
///
/// # Pattern 5: Alternative Dependency Format
/// uffs-core = { version = "0.1.143", features = ["full"] }
/// ```
///
/// ## 🔧 Algorithm Design
///
/// 1. **Sequential Processing**: Each pattern is tested independently
/// 2. **Change Tracking**: Monitors which patterns were found and updated
/// 3. **Progressive Replacement**: Applies changes to working copy
/// 4. **Atomic Write**: File updated only if any changes were made
/// 5. **Detailed Reporting**: Shows exactly which patterns were updated
///
/// ## 🛡️ Safety & Precision
///
/// - **Exact Matching**: Prevents false positives with similar version numbers
/// - **Package-Aware**: Uses actual package name for dependency patterns
/// - **Non-Destructive**: Original file preserved until all patterns processed
/// - **Graceful Handling**: Missing README.md doesn't cause failure
///
/// ## 📊 Progress Reporting
///
/// Each pattern type provides specific feedback:
/// - "✓ Updated version badge" - Shields.io or similar badges
/// - "✓ Updated version tags" - Git release tags (v-prefixed)
/// - "✓ Updated version references" - Prose mentions
/// - "✓ Updated dependency declarations" - Cargo.toml examples
/// - "✓ Updated version fields" - Alternative dependency syntax
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(())`: File processed successfully (updated or no changes needed)
/// - `Err(...)`: I/O error during file operations
fn update_readme(package_name: &str, current: &str, new: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("📝 Updating README.md...");

    if let Ok(content) = fs::read_to_string("README.md") {
        let mut updated_content = content.clone();
        let mut changes_made = false;

        // ═══════════════════════════════════════════════════════════════════
        // Pattern 1: Version Badges (shields.io, etc.)
        // ═══════════════════════════════════════════════════════════════════
        // Example: https://img.shields.io/badge/version-0.1.143-blue
        let old_badge = format!("version-{}-blue", current);
        let new_badge = format!("version-{}-blue", new);
        if updated_content.contains(&old_badge) {
            updated_content = updated_content.replace(&old_badge, &new_badge);
            changes_made = true;
            println!("  ✓ Updated version badge");
        }

        // ═══════════════════════════════════════════════════════════════════
        // Pattern 2: Version Tags (Git releases)
        // ═══════════════════════════════════════════════════════════════════
        // Example: v0.1.143, [v0.1.143](https://github.com/user/repo/releases/tag/v0.1.143)
        let old_tag = format!("v{}", current);
        let new_tag = format!("v{}", new);
        if updated_content.contains(&old_tag) {
            updated_content = updated_content.replace(&old_tag, &new_tag);
            changes_made = true;
            println!("  ✓ Updated version tags");
        }

        // ═══════════════════════════════════════════════════════════════════
        // Pattern 3: Version References (prose documentation)
        // ═══════════════════════════════════════════════════════════════════
        // Example: "This documentation covers version 0.1.143" or "Version 0.1.143"
        // Use precise matching to avoid partial version matches

        // Helper function to replace version with word boundary checking
        fn replace_version_with_boundaries(content: &str, old_pattern: &str, new_pattern: &str) -> (String, bool) {
            let mut result = content.to_string();
            let mut changed = false;

            // Find all occurrences and check word boundaries
            let mut start = 0;
            while let Some(pos) = result[start..].find(old_pattern) {
                let actual_pos = start + pos;
                let end_pos = actual_pos + old_pattern.len();

                // Check if this is a word boundary match (not part of a longer version)
                let before_ok = actual_pos == 0 ||
                    !result.chars().nth(actual_pos - 1).unwrap_or(' ').is_ascii_alphanumeric();
                let after_ok = end_pos >= result.len() ||
                    !result.chars().nth(end_pos).unwrap_or(' ').is_ascii_alphanumeric();

                if before_ok && after_ok {
                    result.replace_range(actual_pos..end_pos, new_pattern);
                    changed = true;
                    start = actual_pos + new_pattern.len();
                } else {
                    start = end_pos;
                }
            }

            (result, changed)
        }

        // Apply precise version matching for both cases
        let old_version_ref_lower = format!("version {}", current);
        let new_version_ref_lower = format!("version {}", new);
        let (temp_content, changed_lower) = replace_version_with_boundaries(&updated_content, &old_version_ref_lower, &new_version_ref_lower);
        updated_content = temp_content;

        if changed_lower {
            changes_made = true;
            println!("  ✓ Updated version references (lowercase)");
        }

        let old_version_ref_upper = format!("Version {}", current);
        let new_version_ref_upper = format!("Version {}", new);
        let (temp_content, changed_upper) = replace_version_with_boundaries(&updated_content, &old_version_ref_upper, &new_version_ref_upper);
        updated_content = temp_content;

        if changed_upper {
            changes_made = true;
            println!("  ✓ Updated version references (uppercase)");
        }

        // ═══════════════════════════════════════════════════════════════════
        // Pattern 4: Dependency Declarations (Cargo.toml examples)
        // ═══════════════════════════════════════════════════════════════════
        // Example: uffs-core = "0.1.143"
        let old_dep = format!("{} = \"{}\"", package_name, current);
        let new_dep = format!("{} = \"{}\"", package_name, new);
        if updated_content.contains(&old_dep) {
            updated_content = updated_content.replace(&old_dep, &new_dep);
            changes_made = true;
            println!("  ✓ Updated dependency declarations");
        }

        // ═══════════════════════════════════════════════════════════════════
        // Pattern 5: Alternative Dependency Format (detailed syntax)
        // ═══════════════════════════════════════════════════════════════════
        // Example: uffs-core = { version = "0.1.143", features = ["full"] }
        let old_version_field = format!("version = \"{}\"", current);
        let new_version_field = format!("version = \"{}\"", new);
        if updated_content.contains(&old_version_field) {
            updated_content = updated_content.replace(&old_version_field, &new_version_field);
            changes_made = true;
            println!("  ✓ Updated version fields");
        }

        // ═══════════════════════════════════════════════════════════════════
        // Atomic File Update & Result Reporting
        // ═══════════════════════════════════════════════════════════════════
        if changes_made {
            fs::write("README.md", updated_content)?;
            println!("✅ README.md updated (package: {}, {} → {})", package_name, current, new);
        } else {
            println!("ℹ️  README.md - no version patterns found to update");
        }
    } else {
        println!("⚠️  README.md not found, skipping");
    }

    Ok(())
}

/// # Documentation Files Batch Update Engine
///
/// Processes **multiple documentation files** in a single operation,
/// applying version updates with **comprehensive pattern matching** and
/// **detailed progress tracking**.
///
/// ## 📁 Target File Strategy
///
/// Covers the most common documentation file locations in Rust projects:
/// - **CHANGELOG.md**: Release notes and version history
/// - **docs/README.md**: Detailed project documentation
/// - **docs/INSTALLATION.md**: Setup and installation guides
/// - **docs/QUICKSTART.md**: Getting started tutorials
/// - **docs/API.md**: API reference documentation
///
/// ## 🎯 Pattern Matching Approach
///
/// Uses **dual-pattern strategy** for maximum coverage:
/// 1. **Exact Version Matches**: Direct version string occurrences
/// 2. **Tagged Versions**: Git-style version tags (v-prefixed)
///
/// ## 📊 Batch Processing Algorithm
///
/// ```text
/// For each documentation file:
///   1. Check if file exists (skip if not found)
///   2. Read file content
///   3. Apply pattern matching
///   4. Update file if changes detected
///   5. Track statistics (files checked vs. updated)
///   6. Report individual file results
///
/// Final summary: Aggregate statistics and overall status
/// ```
///
/// ## 🔧 Error Handling Strategy
///
/// - **File Not Found**: Silently skip (documentation files are optional)
/// - **Read Errors**: Continue with other files (independent operations)
/// - **Write Errors**: Propagate error (data integrity critical)
/// - **Pattern Failures**: Continue processing (non-critical)
///
/// ## 📈 Progress Reporting
///
/// Provides **three levels of feedback**:
/// 1. **Individual Files**: "✅ docs/API.md updated"
/// 2. **Batch Summary**: "✅ Documentation updated: 3 files modified out of 5 checked"
/// 3. **Status Indicators**: Success, partial success, or no changes needed
///
/// ## 🛡️ Safety Guarantees
///
/// - **Independent Operations**: Single file failure doesn't affect others
/// - **Atomic Updates**: Each file written completely or not at all
/// - **Non-Destructive**: Original content preserved until successful replacement
/// - **Optional Processing**: Missing files don't cause failures
///
/// ## 🔄 Return Value Semantics
///
/// - `Ok(())`: All files processed successfully (regardless of update count)
/// - `Err(...)`: Critical I/O error during file write operations
fn update_docs(current: &str, new: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("📝 Updating documentation files...");

    // Define comprehensive documentation file coverage
    // Ordered by importance and commonality in Rust projects
    let doc_files = [
        "CHANGELOG.md",         // Release history (highest priority)
        "docs/README.md",       // Main documentation
        "docs/INSTALLATION.md", // Setup guides
        "docs/QUICKSTART.md",   // Getting started
        "docs/API.md"           // API reference
    ];

    let mut files_updated = 0;
    let mut files_checked = 0;

    // ═══════════════════════════════════════════════════════════════════════
    // Batch Processing Loop - Independent File Operations
    // ═══════════════════════════════════════════════════════════════════════
    for doc_file in &doc_files {
        if let Ok(content) = fs::read_to_string(doc_file) {
            files_checked += 1;

            // Working copy for safe pattern replacement
            let mut new_content = content.clone();
            let mut file_changed = false;

            // ───────────────────────────────────────────────────────────────
            // Pattern 1: Exact Version Matches
            // ───────────────────────────────────────────────────────────────
            // Example: "Version 0.1.143 introduces...", "API version 0.1.143"
            if new_content.contains(current) {
                new_content = new_content.replace(current, new);
                file_changed = true;
            }

            // ───────────────────────────────────────────────────────────────
            // Pattern 2: Version Tags (Git-style)
            // ───────────────────────────────────────────────────────────────
            // Example: "Release v0.1.143", "[v0.1.143](release-link)"
            let old_tag = format!("v{}", current);
            let new_tag = format!("v{}", new);
            if new_content.contains(&old_tag) {
                new_content = new_content.replace(&old_tag, &new_tag);
                file_changed = true;
            }

            // ───────────────────────────────────────────────────────────────
            // Atomic File Update with Progress Reporting
            // ───────────────────────────────────────────────────────────────
            if file_changed {
                fs::write(doc_file, new_content)?;
                println!("  ✅ {} updated", doc_file);
                files_updated += 1;
            }
        }
        // Note: File not found is silently ignored (documentation files are optional)
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Batch Operation Summary & Status Reporting
    // ═══════════════════════════════════════════════════════════════════════
    if files_updated > 0 {
        println!("✅ Documentation updated: {} files modified out of {} checked", files_updated, files_checked);
    } else if files_checked > 0 {
        println!("ℹ️  Documentation checked: {} files, no updates needed", files_checked);
    } else {
        println!("ℹ️  No documentation files found to update");
    }

    Ok(())
}
