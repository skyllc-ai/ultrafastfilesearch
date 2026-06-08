#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! Cross-Tool Benchmark — UFFS (Rust) vs UFFS (C++) vs Everything
//!
//! Public-facing benchmark comparing UFFS against third-party NTFS search
//! tools on identical drives, patterns, and measurement methodology.
//!
//! # Design
//!
//!   - Each tool tested via its documented CLI — no GUI automation.
//!   - PASS/DNF: 30 s timeout → DNF.  Missing executable → SKIP.
//!   - UFFS (Rust): three phases (COLD / WARM / HOT).
//!     UFFS (C++): reads MFT every invocation (no daemon).
//!     Everything: always-hot (daemon model, index pre-loaded).
//!   - Same patterns, same drives, same result cap.
//!   - Percentile reporting: p50/p95 from N rounds per pattern.
//!
//! # Tool CLI references
//!
//!   UFFS (Rust): uffs.exe "<pattern>" --drive <X> --out=bench_out.csv \
//!                --columns Path --hide-system --hide-ads
//!     - Search is the default action (no "search" subcommand).
//!     - No limit — all results written to file.  Path-only for fair I/O.
//!     - Daemon model: COLD/WARM/HOT phases.
//!     - `--profile` is intentionally OFF by default: a normal user does
//!       not pass it, so the bench should measure the exact command shape
//!       a user actually types.  Previous runs (pre-2026-04-21) hard-
//!       coded `--profile` to capture `daemon_ms` via stderr parsing —
//!       overhead is <0.2% on warm queries but the flag itself is a
//!       non-default codepath (enables the full `SearchProfile` payload
//!       on the wire), which we want out of the apples-to-apples wall-
//!       clock comparison.  `daemon_ms` remains parseable if a user
//!       manually appends `--profile` to `UFFS_EXTRA_ARGS`, but the
//!       summary tables rely on `wall_ms` only.
//!     - `--hide-system` + `--hide-ads` are PARITY filters: Everything does
//!       not index NTFS system files (`$MFT`, `$Bitmap`, …) or Alternate
//!       Data Streams by default, while UFFS does. Without these flags, UFFS
//!       returns 30–70% more rows than Everything for broad patterns
//!       (`win*`, `config`, …), which makes timing comparisons apples-vs-
//!       pears. With them, row counts align within a few rows across tools.
//!     - Ref: internal — see `uffs.exe --help` and
//!       `docs/user-manual/filters.md` §1 (Scope Filters).
//!   UFFS (C++):  uffs.com <pattern> --drives=<X> --columns=path
//!     - No daemon, no --limit. Reads MFT every invocation. Outputs ALL results.
//!     - Path-only output (--columns=path) for fair comparison.
//!     - Extension filter: --ext=dll (separate flag, not glob *.dll)
//!     - Substring: *config* (glob wildcards needed)
//!     - Ref: https://github.com/githubrobbi/Ultra-Fast-File-Search
//!   Everything:  es.exe "<X>:\" <pattern> -export-csv bench_out.csv
//!     - No limit — all results written to file.  Outputs Filename (path) only.
//!     - Requires Everything service running.
//!     - Ref: https://www.voidtools.com/support/everything/command_line_interface/
//!
//! # Excluded tools
//!
//!   UltraSearch (JAM Software): Evaluated but excluded.  The /CLIPBOARD /NOGUI
//!     /CLOSE flags launch a GUI process that exits before the MFT scan completes.
//!     No stdout mode, no headless search — results only go to clipboard IF the
//!     GUI renders them first.  Not viable for automated CLI benchmarking.
//!     Ref: https://manuals.jam-software.com/ultrasearch/EN/CommandLine.html
//!   WizFile: No CLI interface at all — GUI only.  Cannot be benchmarked.
//!   Windows Search: Content indexer, not MFT-based filename search.
//!
//! # Output sinks
//!
//!   `--sinks file,stdout,null` selects which output targets HOT runs exercise.
//!   - `file`   (default) — `--out=uffs_bench_out.csv` / `-export-csv` → disk.
//!                           Measures the daemon-direct file-write path.
//!   - `stdout` — drop `--out=`, capture stdout via a Rust pipe.  Measures the
//!                Phase 3.2 single-buffer multi-column render path.
//!   - `null`   — spawn via `cmd /C "<tool> ... > NUL"` so the child sees a
//!                real NUL-device handle.  Measures the Phase 3.1 NUL fast
//!                path (UFFS short-circuits row materialisation when it
//!                detects a NUL handle via `GetFileType`).  This matches what
//!                real CLI users type, and — crucially — exercises the
//!                detection logic that `Stdio::from(File::create("NUL"))`
//!                does not.
//!   COLD / WARM always run in `file` mode (I/O-bound on MFT load, the output
//!   sink is noise at that scale).  HOT / C++ / Everything loop over the
//!   requested sinks.
//!
//! # Usage
//!
//! ```powershell
//! rust-script scripts\windows\cross-tool-benchmark.rs
//! rust-script scripts\windows\cross-tool-benchmark.rs --drives C,D
//! rust-script scripts\windows\cross-tool-benchmark.rs --rounds 20
//! rust-script scripts\windows\cross-tool-benchmark.rs --tools uffs,everything
//! rust-script scripts\windows\cross-tool-benchmark.rs --skip-cold
//! rust-script scripts\windows\cross-tool-benchmark.rs --sinks file,stdout,null
//! rust-script scripts\windows\cross-tool-benchmark.rs --uffs-bin C:\tools\uffs.exe
//!
//! # Opt-in: capture daemon_ms via --profile (otherwise `wall_ms` only).
//! $env:UFFS_EXTRA_ARGS = "--profile"
//! rust-script scripts\windows\cross-tool-benchmark.rs
//! ```
//!
//! ```cargo
//! [dependencies]
//! ```
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_ROUNDS: usize = 10;
const DEFAULT_DRIVES: &[&str] = &["C", "D"];

/// Bench output file in current working directory.
/// C++ UFFS cannot write to absolute paths — relative paths work fine.
fn bench_out_path() -> String {
    "uffs_bench_out.csv".to_string()
}
/// (label, uffs_rust_pattern, es_search, cpp_pattern, cpp_ext, validate)
/// cpp_ext: if non-empty, C++ UFFS uses `* --ext=<val>` instead of glob
/// validate: case-insensitive substring that every result line must contain
///           (empty = skip validation, e.g. full_scan)
/// cpp_pattern: empty string means C++ does not support this pattern — skip.
///
/// `ext_regex_alt` exercises the v0.5.66 regex-alternation → ExtensionIndex
/// promotion (`extract_extensions_from_regex`).  UFFS takes the regex form
/// so the promotion has to fire at dispatch / parse time; Everything and
/// C++ UFFS take their native multi-ext filter form for a fair head-to-head
/// (identical result sets, different syntax).
///
/// # Extension triple choice (`wav`, `idrc`, `cmake`)
///
/// Chosen empirically on the reference AMD 3900XT workstation to land near
/// **~149 K combined rows** across the 7-drive corpus — comfortably under
/// Everything's `~150 K-row` `es.exe -export-csv` IPC buffer ceiling so the
/// head-to-head run completes cleanly on both tools.  Broader triples that
/// exceed this cap (e.g. `jpg|png|heic` → 882 K rows on a media drive)
/// trigger `es.exe` to abort in ~50 ms with a non-zero exit code — visible
/// in the `is_fast_deterministic_fail` short-circuit below.  UFFS itself
/// has no such ceiling (882 K rows takes ~325 ms), so larger triples are
/// still perfectly valid for UFFS-only benches — just not for the cross-tool
/// comparison.
///
/// The Everything query uses **OR-alternation glob syntax** (`*.wav|*.idrc|*.cmake`)
/// rather than `ext:wav;idrc;cmake`.  Both are documented at
/// <https://www.voidtools.com/support/everything/searching/>, but the `;`
/// inside `ext:` is a 1.4.1+ feature and parses as implicit-AND in older
/// builds.  The `|` top-level OR operator has worked cleanly since
/// Everything 1.3 and costs nothing extra to use.
///
/// Validation is disabled (empty string) because the result set spans
/// multiple extensions and the current tuple shape only supports a single
/// substring check; row-count parity between UFFS and Everything remains
/// the correctness signal in the summary table.
const PATTERNS: &[(&str, &str, &str, &str, &str, &str)] = &[
    ("full_scan",     "*",                         "*",                    "*",           "",              ""),
    ("exact",         "notepad.exe",               "notepad.exe",          "notepad.exe", "",              "notepad"),
    ("prefix",        "win*",                      "win*",                 "",            "",              "win"),
    // ^^ cpp_pattern="" — uffs.com does not support trailing-wildcard prefix
    //    glob (win* returns nothing/errors).  UFFS Rust vs Everything only.
    ("ext_rare",      "*.dbt",                     "ext:dbt",              "*.dbt",       "dbt",           ".dbt"),
    ("ext_dll",       "*.dll",                     "ext:dll",              "*.dll",       "dll",           ".dll"),
    ("ext_regex_alt", ">.*\\.(wav|idrc|cmake)$",   "*.wav|*.idrc|*.cmake", "*",           "wav,idrc,cmake", ""),
    ("substring",     "config",                    "config",               "config",      "",              "config"),
];

