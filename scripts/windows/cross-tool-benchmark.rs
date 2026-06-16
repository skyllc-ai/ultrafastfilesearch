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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_ROUNDS: usize = 10;
const DEFAULT_DRIVES: &[&str] = &["C", "D"];
/// Warm rounds run against the daemon (and implicitly the OS FS cache) right
/// before each HOT head-to-head, so UFFS competes from a fully-primed index —
/// fair against ES (always pre-indexed) and the C++ tool (re-reads every MFT).
const PRIME_ROUNDS: usize = 3;

/// Resolved root for ALL benchmark artifacts, set once in `main` from the
/// `--out-dir` flag / `$UFFS_BENCH_DIR` (see [`resolve_bench_dir`]).  Everything
/// this script writes lands under here instead of scattering CSVs across the
/// cwd.  Falls back to `.` only if `main` somehow never initialised it.
static BENCH_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the consolidated bench-artifact root.  Precedence:
///   1. `--out-dir <path>` (caller-supplied)
///   2. `$UFFS_BENCH_DIR`
///   3. `%LOCALAPPDATA%\uffs-bench` (Windows)
///   4. `$XDG_CACHE_HOME|~/.cache` + `/uffs-bench`
///
/// This mirrors the `_bench-dir` helper in `just/bench_uffs.just` exactly, so
/// the standalone script and the `just` flow write to the SAME tree (and share
/// one `baseline.json`).
fn resolve_bench_dir(flag: Option<&Path>) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    if let Ok(v) = env::var("UFFS_BENCH_DIR") {
        if !v.is_empty() { return PathBuf::from(v); }
    }
    if let Ok(v) = env::var("LOCALAPPDATA") {
        if !v.is_empty() { return PathBuf::from(v).join("uffs-bench"); }
    }
    let base = env::var("XDG_CACHE_HOME").ok().filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cache"));
    base.join("uffs-bench")
}

