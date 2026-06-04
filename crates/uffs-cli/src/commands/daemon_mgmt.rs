// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `uffs daemon {status|stop|kill|restart}` subcommand handlers.

use anyhow::{Context as _, Result};
use uffs_client::connect_sync::UffsClientSync;
use uffs_client::daemon_ctl::{pid_file_path, socket_path};
use uffs_client::protocol::response::{DaemonStatus, DriveInfo, ShardTier};

use crate::args::DaemonAction;
use crate::commands::{daemon_load, daemon_tiering};

/// Execute a daemon management action.
///
/// Every command that mutates daemon state (stop, kill, restart, load,
/// preload, hibernate, forget) requires an elevated shell:
/// - **Windows**: Administrator (UAC-elevated) token.
/// - **Unix** (Linux, macOS, …): effective user ID 0 (root / sudo).
///
/// `uffsd` runs with elevated privileges to read raw filesystem data;
/// a non-privileged caller must not stop or restart it — doing so would
/// kill the running daemon with no safe path to bring it back.
///
/// Read-only queries (`status`, `stats`, `status_drives`) are always
/// permitted without elevation.  `daemon start --elevate` is also
/// permitted on Windows; it opts in to an explicit UAC prompt.
///
/// # Errors
///
/// Returns an error if the operation fails, or if a mutating command is
/// attempted from a non-elevated shell.
pub(crate) fn daemon(action: &DaemonAction) -> Result<()> {
    // Elevation gate — checked here, once, before any action is dispatched,
    // so no individual subcommand handler can accidentally bypass it.
    {
        let is_read_only_or_uac_start = matches!(
            action,
            DaemonAction::Status
                | DaemonAction::Stats
                | DaemonAction::StatusDrives
                | DaemonAction::Start { elevate: true, .. }
        );
        if !is_read_only_or_uac_start && !uffs_mft::is_elevated() {
            #[cfg(windows)]
            anyhow::bail!(
                "Daemon management commands require an elevated (Administrator) shell.\n\n\
                 uffsd runs with admin privileges to read the NTFS Master File Table.\n\
                 A non-elevated process must not stop or restart it — doing so would\n\
                 kill the running daemon with no way to bring it back.\n\n\
                 To run this command, pick one:\n\
                 \x20 1. Relaunch PowerShell / cmd as Administrator\n\
                 \x20    (right-click \u{2192} \"Run as administrator\"), then retry.\n\
                 \x20 2. For `daemon start`, add --elevate to get a UAC prompt:\n\
                 \x20      uffs daemon start --elevate\n\
                 \x20 3. Install the broker service (one-time setup, no future UAC):\n\
                 \x20      uffs-broker --install"
            );
            #[cfg(unix)]
            anyhow::bail!(
                "Daemon management commands require root privileges.\n\n\
                 uffsd runs as root to read raw filesystem data.\n\
                 A non-root process must not stop or restart it — doing so would\n\
                 kill the running daemon with no way to bring it back.\n\n\
                 To run this command, prefix it with sudo:\n\
                 \x20  sudo uffs daemon <subcommand>"
            );
            // Fallback for platforms that are neither Windows nor Unix
            // (e.g. WASM, bare-metal targets — should not arise in practice).
            #[cfg(not(any(windows, unix)))]
            anyhow::bail!(
                "Daemon management commands require elevated privileges.\n\
                 Please run this command as a privileged user."
            );
        }
    }

    match action {
        DaemonAction::Start {
            mft_file,
            data_dir,
            drives,
            no_cache,
            log_level,
            log_file,
            elevate,
        } => daemon_start(
            mft_file,
            data_dir.as_deref(),
            drives,
            *no_cache,
            log_level,
            log_file.as_deref(),
            *elevate,
        ),
        DaemonAction::Status => daemon_status(),
        DaemonAction::Stats => daemon_stats(),
        DaemonAction::Stop => daemon_stop(),
        DaemonAction::Kill => {
            daemon_kill();
            Ok(())
        }
        DaemonAction::Restart => daemon_restart(),
        DaemonAction::Load {
            mft_file,
            data_dir,
            drives,
            no_cache,
        } => daemon_load::daemon_load(mft_file, data_dir.as_deref(), drives, *no_cache),
        DaemonAction::Hibernate { drives } => daemon_tiering::daemon_hibernate(drives),
        DaemonAction::Preload {
            drives,
            pin_minutes,
        } => daemon_tiering::daemon_preload(drives, *pin_minutes),
        DaemonAction::Forget { drives, force } => daemon_tiering::daemon_forget(drives, *force),
        DaemonAction::StatusDrives => daemon_tiering::daemon_status_drives(),
    }
}