// ── Types ────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)] enum Tool { Uffs, UffsCpp, Everything }
impl Tool { fn label(self) -> &'static str { match self { Self::Uffs=>"UFFS", Self::UffsCpp=>"UFFS-C++", Self::Everything=>"Everything" } } }

#[derive(Clone, Copy, PartialEq, Eq)] enum Phase { Cold, Warm, Hot }
impl Phase { fn label(self) -> &'static str { match self { Self::Cold=>"COLD", Self::Warm=>"WARM", Self::Hot=>"HOT" } } }

/// Where the tool is asked to emit its rows.  HOT / C++ / Everything loop
/// across the requested sinks; COLD / WARM always use `File` (see header).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputSink { File, Stdout, Null }
impl OutputSink {
    fn label(self) -> &'static str {
        match self { Self::File=>"file", Self::Stdout=>"stdout", Self::Null=>"null" }
    }
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "file" | "f"                       => Some(Self::File),
            "stdout" | "out" | "tty" | "pipe"  => Some(Self::Stdout),
            "null" | "nul" | "devnull"         => Some(Self::Null),
            _ => None,
        }
    }
    /// Parse a comma-separated list; unknown tokens are reported and skipped.
    fn parse_list(s: &str) -> Vec<Self> {
        let mut out = Vec::new();
        for part in s.split(',') {
            match Self::parse(part) {
                Some(sink) if !out.contains(&sink) => out.push(sink),
                Some(_)  => {}                           // dedup silently
                None     => eprintln!("Unknown sink: {part:?}"),
            }
        }
        out
    }
}

#[derive(Clone, Default)]
#[allow(dead_code)] // fields read in summary output and live progress lines
struct Timing { wall_ms: u64, daemon_ms: u64, rows: u64, bad_rows: u64, ok: bool, dnf: bool, err: String }

struct Row { tool: Tool, phase: Phase, sink: OutputSink, drive: String, pat: String, runs: Vec<Timing> }
struct Cfg { uffs: PathBuf, uffs_cpp: Option<PathBuf>, es: Option<PathBuf>,
             drives: Vec<String>, rounds: usize,
             tools: Vec<Tool>, sinks: Vec<OutputSink>, skip_cold: bool,
             patterns: Option<Vec<String>>,
             /// Named Everything instance to connect to via `-instance <name>`.
             /// When the bench tool launches a private Everything.exe with
             /// `-instance uffs-bench`, the default IPC window is absent and
             /// es.exe must be told which instance to query.
             es_instance: Option<String>,
             /// Optional summary CSV output path.  When `Some`, the post-run
             /// summary (one row per Tool × Phase × Sink × Drive × Pattern
             /// combination, with p50/p95/rows/bad/verdict) is written to
             /// this path after the stdout tables.  Distinct from the daemon's
             /// own intermediate `uffs_bench_out.csv` in the cwd, which holds
             /// the raw per-query row dumps and is overwritten every round.
             out: Option<PathBuf> }
impl Cfg {
    fn skip_pattern(&self, label: &str) -> bool {
        self.patterns.as_ref().map_or(false, |ps| !ps.iter().any(|p| p == &label.to_lowercase()))
    }
}


// ── Helpers ──────────────────────────────────────────────────────────────────
fn flush() { std::io::stderr().flush().ok(); std::io::stdout().flush().ok(); }
fn fms(ms: u64) -> String {
    if ms >= 60_000 { format!("{}m{:02}s", ms/60_000, (ms%60_000)/1000) }
    else if ms >= 1000 { format!("{}.{:01}s", ms/1000, (ms%1000)/100) }
    else { format!("{ms} ms") }
}
fn p50(s: &[u64]) -> u64 { if s.is_empty() { 0 } else { s[s.len()/2] } }
fn p95(s: &[u64]) -> u64 { if s.is_empty() { 0 } else { s[(s.len() as f64 * 0.95) as usize % s.len()] } }
fn sw(runs: &[Timing]) -> Vec<u64> { let mut v: Vec<u64> = runs.iter().filter(|r| r.ok).map(|r| r.wall_ms).collect(); v.sort(); v }

/// Shuffle three indices [0,1,2] using a minimal LCG seeded from a nanosecond
/// timestamp.  No external deps — works inside `rust-script` with zero cargo
/// overhead.  The LCG constants are the same ones used by glibc's `rand()`.
fn lcg_shuffle3(seed: u64) -> [usize; 3] {
    let mut s = seed ^ (seed >> 33);
    let mut next = || -> u64 {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        s
    };
    let mut idx = [0usize, 1, 2];
    // Fisher-Yates on 3 elements
    let j = (next() % 3) as usize;
    idx.swap(0, j);
    let j = 1 + (next() % 2) as usize;
    idx.swap(1, j);
    idx
}

// ── Discovery ────────────────────────────────────────────────────────────────
fn find_in(cs: &[PathBuf]) -> Option<PathBuf> { cs.iter().find(|p| p.exists()).cloned() }
fn where_exe(name: &str) -> Option<PathBuf> {
    Command::new("where").arg(name).output().ok().and_then(|o| {
        let s = String::from_utf8_lossy(&o.stdout);
        let l = s.lines().next().unwrap_or("").trim();
        if !l.is_empty() && Path::new(l).exists() { Some(PathBuf::from(l)) } else { None }
    })
}
fn find_uffs() -> Option<PathBuf> {
    where_exe("uffs.exe").or_else(|| {
        let h = env::var("USERPROFILE").unwrap_or_default();
        find_in(&[PathBuf::from(&h).join("bin").join("uffs.exe")])
    })
}
// The pinned competitor *version* (currently Everything 1.1.0.30) is recorded
// in `scripts/windows/competitors.toml` — the single source of truth. The
// directory names below are only install-location candidates for *discovering*
// an `es.exe` on disk (any installed build works); they are not version claims.
fn find_es() -> Option<PathBuf> {
    where_exe("es.exe").or_else(|| {
        let (h, pf, pf86) = (env::var("USERPROFILE").unwrap_or_default(),
            env::var("ProgramFiles").unwrap_or_default(),
            env::var("ProgramFiles(x86)").unwrap_or_default());
        find_in(&[
            PathBuf::from(&h).join("bin").join("es.exe"),
            PathBuf::from(&pf).join("Everything").join("es.exe"),
            PathBuf::from(&pf86).join("Everything").join("es.exe"),
            PathBuf::from(&pf).join("Everything 1.5a").join("es.exe"),
        ])
    })
}
fn find_uffs_cpp() -> Option<PathBuf> {
    where_exe("uffs.com").or_else(|| {
        let h = env::var("USERPROFILE").unwrap_or_default();
        find_in(&[PathBuf::from(&h).join("bin").join("uffs.com")])
    })
}


