// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! UFFS (Ultra Fast File Search) CLI — thin synchronous client.
//!
//! All heavy lifting (including CLI arg parsing) happens in the daemon.
//! This binary detects subcommands and forwards raw search args via
//! `search_cli` RPC.  Argument transforms specific to the search
//! subcommand live in [`commands::search::args`].

// CLI main module uses single-call functions by design
#![expect(
    clippy::single_call_fn,
    reason = "CLI entry point functions are called once from main"
)]

use anyhow::{Context as _, Result};
#[cfg(test)]
use assert_cmd as _;

pub mod args;
pub mod commands;

/// Run the CLI and return a result.
fn run() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();
    let tokens: Vec<&str> = raw_args.iter().skip(1).map(String::as_str).collect();

    // Fast paths: help / version / no args.
    match tokens.first().copied() {
        None | Some("--help" | "-h" | "help") => {
            args::print_help();
            return Ok(());
        }
        Some("--version" | "-V" | "version") => {
            args::print_version();
            return Ok(());
        }
        _ => {}
    }

    // Detect subcommand as first non-flag token.
    let first = tokens.first().copied().unwrap_or("");
    let subcmd_args = raw_args.get(2..).unwrap_or_default();
    match first {
        "stats" => run_stats(subcmd_args)?,
        "aggregate" | "agg" => run_aggregate(subcmd_args)?,
        "daemon" => run_daemon(subcmd_args)?,
        "mcp" => commands::mcp_mgmt::mcp_from_args(subcmd_args)?,
        "status" => {
            if subcmd_args.iter().any(|arg| arg == "--help" || arg == "-h") {
                args::print_status_help();
            } else {
                commands::system_status::system_status();
            }
        }
        _ => {
            // Default: search — forward ALL args after "uffs" to daemon.
            run_search(raw_args.get(1..).unwrap_or_default())?;
        }
    }

    Ok(())
}

/// Timing + payload summary forwarded to [`print_client_profile`].
///
/// Packaging these into a struct keeps `run_search` under the
/// `clippy::too-many-lines` cap and lets the profile helper take one
/// argument instead of six.
struct ClientProfile<'a> {
    /// Wall-clock time spent in `UffsClientSync::connect_with_args`.
    connect_ms: u128,
    /// Wall-clock time spent in `await_ready` (daemon warm-up).
    ready_ms: u128,
    /// Wall-clock time spent in the `search_cli` IPC round-trip.
    ipc_ms: u128,
    /// Daemon-reported search duration (from the response envelope).
    duration_ms: u64,
    /// Payload delivery channel the daemon picked for this response.
    /// Used by [`print_client_profile`] to show the transport name
    /// and to pick the cheapest authoritative row-count source.
    payload: &'a uffs_client::protocol::response::SearchPayload,
    /// Total row count reported by the daemon, independent of which
    /// transport carries the payload.  Used to display the "Rows
    /// returned:" line when the transport is a shmem blob — counting
    /// newlines in the mmap would consume the file before the stdout
    /// write and double the syscall cost.
    total_count: u64,
    /// Daemon-side `profile` object from the response envelope.  When
    /// populated, its `scan_ms` / `sort_ms` / `path_resolve_ms` /
    /// `write_ms` fields are rendered as a sub-phase breakdown inside
    /// the daemon block so the `--profile` output pinpoints where the
    /// per-query cost sits (scan vs sort vs path resolution vs disk
    /// write).
    daemon_profile: Option<&'a uffs_client::protocol::response::SearchProfile>,
}