/// `uffs daemon start` — start the daemon, forwarding data-source flags
/// as-is so the daemon resolves them internally (DRY).
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
#[expect(
    clippy::use_debug,
    reason = "[diag] diagnostic tracing — remove after D: drive issue is resolved"
)]
fn daemon_start(
    mft_files: &[std::path::PathBuf],
    data_dir: Option<&std::path::Path>,
    drives: &[uffs_mft::platform::DriveLetter],
    no_cache: bool,
    log_level: &str,
    log_file: Option<&std::path::Path>,
    elevate: bool,
) -> Result<()> {
    // Already running?
    if UffsClientSync::connect_raw().is_ok() {
        println!("Daemon is already running. Use `uffs daemon restart` to reload.");
        return Ok(());
    }

    // [diag] Show what the CLI received before building spawn args.
    println!(
        "[diag] daemon_start: drives={drives:?}  log_level={log_level:?}  log_file={log_file:?}"
    );

    // Build spawn args — forward raw, let daemon handle discovery.
    let mut spawn_args = Vec::new();
    if let Some(dir) = data_dir {
        spawn_args.push("--data-dir".to_owned());
        spawn_args.push(dir.to_string_lossy().into_owned());
    }
    for mft_path in mft_files {
        spawn_args.push("--mft-file".to_owned());
        spawn_args.push(mft_path.to_string_lossy().into_owned());
    }
    for letter in drives {
        spawn_args.push("--drive".to_owned());
        spawn_args.push(letter.to_string());
    }
    if no_cache {
        spawn_args.push("--no-cache".to_owned());
    }

    // ── Env-var forwarding ────────────────────────────────────────────────
    // The spawned daemon is a detached background process.  On Windows it is
    // often elevated via ShellExecuteW("runas"), which starts a new session
    // and does NOT reliably inherit the parent PowerShell's env block.
    // We therefore bake RUST_LOG / UFFS_LOG / UFFS_LOG_DIR into argv so the
    // daemon always receives them regardless of how it is elevated.

    // Probe env vars (read once so we can print them before forwarding).
    //
    // IMPORTANT: PowerShell (and other shells in long-lived sessions) often
    // leave variables set to the EMPTY STRING after a script unsets them —
    // `std::env::var("X")` then returns `Ok("")`, not `Err(NotPresent)`.
    // Treating an empty-string env var as a real value is what caused the
    // `--log-level "" --log-file uffsd.log` silent-failure regression: uffsd
    // received an empty EnvFilter (dropping all logs) and a relative log
    // path whose parent `""` tripped tracing_appender's `.expect(...)` panic.
    // So normalise `Some("")` to `None` at the source via `non_empty_env`.
    let env_rust_log = non_empty_env(std::env::var("RUST_LOG").ok());
    let env_uffs_log = non_empty_env(std::env::var("UFFS_LOG").ok());
    let env_uffs_log_dir = non_empty_env(std::env::var("UFFS_LOG_DIR").ok());

    // Effective log level: CLI arg wins; fall back to UFFS_LOG then RUST_LOG.
    let effective_log_level: String = if log_level == "info" {
        env_uffs_log
            .clone()
            .or_else(|| env_rust_log.clone())
            .unwrap_or_else(|| log_level.to_owned())
    } else {
        log_level.to_owned()
    };
    if effective_log_level != "info" {
        spawn_args.push("--log-level".to_owned());
        spawn_args.push(effective_log_level.clone());
    }

    // Effective log file: CLI arg wins; fall back to $UFFS_LOG_DIR/uffsd.log.
    // The `non_empty` filter above guarantees `env_uffs_log_dir` is a real,
    // non-empty path — otherwise `PathBuf::from("").join("uffsd.log")` would
    // produce a relative `uffsd.log`, which in turn breaks the detached
    // daemon's file appender (empty parent dir → create_dir_all fails →
    // rolling-appender panics at startup, uffsd dies before binding IPC).
    let derived_log_file = env_uffs_log_dir
        .as_deref()
        .map(|dir| std::path::PathBuf::from(dir).join("uffsd.log"));
    let effective_log_file = log_file
        .map(std::path::Path::to_path_buf)
        .or(derived_log_file);
    if let Some(path) = &effective_log_file {
        spawn_args.push("--log-file".to_owned());
        spawn_args.push(path.to_string_lossy().into_owned());
    }

    // [diag] Print every diagnostic variable so we can trace the full chain.
    println!("[diag] env  RUST_LOG    = {env_rust_log:?}");
    println!("[diag] env  UFFS_LOG    = {env_uffs_log:?}");
    println!("[diag] env  UFFS_LOG_DIR= {env_uffs_log_dir:?}");
    println!("[diag] eff  log_level   = {effective_log_level:?}");
    println!("[diag] eff  log_file    = {effective_log_file:?}");
    println!("[diag] full spawn_args  = {spawn_args:?}");

    if !cfg!(windows) && spawn_args.is_empty() {
        anyhow::bail!(
            "No MFT data sources specified.\n\
             Provide --mft-file <path> or --data-dir <path>."
        );
    }

    println!("Starting daemon...");

    // `--elevate` (or UFFS_ELEVATE=1) opts in to a UAC prompt on Windows
    // when the current shell is not elevated.  The default path refuses
    // to trigger UAC silently and returns DaemonNeedsElevation, which
    // `main.rs` formats into an actionable multi-option help message.
    let mut client = if elevate {
        UffsClientSync::connect_with_elevation(&spawn_args)
            .with_context(|| "Failed to start daemon (with elevation)")?
    } else {
        UffsClientSync::connect_with_args(&spawn_args).with_context(|| "Failed to start daemon")?
    };

    client
        .await_ready(core::time::Duration::from_mins(2))
        .with_context(|| "Daemon did not become ready in time")?;

    println!("Daemon started and ready.");
    Ok(())
}

