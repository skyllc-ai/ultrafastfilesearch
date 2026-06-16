#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! Startup Profiler — Phase 2 measurement toolkit for UFFS CLI performance.
//!
//! Measures process startup overhead, IPC transport costs, output costs,
//! and establishes the true Windows process creation floor for our toolchain.
//!
//! # Measurement Modes
//!
//!   `--mode null-matrix`   Build & measure null-binary variants (requires cargo)
//!   `--mode startup`       Measure uffs.exe startup breakdown
//!   `--mode output`        Measure output cost (console vs NUL vs file)
//!   `--mode ipc`           Measure IPC transport round-trip
//!   `--mode all`           Run all measurements (default)
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\startup-profiler.rs
//! rust-script scripts\windows\startup-profiler.rs --mode null-matrix
//! rust-script scripts\windows\startup-profiler.rs --mode startup --rounds 50
//! rust-script scripts\windows\startup-profiler.rs --uffs-bin C:\tools\uffs.exe
//! rust-script scripts\windows\startup-profiler.rs --mode output --drive D
//! ```
//!
//! ```cargo
//! [dependencies]
//! ```
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_ROUNDS: usize = 30;
const TIMEOUT: Duration = Duration::from_secs(30);
const WARMUP_ROUNDS: usize = 3;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { NullMatrix, Startup, Output, Ipc, All }