/// Print the `--profile` / `--benchmark` client-side timing block to
/// stderr (matches the daemon-side profile formatting).
#[expect(
    clippy::print_stderr,
    reason = "intentional --profile output to stderr"
)]
fn print_client_profile(prof: &ClientProfile<'_>) {
    use uffs_client::protocol::response::SearchPayload;

    eprintln!("=== PROFILE: Client → Daemon ===");
    eprintln!("  Connect:         {:>6} ms", prof.connect_ms);
    eprintln!("  Await ready:     {:>6} ms", prof.ready_ms);
    eprintln!(
        "  Search (IPC):    {:>6} ms  (daemon: {} ms)",
        prof.ipc_ms, prof.duration_ms
    );
    // Sub-phase breakdown from the daemon profile.  Any non-zero
    // component is printed; all-zero (regex/trigram paths, legacy
    // daemons) collapses to a single-line total.
    if let Some(dp) = prof.daemon_profile {
        let scan = dp.scan_ms;
        let sort = dp.sort_ms;
        let resolve = dp.path_resolve_ms;
        let write = dp.write_ms;
        if (scan | sort | resolve | write) > 0 {
            eprintln!(
                "    scan={scan} ms  sort={sort} ms  path_resolve={resolve} ms  write={write} ms"
            );
        }
        // Deep-profile breakdown: only present when the numeric-sort
        // branch populated the `path_*` sub-counters.  Prints per-
        // record averages derived from ns totals so the user can see
        // immediately whether the bottleneck is path-walking or
        // row-building, and whether the DirCache hit rate is high
        // enough to warrant a locality optimisation.
        let candidates = dp.path_candidates;
        let cache_entries = dp.path_cache_entries;
        let resolve_ns = dp.path_resolve_fn_ns;
        let build_ns = dp.path_build_row_ns;
        if candidates > 0 {
            let hits = candidates.saturating_sub(cache_entries);
            // Integer-math hit rate in permille (0–1000) to avoid
            // float arithmetic — clippy::float_arithmetic is banned
            // in production lints.  `permille / 10 . permille % 10`
            // prints as "99.7" for 997.
            let hit_permille = hits.saturating_mul(1000) / candidates;
            let hit_whole = hit_permille / 10;
            let hit_frac = hit_permille % 10;
            let avg_resolve_ns = resolve_ns / candidates;
            let avg_build_ns = build_ns / candidates;
            eprintln!(
                "    deep: candidates={candidates}  unique_parents={cache_entries}  \
                 hit_rate={hit_whole}.{hit_frac}%"
            );
            eprintln!(
                "          resolve_fn={} ms ({} ns/rec)  build_row={} ms ({} ns/rec)",
                resolve_ns / 1_000_000,
                avg_resolve_ns,
                build_ns / 1_000_000,
                avg_build_ns,
            );
        }
    }
    // Row count resolution — pick the cheapest authoritative source
    // depending on which payload variant the daemon used:
    // 1. `ShmemBlob` → mmap'd file; counting newlines would read every page just to
    //    discard the count, so use the daemon's pre- computed `total_count`
    //    instead.
    // 2. `InlineBlob` → inline string already in memory; scanning for `\n` is ~5
    //    GB/s, cheap.
    // 3. Rows variants (`InlineRows`, `ShmemRows`) → `row_count_hint()` is O(1) —
    //    `Vec::len` or the daemon's pre-computed count.
    // 4. `Empty` → zero rows, nothing to count.
    let row_count = match prof.payload {
        SearchPayload::ShmemBlob(_) => {
            // `try_from` instead of `as` to preserve correctness on
            // hypothetical 32-bit targets where `u64` would truncate
            // (clippy::cast_possible_truncation).  `u64::MAX` is a
            // strictly larger fallback than any realistic row count.
            usize::try_from(prof.total_count).unwrap_or(usize::MAX)
        }
        SearchPayload::InlineBlob(blob) => blob.bytes().filter(|byte| *byte == b'\n').count(),
        SearchPayload::InlineRows(_) | SearchPayload::ShmemRows { .. } | SearchPayload::Empty => {
            prof.payload.row_count_hint().unwrap_or(0)
        }
    };
    eprintln!("  Rows returned:   {row_count:>6}");
    match prof.payload {
        SearchPayload::ShmemBlob(_) => {
            eprintln!("  Transport:       shmem_blob (mmap + write_all, binary)");
        }
        SearchPayload::InlineBlob(_) => {
            eprintln!("  Transport:       inline_blob (single write_all)");
        }
        SearchPayload::ShmemRows { .. } => {
            eprintln!("  Transport:       shmem_rows (mmap + per-row format)");
        }
        SearchPayload::InlineRows(_) | SearchPayload::Empty => {
            // inline_rows is the default — no extra line needed.
            // empty responses skip the transport line entirely.
        }
    }
}