/// `uffs daemon status` — show daemon status, PID, loaded drives.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn daemon_status() -> Result<()> {
    let Ok(mut client) = UffsClientSync::connect_raw() else {
        print_not_running();
        return Ok(());
    };

    let Ok(status) = client.status() else {
        print_not_running();
        return Ok(());
    };

    let uptime = core::time::Duration::from_secs(status.uptime_secs);
    println!(
        "Version:       {}",
        crate::commands::version_summary(&status.version)
    );
    println!("Daemon PID:    {}", status.pid);
    println!(
        "Uptime:        {}",
        uffs_client::format::format_duration(uptime)
    );
    match &status.status {
        DaemonStatus::Loading {
            drives_loaded,
            drives_total,
        } => {
            println!("Status:        Loading ({drives_loaded}/{drives_total} drives)");
        }
        DaemonStatus::Ready => {
            println!("Status:        Ready");
        }
        DaemonStatus::Refreshing { drives } => {
            let drive_list: String = drives
                .iter()
                .map(|letter| format!("{letter}:"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("Status:        Refreshing ({drive_list})");
        }
    }
    println!("Connections:   {}", status.connections);

    // Memory info.  Three numbers, in increasing order of "what the OS
    // sees": logical heap (sum of per-drive `heap_size_bytes`), then
    // mimalloc's committed pages, then the OS-reported RSS.  All three
    // come from the same `status` payload so they are consistent.
    if let Some(heap) = status.index_heap_bytes {
        println!("Index heap:    {} MB", heap / (1024 * 1024));
    }
    if let Some(committed) = status.mimalloc_committed_bytes {
        println!(
            "Mimalloc:      {} MB (committed)",
            committed / (1024 * 1024)
        );
    }
    if let Some(rss) = status.rss_bytes {
        println!("RSS:           {} MB", rss / (1024 * 1024));
    }

    // Also show loaded drives.  The `drives` RPC returns every shard
    // in the registry — Warm/Hot with their full memory breakdown,
    // Parked/Cold with just the tier marker (no body in RAM).  Empty
    // registry still renders `(none loaded)` so cold-boot detection in
    // external scripts (api-validation, mcp-validation) keeps working.
    let drives = client.drives().with_context(|| "Failed to query drives")?;
    if drives.drives.is_empty() {
        println!("Drives:        (none loaded)");
    } else {
        println!("Drives:");
        for dr in &drives.drives {
            print_drive_line(dr, &status.drive_memory);
        }
    }
    Ok(())
}

/// Render one row of the `Drives:` block in `daemon status`.
///
/// Format depends on the shard's tier (per Phase 5 task 5.11):
/// * Warm/Hot — full breakdown (records count, source, memory rec= / names= /
///   tri= / ch= / ext=).
/// * Parked  — `[Parked]` marker + bloom + trie kept resident note.
/// * Cold    — `[Cold]` marker only (no body, no filters).
/// * Other   — fall back to the legacy single-line format so the formatter
///   never panics on a state we haven't taught it about.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn print_drive_line(
    dr: &DriveInfo,
    drive_memory: &[uffs_client::protocol::response::DriveMemoryInfo],
) {
    let tier_marker = tier_marker(dr.tier);
    match dr.tier {
        Some(ShardTier::Warm | ShardTier::Hot) | None => {
            let mem = drive_memory.iter().find(|dm| dm.drive == dr.letter);
            if let Some(dm) = mem {
                let mb = |bytes: u64| bytes / (1024 * 1024);
                println!(
                    "  {} {}: — {:>10} records ({}) — {} MB  [rec={} names={} tri={} ch={} ext={}]",
                    tier_marker,
                    dr.letter,
                    uffs_client::format::format_number_commas(dr.records as u64),
                    dr.source,
                    mb(dm.heap_bytes),
                    mb(dm.records_bytes),
                    mb(dm.names_bytes),
                    mb(dm.trigram_bytes),
                    mb(dm.children_bytes),
                    mb(dm.ext_index_bytes),
                );
            } else {
                println!(
                    "  {} {}: — {:>10} records ({})",
                    tier_marker,
                    dr.letter,
                    uffs_client::format::format_number_commas(dr.records as u64),
                    dr.source
                );
            }
        }
        Some(ShardTier::Parked) => {
            println!(
                "  {} {}: — bloom + trie kept resident; body released",
                tier_marker, dr.letter
            );
        }
        Some(ShardTier::Cold) => {
            println!(
                "  {} {}: — encrypted cache only; nothing in RAM",
                tier_marker, dr.letter
            );
        }
        Some(ShardTier::Evicting | ShardTier::Unknown) => {
            println!("  {} {}: — ({})", tier_marker, dr.letter, dr.source);
        }
    }
}