// ── UFFS lifecycle ───────────────────────────────────────────────────────────
fn uffs_stop(bin: &Path) {
    // "daemon kill" is a hard kill; "daemon stop" is a graceful shutdown
    // that may leave shared memory / cache warm.
    let _ = Command::new(bin).args(["daemon","kill"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    std::thread::sleep(Duration::from_secs(2));
}
/// Start the daemon with bench-safe idle-demote TTLs so it never demotes
/// Hot→Warm or Warm→Parked mid-run.  These env vars are scoped to the
/// daemon child process only — the bench script's own env is unchanged,
/// and teardown's next `uffs daemon start` gets production defaults.
fn uffs_start(bin: &Path) {
    let _ = Command::new(bin)
        .args(["daemon", "start"])
        .env("UFFS_HOT_TO_WARM_IDLE_SECS",   "3600")
        .env("UFFS_WARM_TO_PARKED_IDLE_SECS", "7200")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status();
    std::thread::sleep(Duration::from_millis(500));
}
fn uffs_purge_cache() {
    // Remove both cache locations:
    //   %LOCALAPPDATA%\uffs\cache\  (primary)
    //   %TEMP%\uffs_index_cache\    (fallback / legacy)
    let cache1 = PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_default()).join("uffs").join("cache");
    let cache2 = PathBuf::from(env::temp_dir()).join("uffs_index_cache");
    for dir in [&cache1, &cache2] {
        let _ = std::fs::remove_dir_all(dir);
    }
}


/// Check if a line is a header, footer, or empty (not a data row).
/// Matches the same logic as verify_parity.rs `is_footer_or_header_line`.
/// Handles all three tools' CSV headers/footers.
fn is_header_or_footer(line: &str) -> bool {
    let t = line.trim();
    let tl = t.to_ascii_lowercase();
    t.is_empty()
        || tl.starts_with("\"path\"")             // UFFS CSV header (quoted)
        || tl.starts_with("path,")                // UFFS daemon CSV header (unquoted)
        || tl.starts_with("path\t")               // TSV header
        || tl == "path"                            // single-column header
        || tl.starts_with("drive,")               // UFFS daemon full CSV header
        || t == "Filename"                          // Everything es.exe -export-csv header (single column)
        || t.starts_with("\"Filename\"")           // Everything es.exe -export-csv header (quoted)
        || t.starts_with("Filename,")              // Everything es.exe alt header (multi-column)
        || t.starts_with("Drives?")               // C++ footer
        || t.starts_with("MMMmmm that was FAST")  // C++ footer
        || t.starts_with("Search path")            // C++ footer
        || t.starts_with("Finished")               // C++ footer
}

/// Count data lines in bench output file, filtering headers/footers.
/// Validates that each data line contains `needle` (case-insensitive).
/// Returns (data_rows, bad_rows).
fn count_and_validate(path: &str, needle: &str) -> (u64, u64) {
    // Try UTF-8 first, then fall back to reading raw bytes (handles UTF-16 BOM)
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            // Fallback: read raw bytes, strip UTF-16 LE BOM, decode lossy
            match std::fs::read(path) {
                Ok(bytes) if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE => {
                    // UTF-16 LE: decode pairs of bytes
                    let u16s: Vec<u16> = bytes[2..].chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16_lossy(&u16s)
                }
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => return (0, 0),
            }
        }
    };
    let data: Vec<&str> = content.lines()
        .filter(|l| !is_header_or_footer(l))
        .collect();
    let total = data.len() as u64;
    if needle.is_empty() || total == 0 {
        return (total, 0);
    }
    let needle_lower = needle.to_lowercase();
    let bad_lines: Vec<&&str> = data.iter().filter(|l| !l.to_lowercase().contains(&needle_lower)).collect();
    if !bad_lines.is_empty() {
        eprintln!("  ⚠ {} rows failed validation (needle={:?}):", bad_lines.len(), needle);
        for (i, line) in bad_lines.iter().enumerate().take(5) {
            let preview: String = line.chars().take(120).collect();
            eprintln!("    bad[{}]: {:?}", i, preview);
        }
    }
    (total, bad_lines.len() as u64)
}
fn cleanup_bench_file() { let p = bench_out_path(); let _ = std::fs::remove_file(&p); }
fn cleanup_file(p: &str) { let _ = std::fs::remove_file(p); }