struct Cfg {
    mode: Mode,
    rounds: usize,
    uffs_bin: PathBuf,
    drive: String,
    build_dir: PathBuf,
    es_bin: Option<PathBuf>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn flush() { std::io::stderr().flush().ok(); std::io::stdout().flush().ok(); }

fn p50(vals: &[f64]) -> f64 {
    if vals.is_empty() { return 0.0; }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

fn p95(vals: &[f64]) -> f64 {
    if vals.is_empty() { return 0.0; }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[(sorted.len() as f64 * 0.95) as usize % sorted.len()]
}

fn p_min(vals: &[f64]) -> f64 {
    vals.iter().copied().fold(f64::INFINITY, f64::min)
}

fn fmt_ms(ms: f64) -> String {
    if ms >= 1000.0 { format!("{:.1}s", ms / 1000.0) }
    else { format!("{:.1} ms", ms) }
}

fn fmt_size(bytes: u64) -> String {
    if bytes >= 1_048_576 { format!("{:.1} MB", bytes as f64 / 1_048_576.0) }
    else if bytes >= 1024 { format!("{:.0} KB", bytes as f64 / 1024.0) }
    else { format!("{} B", bytes) }
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Time a single process execution, returning wall-clock milliseconds.
/// Returns None if the process fails or times out.
fn time_process(bin: &Path, args: &[&str], stdout_target: Stdio) -> Option<f64> {
    let start = Instant::now();
    let result = Command::new(bin)
        .args(args)
        .stdout(stdout_target)
        .stderr(Stdio::null())
        .output();
    match result {
        Ok(output) => {
            let elapsed = start.elapsed();
            if elapsed > TIMEOUT {
                eprintln!("  ✗ Timed out: {}", bin.display());
                return None;
            }
            let ms = elapsed.as_secs_f64() * 1000.0;
            if output.status.success() { Some(ms) } else { None }
        }
        Err(e) => {
            eprintln!("  ✗ Failed to run {}: {}", bin.display(), e);
            None
        }
    }
}

/// Measure a binary N times, with warmup, returning sorted wall-clock times.
fn measure_binary(bin: &Path, args: &[&str], rounds: usize, label: &str,
                  stdout_target: impl Fn() -> Stdio) -> Vec<f64> {
    eprint!("  {:<40} ", label);
    flush();
    // Warmup
    for _ in 0..WARMUP_ROUNDS {
        time_process(bin, args, stdout_target());
    }
    let mut times = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        if let Some(ms) = time_process(bin, args, stdout_target()) {
            times.push(ms);
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if times.is_empty() {
        eprintln!("FAILED (no successful runs)");
    } else {
        eprintln!("p50={:>8}  p95={:>8}  min={:>8}  n={}",
            fmt_ms(p50(&times)), fmt_ms(p95(&times)), fmt_ms(p_min(&times)), times.len());
    }
    times
}

// ── Null-binary source generators ────────────────────────────────────────────

/// Rust source for a minimal null binary (just exits).
const NULL_RUST_SRC: &str = r#"fn main() {}"#;

/// Rust source that opens a TCP socket (measures Winsock/ws2_32 import cost on Windows).
/// std does not expose AF_UNIX on stable Windows; TCP connect pulls in the same
/// Winsock DLLs so this is a faithful proxy for AF_UNIX import overhead.
const NULL_RUST_SOCKET_SRC: &str = r#"use std::net::TcpStream;
use std::time::Duration;
fn main() {
    // Connect to a port that is almost certainly closed — we only care about
    // the cost of loading ws2_32.dll and initializing Winsock, not the result.
    let _ = TcpStream::connect_timeout(&"127.0.0.1:1".parse().unwrap(), Duration::from_millis(50));
}
"#;

/// Rust source that opens a named pipe (measures kernel32-only path).
const NULL_RUST_PIPE_SRC: &str = r#"
use std::fs::OpenOptions;
fn main() {
    // Named pipe open — kernel32 only, no Winsock
    let _ = OpenOptions::new().read(true).write(true)
        .open(r"\\.\pipe\uffs-null-test");
}
"#;

/// Rust source that parses a small JSON blob (measures serde_json cost).
/// NOTE: requires serde_json dependency in generated Cargo.toml.
#[allow(dead_code)]
const NULL_RUST_JSON_SRC: &str =
"fn main() {\n\
    let json = \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"result\\\":{\\\"rows\\\":[]},\\\"id\\\":1}\";\n\
    let _: serde_json::Value = serde_json::from_str(json).unwrap();\n\
}\n";

/// Null binary variant definitions.
struct NullVariant {
    name: &'static str,
    source: &'static str,
    deps: &'static str,        // Cargo.toml [dependencies] section
    crt_static: bool,
    description: &'static str,
}

const NULL_VARIANTS: &[NullVariant] = &[
    NullVariant {
        name: "null-rust",
        source: NULL_RUST_SRC,
        deps: "",
        crt_static: false,
        description: "Minimal fn main(){} — Rust runtime floor",
    },
    NullVariant {
        name: "null-rust-crt-static",
        source: NULL_RUST_SRC,
        deps: "",
        crt_static: true,
        description: "Static CRT — fewer DLLs, larger binary",
    },
    NullVariant {
        name: "null-rust-pipe",
        source: NULL_RUST_PIPE_SRC,
        deps: "",
        crt_static: false,
        description: "Named pipe open — kernel32-only IPC path",
    },
    NullVariant {
        name: "null-rust-socket",
        source: NULL_RUST_SOCKET_SRC,
        deps: "",
        crt_static: false,
        description: "TCP connect — Winsock/ws2_32 import cost",
    },
    NullVariant {
        name: "null-rust-json",
        source: NULL_RUST_JSON_SRC,
        deps: "serde_json = \"1\"",
        crt_static: false,
        description: "serde_json parse — serialization cost",
    },
];

/// Build a null-binary variant, returning the path to the built executable.
fn build_null_variant(variant: &NullVariant, build_dir: &Path) -> Option<PathBuf> {
    let proj_dir = build_dir.join(variant.name);
    let src_dir = proj_dir.join("src");
    let _ = fs::create_dir_all(&src_dir);

    // Write main.rs
    let main_path = src_dir.join("main.rs");
    if let Err(e) = fs::write(&main_path, variant.source) {
        eprintln!("  ✗ Failed to write {}: {}", main_path.display(), e);
        return None;
    }

    // Write Cargo.toml
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
{deps}

[profile.release]
opt-level = "z"
lto = "thin"
codegen-units = 1
panic = "abort"
strip = "symbols"
"#,
        name = variant.name,
        deps = variant.deps,
    );
    if let Err(e) = fs::write(proj_dir.join("Cargo.toml"), &cargo_toml) {
        eprintln!("  ✗ Failed to write Cargo.toml: {}", e);
        return None;
    }

    // Write .cargo/config.toml for crt-static if needed
    if variant.crt_static {
        let cargo_dir = proj_dir.join(".cargo");
        let _ = fs::create_dir_all(&cargo_dir);
        let config = r#"[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
"#;
        let _ = fs::write(cargo_dir.join("config.toml"), config);
    }

    // Build — force a project-local target dir, ignoring any inherited
    // CARGO_TARGET_DIR / config that points elsewhere (which breaks our
    // exe-path assumption and can cause cross-project build-script lock
    // collisions on shared target directories).
    //
    // Retry up to 3x on transient "Access is denied (os error 5)" link-copy
    // failures caused by Windows Defender briefly locking freshly-written
    // build-script binaries during cargo's rename step.
    let local_target = proj_dir.join("target");
    eprint!("  Building {:<35} ", variant.name);
    flush();
    let mut output = None;
    for attempt in 1..=3u32 {
        // `-j 1` serializes build-script link/rename steps, avoiding the
        // Windows "file briefly locked between write and rename" race that
        // produces "Access is denied (os error 5)" on cargo's link-or-copy
        // step. The tiny null-binary builds don't benefit from parallelism
        // anyway.
        let out = Command::new("cargo")
            .args(["build", "--release", "-j", "1"])
            .current_dir(&proj_dir)
            .env("CARGO_TARGET_DIR", &local_target)
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        let retry = match &out {
            Ok(o) if !o.status.success() => {
                let err = String::from_utf8_lossy(&o.stderr);
                err.contains("Access is denied") || err.contains("os error 5")
            }
            _ => false,
        };
        output = Some(out);
        if !retry { break; }
        eprint!("(retry {}) ", attempt);
        flush();
        std::thread::sleep(Duration::from_millis(500));
    }
    let output = output.unwrap();

    match output {
        Ok(o) if o.status.success() => {
            let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
            let exe_name = format!("{}{}", variant.name, exe_suffix);
            let exe_path = local_target.join("release").join(&exe_name);
            if exe_path.exists() {
                eprintln!("OK  ({})", fmt_size(file_size(&exe_path)));
                Some(exe_path)
            } else {
                eprintln!("BUILT but exe not found at {}", exe_path.display());
                None
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!("FAILED");
            // Show last 5 lines of error
            for line in stderr.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
                eprintln!("    {}", line);
            }
            None
        }
        Err(e) => {
            eprintln!("FAILED (cargo not found? {})", e);
            None
        }
    }
}

// ── Mode: Null-binary matrix ─────────────────────────────────────────────────

fn run_null_matrix(cfg: &Cfg) {
    println!("\n{}", "=".repeat(72));
    println!("  NULL-BINARY MATRIX — Process Creation Floor");
    println!("  Rounds: {} (+ {} warmup)  |  Build profile: release opt-z lto-thin",
        cfg.rounds, WARMUP_ROUNDS);
    println!("{}\n", "=".repeat(72));

    let mut results: Vec<(String, String, u64, f64, f64, f64)> = Vec::new(); // name, desc, size, p50, p95, min

    // Build and measure null variants
    for variant in NULL_VARIANTS {
        eprintln!("\n── {} ──", variant.name);
        eprintln!("  {}", variant.description);

        if let Some(exe_path) = build_null_variant(variant, &cfg.build_dir) {
            let size = file_size(&exe_path);
            let times = measure_binary(&exe_path, &[], cfg.rounds, variant.name, || Stdio::null());
            if !times.is_empty() {
                results.push((
                    variant.name.to_string(),
                    variant.description.to_string(),
                    size, p50(&times), p95(&times), p_min(&times),
                ));
            }
        }
    }

    // Measure system binaries as floor references
    eprintln!("\n── System binary references ──");
    let system_bins: &[(&str, &[&str])] = &[
        ("cmd.exe /c exit", &["/c", "exit"]),
        ("where.exe /?", &["/?"]),
    ];
    for (label, args) in system_bins {
        let bin_name = label.split_whitespace().next().unwrap_or("unknown");
        if let Some(bin_path) = which_bin(bin_name) {
            let size = file_size(&bin_path);
            let times = measure_binary(&bin_path, args, cfg.rounds, label, || Stdio::null());
            if !times.is_empty() {
                results.push((
                    label.to_string(), "System binary reference".to_string(),
                    size, p50(&times), p95(&times), p_min(&times),
                ));
            }
        }
    }

    // Measure uffs.exe
    eprintln!("\n── uffs.exe (current thin client) ──");
    if cfg.uffs_bin.exists() {
        let size = file_size(&cfg.uffs_bin);
        // uffs version (should be fast, just prints and exits)
        let times = measure_binary(&cfg.uffs_bin, &["version"], cfg.rounds,
            "uffs version", || Stdio::null());
        if !times.is_empty() {
            results.push((
                "uffs.exe version".to_string(), "Thin client — version only".to_string(),
                size, p50(&times), p95(&times), p_min(&times),
            ));
        }
        // uffs --search (hot, tiny result, to NUL)
        let drive_arg = format!("--drive={}", cfg.drive);
        let times = measure_binary(&cfg.uffs_bin,
            &["notepad.exe", &drive_arg, "--columns", "Path"],
            cfg.rounds, "uffs notepad.exe → NUL", || Stdio::null());
        if !times.is_empty() {
            results.push((
                "uffs.exe search → NUL".to_string(), "Thin client search, stdout to NUL".to_string(),
                size, p50(&times), p95(&times), p_min(&times),
            ));
        }
    } else {
        eprintln!("  ⚠ uffs.exe not found at {}", cfg.uffs_bin.display());
    }

    // Measure es.exe if available
    if let Some(ref es) = cfg.es_bin {
        eprintln!("\n── es.exe (Everything) ──");
        let size = file_size(es);
        let times = measure_binary(es, &["/?"], cfg.rounds,
            "es.exe /?", || Stdio::null());
        if !times.is_empty() {
            results.push((
                "es.exe /?".to_string(), "Everything CLI — help".to_string(),
                size, p50(&times), p95(&times), p_min(&times),
            ));
        }
    }

    // Print summary table
    println!("\n{}", "─".repeat(100));
    println!("{:<35} {:>10} {:>10} {:>10} {:>10}", "Variant", "Size", "p50", "p95", "min");
    println!("{}", "─".repeat(100));
    for (name, _desc, size, p50v, p95v, minv) in &results {
        println!("{:<35} {:>10} {:>10} {:>10} {:>10}",
            name, fmt_size(*size), fmt_ms(*p50v), fmt_ms(*p95v), fmt_ms(*minv));
    }
    println!("{}", "─".repeat(100));
}

fn which_bin(name: &str) -> Option<PathBuf> {
    // On Windows, use `where`; on other platforms, use `which`
    let cmd = if cfg!(windows) { "where" } else { "which" };
    Command::new(cmd).arg(name).output().ok().and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout);
        let l = s.lines().next().unwrap_or("").trim();
        if !l.is_empty() && Path::new(l).exists() { Some(PathBuf::from(l)) } else { None }
    })
}