/// Forward raw search args to the daemon via `search_cli` RPC.
fn run_search(args: &[String]) -> Result<()> {
    if args.is_empty() {
        args::print_help();
        return Ok(());
    }

    // Extract daemon-spawn args (--data-dir, --mft-file, --no-cache)
    // from the raw args so we can auto-start the daemon if needed.
    let spawn_args = commands::search::args::extract_spawn_args(args);

    let t_connect = std::time::Instant::now();
    let mut client = uffs_client::connect_sync::UffsClientSync::connect_with_args(&spawn_args)
        .with_context(|| "Failed to connect to UFFS daemon")?;
    let connect_ms = t_connect.elapsed().as_millis();

    let t_ready = std::time::Instant::now();
    // 2 minutes — `from_mins` is nightly-only as of 2026-04.
    #[expect(
        clippy::duration_suboptimal_units,
        reason = "Duration::from_mins is nightly-only"
    )]
    let ready_timeout = core::time::Duration::from_secs(120);
    client
        .await_ready(ready_timeout)
        .with_context(|| "Daemon did not become ready in time")?;
    let ready_ms = t_ready.elapsed().as_millis();

    let t_search = std::time::Instant::now();
    // Resolve relative --out paths to absolute using the CLI's cwd, since the
    // daemon process runs in a different working directory.
    // Phase 3.1 NUL fast path: when stdout is redirected to the null
    // device (e.g. `uffs *.dll > NUL`), inject `--no-output` so the
    // daemon skips row materialisation + `paths_blob` construction
    // + IPC row transfer entirely.  Saves ~20-30 ms on medium result
    // sets that would otherwise push 3.5 MB through the pipe just to
    // discard the bytes client-side.
    let args_owned: Vec<String> = commands::search::args::inject_no_output_for_null_stdout(
        commands::search::args::resolve_out_path(args),
    );
    let raw_response = client
        .search_cli_raw(&args_owned)
        .with_context(|| "Daemon search_cli failed")?;
    let ipc_ms = t_search.elapsed().as_millis();

    // v0.5.62: deserialise the daemon response into the typed
    // `SearchResponse` struct.  The `SearchPayload` enum is
    // self-describing (serde tag = "kind", content = "data") so the
    // CLI no longer needs to probe individual fields like
    // `paths_blob`, `paths_blob_shmem`, `shmem_path`, etc. — the
    // enum's variant is the single source of truth for which
    // transport the daemon picked.
    //
    // Unknown fields on the wire are silently ignored (serde default),
    // so newer daemons that add optional response fields are still
    // forward-compatible with this CLI.
    let response: uffs_client::protocol::response::SearchResponse =
        serde_json::from_value(raw_response)
            .with_context(|| "Failed to deserialize search response from daemon")?;

    if args
        .iter()
        .any(|arg| arg == "--profile" || arg == "--benchmark")
    {
        print_client_profile(&ClientProfile {
            connect_ms,
            ready_ms,
            ipc_ms,
            duration_ms: response.duration_ms,
            payload: &response.payload,
            total_count: response.total_count,
            daemon_profile: response.profile.as_ref(),
        });
    }

    // OPT-4: When --out is specified, the daemon writes the file directly
    // and returns `SearchPayload::Empty`.  Don't overwrite the file.
    // Handles both `--out foo.csv` (separate arg) and `--out=foo.csv` (= form).
    let has_out = args
        .iter()
        .any(|arg| arg == "--out" || arg.starts_with("--out="));
    let daemon_wrote_file = has_out && response.payload.is_empty();

    // Phase 3.1 NUL fast path: `--no-output` (explicit or auto-injected
    // for NUL stdout) skips every client-side stdout write.
    let suppress_stdout = args_owned.iter().any(|arg| arg == "--no-output");

    if !daemon_wrote_file && !suppress_stdout {
        write_search_payload_to_stdout(response.payload, args)?;
    }

    if !suppress_stdout && !response.aggregations.is_empty() {
        // `write_aggregations` still consumes `&[serde_json::Value]`
        // for format flexibility — re-serialise the typed
        // `AggregateResultWire` list via `to_value` once up front
        // and pass the slice to the helper.  Allocation is one per
        // aggregation bucket, which is trivial compared to the
        // aggregation itself.
        let agg_values: Vec<serde_json::Value> = response
            .aggregations
            .iter()
            .filter_map(|agg| serde_json::to_value(agg).ok())
            .collect();
        commands::search::dispatch::write_aggregations(&agg_values, args)?;
    }

    Ok(())
}