/// Extract the path column from a tool's output file or byte buffer into a
/// normalised (lowercase, trimmed) set of strings.  Headers, footers, and
/// empty lines are stripped by `is_header_or_footer` so the set only
/// contains actual filesystem paths.  Used for path-set superset checks.
fn extract_paths_from_file(path: &str) -> std::collections::HashSet<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => match std::fs::read(path) {
            Ok(bytes) if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE => {
                let u16s: Vec<u16> = bytes[2..].chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&u16s)
            }
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => return std::collections::HashSet::new(),
        },
    };
    content.lines()
        .filter(|l| !is_header_or_footer(l))
        .map(|l| {
            // The Path column may be quoted CSV: strip surrounding quotes and
            // take only the first comma-delimited field (path is always first).
            let trimmed = l.trim().trim_matches('"');
            trimmed.split(',').next().unwrap_or(trimmed).trim().to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .collect()
}


/// Check that `subset` is a subset of `superset`; return a short summary
/// string.  Reports the first few missing paths on a violation.
fn check_subset(
    subset_label: &str,
    subset: &std::collections::HashSet<String>,
    superset_label: &str,
    superset: &std::collections::HashSet<String>,
) -> Option<String> {
    if subset.is_empty() || superset.is_empty() { return None; }
    let missing: Vec<&String> = subset.iter().filter(|p| !superset.contains(*p)).collect();
    if missing.is_empty() { return None; }
    let preview: Vec<&str> = missing.iter().take(3).map(|s| s.as_str()).collect();
    Some(format!("{subset_label}⊄{superset_label}: {} paths missing (e.g. {})",
        missing.len(), preview.join(", ")))
}

/// Count data lines in a captured stdout byte buffer, filtering the same
/// headers/footers as `count_and_validate` so the two validators stay aligned.
fn count_and_validate_bytes(bytes: &[u8], needle: &str) -> (u64, u64) {
    let content = String::from_utf8_lossy(bytes);
    let data: Vec<&str> = content.lines().filter(|l| !is_header_or_footer(l)).collect();
    let total = data.len() as u64;
    if needle.is_empty() || total == 0 { return (total, 0); }
    let nl = needle.to_lowercase();
    let bad: Vec<&&str> = data.iter().filter(|l| !l.to_lowercase().contains(&nl)).collect();
    if !bad.is_empty() {
        eprintln!("  ⚠ {} stdout rows failed validation (needle={needle:?}):", bad.len());
        for (i, line) in bad.iter().enumerate().take(5) {
            let preview: String = line.chars().take(120).collect();
            eprintln!("    bad[{i}]: {preview:?}");
        }
    }
    (total, bad.len() as u64)
}

/// One tool's captured run: status + stdout + stderr + wall time.
struct ToolOutput { wall_ms: u64, ok: bool, exit_code: Option<i32>, stdout: Vec<u8>, stderr: Vec<u8> }

/// Spawn a tool via `cmd /C "<bin> <args...> > NUL"` so the child inherits a
/// genuine Windows NUL-device handle from the shell.  `Stdio::from(File::create(
/// "NUL"))` classifies differently under `GetFileType` and would bypass UFFS'
/// Phase 3.1 NUL fast-path detection, defeating the whole point of NUL mode.
/// stderr stays piped so we can still parse `--profile` output from UFFS.
fn spawn_with_nul_redirect(bin: &Path, args: &[String]) -> (Option<i32>, Vec<u8>, u64) {
    fn quote(a: &str) -> String {
        if a.chars().any(char::is_whitespace) { format!("\"{a}\"") } else { a.to_string() }
    }
    let mut line = quote(&bin.display().to_string());
    for a in args { line.push(' '); line.push_str(&quote(a)); }
    line.push_str(" > NUL");
    let t = Instant::now();
    let r = Command::new("cmd").args(["/C", &line])
        .stdout(Stdio::null()).stderr(Stdio::piped())
        .output();
    let wall = t.elapsed().as_millis() as u64;
    match r {
        Ok(o)  => (o.status.code(), o.stderr, wall),
        Err(e) => (None, e.to_string().into_bytes(), wall),
    }
}

/// Spawn a tool and capture output according to the sink.
///   - `File` / `Stdout`: direct spawn, stdout+stderr piped via `.output()`.
///   - `Null`           : wrap in `cmd /C "... > NUL"` via `spawn_with_nul_redirect`.
///
/// Callers are responsible for emitting (or suppressing) `--out=` in args;
/// this function is sink-aware only in its spawning strategy.
fn run_tool_with_sink(bin: &Path, args: &[String], sink: OutputSink) -> Result<ToolOutput, String> {
    match sink {
        OutputSink::File | OutputSink::Stdout => {
            let t = Instant::now();
            let r = Command::new(bin).args(args)
                .stdout(Stdio::piped()).stderr(Stdio::piped())
                .output();
            let wall_ms = t.elapsed().as_millis() as u64;
            match r {
                Ok(o)  => Ok(ToolOutput {
                    wall_ms, ok: o.status.success(), exit_code: o.status.code(),
                    stdout: o.stdout, stderr: o.stderr,
                }),
                Err(e) => Err(e.to_string()),
            }
        }
        OutputSink::Null => {
            let (exit_code, stderr, wall_ms) = spawn_with_nul_redirect(bin, args);
            Ok(ToolOutput {
                wall_ms, ok: exit_code == Some(0), exit_code,
                stdout: Vec::new(), stderr,
            })
        }
    }
}

/// Validate output rows according to the sink.  `File` reads the bench file;
/// `Stdout` parses captured bytes; `Null` reports `(0, 0)` — correctness on
/// identical patterns is already covered by the file-mode passes, and there
/// is nothing left to count once stdout has been redirected to the device.
fn validate_output(sink: OutputSink, path: &str, stdout: &[u8], needle: &str) -> (u64, u64) {
    match sink {
        OutputSink::File   => count_and_validate(path, needle),
        OutputSink::Stdout => count_and_validate_bytes(stdout, needle),
        OutputSink::Null   => (0, 0),
    }
}

// ── Run: UFFS (Rust) ─────────────────────────────────────────────────────────
/// uffs.exe pattern --drive X [--out=<file>] ...
/// `sink` controls whether `--out=` is emitted and how output is captured.
/// No limit — all results are returned.  Search is the default action.
///
/// `--profile` is intentionally NOT included in the default arg list so
/// the bench measures the exact command shape a normal user would type
/// (see the module-level header note).  If `UFFS_EXTRA_ARGS` is set in
/// the environment, its whitespace-separated tokens are appended to
/// every UFFS invocation — useful for one-off profile captures without
/// having to patch this script.
fn run_uffs(bin: &Path, drive: &str, pattern: &str, validate: &str, sink: OutputSink) -> Timing {
    run_uffs_to(bin, drive, pattern, validate, sink, &bench_out_path())
}
fn run_uffs_to(bin: &Path, drive: &str, pattern: &str, validate: &str, sink: OutputSink, bpath: &str) -> Timing {
    cleanup_file(bpath);
    let bpath = bpath.to_owned();
    // Path-only output for fair comparison with es.exe (which only outputs Filename).
    // --hide-system + --hide-ads bring UFFS result semantics in line with
    // Everything (which does not index NTFS system files or Alternate Data
    // Streams by default). Without these flags, UFFS returns 30-70% more
    // rows for broad patterns and the timing comparison is meaningless.
    let mut args: Vec<String> = vec![
        pattern.into(), "--drive".into(), drive.into(),
        "--columns".into(), "Path".into(),
        "--hide-system".into(), "--hide-ads".into(),
    ];
    if matches!(sink, OutputSink::File) {
        args.push(format!("--out={bpath}"));
    }
    // Opt-in extras (e.g. `UFFS_EXTRA_ARGS="--profile"`).  Still lets
    // the daemon-timing column populate for anyone who wants it, but
    // without imposing `--profile` on every user of the bench harness.
    //
    // NOTE: written as a nested `if let` rather than a let-chain
    // (`if let ... && ...`) because `rust-script` drives this file
    // through cargo's default edition (currently Rust 2021), which
    // does not stabilise let-chains.  Upgrading to edition 2024 here
    // would mean pinning an `//! ```cargo` manifest in the doc header
    // just for this single site; the explicit nested form is cheaper.
    if let Ok(extra) = env::var("UFFS_EXTRA_ARGS") {
        if !extra.trim().is_empty() {
            for tok in extra.split_whitespace() {
                args.push(tok.to_string());
            }
        }
    }
    eprintln!("      CMD: & '{}' {}  [sink={}]", bin.display(), args.join(" "), sink.label());
    let out = match run_tool_with_sink(bin, &args, sink) {
        Ok(o)  => o,
        Err(e) => return Timing { wall_ms: 0, err: e, ..Default::default() },
    };
    // `parse_daemon_ms` returns 0 when the profile block is absent
    // (i.e. the default path).  The summary tables key off `wall_ms`
    // so the missing column is cosmetic and the progress line just
    // drops the `daemon_p50=` suffix.
    let dms = parse_daemon_ms(&String::from_utf8_lossy(&out.stderr));
    if !out.ok {
        cleanup_bench_file();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        return Timing {
            wall_ms: out.wall_ms,
            err: format!("exit={:?} {err}", out.exit_code),
            ..Default::default()
        };
    }
    let (rows, bad_rows) = validate_output(sink, &bpath, &out.stdout, validate);
    cleanup_bench_file();
    Timing { wall_ms: out.wall_ms, daemon_ms: dms, rows, bad_rows, ok: true, ..Default::default() }
}
/// Parse the `daemon: N ms` tail of the `--profile` `Search (IPC): X ms  (daemon: Y ms)`
/// line.  Returns 0 when the profile block is absent — the bench
/// defaults (post-2026-04-21) omit `--profile` so this is the common
/// path, and callers fall back to `wall_ms` accordingly.
fn parse_daemon_ms(s: &str) -> u64 {
    for line in s.lines() {
        if (line.contains("Search") || line.contains("search")) && line.contains("ms") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for (i, p) in parts.iter().enumerate() {
                if *p == "ms" && i > 0 { if let Ok(v) = parts[i-1].trim_end_matches(',').parse() { return v; } }
            }
        }
    }
    0
}

// ── Run: Everything (es.exe) ─────────────────────────────────────────────────
/// es.exe "<D>:\" <pattern> [-export-csv <file>]
/// File sink: `-export-csv <file>`; Stdout sink: default CSV to stdout; Null
/// sink: default to stdout then redirect via cmd.  No -n limit — all results
/// are returned.
fn run_es_to(bin: &Path, drive: &str, pattern: &str, validate: &str, sink: OutputSink, es_instance: Option<&str>, bpath: &str) -> Timing {
    cleanup_file(bpath);
    let bpath = bpath.to_owned();
    // es.exe expects path filter and search term as SEPARATE arguments:
    //   es.exe "C:\" ext:dll -export-csv file.csv
    // NOT as one combined string.
    // When a named instance is in use (private bench instance launched with
    // -instance <name>), prepend -instance <name> so es.exe connects to the
    // correct IPC window instead of the default one.
    let drive_path = format!("{drive}:\\");
    let mut args: Vec<String> = Vec::new();
    if let Some(inst) = es_instance {
        args.push("-instance".into());
        args.push(inst.into());
    }
    args.push(drive_path);
    if pattern != "*" { args.push(pattern.into()); }
    if matches!(sink, OutputSink::File) {
        args.push("-export-csv".into());
        args.push(bpath.clone());
    }
    eprintln!("      CMD: & '{}' {}  [sink={}]", bin.display(), args.join(" "), sink.label());
    let out = match run_tool_with_sink(bin, &args, sink) {
        Ok(o)  => o,
        Err(e) => return Timing { wall_ms: 0, err: e, ..Default::default() },
    };
    if !out.ok {
        cleanup_bench_file();
        return Timing {
            wall_ms: out.wall_ms,
            err: format!("exit={:?}", out.exit_code),
            ..Default::default()
        };
    }
    let (rows, bad_rows) = validate_output(sink, &bpath, &out.stdout, validate);
    cleanup_bench_file();
    Timing { wall_ms: out.wall_ms, rows, bad_rows, ok: true, ..Default::default() }
}