/// Format the bracket-style tier marker for `daemon status`'s drive
/// list.  An 8-character right-padded label so the per-drive lines
/// align in the operator's terminal.
const fn tier_marker(tier: Option<ShardTier>) -> &'static str {
    match tier {
        Some(ShardTier::Hot) => "[Hot]   ",
        Some(ShardTier::Warm) => "[Warm]  ",
        Some(ShardTier::Parked) => "[Parked]",
        Some(ShardTier::Cold) => "[Cold]  ",
        Some(ShardTier::Evicting) => "[Evict] ",
        Some(ShardTier::Unknown) => "[?]     ",
        None => "        ",
    }
}

/// Print the "not running" message with optional stale-PID hint.
///
/// Visible to sibling command modules (`daemon_tiering.rs`) so the
/// graceful "daemon down" rendering stays consistent across every
/// read-only daemon command — the operator sees the **same** stdout
/// shape from `uffs daemon status` and `uffs daemon status_drives`
/// when the daemon happens to be down.  Mutating commands
/// (`hibernate` / `preload` / `forget`) deliberately stay on the
/// bail-with-error path because the operator should know their
/// requested mutation didn't run.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
pub(crate) fn print_not_running() {
    println!("Daemon is not running.");
    let pid_path = pid_file_path();
    if pid_path.exists() {
        println!("  (stale PID file exists at {})", pid_path.display());
    }
}