/// Write the daemon's search payload to stdout, picking the fastest
/// transport the daemon selected for this response.
///
/// Priority order matches the [`SearchPayload`] variant dispatch:
///
/// 1. [`SearchPayload::ShmemBlob`] → mmap the raw-bytes file and stream
///    directly to stdout via [`uffs_client::shmem::stream_paths_blob_into`].
///    Zero-copy, zero JSON decode, zero UTF-8 re-validation.  Used for blobs
///    above [`uffs_client::shmem::PATHS_BLOB_SHMEM_THRESHOLD`].
/// 2. [`SearchPayload::InlineBlob`] → single `write_all` of the inline UTF-8
///    buffer.  Skips per-row formatting but still paid ~40 ms of JSON decode on
///    the way in.
/// 3. [`SearchPayload::ShmemRows`] → read the shmem file into a
///    `Vec<SearchRow>` (client's `connect_sync` shim doesn't do transparent
///    resolution for `search_cli`), then fall through to per-row format
///    dispatch.
/// 4. [`SearchPayload::InlineRows`] → traditional per-row format + write
///    dispatch in [`commands::search::dispatch::write_rows`].
/// 5. [`SearchPayload::Empty`] → nothing to write.
///
/// Extracted from `run_search` to keep that function under the
/// `clippy::too_many_lines` cap.
///
/// [`SearchPayload`]: uffs_client::protocol::response::SearchPayload
/// [`SearchPayload::ShmemBlob`]: uffs_client::protocol::response::SearchPayload::ShmemBlob
/// [`SearchPayload::InlineBlob`]: uffs_client::protocol::response::SearchPayload::InlineBlob
/// [`SearchPayload::ShmemRows`]: uffs_client::protocol::response::SearchPayload::ShmemRows
/// [`SearchPayload::InlineRows`]: uffs_client::protocol::response::SearchPayload::InlineRows
/// [`SearchPayload::Empty`]: uffs_client::protocol::response::SearchPayload::Empty
fn write_search_payload_to_stdout(
    payload: uffs_client::protocol::response::SearchPayload,
    args: &[String],
) -> Result<()> {
    use uffs_client::protocol::response::SearchPayload;
    match payload {
        SearchPayload::Empty => {
            // Nothing to write — no-match query, `--no-output`
            // injection, or `--out=file` (daemon already wrote to
            // disk).  The earlier `daemon_wrote_file` guard also
            // handles the latter case at the call site.
        }
        SearchPayload::ShmemBlob(shmem_path_str) => {
            // Binary shmem transport: mmap the file and write bytes
            // directly to stdout with one syscall, then delete the
            // file.  No JSON decode, no intermediate allocation, no
            // UTF-8 re-validation — stdout takes bytes.
            let shmem_path = std::path::Path::new(&shmem_path_str);
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            uffs_client::shmem::stream_paths_blob_into(shmem_path, &mut handle)
                .with_context(|| format!("Failed to stream shmem_blob from {shmem_path_str}"))?;
        }
        SearchPayload::InlineBlob(blob) => {
            // Single write_all to stdout — the buffer is one
            // contiguous slice; the whole point of the blob
            // inline transport.
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            std::io::Write::write_all(&mut handle, blob.as_bytes())
                .with_context(|| "Failed to write inline_blob to stdout")?;
        }
        SearchPayload::ShmemRows { path, .. } => {
            // Shmem rows variant: read the file (returns a
            // `SearchResponse` with `InlineRows`) and dispatch to
            // the per-row writer.  Re-encode rows to `Value` so the
            // existing `write_rows` path (which handles `--format`,
            // `--sep`, `--header`, column resolution, etc.) stays
            // untouched — one Vec allocation scales O(N) but beats
            // duplicating the column-resolution logic.
            let shmem_resp = uffs_client::shmem::read_search_results(std::path::Path::new(&path))
                .with_context(|| format!("Failed to read shmem_rows from {path}"))?;
            let row_values: Vec<serde_json::Value> = shmem_resp
                .payload
                .into_inline_rows()
                .unwrap_or_default()
                .iter()
                .filter_map(|row| serde_json::to_value(row).ok())
                .collect();
            commands::search::dispatch::write_rows(&row_values, args)?;
        }
        SearchPayload::InlineRows(rows) => {
            // Traditional per-row format dispatch.  `write_rows`
            // accepts `&[serde_json::Value]` for format flexibility
            // (extract_field, parity-compat, drilldown), so re-
            // serialise the typed rows once up front.
            let row_values: Vec<serde_json::Value> = rows
                .iter()
                .filter_map(|row| serde_json::to_value(row).ok())
                .collect();
            commands::search::dispatch::write_rows(&row_values, args)?;
        }
    }
    Ok(())
}