// ── Run: UFFS C++ (uffs.com) ─────────────────────────────────────────────────
/// C++ UFFS reads MFT every invocation (no daemon). No --limit flag.
/// Extension filter uses --ext=<ext> instead of glob *.ext.
/// Substring match needs *needle* glob wildcards.
///
/// # Sink notes
///
/// - `File`: emit `--out=<bench>` and use `Stdio::inherit()` on stdout/stderr.
///   The C++ binary internally `freopen()`s stdout onto the `--out=` file;
///   pre-redirecting stdout to a Rust pipe or NUL makes freopen fail silently
///   and the output file comes out empty.  Inherit is the only safe choice here.
/// - `Stdout` / `Null`: drop `--out=` entirely.  With no `--out=` the freopen
///   path never fires, so piped-capture (Stdout) and `cmd /C "... > NUL"`
///   (Null) behave normally.
fn run_uffs_cpp_to(bin: &Path, drive: &str, pattern: &str, cpp_ext: &str, validate: &str, sink: OutputSink, bpath: &str) -> Timing {
    cleanup_file(bpath);
    let bpath = bpath.to_owned();
    let mut args: Vec<String> = Vec::new();
    if !cpp_ext.is_empty() {
        args.push("*".into());
        args.push(format!("--ext={cpp_ext}"));
    } else if !pattern.contains('*') && !pattern.contains('?') && pattern != "*" {
        args.push(format!("*{pattern}*"));
    } else {
        args.push(pattern.into());
    }
    args.push(format!("--drives={drive}"));
    // Path-only output for fair comparison with es.exe.
    args.push("--columns=path".into());
    if matches!(sink, OutputSink::File) {
        args.push(format!("--out={bpath}"));
    }
    eprintln!("      CMD: & '{}' {}  [sink={}]", bin.display(), args.join(" "), sink.label());
    let out: ToolOutput = match sink {
        OutputSink::File => {
            // freopen on --out= requires inherited stdout.  Inherit both streams
            // and use .status(); we get no captured bytes back but the file is
            // what we validate here anyway.
            let t = Instant::now();
            let r = Command::new(bin).args(&args)
                .stdout(Stdio::inherit()).stderr(Stdio::inherit())
                .status();
            let wall_ms = t.elapsed().as_millis() as u64;
            match r {
                Ok(s)  => ToolOutput {
                    wall_ms, ok: s.success(), exit_code: s.code(),
                    stdout: Vec::new(), stderr: Vec::new(),
                },
                Err(e) => return Timing { wall_ms, err: e.to_string(), ..Default::default() },
            }
        }
        OutputSink::Stdout | OutputSink::Null => match run_tool_with_sink(bin, &args, sink) {
            Ok(o)  => o,
            Err(e) => return Timing { wall_ms: 0, err: e, ..Default::default() },
        },
    };
    if !out.ok {
        cleanup_bench_file();
        return Timing {
            wall_ms: out.wall_ms,
            err: format!("exit={:?}", out.exit_code),
            ..Default::default()
        };
    }
    let (rows, bad_rows) = validate_output(sink, &bpath, &out.stdout, validate);
    cleanup_bench_file();
    Timing { wall_ms: out.wall_ms, rows, bad_rows, ok: true, ..Default::default() }
}

fn check_dnf(mut t: Timing) -> Timing {
    if t.wall_ms > TIMEOUT.as_millis() as u64 { t.dnf = true; }
    t
}

/// True when a tool invocation is a **deterministic** fast failure (exited
/// non-zero in under a second without hitting the DNF timeout).
///
/// Used by the HOT-phase loops to short-circuit the remaining rounds when
/// the first invocation errors in a way that won't improve with retries.
/// The canonical trigger is `es.exe -export-csv` on drives where the
/// combined result set overflows Everything's IPC buffer (e.g. on a media
/// drive, `>.*\.(jpg|png|heic)$` matches ~880 K files → buffer overflow →
/// `es.exe` exits in ~50 ms with a non-zero code).  Retrying the same query
/// 29 more times just wastes wall time, so we bail after the first failure.
///
/// The `< 1_000` threshold keeps slow-but-recoverable errors (long blocking
/// timeouts, partial reads) in the 30-round loop where the p95 tail might
/// still tell us something useful.
fn is_fast_deterministic_fail(t: &Timing) -> bool {
    !t.ok && !t.dnf && t.wall_ms < 1_000
}

// ── Arg parsing ──────────────────────────────────────────────────────────────
fn parse_args() -> Cfg {
    let args: Vec<String> = env::args().collect();
    let mut drives: Option<Vec<String>> = None;
    let mut rounds = DEFAULT_ROUNDS;
    let mut tools_str: Option<String> = None;
    let mut sinks_str: Option<String> = None;
    let mut skip_cold = false;
    let mut uffs_bin: Option<PathBuf> = None;
    let mut patterns_filter: Option<Vec<String>> = None;
    let mut out: Option<PathBuf> = None;
    let mut es_instance: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--drives" => { i += 1; drives = Some(args[i].split(',').map(|s| s.trim().to_uppercase()).collect()); }
            "--rounds" => { i += 1; rounds = args[i].parse().unwrap_or(DEFAULT_ROUNDS); }
            "--tools"  => { i += 1; tools_str = Some(args[i].clone()); }
            "--sinks"  => { i += 1; sinks_str = Some(args[i].clone()); }
            "--patterns" => { i += 1; patterns_filter = Some(args[i].split(',').map(|s| s.trim().to_lowercase()).collect()); }
            "--skip-cold" => { skip_cold = true; }
            "--uffs-bin" => { i += 1; uffs_bin = Some(PathBuf::from(&args[i])); }
            "--out" => { i += 1; out = Some(PathBuf::from(&args[i])); }
            "--es-instance" => { i += 1; es_instance = Some(args[i].clone()); }
            "--help" | "-h" => { print_help(); std::process::exit(0); }
            other => {
                if other.starts_with('-') {
                    eprintln!("warning: unknown flag {other:?} ignored (use --help for the supported list)");
                }
            }
        }
        i += 1;
    }
    let uffs = uffs_bin.or_else(find_uffs).expect("ERROR: uffs.exe not found.  Use --uffs-bin <path>.");
    let uffs_cpp = find_uffs_cpp();
    let es = find_es();
    let drives = drives.unwrap_or_else(|| DEFAULT_DRIVES.iter().map(|s| s.to_string()).collect());
    let mut tools = Vec::new();
    if let Some(ts) = tools_str {
        for t in ts.split(',') {
            match t.trim().to_lowercase().as_str() {
                "uffs" => tools.push(Tool::Uffs),
                "uffs-cpp" | "uffs_cpp" | "cpp" => if uffs_cpp.is_some() { tools.push(Tool::UffsCpp); },
                "everything" | "es" => if es.is_some() { tools.push(Tool::Everything); },
                _ => eprintln!("Unknown tool: {t}"),
            }
        }
    } else {
        tools.push(Tool::Uffs);
        if uffs_cpp.is_some() { tools.push(Tool::UffsCpp); }
        if es.is_some() { tools.push(Tool::Everything); }
    }
    let sinks = sinks_str
        .as_deref()
        .map(OutputSink::parse_list)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![OutputSink::File]);
    Cfg { uffs, uffs_cpp, es, drives, rounds, tools, sinks, skip_cold, patterns: patterns_filter, es_instance, out }
}

fn print_help() {
    eprintln!("Cross-Tool Benchmark — UFFS (Rust) vs UFFS (C++) vs Everything");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  --drives C,D          Drives to benchmark (default: C,D)");
    eprintln!("  --rounds 10           Rounds per pattern (default: 10)");
    eprintln!("  --tools uffs,cpp,es   Comma-separated tools (default: all found)");
    eprintln!("                        Values: uffs, cpp/uffs-cpp, es/everything");
    eprintln!("  --sinks file,stdout,null");
    eprintln!("                        Comma-separated output sinks for HOT runs");
    eprintln!("                        (default: file).  COLD/WARM always use file.");
    eprintln!("  --patterns full_scan,ext_dll,ext_regex_alt,...");
    eprintln!("                        Comma-separated pattern labels to run.");
    eprintln!("                        Known labels: full_scan, exact, prefix,");
    eprintln!("                        ext_rare, ext_dll, ext_regex_alt, substring");
    eprintln!("  --skip-cold           Skip UFFS COLD and WARM phases");
    eprintln!("  --uffs-bin <path>     Path to uffs.exe (Rust)");
    eprintln!("  --es-instance <name>  Connect es.exe to a named Everything instance");
    eprintln!("                        (passes -instance <name> to every es.exe call).");
    eprintln!("                        Required when Everything.exe was launched with");
    eprintln!("                        -instance <name> (e.g. uffs-bench private instance).");
    eprintln!("  --out <path>          Write the post-run summary table to CSV at <path>.");
    eprintln!("                        Columns: tool,phase,sink,drive,pattern,p50_ms,");
    eprintln!("                        p95_ms,rows,bad,verdict,rounds_ok,rounds_total.");
    eprintln!("                        Distinct from the daemon's intermediate");
    eprintln!("                        uffs_bench_out.csv in the cwd (raw per-query rows,");
    eprintln!("                        overwritten every round).");
    eprintln!("  --help                This message");
}