/// Pick a build directory that is *not* inside `%TEMP%` on Windows.
/// `%TEMP%` (under `AppData\Local\Temp`) is aggressively scanned by
/// SmartScreen and the Windows Search Indexer, which briefly locks newly
/// written build-script binaries and causes cargo's link-or-copy step to
/// fail with `Access is denied (os error 5)` even when all AV is disabled.
fn pick_build_dir() -> PathBuf {
    const DIR_NAME: &str = "uffs-startup-profiler";
    if cfg!(windows) {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join(DIR_NAME);
        }
        if let Some(home) = env::var_os("USERPROFILE") {
            return PathBuf::from(home).join("AppData").join("Local").join(DIR_NAME);
        }
    } else {
        if let Some(cache) = env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(cache).join(DIR_NAME);
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(".cache").join(DIR_NAME);
        }
    }
    env::temp_dir().join(DIR_NAME)
}

/// Locate a binary by stem name (e.g. "uffs", "es"), trying in order:
///   1. PATH lookup (`where`/`which`)
///   2. `~/bin/<name>[.exe]` — the user keeps all tools here on Windows
///   3. Repo-local `target/release/<name>[.exe]` and `target/debug/<name>[.exe]`
fn find_bin(stem: &str) -> Option<PathBuf> {
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let with_ext = format!("{}{}", stem, exe_suffix);

    // 1. PATH
    if let Some(p) = which_bin(&with_ext) { return Some(p); }
    if let Some(p) = which_bin(stem) { return Some(p); }

    // 2. ~/bin/<name>[.exe]
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from);
    if let Some(h) = home {
        let candidate = h.join("bin").join(&with_ext);
        if candidate.exists() { return Some(candidate); }
    }

    // 3. Repo-local target/<profile>/<name>[.exe]
    if let Ok(cwd) = env::current_dir() {
        for profile in ["release", "debug"] {
            let candidate = cwd.join("target").join(profile).join(&with_ext);
            if candidate.exists() { return Some(candidate); }
        }
    }

    None
}