/// Handle `uffs stats [path] [--top N] [--data-dir ...] [--mft-file ...]`.
fn run_stats(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        args::print_stats_help();
        return Ok(());
    }
    // Simple arg extraction for stats subcommand.
    let mut path: Option<std::path::PathBuf> = None;
    let mut top: u32 = 10;
    let mut data_dir: Option<std::path::PathBuf> = None;
    let mut mft_file: Vec<std::path::PathBuf> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--top" => {
                if let Some(val) = iter.next() {
                    top = val
                        .parse()
                        .map_err(|err| anyhow::anyhow!("Bad --top: {err}"))?;
                }
            }
            "--data-dir" => {
                if let Some(val) = iter.next() {
                    data_dir = Some(val.into());
                }
            }
            "--mft-file" => {
                if let Some(val) = iter.next() {
                    mft_file = val.split(',').map(|part| part.trim().into()).collect();
                }
            }
            other if !other.starts_with('-') && path.is_none() => {
                path = Some(other.into());
            }
            _ => {}
        }
    }

    if let Some(stats_path) = path {
        commands::stats::stats(Some(&stats_path), top)?;
    } else {
        // Synthesise search args for an aggregate-only overview query.
        let mut synth_args = vec![
            "*".to_owned(),
            "--agg".to_owned(),
            "overview".to_owned(),
            "--format".to_owned(),
            "table".to_owned(),
            "--limit".to_owned(),
            "0".to_owned(),
        ];
        if let Some(dir) = data_dir {
            synth_args.extend(["--data-dir".to_owned(), dir.to_string_lossy().into_owned()]);
        }
        for mf in &mft_file {
            synth_args.extend(["--mft-file".to_owned(), mf.to_string_lossy().into_owned()]);
        }
        run_search(&synth_args)?;
    }
    Ok(())
}