/// Scratch subdir under the resolved root for transient per-query / per-tool
/// CSVs that are overwritten every round and cleaned up after each diff.
/// Created on demand; falls back to cwd before `main` initialises [`BENCH_DIR`].
fn bench_scratch_dir() -> PathBuf {
    let dir = BENCH_DIR.get().map_or_else(|| PathBuf::from("."), |d| d.join("scratch"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Absolute path to the daemon's intermediate per-query dump under the scratch
/// dir.  Both the C++ tool (`--out=`) and Everything (`-export-csv`) accept
/// absolute paths, so no chdir / relative-path juggling is needed.
fn bench_out_path() -> String {
    bench_scratch_dir().join("uffs_bench_out.csv").to_string_lossy().into_owned()
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

/// Patterns worth running in the COLD and WARM phases.
///
/// COLD/WARM wall time is dominated by the MFT read + index (de)serialisation
/// — a fixed per-drive cost that is **independent of the query pattern**.  The
/// only thing the pattern changes in those phases is the output-write cost,
/// which scales with result-set size.  Running all 7 patterns there just
/// re-measures the same index-load floor 7 times, so we restrict COLD/WARM to:
///
///   - `exact`     — tiny result set ⇒ measures the pure index-load floor.
///   - `full_scan` — 3M+ rows       ⇒ measures the max output-write cost.
///
/// All 7 patterns still run in HOT, where pure query execution dominates and
/// the engines (trie / prefix / regex / full-scan) genuinely diverge.
const COLD_WARM_PATTERNS: &[&str] = &["exact", "full_scan"];

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
             /// own intermediate `uffs_bench_out.csv` under the scratch dir,
             /// which holds the raw per-query row dumps and is overwritten
             /// every round.
             out: Option<PathBuf>,
             /// Resolved consolidated bench-artifact root (see
             /// [`resolve_bench_dir`]).  All transient scratch CSVs land under
             /// `<out_dir>/scratch/`.
             out_dir: PathBuf }
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
/// Locate `Everything.exe` (the GUI/service binary) so the bench can launch a
/// private, drive-scoped instance.  Distinct from `es.exe` (the CLI client).
fn find_everything() -> Option<PathBuf> {
    where_exe("Everything.exe").or_else(|| {
        let (h, pf, pf86) = (env::var("USERPROFILE").unwrap_or_default(),
            env::var("ProgramFiles").unwrap_or_default(),
            env::var("ProgramFiles(x86)").unwrap_or_default());
        find_in(&[
            PathBuf::from(&pf).join("Everything").join("Everything.exe"),
            PathBuf::from(&pf86).join("Everything").join("Everything.exe"),
            PathBuf::from(&h).join("bin").join("Everything.exe"),
        ])
    })
}


// ── UFFS lifecycle ───────────────────────────────────────────────────────────
fn uffs_stop(bin: &Path) {
    // "daemon kill" is a hard kill; "daemon stop" is a graceful shutdown
    // that may leave shared memory / cache warm.
    let _ = Command::new(bin).args(["--daemon","kill"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    std::thread::sleep(Duration::from_secs(2));
}
/// Start the daemon with bench-safe idle-demote TTLs so it never demotes
/// Hot→Warm or Warm→Parked mid-run.  These env vars are scoped to the
/// daemon child process only — the bench script's own env is unchanged,
/// and teardown's next `uffs --daemon start` gets production defaults.
///
/// `drives` scopes which drives the daemon loads on startup.  This is
/// **essential** for fair timing: without an explicit `--drive` flag the
/// daemon discovers and loads *every* NTFS volume on the host, so the
/// WARM/HOT load cost would reflect all drives rather than just the one(s)
/// under test.  Each letter is forwarded as a separate `--drive <X>`
/// (`parse_daemon_start` in uffs-cli accumulates them).
fn uffs_start(bin: &Path, drives: &[String]) {
    let mut args: Vec<String> = vec!["--daemon".into(), "start".into()];
    for d in drives {
        args.push("--drive".into());
        args.push(d.clone());
    }
    let _ = Command::new(bin)
        .args(&args)
        .env("UFFS_HOT_TO_WARM_IDLE_SECS",   "3600")
        .env("UFFS_WARM_TO_PARKED_IDLE_SECS", "7200")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status();
    std::thread::sleep(Duration::from_millis(500));
}
// ── As-found run-state snapshot/restore ──────────────────────────────────────
// The bench repeatedly kills the daemon and restarts it scoped to the drives
// under test.  To leave the host as we found it, snapshot the original run-state
// (which drives the daemon had loaded, MCP up/down) BEFORE the first kill and
// restore it at teardown.  ES is untouched — it always runs as a private
// `-instance` sandbox.

/// The host's UFFS run-state at bench start.
struct RunState {
    /// Drive letters the daemon had loaded, or `None` if it was not running.
    daemon_drives: Option<Vec<String>>,
    /// Whether the MCP HTTP gateway was running.
    mcp_running: bool,
}

/// Whether a `Status:` value reads as "running" (`"not running"` contains the
/// substring `"running"`, so it is excluded first).
fn status_is_running(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !lower.contains("not running") && !lower.contains("not responding") && lower.contains("running")
}

/// Extract a drive letter from a `uffs --status` line like `"[W] G:  … records"`.
fn status_drive_letter(line: &str) -> Option<String> {
    let after = line.strip_prefix('[')?.split_once(']')?.1.trim_start();
    let mut chars = after.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':'))
        .then(|| letter.to_ascii_uppercase().to_string())
}

/// Parse `uffs --status` stdout into a [`RunState`], scoping each `Status:` line
/// and the `[T] L:` drive lines to their section.
fn parse_run_state(stdout: &str) -> RunState {
    let (mut section, mut daemon_running, mut daemon_seen, mut mcp_running) = (0_u8, false, false, false);
    let mut drives: Vec<String> = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.contains("── Daemon") { section = 1; daemon_seen = true; continue; }
        if line.contains("MCP HTTP Gateway") { section = 2; continue; }
        if line.contains("MCP Stdio") { section = 3; continue; }
        match section {
            1 => {
                if let Some(rest) = line.strip_prefix("Status:") { daemon_running = status_is_running(rest); }
                else if let Some(d) = status_drive_letter(line) { drives.push(d); }
            }
            2 => if let Some(rest) = line.strip_prefix("Status:") { mcp_running = status_is_running(rest); },
            _ => {}
        }
    }
    RunState {
        daemon_drives: if daemon_seen && daemon_running { Some(drives) } else { None },
        mcp_running,
    }
}

/// Capture the as-found daemon + MCP state via `<bin> status`.
fn capture_run_state(bin: &Path) -> RunState {
    match Command::new(bin).arg("--status").output() {
        Ok(out) => parse_run_state(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => RunState { daemon_drives: None, mcp_running: false },
    }
}

/// Restore the daemon + MCP to the captured `state`, using **production** TTL
/// defaults (not the bench-safe idle envs `uffs_start` sets).
fn restore_run_state(bin: &Path, state: &RunState) {
    eprintln!("── Restoring UFFS run-state to as-found ──");
    uffs_stop(bin); // hard-kill whatever the bench left running
    match &state.daemon_drives {
        None => eprintln!("  daemon was stopped at start — leaving it stopped"),
        Some(drives) => {
            let scope = if drives.is_empty() { "(all)".to_string() } else { drives.join(",") };
            eprintln!("  restarting daemon on as-found drives: {scope}");
            let mut args: Vec<String> = vec!["--daemon".into(), "start".into()];
            for d in drives { args.push("--drive".into()); args.push(d.clone()); }
            let _ = Command::new(bin).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
    if state.mcp_running {
        eprintln!("  restarting MCP gateway (was up at start)");
        let _ = Command::new(bin).args(["--mcp", "start"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
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
/// Prime the daemon for peak HOT performance over `drive_spec` (a single `"C"`
/// or a CSV `"C,D,G"`): run `rounds` warm full-scan searches with `--no-output`
/// (rows discarded — the daemon still executes the search, warming the hot tier
/// and OS FS cache).  This makes the UFFS HOT comparison fair against ES (fully
/// pre-indexed) and the C++ tool (re-reads every MFT each invocation).
fn prime_daemon(bin: &Path, drive_spec: &str, rounds: usize) {
    eprint!("  priming daemon ({rounds} rounds, drives={drive_spec})...");
    flush();
    for _ in 0..rounds {
        let mut args: Vec<String> = vec!["*".into()];
        if drive_spec.contains(',') {
            args.push(format!("--drives={drive_spec}"));
        } else {
            args.push("--drive".into());
            args.push(drive_spec.into());
        }
        args.push("--no-output".into());
        let _ = Command::new(bin).args(&args)
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    eprintln!(" ready.");
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

/// Extract the first CSV field from `line`, honouring double-quote quoting so a
/// path that itself contains a comma (`"c:\a,b\c",123`) stays one field instead
/// of being split.  Unquoted lines fall back to a plain comma split.
fn first_csv_field(line: &str) -> &str {
    let line = line.trim();
    match line.strip_prefix('"') {
        Some(rest) => rest
            .find('"')
            .map_or(rest, |end| rest.get(..end).unwrap_or(rest)),
        None => line.split(',').next().unwrap_or(line),
    }
}

/// Canonicalise a path so the same filesystem entry compares equal across
/// tools.  The tools disagree on directory formatting: `uffs.com` (C++) emits a
/// trailing `\` on directories (`c:\config.msi\`) while Everything and
/// UFFS-Rust do not (`c:\config.msi`).  Left unnormalised, every directory hit
/// becomes a spurious "only in cpp" / "only in es" pair (this was the bulk of
/// the reported 11226 / 3736 diff).  Canonical form: `/`→`\`, lowercase, and a
/// single stripped trailing separator.  The bare drive root keeps its slash
/// (`c:` / `c:\` / `c:\\` all collapse to `c:\`) so it never degrades to `c:`.
fn canon_path(raw: &str) -> String {
    let lower = raw.replace('/', "\\").to_lowercase();
    let stripped = lower.trim_end_matches('\\');
    if stripped.len() == 2 && stripped.as_bytes().get(1) == Some(&b':') {
        format!("{stripped}\\") // bare drive root → `c:\`
    } else {
        stripped.to_owned()
    }
}

/// Read a tool's output file, strip headers/footers, canonicalise each path
/// (first CSV field, `/`→`\`, lowercase, no trailing dir separator), and return
/// a **sorted, de-duplicated** vec.  Handles both UTF-8 and UTF-16 LE (BOM)
/// output.  The canonicalisation is what lets a C++ `c:\dir\` line match an
/// Everything `c:\dir` line — see [`canon_path`].
fn normalise_paths(path: &str) -> Vec<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => match std::fs::read(path) {
            Ok(bytes) if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE => {
                let u16s: Vec<u16> = bytes[2..].chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&u16s)
            }
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => return Vec::new(),
        },
    };
    let mut v: Vec<String> = content.lines()
        .filter(|l| !is_header_or_footer(l))
        .map(|l| canon_path(first_csv_field(l)))
        .filter(|s| !s.is_empty())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Result of comparing two sorted normalised path lists.
struct DiffResult {
    only_in_a: Vec<String>,  // paths present in A but not B
    only_in_b: Vec<String>,  // paths present in B but not A
}

impl DiffResult {
    fn is_identical(&self) -> bool { self.only_in_a.is_empty() && self.only_in_b.is_empty() }
}

/// Compare two **sorted** path vecs using a merge-walk (O(n)).  Collects all
/// differences; the caller prints at most `max_examples` from each side.
fn diff_paths(a: &[String], b: &[String]) -> DiffResult {
    let mut only_in_a = Vec::new();
    let mut only_in_b = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal   => { i += 1; j += 1; }
            std::cmp::Ordering::Less    => { only_in_a.push(a[i].clone()); i += 1; }
            std::cmp::Ordering::Greater => { only_in_b.push(b[j].clone()); j += 1; }
        }
    }
    only_in_a.extend_from_slice(&a[i..]);
    only_in_b.extend_from_slice(&b[j..]);
    DiffResult { only_in_a, only_in_b }
}

/// Resolve a (possibly relative) bench output path to an absolute path string
/// for display, so the diff header names the exact file each side came from.
fn abs_display(path: &str) -> String {
    env::current_dir()
        .map(|d| d.join(path))
        .unwrap_or_else(|_| PathBuf::from(path))
        .display()
        .to_string()
}

/// Print a human-readable diff summary between two tool outputs.
///
/// The header names the **full source file** each side was read from and its
/// row count; both lists were sorted + normalised by `normalise_paths` before
/// the merge-walk, so the examples appear in sorted order.  Shows up to
/// `max_examples` lines from each side so operators can spot patterns.
fn print_diff(
    a_label: &str, a_path: &str, a_n: usize,
    b_label: &str, b_path: &str, b_n: usize,
    diff: &DiffResult, max_examples: usize,
) {
    eprintln!("      diff {a_label} vs {b_label}  (sorted + normalised):");
    eprintln!("        {a_label:<4} ({a_n:>8} rows): {}", abs_display(a_path));
    eprintln!("        {b_label:<4} ({b_n:>8} rows): {}", abs_display(b_path));
    if diff.is_identical() {
        eprintln!("        result: identical ✓");
        return;
    }
    let show_a = diff.only_in_a.len().min(max_examples);
    let show_b = diff.only_in_b.len().min(max_examples);
    eprintln!("        result: ⚠ {} only in {a_label}, {} only in {b_label}",
        diff.only_in_a.len(), diff.only_in_b.len());
    if show_a > 0 {
        eprintln!("        only in {a_label} (first {show_a}):");
        for p in &diff.only_in_a[..show_a] { eprintln!("          - {p}"); }
        if diff.only_in_a.len() > max_examples {
            eprintln!("          ... and {} more", diff.only_in_a.len() - max_examples);
        }
    }
    if show_b > 0 {
        eprintln!("        only in {b_label} (first {show_b}):");
        for p in &diff.only_in_b[..show_b] { eprintln!("          + {p}"); }
        if diff.only_in_b.len() > max_examples {
            eprintln!("          ... and {} more", diff.only_in_b.len() - max_examples);
        }
    }
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
/// `drive` is either a single letter (`"C"`) for a per-drive step or a CSV
/// drive-spec (`"C,D,G"`) for the all-drives aggregate step.  The former emits
/// `--drive C`; the latter emits `--drives=C,D,G` (the uffs CLI parses the CSV
/// into multiple drive targets — see `commands/search/dispatch.rs`).
fn run_uffs_to(bin: &Path, drive: &str, pattern: &str, validate: &str, sink: OutputSink, bpath: &str) -> Timing {
    cleanup_file(bpath);
    let bpath = bpath.to_owned();
    // Path-only output for fair comparison with es.exe (which only outputs Filename).
    // --hide-system + --hide-ads bring UFFS result semantics in line with
    // Everything (which does not index NTFS system files or Alternate Data
    // Streams by default). Without these flags, UFFS returns 30-70% more
    // rows for broad patterns and the timing comparison is meaningless.
    let mut args: Vec<String> = vec![pattern.into()];
    if drive.contains(',') {
        args.push(format!("--drives={drive}"));
    } else {
        args.push("--drive".into());
        args.push(drive.into());
    }
    args.push("--columns".into()); args.push("Path".into());
    args.push("--hide-system".into()); args.push("--hide-ads".into());
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
    let mut args: Vec<String> = Vec::new();
    if let Some(inst) = es_instance {
        args.push("-instance".into());
        args.push(inst.into());
    }
    // Scope: a single drive uses the "<D>:\" path filter; the aggregate
    // drive-spec ("C,D,G") omits the filter so es searches the whole
    // instance — which the relaunched sandbox has scoped to exactly those
    // drives, so no per-path filter is needed (and one would only match a
    // single drive anyway).
    if !drive.contains(',') {
        args.push(format!("{drive}:\\"));
    }
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

// ── Everything: isolated bench instance ─────────────────────────────────────
// Ported from `crates/uffs-bench/src/run/es_instance.rs`.  When ES is part of
// the run the bench KILLS every running `Everything.exe` and launches a private
// sandbox instance:
//
//     Everything.exe -config <tempini> -instance uffs-bench -startup
//
// `<tempini>` is generated from the permanent `Everything.ini` but with
// `ntfs_volume_includes`/`ntfs_volume_monitors` set to 1 ONLY for the requested
// drives (0 for the rest) and all `auto_include_*`/`auto_remove_*` keys forced
// to 0 so ES does not auto-discover other volumes.  The permanent ini is never
// modified.  `es.exe` queries target the sandbox via `-instance uffs-bench`.

/// Named instance used for the bench-local Everything process.
const ES_INSTANCE_NAME: &str = "uffs-bench";
/// Poll budget waiting for the bench instance to finish indexing (60×5s = 5m).
const ES_LOAD_POLL_ATTEMPTS: u32 = 60;
const ES_LOAD_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace after asking existing instances to exit before spawning ours.
const ES_KILL_GRACE: Duration = Duration::from_secs(3);
/// Grace after spawning before the first IPC readiness poll.
const ES_STARTUP_GRACE: Duration = Duration::from_secs(5);

/// Permanent Everything.ini path (`%APPDATA%\Everything\Everything.ini`).
fn everything_ini_path() -> PathBuf {
    PathBuf::from(env::var("APPDATA").unwrap_or_default())
        .join("Everything").join("Everything.ini")
}

/// Temp path for the bench ini (prefers `%TEMP%`).
fn bench_ini_path() -> PathBuf {
    env::temp_dir().join("uffs-bench-everything.ini")
}

/// Temp path for the bench instance's Everything database.  Pinned (rather
/// than the per-instance default) so it can be deleted before every launch,
/// forcing a fresh index scoped to the current `ntfs_volume_includes` mask.
/// Without this the `uffs-bench` instance reuses the db from the PREVIOUS
/// per-drive launch (e.g. the C run), loads those drives, ignores the new
/// includes mask, and rewrites our temp ini to match — so an E run ends up
/// indexing C.
fn bench_db_path() -> PathBuf {
    env::temp_dir().join("uffs-bench-everything.db")
}

/// Parse a `key=val1,val2,...` Everything.ini array value into tokens.
/// Handles the quoted-string format ES uses (`"C:","D:"`); quoted tokens are
/// kept whole.
fn parse_ini_array(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        if rest.starts_with('"') {
            let close = rest.char_indices().skip(1).find(|(_, ch)| *ch == '"');
            let end = close.map_or(rest.len(), |(idx, _)| idx + 1);
            let (tok, tail) = rest.split_at(end);
            tokens.push(tok.to_owned());
            rest = tail.trim_start_matches(',');
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            let (tok, tail) = rest.split_at(end);
            tokens.push(tok.to_owned());
            rest = tail.trim_start_matches(',');
        }
    }
    tokens
}

/// Rebuild ini text replacing `ntfs_volume_includes`, `ntfs_volume_monitors`,
/// and `ntfs_volume_load_recent_changes` with the provided bitmask, pinning
/// `db_location` to `db_location`, and forcing the `auto_include_*`/
/// `auto_remove_*` keys to `0`.  Every other line is copied verbatim.
fn rebuild_ini(text: &str, includes: &str, monitors: &str, db_location: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let key = line.split_once('=').map_or("", |(k, _)| k.trim());
        match key {
            "ntfs_volume_includes" => {
                out.push_str("ntfs_volume_includes=");
                out.push_str(includes);
                out.push('\n');
            }
            "ntfs_volume_monitors" => {
                out.push_str("ntfs_volume_monitors=");
                out.push_str(monitors);
                out.push('\n');
            }
            "ntfs_volume_load_recent_changes" => {
                out.push_str("ntfs_volume_load_recent_changes=");
                out.push_str(includes);
                out.push('\n');
            }
            // Pin the db to our known temp path so es_launch can delete it
            // before each launch, guaranteeing a fresh index that honours the
            // includes mask above (instead of reusing a prior run's db).
            "db_location" => {
                out.push_str("db_location=");
                out.push_str(db_location);
                out.push('\n');
            }
            // Force to 0 — without this ES ignores ntfs_volume_paths and
            // auto-discovers every fixed NTFS drive on the machine.
            "auto_include_fixed_volumes"
            | "auto_include_removable_volumes"
            | "auto_include_fixed_refs_volumes"
            | "auto_include_removable_refs_volumes"
            | "auto_remove_offline_ntfs_volumes"
            | "auto_remove_moved_ntfs_volumes"
            | "auto_remove_offline_refs_volumes"
            | "auto_remove_moved_refs_volumes" => {
                out.push_str(key);
                out.push_str("=0\n");
            }
            _ => { out.push_str(line); out.push('\n'); }
        }
    }
    out
}

/// Write the bench `Everything.ini` into `ini_out`, including only `drives`.
fn write_bench_ini(ini_out: &Path, drives: &[String]) -> std::io::Result<()> {
    let permanent = everything_ini_path();
    let text = std::fs::read_to_string(&permanent).unwrap_or_default();
    let bench_set: Vec<char> = drives.iter()
        .filter_map(|d| d.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    // Map positional ntfs_volume_paths → include bit (1 for bench drives).
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "ntfs_volume_paths" { paths = parse_ini_array(v); break; }
        }
    }
    let includes: String = paths.iter().map(|tok| {
        let letter = tok.trim_matches('"').chars().next().unwrap_or(' ').to_ascii_uppercase();
        if bench_set.contains(&letter) { "1" } else { "0" }
    }).collect::<Vec<_>>().join(",");
    let monitors = includes.clone();
    // Diagnostic: surface the exact bit→volume mapping so a wrong bitpattern
    // (or an empty/garbled ntfs_volume_paths that would misalign the mask) is
    // visible in the run log. e.g. "C:=0 D:=0 E:=1 F:=0 ...".
    let map: String = paths.iter().zip(includes.split(','))
        .map(|(tok, bit)| format!("{}={bit}", tok.trim_matches('"')))
        .collect::<Vec<_>>().join(" ");
    eprintln!("  [es-instance] ini volumes ({} entries): {map}", paths.len());
    let db = bench_db_path();
    let db_str = db.to_string_lossy();
    let out = rebuild_ini(&text, &includes, &monitors, &db_str);
    std::fs::write(ini_out, out.as_bytes())
}

/// Ask any running Everything instances (default + stale bench) to exit.
fn es_kill_existing(everything: &Path) {
    let _ = Command::new(everything).args(["-exit"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = Command::new(everything).args(["-instance", ES_INSTANCE_NAME, "-exit"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Launch the sandboxed Everything instance indexing only `drives`.  Returns
/// the temp ini path so the caller can remove it on [`es_stop`].
fn es_launch(everything: &Path, drives: &[String]) -> Option<PathBuf> {
    if drives.is_empty() { return None; }
    let ini = bench_ini_path();
    // ORDER MATTERS. Kill any running instance FIRST and wait for it to fully
    // exit. A previous uffs-bench instance was launched with `-config <this
    // same temp ini>`; on shutdown Everything writes its current (wrong-drive)
    // volume state back to that ini. If we wrote the ini before killing, the
    // dying instance would clobber our freshly-written includes mask — which
    // is exactly why the relaunched instance kept indexing the old drive.
    es_kill_existing(everything);
    std::thread::sleep(ES_KILL_GRACE);
    // Delete the prior bench db so the relaunched instance rebuilds a fresh
    // index from the includes mask, rather than loading the previous per-drive
    // run's db.
    let db = bench_db_path();
    if db.exists() {
        if let Err(e) = std::fs::remove_file(&db) {
            eprintln!("  [es-instance] WARNING: could not remove stale db {} — {e}", db.display());
        } else {
            eprintln!("  [es-instance] removed stale db {}", db.display());
        }
    }
    // Now that no instance is alive to overwrite it, write the fresh ini.
    if let Err(e) = write_bench_ini(&ini, drives) {
        eprintln!("  [es-instance] WARNING: could not write temp ini — {e}");
        return None;
    }
    eprintln!("  [es-instance] launching Everything (drives: {}) …", drives.join(","));
    let ini_str = ini.to_string_lossy().to_string();
    let args = ["-config", ini_str.as_str(), "-instance", ES_INSTANCE_NAME, "-startup"];
    eprintln!("  [es-instance] spawn: {} {}", everything.display(), args.join(" "));
    match Command::new(everything).args(args)
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(_)  => Some(ini),
        Err(e) => { eprintln!("  [es-instance] WARNING: could not launch Everything — {e}"); None }
    }
}

/// Poll `es.exe -instance uffs-bench` until every drive reports a non-zero
/// result count, or the poll budget is exhausted.  Returns `true` when loaded.
fn es_wait_until_loaded(es: &Path, drives: &[String]) -> bool {
    std::thread::sleep(ES_STARTUP_GRACE);
    for attempt in 1..=ES_LOAD_POLL_ATTEMPTS {
        let counts: Vec<(String, u64)> = drives.iter().map(|d| {
            let search = format!("{d}:");
            let n = Command::new(es)
                .args(["-instance", ES_INSTANCE_NAME, search.as_str(), "-get-result-count"])
                .stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
                .unwrap_or(0);
            (d.clone(), n)
        }).collect();
        if counts.iter().all(|(_, n)| *n > 0) {
            eprintln!("  [es-instance] Everything index loaded — proceeding");
            return true;
        }
        let cs = counts.iter().map(|(d, n)| format!("{d}:{n}")).collect::<Vec<_>>().join(" ");
        eprintln!("  [es-instance] waiting for Everything to finish indexing … (attempt {attempt}/{ES_LOAD_POLL_ATTEMPTS}) [{cs}]");
        std::thread::sleep(ES_LOAD_POLL_INTERVAL);
    }
    eprintln!("  [es-instance] WARNING: Everything did not finish indexing within 5 minutes — ES cells measured with a partial index");
    false
}

/// Send `Everything.exe -instance uffs-bench -exit` and remove the temp ini
/// and pinned bench db so nothing stale is left for the next run.
fn es_stop(everything: &Path, ini: Option<&Path>) {
    let _ = Command::new(everything).args(["-instance", ES_INSTANCE_NAME, "-exit"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status();
    std::thread::sleep(ES_KILL_GRACE);
    if let Some(p) = ini { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(bench_db_path());
}

// ── Run: UFFS C++ (uffs.com) ─────────────────────────────────────────────────
/// C++ UFFS reads MFT every invocation (no daemon). No --limit flag.
/// Extension filter uses --ext=<ext> instead of glob *.ext.
/// Substring match needs *needle* glob wildcards.
///
/// # Sink notes
///
/// - `File`: emit `--out=<bench>`, inherit stdout (the C++ binary internally
///   `freopen()`s stdout onto the `--out=` file; pre-redirecting stdout to a
///   Rust pipe or NUL makes freopen fail silently and the output file comes out
///   empty), and send stderr to NUL.  stderr carries only the decorative
///   `Drives? … / Finished in N s` footer, which otherwise spams the console
///   and makes the per-round diff output unreadable.
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
            // freopen on --out= requires inherited stdout.  stderr (the
            // decorative footer) is discarded so it does not clutter the
            // bench's own progress/diff output.  We get no captured bytes
            // back but the file is what we validate here anyway.
            let t = Instant::now();
            let r = Command::new(bin).args(&args)
                .stdout(Stdio::inherit()).stderr(Stdio::null())
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
    let mut out_dir_flag: Option<PathBuf> = None;
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
            "--out-dir" | "--paths" => { i += 1; out_dir_flag = Some(PathBuf::from(&args[i])); }
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
    let out_dir = resolve_bench_dir(out_dir_flag.as_deref());
    Cfg { uffs, uffs_cpp, es, drives, rounds, tools, sinks, skip_cold, patterns: patterns_filter, es_instance, out, out_dir }
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
    eprintln!("                        uffs_bench_out.csv under <out-dir>/scratch/ (raw");
    eprintln!("                        per-query rows, overwritten every round).");
    eprintln!("  --out-dir <dir>       Consolidated root for ALL bench artifacts");
    eprintln!("                        (alias: --paths).  Transient scratch CSVs land");
    eprintln!("                        in <dir>/scratch/.  Precedence: this flag >");
    eprintln!("                        $UFFS_BENCH_DIR > %LOCALAPPDATA%\\uffs-bench >");
    eprintln!("                        ~/.cache/uffs-bench.  Matches `just _bench-dir`.");
    eprintln!("  --help                This message");
}

// ── HOT head-to-head comparison ──────────────────────────────────────────────
/// Run the HOT cross-tool comparison for `drive` — either a single letter
/// (`"C"`) or a CSV drive-spec (`"C,D,G"`) for the all-drives aggregate.
///
/// The caller is responsible for having ALREADY scoped every tool to exactly
/// these drives (daemon restarted + primed, ES sandbox relaunched with the same
/// drive set) so the timings are apples-to-apples.  For each sink × pattern this
/// runs `cfg.rounds` rounds in a freshly-shuffled tool order, prints a per-round
/// row-count line, diffs the normalised path lists (File sink), and appends one
/// summary `Row` per tool to `all_rows`.
fn run_hot_compare(cfg: &Cfg, drive: &str, all_rows: &mut Vec<Row>) {
    for sink in cfg.sinks.iter().copied() {
        if cfg.sinks.len() > 1 {
            println!("  ── sink={}  ──────────────────────────────────────────────", sink.label());
        }

        for &(label, pat, es_pat, cpp_pat, cpp_ext, validate) in PATTERNS {
            if cfg.skip_pattern(label) { continue; }

            let es_skip  = label == "full_scan"; // es.exe 2GB IPC ceiling on large drives
            let cpp_skip = cpp_pat.is_empty();   // pattern not supported by C++ tool

            let run_uffs_tool = cfg.tools.contains(&Tool::Uffs);
            let run_cpp_tool  = cfg.tools.contains(&Tool::UffsCpp) && cfg.uffs_cpp.is_some() && !cpp_skip;
            let run_es_tool   = cfg.tools.contains(&Tool::Everything) && cfg.es.is_some() && !es_skip;

            if !run_uffs_tool && !run_cpp_tool && !run_es_tool { continue; }

            eprintln!();
            if es_skip  { eprintln!("  HOT  {label:<12}  ES  SKIP (es.exe 2GB IPC limit)"); }
            if cpp_skip { eprintln!("  HOT  {label:<12}  C++ SKIP (pattern not supported)"); }

            eprintln!("  HOT [{s}] {label:<12}  {} rounds  (tools shuffled each round)",
                cfg.rounds, s = sink.label());

            let mut uffs_runs: Vec<Timing> = Vec::new();
            let mut cpp_runs:  Vec<Timing> = Vec::new();
            let mut es_runs:   Vec<Timing> = Vec::new();
            let mut es_aborted = false;

            // Separate output file per tool per round, under the consolidated
            // scratch dir (absolute paths — both the C++ tool's `--out=` and
            // Everything's `-export-csv` accept absolute targets).  Files are
            // cleaned up immediately after the per-round diff so disk stays low.
            let scratch = bench_scratch_dir();
            let f_uffs = scratch.join(format!("bench_uffs_{label}.csv")).to_string_lossy().into_owned();
            let f_cpp  = scratch.join(format!("bench_cpp_{label}.csv")).to_string_lossy().into_owned();
            let f_es   = scratch.join(format!("bench_es_{label}.csv")).to_string_lossy().into_owned();

            for round in 0..cfg.rounds {
                // Fresh LCG seed every round so tool order varies.
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64 + round as u64 * 1_000_000_007)
                    .unwrap_or(round as u64 + 1);
                let order = lcg_shuffle3(seed);

                let order_labels: Vec<&str> = order.iter().map(|&s| match s {
                    0 => "uffs", 1 => "cpp", 2 => "es", _ => "?"
                }).collect();
                eprintln!("    [round {:>2}/{}] order=[{}]",
                    round + 1, cfg.rounds, order_labels.join(","));
                flush();

                let mut round_rows: [Option<u64>; 3] = [None; 3]; // [uffs, cpp, es]

                // ── Run tools in shuffled order ───────────────────────
                for &slot in &order {
                    match slot {
                        0 if run_uffs_tool => {
                            let t = check_dnf(run_uffs_to(
                                &cfg.uffs, drive, pat, validate, sink, &f_uffs));
                            round_rows[0] = t.ok.then_some(t.rows);
                            uffs_runs.push(t);
                        }
                        1 if run_cpp_tool => {
                            let cpp = cfg.uffs_cpp.as_ref().unwrap();
                            let t = check_dnf(run_uffs_cpp_to(
                                cpp, drive, cpp_pat, cpp_ext, validate, sink, &f_cpp));
                            round_rows[1] = t.ok.then_some(t.rows);
                            cpp_runs.push(t);
                        }
                        2 if run_es_tool && !es_aborted => {
                            let es = cfg.es.as_ref().unwrap();
                            let t = check_dnf(run_es_to(
                                es, drive, es_pat, validate, sink,
                                cfg.es_instance.as_deref(), &f_es));
                            if round == 0 && is_fast_deterministic_fail(&t) {
                                eprintln!("      es.exe fast-fail ({}); skipping remaining es rounds", t.err);
                                es_aborted = true;
                            }
                            round_rows[2] = t.ok.then_some(t.rows);
                            es_runs.push(t);
                        }
                        _ => {}
                    }
                }

                // ── Line-count summary for this round ─────────────────
                eprintln!("      rows:  uffs={}  cpp={}  es={}",
                    round_rows[0].map_or("-".into(), |n| n.to_string()),
                    round_rows[1].map_or("-".into(), |n| n.to_string()),
                    round_rows[2].map_or("-".into(), |n| n.to_string()),
                );

                // ── Content diff (File sink only) ─────────────────────
                // Expected subset invariant: ES ⊆ CPP ⊆ UFFS.
                // Only print a diff when that invariant is violated —
                // i.e. when the smaller tool has rows the larger doesn't.
                // Identical sets and clean supersets are silent.
                if matches!(sink, OutputSink::File) {
                    let uffs_paths = if run_uffs_tool { normalise_paths(&f_uffs) } else { Vec::new() };
                    let cpp_paths  = if run_cpp_tool  { normalise_paths(&f_cpp)  } else { Vec::new() };
                    let es_paths   = if run_es_tool && !es_aborted { normalise_paths(&f_es) } else { Vec::new() };

                    // cpp ⊆ uffs: violation = something in cpp not in uffs
                    if run_uffs_tool && run_cpp_tool && !uffs_paths.is_empty() && !cpp_paths.is_empty() {
                        let d = diff_paths(&uffs_paths, &cpp_paths);
                        if !d.only_in_b.is_empty() {
                            print_diff("uffs", &f_uffs, uffs_paths.len(),
                                       "cpp",  &f_cpp,  cpp_paths.len(), &d, 10);
                        }
                    }
                    // es ⊆ uffs: violation = something in es not in uffs
                    if run_uffs_tool && run_es_tool && !es_aborted
                        && !uffs_paths.is_empty() && !es_paths.is_empty() {
                        let d = diff_paths(&uffs_paths, &es_paths);
                        if !d.only_in_b.is_empty() {
                            print_diff("uffs", &f_uffs, uffs_paths.len(),
                                       "es",   &f_es,   es_paths.len(), &d, 10);
                        }
                    }
                    // es ⊆ cpp: violation = something in es not in cpp
                    if run_cpp_tool && run_es_tool && !es_aborted
                        && !cpp_paths.is_empty() && !es_paths.is_empty() {
                        let d = diff_paths(&cpp_paths, &es_paths);
                        if !d.only_in_b.is_empty() {
                            print_diff("cpp", &f_cpp, cpp_paths.len(),
                                       "es",  &f_es,  es_paths.len(), &d, 10);
                        }
                    }
                }

                // Clean up per-tool output files before the next round.
                cleanup_file(&f_uffs);
                cleanup_file(&f_cpp);
                cleanup_file(&f_es);
            }

            // ── Per-tool timing summary after all rounds ──────────────
            if run_uffs_tool && !uffs_runs.is_empty() {
                let s = sw(&uffs_runs);
                let mut dm: Vec<u64> = uffs_runs.iter()
                    .filter(|r| r.ok && r.daemon_ms > 0).map(|r| r.daemon_ms).collect();
                dm.sort();
                let daemon_str = if dm.is_empty() { String::new() }
                    else { format!("  daemon_p50={}", fms(p50(&dm))) };
                let any_bad = uffs_runs.iter().any(|r| r.bad_rows > 0);
                let verdict = if uffs_runs.iter().any(|r| r.dnf) { "DNF" }
                    else if any_bad { "WRONG" } else { "PASS" };
                let first_ok = uffs_runs.iter().find(|r| r.ok);
                eprintln!("    UFFS     p50={:>6}  p95={:>6}{}  rows={}  {}",
                    fms(p50(&s)), fms(p95(&s)), daemon_str,
                    first_ok.map_or(0, |r| r.rows), verdict);
                all_rows.push(Row { tool: Tool::Uffs, phase: Phase::Hot, sink,
                    drive: drive.to_string(), pat: label.into(), runs: uffs_runs });
            }
            if run_cpp_tool && !cpp_runs.is_empty() {
                let s = sw(&cpp_runs);
                let any_bad = cpp_runs.iter().any(|r| r.bad_rows > 0);
                let verdict = if cpp_runs.iter().any(|r| r.dnf) { "DNF" }
                    else if any_bad { "WRONG" }
                    else if cpp_runs.iter().all(|r| r.ok) { "PASS" } else { "ERROR" };
                let first_ok = cpp_runs.iter().find(|r| r.ok);
                eprintln!("    UFFS-C++ p50={:>6}  p95={:>6}  rows={}  {}",
                    fms(p50(&s)), fms(p95(&s)), first_ok.map_or(0, |r| r.rows), verdict);
                all_rows.push(Row { tool: Tool::UffsCpp, phase: Phase::Hot, sink,
                    drive: drive.to_string(), pat: label.into(), runs: cpp_runs });
            }
            if run_es_tool && !es_runs.is_empty() {
                let s = sw(&es_runs);
                let any_bad = es_runs.iter().any(|r| r.bad_rows > 0);
                let abort_str = if es_aborted {
                    format!("  (fast-fail after {} round(s))", es_runs.len())
                } else { String::new() };
                let verdict = if es_runs.iter().any(|r| r.dnf) { "DNF" }
                    else if any_bad { "WRONG" }
                    else if es_runs.iter().all(|r| r.ok) { "PASS" } else { "ERROR" };
                let first_ok = es_runs.iter().find(|r| r.ok);
                eprintln!("    ES       p50={:>6}  p95={:>6}  rows={}  {}{}",
                    fms(p50(&s)), fms(p95(&s)),
                    first_ok.map_or(0, |r| r.rows), verdict, abort_str);
                all_rows.push(Row { tool: Tool::Everything, phase: Phase::Hot, sink,
                    drive: drive.to_string(), pat: label.into(), runs: es_runs });
            }
        }
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────
fn main() {
    let mut cfg = parse_args();

    // Pin the consolidated artifact root for the whole run, then create it so
    // the first scratch write never races a missing dir.
    let _ = BENCH_DIR.set(cfg.out_dir.clone());
    let _ = std::fs::create_dir_all(cfg.out_dir.join("scratch"));

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
    println!("║  Bench artifact dir: {}", cfg.out_dir.display());
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

    // Snapshot the as-found UFFS run-state BEFORE the first daemon kill, so we
    // can put the host back exactly as we found it at teardown.
    let as_found = capture_run_state(&cfg.uffs);
    match &as_found.daemon_drives {
        None => eprintln!("[run-state] as-found: daemon stopped; mcp {}",
            if as_found.mcp_running { "up" } else { "down" }),
        Some(drives) => eprintln!("[run-state] as-found: daemon on {}; mcp {}",
            if drives.is_empty() { "(all)".to_string() } else { drives.join(",") },
            if as_found.mcp_running { "up" } else { "down" }),
    }

    let mut all_rows: Vec<Row> = Vec::new();

    // ── Everything: discover Everything.exe for the private sandbox ───────
    // The sandbox instance (`-instance uffs-bench`) is (re)launched per step
    // scoped to EXACTLY the drives under test — see `scope_everything` — so
    // ES's working set always matches the daemon's.  Here we only locate
    // Everything.exe and reserve the instance name.  Skipped when the operator
    // already passed `--es-instance` (they manage their own) or it is missing.
    let mut es_everything: Option<PathBuf> = None;
    let mut es_bench_ini: Option<PathBuf> = None;
    let manage_es = cfg.tools.contains(&Tool::Everything) && cfg.es.is_some() && cfg.es_instance.is_none();
    if manage_es {
        match find_everything() {
            Some(ev) => { cfg.es_instance = Some(ES_INSTANCE_NAME.to_string()); es_everything = Some(ev); }
            None => eprintln!(
                "  [es-instance] WARNING: Everything.exe not found — es.exe will \
                 query whatever instance is running (if any)"),
        }
    }
    // Re-scope ES + daemon to exactly `drives`, primed for peak HOT perf.
    // Returns the temp-ini path of the (re)launched ES sandbox, if any.
    let scope_tools = |cfg: &Cfg, drives: &[String]| -> Option<PathBuf> {
        let ini = es_everything.as_ref().and_then(|ev| {
            eprintln!();
            let p = es_launch(ev, drives);
            if p.is_some() {
                if let Some(ref es) = cfg.es { es_wait_until_loaded(es, drives); }
            }
            p
        });
        if cfg.tools.contains(&Tool::Uffs) {
            uffs_stop(&cfg.uffs);
            uffs_start(&cfg.uffs, drives);
            eprintln!();
            prime_daemon(&cfg.uffs, &drives.join(","), PRIME_ROUNDS);
        }
        ini
    };

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
                if !COLD_WARM_PATTERNS.contains(&label) { continue; }
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
            eprintln!();
            eprint!("  UFFS WARM: stopping daemon (cache stays)...");  flush();
            uffs_stop(&cfg.uffs);
            eprintln!(" done.");

            for &(label, pat, _, _, _, validate) in PATTERNS {
                if !COLD_WARM_PATTERNS.contains(&label) { continue; }
                if cfg.skip_pattern(label) { continue; }
                eprint!("    {label:<12} ");  flush();
                uffs_stop(&cfg.uffs);
                uffs_start(&cfg.uffs, std::slice::from_ref(drive));
                let t = check_dnf(run_uffs(&cfg.uffs, drive, pat, validate, OutputSink::File));
                let verdict = if t.dnf { "DNF" } else if t.bad_rows > 0 { "WRONG" } else if t.ok { "PASS" } else { "ERROR" };
                let bad_str = if t.bad_rows > 0 { format!("  bad={}", t.bad_rows) } else { String::new() };
                eprintln!("{:>8}  rows={:<8}{} {}", fms(t.wall_ms), t.rows, bad_str, verdict);
                all_rows.push(Row { tool: Tool::Uffs, phase: Phase::Warm, sink: OutputSink::File,
                    drive: drive.clone(), pat: label.into(), runs: vec![t] });
            }
        }

        // ── HOT prep: reset + warm-prime the daemon AND reload the ES
        //    sandbox, both scoped to EXACTLY this drive, so the head-to-head
        //    runs against a fully-warmed, same-working-set index — fair vs ES
        //    (always pre-indexed) and the C++ tool (re-reads the MFT each call).
        es_bench_ini = scope_tools(&cfg, std::slice::from_ref(drive));

        // ── HOT: head-to-head comparison on this single drive ────────────
        run_hot_compare(&cfg, drive, &mut all_rows);

        println!();
    }

    // ── ALL-drives aggregate step ─────────────────────────────────────────
    // When more than one drive is under test, run a final head-to-head with
    // every tool scoped to ALL requested drives at once: ES sandbox reloaded
    // with the full set, daemon restarted + primed across the full set, and
    // queries spanning every drive (uffs `--drives=C,D,G`, es with no path
    // filter, uffs.com `--drives=C,D,G`).  Mirrors the per-drive fairness at
    // aggregate scale.
    if cfg.drives.len() > 1 {
        let all = cfg.drives.join(",");
        println!("━━━ ALL drives {all}  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", );
        es_bench_ini = scope_tools(&cfg, &cfg.drives);
        run_hot_compare(&cfg, &all, &mut all_rows);
        println!();
    }

    // ── Everything: tear down the private bench instance ──────────────────
    if let Some(ev) = &es_everything {
        es_stop(ev, es_bench_ini.as_deref());
    }

    // ── UFFS daemon + MCP: restore to as-found state ──────────────────────
    // Done before the summary (and before any `process::exit` in the CSV-sink
    // path below) so the host is restored even on the error exit.
    restore_run_state(&cfg.uffs, &as_found);

    // ── Summary table ────────────────────────────────────────────────────────
    print_summary(&cfg, &all_rows);

    // ── Optional CSV sink for the summary (one row per Tool × Phase × Sink ×
    //    Drive × Pattern combination).  Distinct from the daemon's intermediate
    //    `uffs_bench_out.csv`, which holds the raw per-query row dumps and is
    //    overwritten every round.  The two never collide: this file lives at
    //    whatever path the operator passes via `--out`, the other is under
    //    `<out-dir>/scratch/uffs_bench_out.csv`.
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

        // Per-drive rows, then the all-drives aggregate spec (e.g. "C,D,G")
        // when more than one drive was tested.
        let mut drive_specs: Vec<String> = cfg.drives.clone();
        if cfg.drives.len() > 1 { drive_specs.push(cfg.drives.join(",")); }
        for drive in &drive_specs {
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
#[cfg(test)]
mod tests {
    use super::{canon_path, diff_paths, first_csv_field, normalise_paths, parse_run_state, resolve_bench_dir};

    #[test]
    fn parse_run_state_reads_drives_and_mcp() {
        // The operator's real `uffs --status` output: 7 drives loaded, MCP up.
        let out = "\
── Daemon ──
  Status:      running (PID 62036)
  Drives:      7 loaded (25,925,871 records, 7 active / 0 parked / 0 cold)
    [W] G:     15,162 records
    [W] M:  1,908,812 records
    [W] C:  3,506,664 records

── MCP HTTP Gateway ──
  Status:      running (PID 60016)

── MCP Stdio Sessions ──
  (none)
";
        let state = parse_run_state(out);
        assert_eq!(state.daemon_drives.as_deref(), Some(["G", "M", "C"].map(str::to_owned).as_slice()));
        assert!(state.mcp_running);
    }

    #[test]
    fn parse_run_state_stopped_daemon_and_down_mcp() {
        let out = "── Daemon ──\n  Status:      not running\n\n── MCP HTTP Gateway ──\n  Status:      not running\n";
        let state = parse_run_state(out);
        assert!(state.daemon_drives.is_none());
        assert!(!state.mcp_running);
    }

    #[test]
    fn out_dir_flag_takes_precedence_over_env() {
        // An explicit --out-dir / --paths value short-circuits every env-based
        // fallback, so it is deterministic regardless of the caller's shell.
        let chosen = resolve_bench_dir(Some(std::path::Path::new("/tmp/uffs-bench-test")));
        assert_eq!(chosen, std::path::PathBuf::from("/tmp/uffs-bench-test"));
    }

    #[test]
    fn resolve_bench_dir_fallback_ends_with_uffs_bench() {
        // With no flag, every fallback branch (UFFS_BENCH_DIR / LOCALAPPDATA /
        // ~/.cache) terminates in a `uffs-bench` leaf — the shared tree the
        // `just` flow also targets.  (Env may be set in the dev shell, so we
        // assert the invariant leaf, not a fixed absolute path.)
        let dir = resolve_bench_dir(None);
        let leaf_ok = dir.file_name().is_some_and(|n| n == "uffs-bench")
            || std::env::var("UFFS_BENCH_DIR").is_ok_and(|v| !v.is_empty());
        assert!(leaf_ok, "fallback bench dir should live under a uffs-bench leaf: {dir:?}");
    }

    #[test]
    fn canon_strips_trailing_dir_slash() {
        // The exact cpp-vs-es mismatch from the field report: a directory hit
        // emitted with vs without a trailing separator must compare equal.
        assert_eq!(canon_path("c:\\config.msi\\"), canon_path("c:\\config.msi"));
        assert_eq!(canon_path("C:\\Config.MSI\\"), "c:\\config.msi");
        assert_eq!(canon_path("c:\\found.000\\dir0524.chk\\config\\"), "c:\\found.000\\dir0524.chk\\config");
    }

    #[test]
    fn canon_keeps_drive_root_slash() {
        // `c:`, `c:\`, and `c:\\` are the same root and must not collapse to `c:`.
        assert_eq!(canon_path("c:\\"), "c:\\");
        assert_eq!(canon_path("c:"), "c:\\");
        assert_eq!(canon_path("C:\\\\"), "c:\\");
    }

    #[test]
    fn canon_normalises_forward_slashes_and_case() {
        assert_eq!(canon_path("C:/Foo/Bar/"), "c:\\foo\\bar");
    }

    #[test]
    fn first_field_handles_quoting() {
        // A path containing a comma stays one field when quoted.
        assert_eq!(first_csv_field("\"c:\\a,b\\c\",123,x"), "c:\\a,b\\c");
        assert_eq!(first_csv_field("c:\\plain\\path,42"), "c:\\plain\\path");
        assert_eq!(first_csv_field("c:\\nocomma"), "c:\\nocomma");
    }

    #[test]
    fn trailing_slash_difference_no_longer_creates_false_diffs() {
        // Two "files" written into temp CSVs: identical entries, one tool with
        // trailing dir slashes (cpp style), one without (es style).  After
        // canonicalisation the diff must be EMPTY — the bug this fix closes.
        let dir = std::env::temp_dir();
        let cpp = dir.join("uffs_xtool_test_cpp.csv");
        let es = dir.join("uffs_xtool_test_es.csv");
        std::fs::write(&cpp, "Path\nc:\\config.msi\\\nc:\\exiftool\\config_files\\\nc:\\windows\\notepad.exe\n").unwrap();
        std::fs::write(&es, "Path\nc:\\config.msi\nc:\\exiftool\\config_files\nc:\\windows\\notepad.exe\n").unwrap();
        let a = normalise_paths(cpp.to_str().unwrap());
        let b = normalise_paths(es.to_str().unwrap());
        let d = diff_paths(&a, &b);
        assert!(d.only_in_a.is_empty(), "trailing-slash dirs must not be cpp-only: {:?}", d.only_in_a);
        assert!(d.only_in_b.is_empty(), "trailing-slash dirs must not be es-only: {:?}", d.only_in_b);
        std::fs::remove_file(&cpp).ok();
        std::fs::remove_file(&es).ok();
    }
}