// ── Mode: Startup decomposition ──────────────────────────────────────────────

fn run_startup(cfg: &Cfg) {
    println!("\n{}", "=".repeat(72));
    println!("  STARTUP DECOMPOSITION — uffs.exe timing breakdown");
    println!("  Rounds: {} | Binary: {} ({})",
        cfg.rounds, cfg.uffs_bin.display(), fmt_size(file_size(&cfg.uffs_bin)));
    println!("{}\n", "=".repeat(72));

    if !cfg.uffs_bin.exists() {
        eprintln!("  ✗ uffs.exe not found at {}", cfg.uffs_bin.display());
        return;
    }

    let drive_arg = format!("--drive={}", cfg.drive);

    // 1. Version only (minimal work — measures process load + arg parse)
    let t_version = measure_binary(&cfg.uffs_bin, &["version"], cfg.rounds,
        "uffs version (process load + args)", || Stdio::null());

    // 2. Status (connects to daemon, no search)
    let t_status = measure_binary(&cfg.uffs_bin, &["status"], cfg.rounds,
        "uffs --status (+ daemon connect)", || Stdio::null());

    // 3. Tiny search → NUL (connect + search + serialize, no output)
    let t_search_nul = measure_binary(&cfg.uffs_bin,
        &["notepad.exe", &drive_arg, "--columns", "Path"],
        cfg.rounds, "uffs --search tiny → NUL", || Stdio::null());

    // 4. Tiny search → stdout (adds console output cost)
    let t_search_stdout = measure_binary(&cfg.uffs_bin,
        &["notepad.exe", &drive_arg, "--columns", "Path"],
        cfg.rounds, "uffs --search tiny → stdout", || Stdio::piped());

    // 5. Tiny search → --out file (daemon-direct file write)
    let out_file = cfg.build_dir.join("startup_bench_out.csv");
    let out_arg = format!("--out={}", out_file.display());
    let t_search_file = measure_binary(&cfg.uffs_bin,
        &["notepad.exe", &drive_arg, "--columns", "Path", &out_arg],
        cfg.rounds, "uffs --search tiny → --out file", || Stdio::null());
    let _ = fs::remove_file(&out_file);

    // 6. Medium search → NUL (more rows, IPC transfer cost)
    let _t_medium_nul = measure_binary(&cfg.uffs_bin,
        &["*.dll", &drive_arg, "--columns", "Path"],
        cfg.rounds, "uffs *.dll → NUL (medium result)", || Stdio::null());

    // Decomposition analysis
    println!("\n{}", "─".repeat(72));
    println!("  STARTUP DECOMPOSITION ANALYSIS");
    println!("{}", "─".repeat(72));

    let v_p50 = p50(&t_version);
    let s_p50 = p50(&t_status);
    let sn_p50 = p50(&t_search_nul);
    let ss_p50 = p50(&t_search_stdout);
    let sf_p50 = p50(&t_search_file);

    println!("  Process load + args:          {:>8} (version p50)", fmt_ms(v_p50));
    println!("  + daemon connect:             {:>8} (status - version)", fmt_ms(s_p50 - v_p50));
    println!("  + search + IPC:               {:>8} (search→NUL - status)", fmt_ms(sn_p50 - s_p50));
    println!("  + console output:             {:>8} (search→stdout - search→NUL)", fmt_ms(ss_p50 - sn_p50));
    println!("  + file write (--out):         {:>8} (search→file - search→NUL)", fmt_ms(sf_p50 - sn_p50));
    println!("{}", "─".repeat(72));
}