/// Handle `uffs aggregate|agg <preset> [--format ...] [--data-dir ...]`.
fn run_aggregate(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        args::print_aggregate_help();
        return Ok(());
    }
    // Extract the preset (first positional arg).
    let preset = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Usage: uffs aggregate <PRESET>\n\
                 Available presets: overview, by_type, by_extension, by_drive, by_size, by_age, count"
            )
        })?;

    // Synthesise search args: `* --agg <preset> --limit 0 [remaining flags]`.
    let mut synth_args = vec![
        "*".to_owned(),
        "--agg".to_owned(),
        preset.clone(),
        "--limit".to_owned(),
        "0".to_owned(),
    ];
    // Default to table format for `uffs agg` unless user specifies --format.
    let has_format = args.iter().any(|arg| arg == "--format" || arg == "-f");
    if !has_format {
        synth_args.extend(["--format".to_owned(), "table".to_owned()]);
    }
    // Forward all flags (skip the preset positional).
    for arg in args {
        if arg == preset {
            continue;
        }
        synth_args.push(arg.clone());
    }
    run_search(&synth_args)
}

/// Handle `uffs daemon <action> [flags...]`.
fn run_daemon(args: &[String]) -> Result<()> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        args::print_daemon_help();
        return Ok(());
    }
    let action = args::parse_daemon_action(args)?;
    commands::daemon_mgmt::daemon(&action)
}

/// Entry point — synchronous, no runtime.
#[expect(
    clippy::print_stderr,
    reason = "intentional user-facing error output to stderr"
)]
fn main() {
    if let Err(err) = run() {
        // Special-case DaemonNeedsElevation: render a multi-option help
        // message instead of the generic `Error: ... Caused by: ...`
        // chain, so a UAC failure reads like advice and not a crash.
        if let Some(needs) = find_needs_elevation(&err) {
            eprintln!("{}", format_elevation_help(needs));
            std::process::exit(1);
        }

        for (idx, cause) in err.chain().enumerate() {
            if idx == 0 {
                eprintln!("Error: {cause}");
            } else {
                eprintln!("  Caused by: {cause}");
            }
        }
        std::process::exit(1);
    }
}

/// Walk an [`anyhow::Error`] chain looking for
/// [`uffs_client::error::ClientError::DaemonNeedsElevation`].
///
/// Returns the daemon path that would have been spawned, so the
/// formatter can quote it back to the user verbatim.  Returns `None`
/// if no elevation error is present in the chain.
fn find_needs_elevation(err: &anyhow::Error) -> Option<&str> {
    for cause in err.chain() {
        if let Some(uffs_client::error::ClientError::DaemonNeedsElevation { daemon_path }) =
            cause.downcast_ref::<uffs_client::error::ClientError>()
        {
            return Some(daemon_path.as_str());
        }
    }
    None
}