// ── Main ─────────────────────────────────────────────────────────────────────
fn main() {
    let cfg = parse_args();

    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                     Cross-Tool Benchmark v1.0                               ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  UFFS (Rust):  {}",  cfg.uffs.display());
    if let Some(ref cpp) = cfg.uffs_cpp { println!("║  UFFS (C++):   {}", cpp.display()); }
    else                                 { println!("║  UFFS (C++):   NOT FOUND (SKIP)"); }
    if let Some(ref es) = cfg.es   { println!("║  Everything:   {}", es.display()); }
    else                            { println!("║  Everything:   NOT FOUND (SKIP)"); }
    println!("║  Drives:       {:?}", cfg.drives);
    if let Some(ref ps) = cfg.patterns {
        println!("║  Patterns:     {} (filtered: {})", PATTERNS.len(), ps.join(", "));
    } else {
        println!("║  Patterns:     {} queries", PATTERNS.len());
    }
    println!("║  Rounds:       {} per pattern per tool", cfg.rounds);
    let sinks_str: Vec<&'static str> = cfg.sinks.iter().map(|s| s.label()).collect();
    println!("║  Sinks (HOT):  {}  (COLD/WARM are always file)", sinks_str.join(", "));
    println!("║  Daemon bench file:  {}  (raw per-query rows, overwritten every round)", bench_out_path());
    if let Some(ref p) = cfg.out {
        println!("║  Summary CSV:        {}  (written post-run via --out)", p.display());
    }
    println!("║  Columns:      path-only (fair: all tools write ~same bytes/row)");
    println!("║  Limit:        none (all results, fair for C++)");
    println!("║  Timeout:      {} s → DNF", TIMEOUT.as_secs());
    println!("║  Skip COLD:    {}", cfg.skip_cold);
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();

    let mut all_rows: Vec<Row> = Vec::new();

    // ── Daemon warmup (once for all drives) ─────────────────────────────
    if cfg.tools.contains(&Tool::Uffs) && cfg.skip_cold {
        // When skipping COLD/WARM, kill+restart with bench-safe TTLs then
        // issue one probe so the daemon is fully loaded before HOT starts.
        eprint!("  Warming up UFFS daemon (all drives)...");  flush();
        uffs_stop(&cfg.uffs);
        uffs_start(&cfg.uffs);
        let _ = Command::new(&cfg.uffs)
            .args(["__uffs_warmup_probe__", "--limit", "1"])
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status();
        eprintln!(" ready.");
    }

    for drive in &cfg.drives {
        println!("━━━ Drive {}:  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", drive);

        // ── UFFS COLD (file sink only; MFT load dominates, sink is noise) ─
        if cfg.tools.contains(&Tool::Uffs) && !cfg.skip_cold {
            eprint!("  UFFS COLD: stopping daemon, purging cache...");
            flush();
            uffs_stop(&cfg.uffs);
            uffs_purge_cache();
            eprintln!(" done.");

            for &(label, pat, _, _, _, validate) in PATTERNS {
                if cfg.skip_pattern(label) { continue; }
                eprint!("    {label:<12} ");  flush();
                // COLD: only 1 round (destructive — restarts daemon each time)
                uffs_stop(&cfg.uffs);
                uffs_purge_cache();
                let t = check_dnf(run_uffs(&cfg.uffs, drive, pat, validate, OutputSink::File));
                let verdict = if t.dnf { "DNF" } else if t.bad_rows > 0 { "WRONG" } else if t.ok { "PASS" } else { "ERROR" };
                let bad_str = if t.bad_rows > 0 { format!("  bad={}", t.bad_rows) } else { String::new() };
                eprintln!("{:>8}  rows={:<8}{} {}", fms(t.wall_ms), t.rows, bad_str, verdict);
                all_rows.push(Row { tool: Tool::Uffs, phase: Phase::Cold, sink: OutputSink::File,
                    drive: drive.clone(), pat: label.into(), runs: vec![t] });
            }
        }

        // ── UFFS WARM (file sink only; same reasoning as COLD) ────────────
        if cfg.tools.contains(&Tool::Uffs) && !cfg.skip_cold {
            eprint!("  UFFS WARM: stopping daemon (cache stays)...");  flush();
            uffs_stop(&cfg.uffs);
            eprintln!(" done.");

            for &(label, pat, _, _, _, validate) in PATTERNS {
                if cfg.skip_pattern(label) { continue; }
                eprint!("    {label:<12} ");  flush();
                uffs_stop(&cfg.uffs);
                uffs_start(&cfg.uffs);
                let t = check_dnf(run_uffs(&cfg.uffs, drive, pat, validate, OutputSink::File));
                let verdict = if t.dnf { "DNF" } else if t.bad_rows > 0 { "WRONG" } else if t.ok { "PASS" } else { "ERROR" };
                let bad_str = if t.bad_rows > 0 { format!("  bad={}", t.bad_rows) } else { String::new() };
                eprintln!("{:>8}  rows={:<8}{} {}", fms(t.wall_ms), t.rows, bad_str, verdict);
                all_rows.push(Row { tool: Tool::Uffs, phase: Phase::Warm, sink: OutputSink::File,
                    drive: drive.clone(), pat: label.into(), runs: vec![t] });
            }
        }

        // ── UFFS HOT daemon warmup (once per drive, before the sink loop) ─
        if cfg.tools.contains(&Tool::Uffs) {
            // Tiny query just to trigger daemon startup + index load for this
            // drive.  Use a pattern that matches nothing, and limit 1 to
            // minimise I/O.  Done once per drive — the daemon stays warm
            // across every sink iteration below.
            eprint!("  UFFS HOT:  warming up daemon...");  flush();
            uffs_start(&cfg.uffs);
            let _ = Command::new(&cfg.uffs)
                .args(["__uffs_warmup_probe__", "--drive", drive, "--limit", "1"])
                .stdout(Stdio::null()).stderr(Stdio::null())
                .status();
            eprintln!(" ready.");
        }

        // ── Per-sink HOT rotation (interleaved rounds, randomised tool order) ─
        //
        // For each (sink, pattern) the tools run in a freshly shuffled order
        // every round so no tool consistently benefits from OS file-system
        // caching warmed up by a prior tool.  After every round the superset
        // constraint is checked immediately:
        //
        //   uffs.exe rows  ≥  uffs.com rows  ≥  es.exe rows
        //
        // This reflects the fact that UFFS (Rust) covers all files including
        // system files / ADS when --hide-system / --hide-ads are NOT passed
        // (here they ARE, so the counts should be close), and that Everything
        // does not index NTFS metadata files that uffs.com does include.
        // A violation is printed as a warning per round — it does not abort —
        // so timing data is preserved even when parity is off.
        for sink in cfg.sinks.iter().copied() {
            if cfg.sinks.len() > 1 {
                println!("  ── sink={}  ──────────────────────────────────────────────", sink.label());
            }

            for &(label, pat, es_pat, cpp_pat, cpp_ext, validate) in PATTERNS {
                if cfg.skip_pattern(label) { continue; }

                // Skip full_scan for Everything (2GB IPC ceiling).
                let es_skip = label == "full_scan";
                // Skip patterns C++ does not support.
                let cpp_skip = cpp_pat.is_empty();

                let run_uffs_tool  = cfg.tools.contains(&Tool::Uffs);
                let run_cpp_tool   = cfg.tools.contains(&Tool::UffsCpp) && cfg.uffs_cpp.is_some() && !cpp_skip;
                let run_es_tool    = cfg.tools.contains(&Tool::Everything) && cfg.es.is_some() && !es_skip;

                if !run_uffs_tool && !run_cpp_tool && !run_es_tool { continue; }

                eprintln!("  HOT [{sink}] {label:<12}  {} rounds  (tools shuffled each round)",
                    cfg.rounds, sink = sink.label());

                let mut uffs_runs: Vec<Timing> = Vec::new();
                let mut cpp_runs:  Vec<Timing> = Vec::new();
                let mut es_runs:   Vec<Timing> = Vec::new();
                let mut es_aborted = false;

                // Per-tool output files for this pattern — kept alive for the
                // duration of a round so path sets can be compared, then deleted.
                // C++ cannot write to absolute paths, so all three use relative names.
                let f_uffs = format!("bench_uffs_{label}.csv");
                let f_cpp  = format!("bench_cpp_{label}.csv");
                let f_es   = format!("bench_es_{label}.csv");

                for round in 0..cfg.rounds {
                    // Fresh random tool order every round — seeded from wall clock
                    // nanoseconds so consecutive rounds get different seeds.
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as u64 + round as u64 * 1_000_000_007)
                        .unwrap_or(round as u64 + 1);
                    let order = lcg_shuffle3(seed);

                    let mut round_rows: [Option<u64>; 3] = [None; 3]; // [uffs, cpp, es]

                    for &slot in &order {
                        match slot {
                            0 if run_uffs_tool => {
                                let t = check_dnf(run_uffs_to(&cfg.uffs, drive, pat, validate, sink, &f_uffs));
                                round_rows[0] = t.ok.then_some(t.rows);
                                uffs_runs.push(t);
                            }
                            1 if run_cpp_tool => {
                                let cpp = cfg.uffs_cpp.as_ref().unwrap();
                                let t = check_dnf(run_uffs_cpp_to(cpp, drive, cpp_pat, cpp_ext, validate, sink, &f_cpp));
                                round_rows[1] = t.ok.then_some(t.rows);
                                cpp_runs.push(t);
                            }
                            2 if run_es_tool && !es_aborted => {
                                let es = cfg.es.as_ref().unwrap();
                                let t = check_dnf(run_es_to(es, drive, es_pat, validate, sink, cfg.es_instance.as_deref(), &f_es));
                                if round == 0 && is_fast_deterministic_fail(&t) {
                                    eprintln!("    [round {round}] es.exe fast-fail (exit={}); skipping remaining es rounds", t.err);
                                    es_aborted = true;
                                }
                                round_rows[2] = t.ok.then_some(t.rows);
                                es_runs.push(t);
                            }
                            _ => {}
                        }
                    }

                    // ── Path-set superset check after every round (File sink only) ──
                    // Superset contract:
                    //   es.exe paths  ⊆  uffs.com paths  ⊆  uffs.exe paths
                    // Stdout/Null sinks don't retain output files so we skip
                    // the set check there; row counts are still shown.
                    let round_order_labels: Vec<&str> = order.iter().map(|&s| match s {
                        0 => "uffs", 1 => "cpp", 2 => "es", _ => "?"
                    }).collect();

                    let mut violations: Vec<String> = Vec::new();
                    if matches!(sink, OutputSink::File) {
                        let uffs_paths = extract_paths_from_file(&f_uffs);
                        let cpp_paths  = extract_paths_from_file(&f_cpp);
                        let es_paths   = extract_paths_from_file(&f_es);

                        if run_es_tool && !es_aborted && run_uffs_tool
                            && !uffs_paths.is_empty() && !es_paths.is_empty() {
                            if let Some(v) = check_subset("es", &es_paths, "uffs", &uffs_paths) {
                                violations.push(v);
                            }
                        }
                        if run_es_tool && !es_aborted && run_cpp_tool
                            && !cpp_paths.is_empty() && !es_paths.is_empty() {
                            if let Some(v) = check_subset("es", &es_paths, "cpp", &cpp_paths) {
                                violations.push(v);
                            }
                        }
                        if run_cpp_tool && run_uffs_tool
                            && !uffs_paths.is_empty() && !cpp_paths.is_empty() {
                            if let Some(v) = check_subset("cpp", &cpp_paths, "uffs", &uffs_paths) {
                                violations.push(v);
                            }
                        }
                    }

                    // Clean up per-tool files for this round.
                    cleanup_file(&f_uffs);
                    cleanup_file(&f_cpp);
                    cleanup_file(&f_es);

                    if violations.is_empty() {
                        eprintln!("    [round {:>2}] order=[{}]  rows: uffs={} cpp={} es={}  ✓ paths ok",
                            round + 1,
                            round_order_labels.join(","),
                            round_rows[0].map_or("-".into(), |n| n.to_string()),
                            round_rows[1].map_or("-".into(), |n| n.to_string()),
                            round_rows[2].map_or("-".into(), |n| n.to_string()),
                        );
                    } else {
                        eprintln!("    [round {:>2}] order=[{}]  rows: uffs={} cpp={} es={}  ⚠ PATH SUPERSET VIOLATION: {}",
                            round + 1,
                            round_order_labels.join(","),
                            round_rows[0].map_or("-".into(), |n| n.to_string()),
                            round_rows[1].map_or("-".into(), |n| n.to_string()),
                            round_rows[2].map_or("-".into(), |n| n.to_string()),
                            violations.join(" | "),
                        );
                    }
                }

                // ── Per-tool summary lines ────────────────────────────────
                if run_uffs_tool && !uffs_runs.is_empty() {
                    let s = sw(&uffs_runs);
                    let mut dm: Vec<u64> = uffs_runs.iter().filter(|r| r.ok && r.daemon_ms > 0).map(|r| r.daemon_ms).collect();
                    dm.sort();
                    let daemon_str = if dm.is_empty() { String::new() } else { format!("  daemon_p50={}", fms(p50(&dm))) };
                    let any_bad = uffs_runs.iter().any(|r| r.bad_rows > 0);
                    let verdict = if uffs_runs.iter().any(|r| r.dnf) { "DNF" } else if any_bad { "WRONG" } else { "PASS" };
                    let first_ok = uffs_runs.iter().find(|r| r.ok);
                    eprintln!("    UFFS     p50={:>6}  p95={:>6}{}  rows={}  {}",
                        fms(p50(&s)), fms(p95(&s)), daemon_str,
                        first_ok.map_or(0, |r| r.rows), verdict);
                    all_rows.push(Row { tool: Tool::Uffs, phase: Phase::Hot, sink,
                        drive: drive.clone(), pat: label.into(), runs: uffs_runs });
                }
                if run_cpp_tool && !cpp_runs.is_empty() {
                    let s = sw(&cpp_runs);
                    let any_bad = cpp_runs.iter().any(|r| r.bad_rows > 0);
                    let verdict = if cpp_runs.iter().any(|r| r.dnf) { "DNF" } else if any_bad { "WRONG" } else if cpp_runs.iter().all(|r| r.ok) { "PASS" } else { "ERROR" };
                    let first_ok = cpp_runs.iter().find(|r| r.ok);
                    eprintln!("    UFFS-C++ p50={:>6}  p95={:>6}  rows={}  {}",
                        fms(p50(&s)), fms(p95(&s)), first_ok.map_or(0, |r| r.rows), verdict);
                    all_rows.push(Row { tool: Tool::UffsCpp, phase: Phase::Hot, sink,
                        drive: drive.clone(), pat: label.into(), runs: cpp_runs });
                }
                if run_es_tool && !es_runs.is_empty() {
                    let s = sw(&es_runs);
                    let any_bad = es_runs.iter().any(|r| r.bad_rows > 0);
                    let abort_str = if es_aborted { format!("  (fast-fail, {} rounds)", es_runs.len()) } else { String::new() };
                    let verdict = if es_runs.iter().any(|r| r.dnf) { "DNF" } else if any_bad { "WRONG" } else if es_runs.iter().all(|r| r.ok) { "PASS" } else { "ERROR" };
                    let first_ok = es_runs.iter().find(|r| r.ok);
                    eprintln!("    ES       p50={:>6}  p95={:>6}  rows={}  {}{}",
                        fms(p50(&s)), fms(p95(&s)), first_ok.map_or(0, |r| r.rows), verdict, abort_str);
                    all_rows.push(Row { tool: Tool::Everything, phase: Phase::Hot, sink,
                        drive: drive.clone(), pat: label.into(), runs: es_runs });
                }
                if run_es_tool && es_skip {
                    eprintln!("    {label:<12} ES SKIP (es.exe 2GB IPC limit)");
                }
                if run_cpp_tool && cpp_skip {
                    eprintln!("    {label:<12} C++ SKIP (pattern not supported)");
                }
            }
        }

        println!();
    }

    // ── Summary table ────────────────────────────────────────────────────────
    print_summary(&cfg, &all_rows);

    // ── Optional CSV sink for the summary (one row per Tool × Phase × Sink ×
    //    Drive × Pattern combination).  Distinct from the daemon's intermediate
    //    `uffs_bench_out.csv`, which holds the raw per-query row dumps and is
    //    overwritten every round.  The two never collide: this file lives at
    //    whatever path the operator passes via `--out`, the other is fixed at
    //    `<cwd>/uffs_bench_out.csv`.
    if let Some(ref path) = cfg.out {
        match write_summary_csv(&cfg, &all_rows, path) {
            Ok(()) => {
                println!();
                println!("Summary CSV written: {}  ({} rows)", path.display(), all_rows.len());
            }
            Err(e) => {
                eprintln!();
                eprintln!("ERROR: failed to write summary CSV to {}: {}", path.display(), e);
                std::process::exit(2);
            }
        }
    }
}