// ── Mode: Output cost isolation ──────────────────────────────────────────────

fn run_output(cfg: &Cfg) {
    println!("\n{}", "=".repeat(72));
    println!("  OUTPUT COST ISOLATION — Console vs NUL vs File");
    println!("  Rounds: {} | Drive: {}", cfg.rounds, cfg.drive);
    println!("{}\n", "=".repeat(72));

    if !cfg.uffs_bin.exists() {
        eprintln!("  ✗ uffs.exe not found at {}", cfg.uffs_bin.display());
        return;
    }

    let drive_arg = format!("--drive={}", cfg.drive);
    let out_file = cfg.build_dir.join("output_bench_out.csv");
    let out_arg = format!("--out={}", out_file.display());

    struct OutputTest {
        label: &'static str,
        pattern: &'static str,
        extra_args: Vec<String>,
        to_null: bool,
        description: &'static str,
    }

    let tests = vec![
        // Tiny results
        OutputTest { label: "tiny → NUL", pattern: "notepad.exe",
            extra_args: vec!["--columns".into(), "Path".into()],
            to_null: true, description: "3 rows, no output" },
        OutputTest { label: "tiny → stdout", pattern: "notepad.exe",
            extra_args: vec!["--columns".into(), "Path".into()],
            to_null: false, description: "3 rows, console output" },
        OutputTest { label: "tiny → --out", pattern: "notepad.exe",
            extra_args: vec!["--columns".into(), "Path".into(), out_arg.clone()],
            to_null: true, description: "3 rows, daemon-direct file" },
        // Medium results (*.dll)
        OutputTest { label: "medium → NUL", pattern: "*.dll",
            extra_args: vec!["--columns".into(), "Path".into()],
            to_null: true, description: "~45K rows, no output" },
        OutputTest { label: "medium → stdout", pattern: "*.dll",
            extra_args: vec!["--columns".into(), "Path".into()],
            to_null: false, description: "~45K rows, console output" },
        OutputTest { label: "medium → --out", pattern: "*.dll",
            extra_args: vec!["--columns".into(), "Path".into(), out_arg.clone()],
            to_null: true, description: "~45K rows, daemon-direct file" },
        // Full columns (measures column formatting cost)
        OutputTest { label: "tiny full-cols → NUL", pattern: "notepad.exe",
            extra_args: vec![],
            to_null: true, description: "3 rows, all columns, no output" },
        OutputTest { label: "tiny full-cols → stdout", pattern: "notepad.exe",
            extra_args: vec![],
            to_null: false, description: "3 rows, all columns, console" },
    ];

    let mut results: Vec<(String, String, f64, f64)> = Vec::new();
    for test in &tests {
        let mut args: Vec<&str> = vec![test.pattern, &drive_arg];
        for a in &test.extra_args { args.push(a); }
        let stdout_fn = if test.to_null {
            || Stdio::null()
        } else {
            || Stdio::piped()
        };
        let label = format!("{} ({})", test.label, test.description);
        let times = measure_binary(&cfg.uffs_bin, &args, cfg.rounds, &label, stdout_fn);
        let _ = fs::remove_file(&out_file);
        if !times.is_empty() {
            results.push((test.label.to_string(), test.description.to_string(),
                p50(&times), p95(&times)));
        }
    }

    println!("\n{}", "─".repeat(80));
    println!("{:<30} {:>12} {:>10} {:>10}", "Scenario", "Description", "p50", "p95");
    println!("{}", "─".repeat(80));
    for (label, desc, p50v, p95v) in &results {
        println!("{:<30} {:>12} {:>10} {:>10}", label, desc, fmt_ms(*p50v), fmt_ms(*p95v));
    }
    println!("{}", "─".repeat(80));
}