/// Render the "daemon needs admin" help message.
///
/// Lists three independent recovery paths so users can pick whichever
/// fits their workflow — scripted, interactive one-off, or permanent.
fn format_elevation_help(daemon_path: &str) -> String {
    format!(
        "Error: UFFS daemon needs admin privileges to read NTFS Master File Tables.\n\
         \n\
         The daemon is not running, and this shell is not elevated.  To start it, pick one:\n\
         \n  \
         1. Relaunch in an elevated shell (PowerShell/cmd \"Run as administrator\"),\n     \
            then retry the command.\n\
         \n  \
         2. Explicitly request a UAC prompt for this invocation:\n       \
               uffs daemon start --elevate\n     \
            Or set it as the default for the current session:\n       \
               set UFFS_ELEVATE=1     (cmd)\n       \
               $env:UFFS_ELEVATE = '1'  (PowerShell)\n\
         \n  \
         3. Install the broker service — one-time setup, no future UAC prompts:\n       \
               uffs-broker --install\n\
         \n\
         Daemon binary that would have been spawned:\n  \
           {daemon_path}"
    )
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::default_numeric_fallback,
        reason = "test module — relaxed linting"
    )]

    use uffs_client::protocol::SearchParams;

    use super::args::parse_drive_letter;
    use super::{find_needs_elevation, format_elevation_help};

    /// The elevation help must name every recovery path the user has,
    /// so a UAC-blocked invocation becomes actionable advice rather
    /// than a dead-end crash.  Locks the contract in place.
    #[test]
    fn elevation_help_lists_all_recovery_paths() {
        let help = format_elevation_help(r"C:\Program Files\uffs\uffsd.exe");
        assert!(help.contains("admin"), "help must mention admin: {help}");
        assert!(
            help.contains("--elevate"),
            "help must document --elevate: {help}"
        );
        assert!(
            help.contains("UFFS_ELEVATE"),
            "help must document the env var: {help}"
        );
        assert!(
            help.contains("uffs-broker --install"),
            "help must document the broker install path: {help}"
        );
        assert!(
            help.contains(r"C:\Program Files\uffs\uffsd.exe"),
            "help must quote the daemon path: {help}"
        );
    }

    /// `find_needs_elevation` must walk through any `.with_context`
    /// layers that the CLI adds on top of the raw `ClientError`.
    #[test]
    fn find_needs_elevation_walks_anyhow_context() {
        let base = anyhow::Error::from(uffs_client::error::ClientError::DaemonNeedsElevation {
            daemon_path: "uffsd-test".to_owned(),
        });
        let wrapped: anyhow::Error = base.context("while connecting");
        assert_eq!(find_needs_elevation(&wrapped), Some("uffsd-test"));
    }

    /// Unrelated errors must not be mistaken for an elevation problem,
    /// so the default `Error: ... / Caused by:` chain is preserved for
    /// everything else.
    #[test]
    fn find_needs_elevation_returns_none_for_other_errors() {
        let other = anyhow::Error::from(uffs_client::error::ClientError::ConnectionFailed(
            "nope".to_owned(),
        ));
        assert!(find_needs_elevation(&other).is_none());
    }

    #[test]
    fn test_parse_drive_letter_accepts_letter_colon_and_whitespace_variants() {
        assert_eq!(parse_drive_letter("c"), Ok('C'));
        assert_eq!(parse_drive_letter("C:"), Ok('C'));
        assert_eq!(parse_drive_letter(" d: "), Ok('D'));
    }

    #[test]
    fn test_parse_drive_letter_rejects_invalid_values() {
        parse_drive_letter("").unwrap_err();
        parse_drive_letter("12").unwrap_err();
        parse_drive_letter("1:").unwrap_err();
        parse_drive_letter("CD").unwrap_err();
    }

    #[test]
    fn test_from_cli_args_basic_search() {
        let args: Vec<String> = [
            "*.rs",
            "--drive",
            "C",
            "--format",
            "json",
            "--tz-offset",
            "-8",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let params = SearchParams::from_cli_args(&args).expect("should parse");
        // `*.rs` is promoted to pattern="*" + ext=Some("rs") so the
        // daemon can route through the ExtensionIndex fast path in
        // `numeric_top_n::ext_fast_path` instead of the trigram + glob
        // path.  See `is_pure_ext_glob` in cli_args.rs for the shape
        // acceptance matrix and `test_from_cli_args_ext_glob_promoted`
        // in uffs-client for the full rewrite semantics.
        assert_eq!(params.pattern, "*");
        assert_eq!(params.ext.as_deref(), Some("rs"));
        assert_eq!(params.drives, vec!['C']);
        assert_eq!(params.output_tz_offset_hours, Some(-8));
    }

    #[test]
    fn test_from_cli_args_sugar_begins_with() {
        let args: Vec<String> = ["--begins-with", "report"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let params = SearchParams::from_cli_args(&args).expect("should parse");
        assert_eq!(params.pattern, "report*");
    }

    #[test]
    fn test_from_cli_args_sugar_between() {
        let args: Vec<String> = ["*", "--between", "2026-01-01,2026-03-31"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let params = SearchParams::from_cli_args(&args).expect("should parse");
        assert_eq!(params.newer.as_deref(), Some("2026-01-01"));
        assert_eq!(params.older.as_deref(), Some("2026-03-31"));
    }
}
