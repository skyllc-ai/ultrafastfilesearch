// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Binary resolution cascades for the tools the bench suite invokes.
//!
//! Each function searches for its target executable in a fixed-priority order
//! and returns the first one that exists (or a bare PATH-fallback name if
//! none is found). All I/O flows through the [`Host`] seam so the logic is
//! fully testable under `MockHost`.

use std::path::PathBuf;

use crate::host::Host;
use crate::preflight::PatternProbe;

/// Resolve the `uffs.exe` binary using the same cascade as the validation
/// scripts: `$USERPROFILE\bin\uffs.exe` → `target\release\uffs.exe` → bare
/// `uffs.exe` (PATH fallback).
///
/// The `target\release` step is intentionally **included** here because it is
/// the Rust artefact; it is omitted for the C++ reference binary.
pub(crate) fn uffs_exe(host: &dyn Host) -> String {
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let bin_name = if cfg!(windows) { "uffs.exe" } else { "uffs" };
    let home = host.env(home_var).unwrap_or_default();
    let candidates = [
        PathBuf::from(&home).join("bin").join(bin_name),
        PathBuf::from("target").join("release").join(bin_name),
    ];
    for candidate in &candidates {
        if host.path_exists(candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    bin_name.to_owned()
}

/// Resolve the `uffs-mft` diagnostic binary (sibling of `uffs`), used to
/// capture the storage-device inventory (`drives --format json`) for the
/// report.
///
/// Same cascade as [`uffs_exe`]: `~/bin/uffs-mft[.exe]` then
/// `target/release/uffs-mft[.exe]`, falling back to the bare name.
pub(crate) fn uffs_mft_exe(host: &dyn Host) -> String {
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let bin_name = if cfg!(windows) {
        "uffs-mft.exe"
    } else {
        "uffs-mft"
    };
    let home = host.env(home_var).unwrap_or_default();
    let candidates = [
        PathBuf::from(&home).join("bin").join(bin_name),
        PathBuf::from("target").join("release").join(bin_name),
    ];
    for candidate in &candidates {
        if host.path_exists(candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    bin_name.to_owned()
}

/// Resolve the `uffs.com` (C++ reference) binary.
///
/// Same cascade as `uffs.exe` minus the `target/release` Rust step — the C++
/// binary is never produced by `cargo build`:
///   1. `$USERPROFILE\bin\uffs.com` — `just use` / manual install
///   2. bare `uffs.com` (PATH fallback)
pub(crate) fn uffs_cpp_exe(host: &dyn Host) -> String {
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let bin_name = "uffs.com";
    let home = host.env(home_var).unwrap_or_default();
    let candidate = PathBuf::from(&home).join("bin").join(bin_name);
    if host.path_exists(&candidate) {
        return candidate.to_string_lossy().into_owned();
    }
    bin_name.to_owned()
}

/// Resolve the `Everything.exe` GUI binary to its absolute path.
///
/// Uses disk/PATH lookups only — no execution.
///
/// Search order:
///   1. `where.exe Everything.exe` / `which Everything` — first PATH hit.
///   2. `%ProgramFiles(x86)%\Everything\Everything.exe` — default installer.
///   3. `%ProgramFiles%\Everything\Everything.exe` — 64-bit install.
///   4. bare `Everything.exe` fallback.
pub(crate) fn everything_exe(host: &dyn Host) -> String {
    let bin_name = if cfg!(windows) {
        "Everything.exe"
    } else {
        "Everything"
    };

    // 1. Ask the OS where the binary lives on PATH.
    let where_cmd = if cfg!(windows) { "where.exe" } else { "which" };
    if let Ok(out) = host.run(where_cmd, &[bin_name]) {
        let first = out.stdout.lines().next().unwrap_or("").trim().to_owned();
        if !first.is_empty() {
            return first;
        }
    }

    // 2-3. Known installer locations.
    for env_var in &["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(pf) = host.env(env_var) {
            let candidate = PathBuf::from(&pf).join("Everything").join(bin_name);
            if host.path_exists(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    // 4. Bare fallback.
    bin_name.to_owned()
}

/// Resolve the `es.exe` binary (Everything CLI) to its absolute path.
///
/// The binary exits 0 even when the Everything daemon is not running, so
/// execution-based probes cannot distinguish "found" from "not found".
/// This function uses disk/PATH lookups only — no execution.
///
/// Search order:
///   1. `where.exe es.exe` (Windows) / `which es` (Unix) — first PATH hit,
///      returns the full absolute path.
///   2. `%USERPROFILE%\bin\es.exe` — `just use` / manual install location.
///   3. `%ProgramFiles%\Everything\es.exe` — default installer location.
///   4. `%ProgramFiles(x86)%\Everything\es.exe` — 32-bit installer on 64-bit.
///   5. bare `es.exe` fallback (OS will error clearly if truly absent).
pub(crate) fn es_exe(host: &dyn Host) -> String {
    let bin_name = if cfg!(windows) { "es.exe" } else { "es" };

    // 1. Ask the shell where the binary lives on PATH (gives full absolute path).
    let where_cmd = if cfg!(windows) { "where.exe" } else { "which" };
    if let Ok(out) = host.run(where_cmd, &[bin_name]) {
        let first = out.stdout.lines().next().unwrap_or("").trim().to_owned();
        if !first.is_empty() {
            return first;
        }
    }

    // 2. ~/bin install location.
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(home) = host.env(home_var) {
        let candidate = PathBuf::from(&home).join("bin").join(bin_name);
        if host.path_exists(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }

    // 3-4. Known Windows installer locations.
    for env_var in &["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = host.env(env_var) {
            let candidate = PathBuf::from(&pf).join("Everything").join(bin_name);
            if host.path_exists(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    // 5. Bare fallback — OS will surface a clear error if truly absent.
    bin_name.to_owned()
}

/// Default measurement patterns: display name + UFFS row-count argument
/// template (`{DRIVE}` is substituted per drive during preflight).
///
/// Correct CLI form: `uffs.exe <pattern> --drives <DRIVE> --count`
pub(crate) const DEFAULT_PATTERNS: [(&str, &[&str]); 2] = [
    ("all_dlls", &["*.dll", "--drives", "{DRIVE}", "--count"]),
    ("full_scan", &["*", "--drives", "{DRIVE}", "--count"]),
];

/// Build the default measurement [`PatternProbe`] set (shared by preflight and
/// the native Stage 3 timing).
pub(crate) fn default_pattern_probes() -> Vec<PatternProbe> {
    DEFAULT_PATTERNS
        .iter()
        .map(|(name, args)| PatternProbe {
            name: (*name).to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        })
        .collect()
}