/// `uffs daemon stats` — show performance metrics.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn daemon_stats() -> Result<()> {
    if let Ok(mut client) = UffsClientSync::connect_raw() {
        let stats = client
            .stats()
            .with_context(|| "Failed to query daemon stats")?;

        let fmt = uffs_client::format::format_duration;
        let uptime = core::time::Duration::from_secs(stats.uptime_secs);
        let startup = core::time::Duration::from_millis(stats.startup_duration_ms);
        let avg_query = core::time::Duration::from_micros(uffs_client::format::f64_to_u64(
            stats.avg_query_time_us,
        ));
        let total_query = core::time::Duration::from_micros(stats.total_query_time_us);

        println!("═══ Daemon Performance Stats ═══");
        println!(
            "Version:           {}",
            crate::commands::version_summary(&stats.version)
        );
        println!("Uptime:            {}", fmt(uptime));
        println!("Startup duration:  {}", fmt(startup));
        println!(
            "Total records:     {}",
            uffs_client::format::format_number_commas(stats.total_records as u64)
        );
        println!("Queries served:    {}", stats.total_queries);
        if stats.total_queries > 0 {
            println!("Avg query time:    {}", fmt(avg_query));
            println!("Total query time:  {}", fmt(total_query));
        }
        println!("Queries/second:    {:.2}", stats.queries_per_second);

        // Aggregate cache observability.  Hit-rate is computed on
        // demand to avoid a division-by-zero for cold daemons.
        let lookups = stats.agg_cache_hits.saturating_add(stats.agg_cache_misses);
        let hit_rate = compute_hit_rate_percent(stats.agg_cache_hits, lookups);
        println!(
            "Agg cache:         {} hits / {} misses ({:.1}% hit-rate, {} entries)",
            stats.agg_cache_hits, stats.agg_cache_misses, hit_rate, stats.agg_cache_entries,
        );
    } else {
        println!("Daemon is not running.");
    }
    Ok(())
}

/// Compute aggregate-cache hit-rate as a percentage for daemon status display.
///
/// Returns `0.0` when no lookups have occurred, avoiding a division by
/// zero on cold daemons.  The `cast_precision_loss` expect is justified
/// for telemetry display: well over `2^53` cache lookups would be
/// required to lose a single bit of precision, and the output is
/// rendered with `{:.1}` so single-bit differences are invisible.
#[expect(
    clippy::float_arithmetic,
    clippy::cast_precision_loss,
    reason = "telemetry hit-rate percent; rendered with `{:.1}` so precision loss is invisible"
)]
fn compute_hit_rate_percent(hits: u64, lookups: u64) -> f64 {
    if lookups == 0 {
        return 0.0_f64;
    }
    (hits as f64 / lookups as f64) * 100.0_f64
}

/// `uffs daemon stop` — graceful shutdown via RPC.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn daemon_stop() -> Result<()> {
    if let Ok(mut client) = UffsClientSync::connect_raw() {
        client
            .shutdown()
            .with_context(|| "Shutdown RPC failed — try `uffs daemon kill` instead")?;
        println!("Daemon shutdown requested.");
    } else {
        println!("Daemon is not running.");
    }
    Ok(())
}

/// `uffs daemon kill` — hard kill via PID file or socket discovery + cleanup.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn daemon_kill() {
    let pid_path = pid_file_path();

    let mut pid =
        uffs_client::daemon_ctl::parse_pid_file(&pid_path).map(|(file_pid, _, _, _)| file_pid);

    // No PID file → try discovering via live socket.
    if pid.is_none()
        && let Ok(mut client) = UffsClientSync::connect_raw()
        && let Ok(status) = client.status()
    {
        pid = Some(status.pid);
    }

    if let Some(target_pid) = pid {
        println!("Killing daemon (PID {target_pid})...");
        kill_pid(target_pid);
    } else {
        println!("No daemon found (no PID file, no socket connection).");
    }

    // Always clean up stale files.
    drop(std::fs::remove_file(&pid_path));
    drop(std::fs::remove_file(socket_path()));
    if pid.is_some() {
        println!("Daemon killed. PID file and socket cleaned up.");
    }
}

/// Send SIGKILL (Unix) or taskkill (Windows) to a process.
fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        drop(
            std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .output(),
        );
    }
    #[cfg(windows)]
    {
        drop(
            std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output(),
        );
    }
}