// ── Summary ──────────────────────────────────────────────────────────────────
fn print_summary(cfg: &Cfg, rows: &[Row]) {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                           SUMMARY TABLE                                    ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!();

    // Header — Sink column appears left of Verdict so tools at the same
    // phase/pattern but different sinks sort adjacently in typical viewers.
    println!("| Drive | Tool         | Phase | Sink   | Pattern      | p50      | p95      | Rows   | Bad  | Verdict |");
    println!("|-------|--------------|-------|--------|--------------|----------|----------|--------|------|---------|");

    for row in rows {
        let s = sw(&row.runs);
        let any_dnf = row.runs.iter().any(|r| r.dnf);
        let all_ok = row.runs.iter().all(|r| r.ok);
        let any_bad = row.runs.iter().any(|r| r.bad_rows > 0);
        let verdict = if any_dnf { "DNF" } else if any_bad { "WRONG" } else if all_ok { "PASS" } else { "ERROR" };

        let p50_str = if s.is_empty() { "—".to_string() } else { fms(p50(&s)) };
        let p95_str = if s.is_empty() { "—".to_string() } else { fms(p95(&s)) };
        // Null-sink rows have nothing to count by design; render "—" instead
        // of a misleading "0" so readers don't mistake it for a failure.
        let rows_cell = |r: &Timing| -> String {
            if matches!(row.sink, OutputSink::Null) { "—".into() } else { format!("{}", r.rows) }
        };
        let bad_cell = |r: &Timing| -> String {
            if matches!(row.sink, OutputSink::Null) { "—".into() }
            else if r.bad_rows == 0 { "0".into() }
            else { format!("{}", r.bad_rows) }
        };
        let rows_str = row.runs.iter().find(|r| r.ok).map_or("—".into(), rows_cell);
        let bad_str  = row.runs.iter().find(|r| r.ok).map_or("—".into(), bad_cell);

        // Print any errors from failed runs
        for r in &row.runs {
            if !r.ok && !r.err.is_empty() {
                eprintln!("  ⚠ {} {} [{}] {}/{}: {}", row.tool.label(), row.phase.label(),
                    row.sink.label(), row.drive, row.pat, r.err);
            }
            if r.bad_rows > 0 {
                eprintln!("  ⚠ {} {} [{}] {}/{}: {} rows failed validation",
                    row.tool.label(), row.phase.label(), row.sink.label(),
                    row.drive, row.pat, r.bad_rows);
            }
        }

        println!("| {:<5} | {:<12} | {:<5} | {:<6} | {:<12} | {:>8} | {:>8} | {:>6} | {:>4} | {:<7} |",
            format!("{}:", row.drive), row.tool.label(), row.phase.label(),
            row.sink.label(), row.pat, p50_str, p95_str, rows_str, bad_str, verdict);
    }

    println!();

    // ── Cross-tool HOT comparison, one table per sink ────────────────────
    // Splitting by sink keeps the columns aligned (no sink column on every
    // row) and makes the sink-to-sink deltas easy to eyeball in one pass.
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                     HOT COMPARISON (head-to-head)                          ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    for sink in cfg.sinks.iter().copied() {
        println!();
        println!("── sink = {} ────────────────────────────────────────────────", sink.label());
        println!("| Drive | Pattern      | UFFS HOT p50 | UFFS-C++ p50 | Everything p50 |");
        println!("|-------|--------------|--------------|--------------|----------------|");

        for drive in &cfg.drives {
            for &(label, _, _, _, _, _) in PATTERNS {
                if cfg.skip_pattern(label) { continue; }
                let uffs_p50 = find_p50(rows, Tool::Uffs,       Phase::Hot, sink, drive, label);
                let cpp_p50  = find_p50(rows, Tool::UffsCpp,    Phase::Hot, sink, drive, label);
                let es_p50   = find_p50(rows, Tool::Everything, Phase::Hot, sink, drive, label);
                println!("| {:<5} | {:<12} | {:>12} | {:>12} | {:>14} |",
                    format!("{}:", drive), label, uffs_p50, cpp_p50, es_p50);
            }
        }
    }

    println!();
    println!("Legend:  PASS = completed within {}s.  DNF = timed out.  SKIP = tool not found.", TIMEOUT.as_secs());
    println!("Note:   UFFS (Rust) has three phases: COLD (no cache), WARM (cache), HOT (daemon).");
    println!("        UFFS (C++) re-reads MFT every invocation (no daemon).");
    println!("        Everything is always-hot (daemon model).");
    println!("        Sinks: file = --out=/-export-csv → disk; stdout = piped stdout;");
    println!("               null = child process writes to a real NUL device via cmd /C.");
    println!("        Null-sink rows show '—' for Rows/Bad (nothing to count after the");
    println!("        redirect); correctness is verified by the file-mode passes.");
    println!("        UltraSearch excluded — no functional headless CLI (see script header).");
}

