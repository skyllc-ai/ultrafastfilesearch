#!/usr/bin/env rust-script
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//! Everything Capacity Probe — discover per-drive IPC/OOM limits of Everything (es.exe).
//!
//! Run on Windows (elevated recommended) from the repo root:
//!   rust-script scripts/windows/everything_capacity_probe.rs           # all drives
//!   rust-script scripts/windows/everything_capacity_probe.rs G         # just G:
//!   rust-script scripts/windows/everything_capacity_probe.rs G C D     # G:, C:, D:
//!
//! What it does:
//!   1. Locates Everything.exe and es.exe on disk.
//!   2. Reads Everything.ini to discover configured NTFS volumes.
//!   3. For each drive, isolates it in the ini and restarts Everything.
//!   4. Runs es.exe with progressively heavier column sets (levels 0–7).
//!   5. Detects crashes / OOM / IPC overflow via exit-code + process liveness.
//!   6. Writes a timestamped log to  scripts/windows/everything_probe_<ts>.log
//!
//! Column levels (ordered smallest → largest IPC payload per row):
//!
//!   NOTE: es.exe auto-includes full path when no -name/-path-column/
//!         -full-path-and-name flag is given.  So we MUST use -name to
//!         avoid the heavy path column in early levels.
//!
//!   Lvl  Flags                                  Est. bytes/row
//!   ───  ─────                                  ──────────────
//!    0   -get-result-count                        0  (single int)
//!    1   -name                                   ~18 (filename only)
//!    2   -name -ext                              ~22 (+ extension)
//!    3   -name -size                             ~28 (+ size number)
//!    4   -name -size -attribs                    ~33 (+ attribute flags)
//!    5   -name -size -attribs -dm                ~53 (+ date-modified)
//!    6   -name -size -attribs -dm -dc -da        ~93 (+ all three dates)
//!    7   (default = full path + name)            ~85 (heavy path string)
//!    8   -size -attribs -dm -dc -da             ~140 (full path + all cols)

use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Column level definitions
// ---------------------------------------------------------------------------

/// Each level is (label, estimated bytes/row, extra args for es.exe).
/// Level 0 is count-only.  Remaining levels are ordered from smallest to
/// largest IPC payload.  Using `-name` suppresses the automatic full-path
/// column, keeping early levels lightweight.
const LEVELS: &[(&str, u32, &[&str])] = &[
    ("L0: count-only",                  0, &["-get-result-count"]),
    ("L1: name",                       18, &["-name"]),
    ("L2: name+ext",                   22, &["-name", "-ext"]),
    ("L3: name+size",                  28, &["-name", "-size"]),
    ("L4: name+size+attribs",          33, &["-name", "-size", "-attribs"]),
    ("L5: name+size+attribs+dm",       53, &["-name", "-size", "-attribs", "-dm"]),
    ("L6: name+size+attribs+dm+dc+da", 93, &["-name", "-size", "-attribs", "-dm", "-dc", "-da"]),
    ("L7: full-path (default)",        85, &[]),
    ("L8: full-path+all",             140, &["-size", "-attribs", "-dm", "-dc", "-da"]),
];

const EVERYTHING_TIMEOUT: Duration = Duration::from_secs(60);

/// Floor timeout for es.exe — even if adaptive baseline is tiny, wait at least this long.
const ES_TIMEOUT_FLOOR: Duration = Duration::from_secs(15);
/// Multiplier applied to the last successful level's time to derive the
/// adaptive timeout for the next level.  3× gives enough headroom for the
/// extra column data while catching a hang quickly.
const ES_TIMEOUT_MULTIPLIER: u32 = 3;

// ---------------------------------------------------------------------------
// Locators
// ---------------------------------------------------------------------------

/// Resolve the consolidated bench-artifact root, mirroring the `_bench-dir`
/// helper in `just/bench_uffs.just` and the other bench scripts so every tool
/// writes under ONE tree.  Precedence: `$UFFS_BENCH_DIR` >
/// `%LOCALAPPDATA%\uffs-bench` > `$XDG_CACHE_HOME|~/.cache` + `/uffs-bench`.
fn shared_bench_root() -> PathBuf {
    if let Some(v) = env::var_os("UFFS_BENCH_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = env::var_os("LOCALAPPDATA") {
        if !v.is_empty() {
            return PathBuf::from(v).join("uffs-bench");
        }
    }
    let base = env::var_os("XDG_CACHE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
        });
    base.join("uffs-bench")
}