// ── Mode: IPC transport comparison ───────────────────────────────────────────

fn run_ipc(cfg: &Cfg) {
    println!("\n{}", "=".repeat(72));
    println!("  IPC TRANSPORT COMPARISON");
    println!("  Rounds: {} | Drive: {}", cfg.rounds, cfg.drive);
    println!("{}\n", "=".repeat(72));

    if !cfg.uffs_bin.exists() {
        eprintln!("  ✗ uffs.exe not found at {}", cfg.uffs_bin.display());
        return;
    }

    let drive_arg = format!("--drive={}", cfg.drive);

    // Current IPC path: uffs --search with --profile (shows IPC breakdown)
    eprintln!("── Current IPC (AF_UNIX socket + JSON-RPC) ──\n");

    // Tiny result — IPC overhead dominates
    let t_tiny = measure_binary(&cfg.uffs_bin,
        &["notepad.exe", &drive_arg, "--columns", "Path", "--profile"],
        cfg.rounds, "IPC tiny (3 rows)", || Stdio::null());

    // Small result
    let t_small = measure_binary(&cfg.uffs_bin,
        &["win*", &drive_arg, "--columns", "Path"],
        cfg.rounds, "IPC small (~37K rows)", || Stdio::null());

    // Medium result
    let t_medium = measure_binary(&cfg.uffs_bin,
        &["*.dll", &drive_arg, "--columns", "Path"],
        cfg.rounds, "IPC medium (~45K rows)", || Stdio::null());

    // Large result — may cross shmem threshold
    let t_large = measure_binary(&cfg.uffs_bin,
        &["*.dll", "--columns", "Path"],
        cfg.rounds, "IPC large (all drives)", || Stdio::null());

    println!("\n  NOTE: Named pipe transport comparison requires daemon-side support.");
    println!("  This section measures the current AF_UNIX path to establish baseline.");
    println!("  After named pipe support is added, re-run to compare.\n");

    println!("{}", "─".repeat(72));
    println!("{:<35} {:>10} {:>10}", "Query", "p50", "p95");
    println!("{}", "─".repeat(72));
    for (label, times) in &[
        ("IPC tiny (3 rows)", &t_tiny),
        ("IPC small (~37K)", &t_small),
        ("IPC medium (~45K)", &t_medium),
        ("IPC large (all drives)", &t_large),
    ] {
        println!("{:<35} {:>10} {:>10}", label, fmt_ms(p50(times)), fmt_ms(p95(times)));
    }
    println!("{}", "─".repeat(72));
}

