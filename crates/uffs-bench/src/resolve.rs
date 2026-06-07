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

/// Resolve the `es.exe` binary (Everything CLI).
///
/// Search order:
///   1. `es.exe` reachable via PATH (already-installed or bundle-staged)
///   2. `%ProgramFiles%\Everything\es.exe` — default installer location
///   3. `%ProgramFiles(x86)%\Everything\es.exe` — 32-bit installer on 64-bit
///   4. bare `es.exe` (PATH fallback — lets the OS surface a clear error)
pub(crate) fn es_exe(host: &dyn Host) -> String {
    let bin_name = "es.exe";
    if host.run(bin_name, &["-get-everything-version"]).is_ok() {
        return bin_name.to_owned();
    }
    for env_var in &["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = host.env(env_var) {
            let candidate = PathBuf::from(&pf).join("Everything").join(bin_name);
            if host.path_exists(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    bin_name.to_owned()
}

/// Default measurement patterns: display name + UFFS row-count argument
/// template (`{DRIVE}` is substituted per drive during preflight).
pub(crate) const DEFAULT_PATTERNS: [(&str, &[&str]); 2] = [
    ("all_dlls", &["{DRIVE}:\\", "*.dll", "--count"]),
    ("full_scan", &["{DRIVE}:\\", "*", "--count"]),
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