fn find_exe(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.exists()).cloned()
}

fn everything_exe() -> Option<PathBuf> {
    let home = env::var("USERPROFILE").unwrap_or_default();
    let pf = env::var("ProgramFiles").unwrap_or_default();
    let pf86 = env::var("ProgramFiles(x86)").unwrap_or_default();
    find_exe(&[
        PathBuf::from(&pf).join("Everything").join("Everything.exe"),
        PathBuf::from(&pf86).join("Everything").join("Everything.exe"),
        PathBuf::from(&home).join("bin").join("Everything.exe"),
    ])
}

fn es_exe() -> Option<PathBuf> {
    let home = env::var("USERPROFILE").unwrap_or_default();
    let pf = env::var("ProgramFiles").unwrap_or_default();
    let pf86 = env::var("ProgramFiles(x86)").unwrap_or_default();
    find_exe(&[
        PathBuf::from(&home).join("bin").join("es.exe"),
        PathBuf::from(&pf).join("Everything").join("es.exe"),
        PathBuf::from(&pf86).join("Everything").join("es.exe"),
    ])
}

fn everything_ini_path() -> PathBuf {
    let appdata = env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("Everything").join("Everything.ini")
}

// ---------------------------------------------------------------------------
// INI helpers
// ---------------------------------------------------------------------------

/// Parse ntfs_volume_paths from Everything.ini → vec of drive letters.
fn parse_drives_from_ini(ini: &str) -> Vec<char> {
    for line in ini.lines() {
        if let Some(rest) = line.strip_prefix("ntfs_volume_paths=") {
            return rest
                .split(',')
                .filter_map(|s| {
                    let s = s.trim().trim_matches('"');
                    s.chars().next().filter(|c| c.is_ascii_alphabetic())
                })
                .map(|c| c.to_ascii_uppercase())
                .collect();
        }
    }
    Vec::new()
}

/// Rewrite ini so only `target_drive` is included.
fn isolate_drive_in_ini(ini_path: &Path, target_drive: char, all_drives: &[char]) {
    let mut content = fs::read_to_string(ini_path).unwrap_or_default();

    // Build includes mask: 1 for target, 0 for others
    let includes: String = all_drives
        .iter()
        .map(|d| if d.eq_ignore_ascii_case(&target_drive) { "1" } else { "0" })
        .collect::<Vec<_>>()
        .join(",");

    let replacements = [
        ("ntfs_volume_includes=", includes.as_str()),
        ("auto_include_fixed_volumes=", "0"),
        ("auto_include_removable_volumes=", "0"),
    ];
    for (key, val) in &replacements {
        if let Some(pos) = content.find(key) {
            let line_end = content[pos..].find('\n').map_or(content.len(), |i| pos + i);
            let replacement = format!("{key}{val}");
            content.replace_range(pos..line_end, &replacement);
        }
    }
    fs::write(ini_path, &content).ok();
}