// ── Argument parsing ─────────────────────────────────────────────────────────

fn parse_args() -> Cfg {
    let args: Vec<String> = env::args().collect();
    let mut mode = Mode::All;
    let mut rounds = DEFAULT_ROUNDS;
    let mut uffs_bin = PathBuf::from("uffs.exe");
    let mut drive = "C".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                i += 1;
                if i < args.len() {
                    mode = match args[i].as_str() {
                        "null-matrix" | "null" => Mode::NullMatrix,
                        "startup" => Mode::Startup,
                        "output" => Mode::Output,
                        "ipc" => Mode::Ipc,
                        "all" => Mode::All,
                        other => {
                            eprintln!("Unknown mode: {other}. Use: null-matrix, startup, output, ipc, all");
                            std::process::exit(1);
                        }
                    };
                }
            }
            "--rounds" => {
                i += 1;
                if i < args.len() {
                    rounds = args[i].parse().unwrap_or(DEFAULT_ROUNDS);
                }
            }
            "--uffs-bin" => {
                i += 1;
                if i < args.len() { uffs_bin = PathBuf::from(&args[i]); }
            }
            "--drive" => {
                i += 1;
                if i < args.len() { drive = args[i].clone(); }
            }
            "--help" | "-h" => {
                println!("Usage: startup-profiler.rs [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --mode <MODE>       null-matrix|startup|output|ipc|all (default: all)");
                println!("  --rounds <N>        Measurement rounds (default: {})", DEFAULT_ROUNDS);
                println!("  --uffs-bin <PATH>   Path to uffs.exe (default: uffs.exe)");
                println!("  --drive <LETTER>    Drive letter for searches (default: C)");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    // Try to find uffs.exe if not specified.
    //   1. PATH (via `where`/`which`)
    //   2. User bin dir: ~/bin/uffs[.exe]   (the user keeps all binaries here on Windows)
    //   3. Repo-local release build: target/release/uffs[.exe]
    //   4. Repo-local debug build:   target/debug/uffs[.exe]
    if !uffs_bin.exists() {
        if let Some(found) = find_bin("uffs") { uffs_bin = found; }
    }

    // Try to find es.exe (same lookup strategy)
    let es_bin = find_bin("es");

    // Build directory for null binaries.
    // We deliberately AVOID %TEMP% / env::temp_dir() on Windows — that path
    // is aggressively scanned by SmartScreen + Windows Search Indexer, which
    // causes transient "Access is denied" rename failures during cargo's
    // link-or-copy step (even with AV disabled). Prefer:
    //   Windows: %LOCALAPPDATA%\uffs-startup-profiler
    //   Unix:    $XDG_CACHE_HOME or ~/.cache/uffs-startup-profiler, else /tmp
    let build_dir = pick_build_dir();
    let _ = fs::create_dir_all(&build_dir);

    Cfg { mode, rounds, uffs_bin, drive, build_dir, es_bin }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let cfg = parse_args();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  UFFS Startup Profiler — Phase 2 Measurement Toolkit       ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Mode:     {:?}", match cfg.mode {
        Mode::NullMatrix => "null-matrix",
        Mode::Startup => "startup",
        Mode::Output => "output",
        Mode::Ipc => "ipc",
        Mode::All => "all",
    });
    println!("  Rounds:   {}", cfg.rounds);
    println!("  uffs.exe: {} ({})", cfg.uffs_bin.display(),
        if cfg.uffs_bin.exists() { fmt_size(file_size(&cfg.uffs_bin)) } else { "NOT FOUND".into() });
    println!("  es.exe:   {}", cfg.es_bin.as_ref().map(|p| p.display().to_string())
        .unwrap_or_else(|| "not found".into()));
    println!("  Drive:    {}", cfg.drive);
    println!("  Build:    {}", cfg.build_dir.display());

    match cfg.mode {
        Mode::NullMatrix => run_null_matrix(&cfg),
        Mode::Startup => run_startup(&cfg),
        Mode::Output => run_output(&cfg),
        Mode::Ipc => run_ipc(&cfg),
        Mode::All => {
            run_null_matrix(&cfg);
            run_startup(&cfg);
            run_output(&cfg);
            run_ipc(&cfg);
        }
    }

    println!("\n  Done. Copy results into docs/research/perf-phase2-measurement-plan.md");
}