/// `uffs daemon restart` — stop, capture data sources, then re-launch.
///
/// If the daemon is running, queries its loaded drives to extract the
/// original `--mft-file` paths, stops it, then re-spawns with the same
/// arguments.  If not running, prints a message and exits.
#[expect(clippy::print_stdout, reason = "CLI user-facing output")]
fn daemon_restart() -> Result<()> {
    let spawn_args = if let Ok(mut client) = UffsClientSync::connect_raw() {
        let drives_resp = client
            .drives()
            .with_context(|| "Failed to query drives before restart")?;

        let mut args = Vec::new();
        for dr in &drives_resp.drives {
            if let Some(path) = dr.source.strip_prefix("file:") {
                args.push("--mft-file".to_owned());
                args.push(path.to_owned());
            }
        }

        let daemon_pid = client.status().map_or(0, |status_resp| status_resp.pid);
        println!("Stopping daemon (PID {daemon_pid})...");

        client.shutdown().with_context(|| {
            format!(
                "Graceful shutdown of PID {daemon_pid} failed.\n\
                 Run `uffs daemon kill` first, then retry."
            )
        })?;

        std::thread::sleep(core::time::Duration::from_secs(1));
        args
    } else {
        println!("Daemon is not running — nothing to restart.");
        return Ok(());
    };

    drop(std::fs::remove_file(pid_file_path()));
    drop(std::fs::remove_file(socket_path()));

    println!(
        "Restarting daemon with {} data source(s)...",
        spawn_args
            .iter()
            .filter(|arg| *arg == "--mft-file" || *arg == "--data-dir")
            .count()
    );

    let mut client = UffsClientSync::connect_with_args(&spawn_args)
        .with_context(|| "Failed to start restarted daemon")?;

    client
        .await_ready(core::time::Duration::from_mins(2))
        .with_context(|| "Restarted daemon did not become ready in time")?;

    let status = client.status();
    if let Ok(resp) = status {
        let state = match &resp.status {
            DaemonStatus::Loading {
                drives_loaded,
                drives_total,
            } => format!("Loading ({drives_loaded}/{drives_total} drives)"),
            DaemonStatus::Ready => "Ready".to_owned(),
            DaemonStatus::Refreshing { .. } => "Refreshing".to_owned(),
        };
        println!("Daemon restarted (PID {}), status: {state}", resp.pid);
    } else {
        println!("Daemon restarted.");
    }

    Ok(())
}

/// Normalise an env-var probe so `Some("")` becomes `None`.
///
/// `std::env::var("X")` returns `Ok("")` when a shell has left `X` set to the
/// empty string (common in PowerShell after a sub-script unsets a variable
/// via assignment rather than `Remove-Item Env:\X`).  Treating that as a real
/// value is what caused the silent `uffs daemon start` failure documented in
/// `LOG/Output`: the CLI forwarded `--log-level ""` and
/// `--log-file uffsd.log` (relative path, from `""+"/uffsd.log"`) to uffsd,
/// uffsd's `tracing_appender::rolling::never("", "uffsd.log")` then panicked
/// via `.expect("initializing rolling file appender failed")`, the panic
/// hook called `process::exit(101)` before IPC could bind, and the client
/// timed out after 20 retries with no diagnostic signal.
#[must_use]
fn non_empty_env(value: Option<String>) -> Option<String> {
    value.filter(|val| !val.is_empty())
}

#[cfg(test)]
mod tests {
    use super::non_empty_env;

    /// Missing env var → `None` flows through unchanged.
    #[test]
    fn non_empty_env_passes_none_through() {
        assert_eq!(non_empty_env(None), None);
    }

    /// **Regression (silent-start bug, `LOG/Output`):** PowerShell leaving
    /// `RUST_LOG=""` / `UFFS_LOG_DIR=""` set to the empty string must be
    /// treated exactly like "unset".  Before this fix, the CLI forwarded the
    /// empty string to uffsd as `--log-level ""` / `--log-file uffsd.log`,
    /// uffsd panicked in the tracing appender, and the client spun through
    /// 20 retries with no diagnostic signal.
    #[test]
    fn non_empty_env_collapses_empty_string_to_none() {
        assert_eq!(non_empty_env(Some(String::new())), None);
    }

    /// A legitimate non-empty value is preserved verbatim — the filter must
    /// not accidentally strip real log levels or directory paths.
    #[test]
    fn non_empty_env_preserves_real_values() {
        assert_eq!(
            non_empty_env(Some("debug".to_owned())),
            Some("debug".to_owned())
        );
        assert_eq!(
            non_empty_env(Some(r"C:\Users\rnio\bin".to_owned())),
            Some(r"C:\Users\rnio\bin".to_owned())
        );
    }

    /// Whitespace-only values are NOT treated as empty.  If someone genuinely
    /// wants `RUST_LOG=" "` we pass it through — our only concern is the
    /// `""` trap created by PowerShell's assignment-to-empty behaviour.
    /// This pins the contract so a future refactor doesn't over-trim.
    #[test]
    fn non_empty_env_keeps_whitespace_only_values() {
        assert_eq!(non_empty_env(Some(" ".to_owned())), Some(" ".to_owned()));
    }
}