fn restore_ini(ini_path: &Path, content: &str) {
    fs::write(ini_path, content).ok();
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn kill_everything() {
    Command::new("taskkill")
        .args(["/F", "/IM", "Everything.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
    std::thread::sleep(Duration::from_secs(2));
}

fn start_everything(exe: &Path) {
    println!("      CMD: \"{}\" -startup -minimized", exe.display());
    match Command::new(exe)
        .args(["-startup", "-minimized"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => {}
        Err(e) => eprintln!("      ⚠️  Failed to start Everything: {e}"),
    }
}

/// Wait until es.exe -get-result-count returns > 0 (index ready).
fn wait_for_index(es: &Path) -> Option<u64> {
    let deadline = Instant::now() + EVERYTHING_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(out) = Command::new(es)
            .arg("-get-result-count")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Ok(n) = s.trim().parse::<u64>() {
                    if n > 0 { return Some(n); }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Probe logic
// ---------------------------------------------------------------------------

/// Kill a hung es.exe process, then kill Everything (dismisses OOM dialog).
fn kill_es_and_everything() {
    Command::new("taskkill")
        .args(["/F", "/IM", "es.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
    kill_everything();
}

/// Result of a single es.exe probe level.
enum ProbeResult {
    /// Query completed successfully: line count, elapsed time.
    Ok { lines: u64, elapsed: Duration },
    /// es.exe timed out — Everything is showing an OOM dialog (IPC blocked).
    Timeout { elapsed: Duration },
    /// es.exe exited with an error code.
    Failed { elapsed: Duration, detail: String },
    /// Could not even spawn es.exe.
    SpawnError(String),
}

/// Run a single es.exe probe at the given level.
///
/// Spawns es.exe and polls it with an **adaptive timeout**.  If es.exe doesn't
/// finish in time, it means Everything is blocked on its OOM modal dialog and
/// will never respond to the IPC pipe.  We kill both processes and report.
///
/// `timeout` is caller-computed: max(floor, last_ok_time × multiplier).
fn run_es_level(es: &Path, drive: char, level_idx: usize, timeout: Duration) -> ProbeResult {
    let (_, _, extra_args) = LEVELS[level_idx];
    let t0 = Instant::now();

    let mut cmd = Command::new(es);
    let mut cmd_display = format!("\"{}\"", es.display());
    if level_idx == 0 {
        cmd.arg("-get-result-count");
        cmd_display.push_str(" -get-result-count");
    } else {
        let drive_arg = format!("{}:", drive);
        cmd.arg(&drive_arg);
        cmd_display.push_str(&format!(" {drive_arg}"));
        for arg in extra_args {
            cmd.arg(arg);
            cmd_display.push_str(&format!(" {arg}"));
        }
    }
    println!("\n      CMD: {cmd_display}");

    // L0 (count-only) returns a single number — safe to pipe.
    // L1+ can output millions of lines.  Piping would deadlock because our
    // poll loop doesn't drain stdout while waiting.  Redirect to a temp file
    // so es.exe can write freely; we read the file after exit.
    let stdout_file = if level_idx == 0 {
        None
    } else {
        let path = std::env::temp_dir().join(format!("es_probe_{}.tmp", level_idx));
        match fs::File::create(&path) {
            Ok(f) => Some((path, f)),
            Err(e) => return ProbeResult::SpawnError(format!("temp file: {e}")),
        }
    };

    if let Some((_, ref f)) = stdout_file {
        cmd.stdout(f.try_clone().unwrap_or_else(|_| f.try_clone().expect("clone")));
    } else {
        cmd.stdout(Stdio::piped());
    }
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ProbeResult::SpawnError(format!("{e}")),
    };

    // Poll until es.exe exits or the adaptive timeout fires.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = t0.elapsed();

                if status.success() {
                    let count = if level_idx == 0 {
                        // Read the small piped output
                        let stdout = child.stdout.take().map(|mut s| {
                            let mut buf = String::new();
                            std::io::Read::read_to_string(&mut s, &mut buf).ok();
                            buf
                        }).unwrap_or_default();
                        stdout.trim().parse::<u64>().unwrap_or(0)
                    } else {
                        // Count newlines in the temp file
                        let path = &stdout_file.as_ref().unwrap().0;
                        count_lines_in_file(path)
                    };
                    cleanup_temp(&stdout_file);
                    return ProbeResult::Ok { lines: count, elapsed };
                } else {
                    let code = status.code().unwrap_or(-1);
                    let stderr = child.stderr.take().map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    }).unwrap_or_default();
                    let detail: String = stderr.chars().take(500).collect();
                    cleanup_temp(&stdout_file);
                    return ProbeResult::Failed {
                        elapsed,
                        detail: format!("exit code {code}; {detail}"),
                    };
                }
            }
            Ok(None) => {
                if t0.elapsed() >= timeout {
                    let elapsed = t0.elapsed();
                    child.kill().ok();
                    child.wait().ok();
                    kill_es_and_everything();
                    cleanup_temp(&stdout_file);
                    return ProbeResult::Timeout { elapsed };
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => {
                child.kill().ok();
                cleanup_temp(&stdout_file);
                return ProbeResult::SpawnError(format!("wait error: {e}"));
            }
        }
    }
}

/// Count newlines in a file without loading it all into memory.
fn count_lines_in_file(path: &Path) -> u64 {
    use std::io::{BufRead, BufReader};
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    BufReader::with_capacity(256 * 1024, file)
        .lines()
        .count() as u64
}

/// Remove the temp file if one was created.
fn cleanup_temp(tmp: &Option<(PathBuf, fs::File)>) {
    if let Some((ref path, _)) = tmp {
        fs::remove_file(path).ok();
    }
}

// ---------------------------------------------------------------------------
// Timestamp (no external crate)
// ---------------------------------------------------------------------------

fn chrono_timestamp() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_per_day: u64 = 86400;
    let days = now / secs_per_day;
    let day_secs = now % secs_per_day;
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}_{h:02}-{m:02}-{s:02}")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let doe = days % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("═══════════════════════════════════════════════════════");
    println!("  Everything Capacity Probe");
    println!("═══════════════════════════════════════════════════════\n");

    // Locate binaries
    let everything = match everything_exe() {
        Some(p) => { println!("✅ Everything.exe: {}", p.display()); p }
        None    => { eprintln!("❌ Everything.exe not found — aborting."); return; }
    };
    let es = match es_exe() {
        Some(p) => { println!("✅ es.exe:          {}", p.display()); p }
        None    => { eprintln!("❌ es.exe not found — aborting."); return; }
    };
    let ini_path = everything_ini_path();
    if !ini_path.exists() {
        eprintln!("❌ Everything.ini not found at {} — aborting.", ini_path.display());
        return;
    }
    println!("✅ Everything.ini:  {}\n", ini_path.display());

    // Parse drives from ini
    let ini_content = fs::read_to_string(&ini_path).unwrap_or_default();
    let all_drives = parse_drives_from_ini(&ini_content);
    if all_drives.is_empty() {
        eprintln!("❌ No NTFS volumes found in Everything.ini — aborting.");
        return;
    }
    println!("📁 Detected drives: {:?}", all_drives);

    // Filter to user-requested drives (if any args given)
    let args: Vec<String> = env::args().skip(1).collect();
    let drives: Vec<char> = if args.is_empty() {
        all_drives.clone()
    } else {
        let requested: Vec<char> = args
            .iter()
            .filter_map(|a| a.trim().chars().next())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let filtered: Vec<char> = requested
            .iter()
            .filter(|c| all_drives.contains(c))
            .copied()
            .collect();
        let unknown: Vec<&char> = requested
            .iter()
            .filter(|c| !all_drives.contains(c))
            .collect();
        if !unknown.is_empty() {
            eprintln!("⚠️  Unknown drives (not in ini): {:?}", unknown);
        }
        if filtered.is_empty() {
            eprintln!("❌ None of the requested drives found in Everything.ini — aborting.");
            return;
        }
        filtered
    };
    println!("🎯 Probing drives:  {:?}\n", drives);

    // Backup ini content for restore at end
    let ini_backup = ini_content.clone();

    // Prepare log buffer
    let ts = chrono_timestamp();
    let log_name = format!("everything_probe_{ts}.log");
    // Write the probe log under the shared bench tree — NOT into the tracked
    // scripts/windows/ source dir (where it used to land and dirty the repo).
    let log_dir = shared_bench_root().join("everything-probe");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(&log_name);
    let mut log = String::new();
    let _ = writeln!(log, "Everything Capacity Probe — {ts}");
    let _ = writeln!(log, "Drives: {:?}", drives);
    let _ = writeln!(log, "{}", "=".repeat(72));

    // ── Probe each drive ──────────────────────────────────────────────────
    for &drive in &drives {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  DRIVE {}:", drive);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        let _ = writeln!(log, "\n--- Drive {}: ---", drive);

        // Isolate this drive in ini and restart Everything
        kill_everything();
        isolate_drive_in_ini(&ini_path, drive, &all_drives);

        // Measure startup + MFT indexing time
        let t_start = Instant::now();
        start_everything(&everything);

        print!("   ⏳ Waiting for index...");
        std::io::stdout().flush().ok();
        match wait_for_index(&es) {
            Some(n) => {
                let startup_secs = t_start.elapsed().as_secs_f64();
                println!(" ready ({n} records in {startup_secs:.2}s)");
                let _ = writeln!(
                    log,
                    "  Startup+index: {startup_secs:.2}s  Records: {n}"
                );
            }
            None => {
                let startup_secs = t_start.elapsed().as_secs_f64();
                println!(" TIMEOUT after {startup_secs:.1}s");
                let _ = writeln!(log, "  TIMEOUT waiting for index ({startup_secs:.1}s)");
                kill_everything();
                continue;
            }
        }

        // Progressive column probe — increase data load until it breaks.
        // Adaptive timeout: use 3× the last successful level's time, with a
        // floor of 15s.  This catches hangs fast instead of waiting minutes.
        let mut max_ok_level: Option<usize> = None;
        let mut last_ok_time = ES_TIMEOUT_FLOOR;
        for (lvl_idx, (label, est_bytes, _)) in LEVELS.iter().enumerate() {
            let timeout = std::cmp::max(
                ES_TIMEOUT_FLOOR,
                last_ok_time * ES_TIMEOUT_MULTIPLIER,
            );
            print!("   {label} (~{est_bytes}B/row, timeout {:.0}s) … ", timeout.as_secs_f64());
            std::io::stdout().flush().ok();

            match run_es_level(&es, drive, lvl_idx, timeout) {
                ProbeResult::Ok { lines, elapsed } => {
                    let secs = elapsed.as_secs_f64();
                    println!("✅  {lines} lines  {secs:.2}s");
                    let _ = writeln!(log, "  {label}: OK  lines={lines}  time={secs:.2}s");
                    max_ok_level = Some(lvl_idx);
                    last_ok_time = elapsed;
                }
                ProbeResult::Timeout { elapsed } => {
                    let secs = elapsed.as_secs_f64();
                    println!("⏱️  HUNG  {secs:.1}s — es.exe killed (OOM dialog)");
                    let _ = writeln!(
                        log,
                        "  {label}: HUNG (OOM)  time={secs:.1}s  timeout={:.0}s",
                        timeout.as_secs_f64()
                    );
                    // Everything is showing its OOM popup, es.exe was blocked
                    // on IPC.  Both killed.  More columns = worse.  Stop.
                    break;
                }
                ProbeResult::Failed { elapsed, detail } => {
                    let secs = elapsed.as_secs_f64();
                    println!("❌  FAIL  {secs:.2}s  {detail}");
                    let _ = writeln!(log, "  {label}: FAIL  time={secs:.2}s  detail={detail}");
                    break;
                }
                ProbeResult::SpawnError(err) => {
                    println!("❌  SPAWN ERROR: {err}");
                    let _ = writeln!(log, "  {label}: SPAWN ERROR  {err}");
                    break;
                }
            }
        }

        // Drive summary
        let summary = match max_ok_level {
            Some(lvl) => format!("Max OK level: {} ({})", lvl, LEVELS[lvl].0),
            None => "ALL LEVELS FAILED".to_string(),
        };
        println!("\n   📊 {summary}\n");
        let _ = writeln!(log, "  RESULT: {summary}");

        kill_everything();
    }

    // ── Cleanup ───────────────────────────────────────────────────────────
    restore_ini(&ini_path, &ini_backup);
    println!("✅ Everything.ini restored\n");

    let _ = writeln!(log, "\n{}", "=".repeat(72));
    let _ = writeln!(log, "Probe complete.");
    fs::write(&log_path, &log).ok();
    println!("📄 Log written to: {}", log_path.display());
    println!("\n═══════════════════════════════════════════════════════");
    println!("  Probe complete");
    println!("═══════════════════════════════════════════════════════");
}