fn find_p50(rows: &[Row], tool: Tool, phase: Phase, sink: OutputSink, drive: &str, pat: &str) -> String {
    rows.iter()
        .find(|r| r.tool == tool && r.phase == phase && r.sink == sink && r.drive == drive && r.pat == pat)
        .map(|r| {
            let s = sw(&r.runs);
            if s.is_empty() { "—".to_string() } else { fms(p50(&s)) }
        })
        .unwrap_or_else(|| "SKIP".to_string())
}


// ── Summary CSV writer ───────────────────────────────────────────────────────
/// Write the post-run summary as CSV at `path`.  One row per Tool × Phase ×
/// Sink × Drive × Pattern combination.  Columns mirror the stdout summary
/// table but are emitted as plain integer milliseconds (no human-friendly
/// `ms` / `s` formatting) so the file is trivially regress-able by downstream
/// tooling (`pandas`, `polars`, `awk`, …).
///
/// Null-sink rows (where the child process redirected output to a real NUL
/// device) report `0` for `rows` and `bad` since nothing is counted by design;
/// the stdout summary distinguishes these with a literal `—` placeholder, but
/// CSV consumers prefer numeric columns.  Verdict carries the same value
/// either way, so the distinction is recoverable when the consumer cares.
///
/// `rounds_ok` counts entries with `t.ok == true`; `rounds_total` is the full
/// run vec length (which may be `< cfg.rounds` for runs short-circuited via
/// `is_fast_deterministic_fail`).
fn write_summary_csv(_cfg: &Cfg, rows: &[Row], path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut f = std::fs::File::create(path)?;
    writeln!(
        f,
        "tool,phase,sink,drive,pattern,p50_ms,p95_ms,rows,bad,verdict,rounds_ok,rounds_total"
    )?;
    for row in rows {
        let s = sw(&row.runs);
        let any_dnf = row.runs.iter().any(|r| r.dnf);
        let all_ok = row.runs.iter().all(|r| r.ok);
        let any_bad = row.runs.iter().any(|r| r.bad_rows > 0);
        let verdict = if any_dnf { "DNF" }
            else if any_bad { "WRONG" }
            else if all_ok { "PASS" }
            else { "ERROR" };
        let p50_ms = if s.is_empty() { 0 } else { p50(&s) };
        let p95_ms = if s.is_empty() { 0 } else { p95(&s) };
        let first_ok = row.runs.iter().find(|r| r.ok);
        let (rows_count, bad_count) = if matches!(row.sink, OutputSink::Null) {
            (0u64, 0u64)
        } else {
            (first_ok.map_or(0, |r| r.rows), first_ok.map_or(0, |r| r.bad_rows))
        };
        let rounds_ok = row.runs.iter().filter(|r| r.ok).count();
        let rounds_total = row.runs.len();
        writeln!(
            f,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            row.tool.label(),
            row.phase.label(),
            row.sink.label(),
            row.drive,
            row.pat,
            p50_ms,
            p95_ms,
            rows_count,
            bad_count,
            verdict,
            rounds_ok,
            rounds_total,
        )?;
    }
    Ok(())
}