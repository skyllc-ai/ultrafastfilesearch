// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

use clap::Parser as _;

use super::{STAGE0_ID, decisions_from_cli, plan_input_hash, run, stage_selected};
use crate::cli::Cli;
use crate::host::{Call, MockHost, ProcOutput};
use crate::state::{State, Status};

/// Build an output with the given stdout (empty stderr, exit 0).
fn stdout_of(text: &str) -> ProcOutput {
    ProcOutput {
        code: Some(0),
        stdout: text.to_owned(),
        stderr: String::new(),
    }
}

/// The `Status:        Ready` line the daemon emits when fully loaded.
const DAEMON_READY_STATUS: &str =
    "Version:       0.0.0\nDaemon PID:    1\nStatus:        Ready\nDrives:\n";

/// Queue the run results needed for the `--dry-run` code path.
///
/// `--dry-run` skips `teardown::baseline`, call order is:
///  1. `daemon_start_if_needed` — `uffs daemon status` (already Ready → skip
///     start)
///  2. `resolve::es_exe`         — `where.exe es.exe` (for `everything`)
///  3. `resolve::es_exe`         — `where.exe es.exe` (for `everything_gui`)
///  4. `resolve::everything_exe` — `where.exe Everything.exe`
///  5. `env::capture` hostname
///  6. `env::capture` cpu
///  7. `env::capture` `logical_cpus`
///  8. `env::capture` `total_ram`
///  9. `env::capture` uffs --version (prefix stripped: `"uffs 0.0.0"` →
///     `"0.0.0"`)
///  10. `env::capture` `uffs_cpp` --version
///  11. `env::capture` es -version
///  12. tasklist (everything state probe — stopped)
///  13. `env::capture` es -get-everything-version
///  14. tasklist (`everything_gui` state probe — stopped)
///  15. `ensure_daemon_ready` — `uffs daemon status` → Ready on first poll
///  16. `preflight` — `es -get-everything-version` (availability check)
///  17. `preflight` — `uffs C:\ * --count` (UFFS record count for C)
///  18. `preflight` — `es -get-result-count C:\` (ES count for C → loaded)
fn dry_run_host() -> MockHost {
    let evr = "C:\\Program Files (x86)\\Everything\\Everything.exe";
    MockHost::new()
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      //  1: daemon status (Ready)
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  2: where.exe es.exe
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  3: where.exe es.exe
        .with_run_result(stdout_of(evr))                      //  4: where.exe Everything.exe
        .with_run_result(stdout_of("myhost"))                 //  5: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          //  6: cpu
        .with_run_result(stdout_of("8"))                      //  7: logical_cpus
        .with_run_result(stdout_of("8589934592"))             //  8: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             //  9: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 10: uffs_cpp --version
        .with_run_result(stdout_of("1.1.0.30"))               // 11: es -version
        .with_run_result(stdout_of(""))                       // 12: tasklist (stopped)
        .with_run_result(stdout_of("1.4.1.1032"))             // 13: es -get-everything-version
        .with_run_result(stdout_of(""))                       // 14: tasklist (stopped)
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 15: ensure_daemon_ready poll
        .with_run_result(stdout_of("1.4.1.1032"))             // 16: preflight es availability
        .with_run_result(stdout_of("3000000"))                // 17: uffs C:\ count
        .with_run_result(stdout_of("1000")) // 18: es result-count C → loaded
}

/// Queue the run results needed for the autopilot (non-dry-run) path.
///
/// `teardown::baseline` fires `uffs daemon status` before the env probes:
///  1. `teardown::baseline`      — `uffs daemon status`
///  2. `daemon_start_if_needed`  — `uffs daemon status` (already Ready → skip
///     start)
///  3. `resolve::es_exe`         — `where.exe es.exe` (for `everything`)
///  4. `resolve::es_exe`         — `where.exe es.exe` (for `everything_gui`)
///  5. `resolve::everything_exe` — `where.exe Everything.exe`
///  6. `env::capture` hostname
///  7. `env::capture` cpu
///  8. `env::capture` `logical_cpus`
///  9. `env::capture` `total_ram`
///  10. `env::capture` uffs --version (prefix stripped: `"uffs 0.0.0"` →
///      `"0.0.0"`)
///  11. `env::capture` `uffs_cpp` --version
///  12. `env::capture` es -version
///  13. tasklist (everything state probe — stopped)
///  14. `env::capture` es -get-everything-version
///  15. tasklist (`everything_gui` state probe — stopped)
///  16. `ensure_daemon_ready`    — `uffs daemon status` → Ready on first poll
///  17. `preflight`              — `es -get-everything-version` (availability)
///  18. `preflight`              — `uffs C:\ * --count`
///  19. `preflight`              — `es -get-result-count C:\` → loaded
fn autopilot_host() -> MockHost {
    let evr = "C:\\Program Files (x86)\\Everything\\Everything.exe";
    MockHost::new()
        .with_run_result(stdout_of("running"))                 //  1: teardown daemon status
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      //  2: daemon_start_if_needed
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  3: where.exe es.exe
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  4: where.exe es.exe
        .with_run_result(stdout_of(evr))                      //  5: where.exe Everything.exe
        .with_run_result(stdout_of("myhost"))                 //  6: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          //  7: cpu
        .with_run_result(stdout_of("8"))                      //  8: logical_cpus
        .with_run_result(stdout_of("8589934592"))             //  9: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             // 10: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 11: uffs_cpp --version
        .with_run_result(stdout_of("1.1.0.30"))               // 12: es -version
        .with_run_result(stdout_of(""))                       // 13: tasklist (stopped)
        .with_run_result(stdout_of("1.4.1.1032"))             // 14: es -get-everything-version
        .with_run_result(stdout_of(""))                       // 15: tasklist (stopped)
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 16: ensure_daemon_ready poll
        .with_run_result(stdout_of("1.4.1.1032"))             // 17: preflight es availability
        .with_run_result(stdout_of("3000000"))                // 18: uffs C:\ count
        .with_run_result(stdout_of("1000")) // 19: es result-count C → loaded
}

/// Whether any recorded call mutated the host filesystem.
fn is_mutation(call: &Call) -> bool {
    matches!(
        call,
        Call::WriteFile(_) | Call::RemoveFile(_) | Call::Rename(_, _) | Call::CreateDirAll(_)
    )
}

/// Paths written via `write_file` during the run, as display strings.
fn writes(host: &MockHost) -> Vec<String> {
    host.calls()
        .into_iter()
        .filter_map(|call| {
            if let Call::WriteFile(path) = call {
                Some(path.display().to_string())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn dry_run_mutates_nothing() {
    let host = dry_run_host();
    let cli = Cli::parse_from(["uffs-bench", "--dry-run", "--drives", "C"]);

    run(&host, &cli).expect("dry run succeeds");

    assert!(
        host.calls().iter().all(|call| !is_mutation(call)),
        "dry-run must perform zero filesystem mutations"
    );
}

#[test]
fn autopilot_writes_stage0_artifacts_and_saves_state() {
    let host = autopilot_host();
    let cli = Cli::parse_from([
        "uffs-bench",
        "--auto",
        "--drives",
        "C",
        "--bundle-root",
        "/out",
    ]);

    run(&host, &cli).expect("autopilot run succeeds");

    let calls = host.calls();
    assert!(
        calls
            .iter()
            .any(|call| matches!(call, Call::CreateDirAll(_))),
        "a bundle directory should be created"
    );
    let written = writes(&host);
    for artifact in ["env.json", "competitor-preflight.json", "matrix.json"] {
        assert!(
            written.iter().any(|path| path.ends_with(artifact)),
            "expected {artifact} to be written, got {written:?}"
        );
    }
    // `state.json` is saved atomically (temp write + rename).
    assert!(calls.iter().any(|call| matches!(call, Call::Rename(_, _))));
}

#[test]
fn resume_skips_cached_stage0() {
    let bundle = "/out/bench-fixed";
    let cli = Cli::parse_from([
        "uffs-bench",
        "--auto",
        "--only-stage",
        "0",
        "--drives",
        "C",
        "--bundle",
        bundle,
    ]);
    // Seed a state where Stage 0 is already Done with the matching hash.
    let seed = MockHost::new();
    let hash = plan_input_hash(&decisions_from_cli(&cli));
    let mut state = State::new(&seed, "test", decisions_from_cli(&cli));
    state.set_step(&seed, STAGE0_ID, Status::Done, hash.as_str(), Vec::new());
    let json = serde_json::to_vec(&state).expect("serialize seed state");
    let host = MockHost::new().with_file(format!("{bundle}/state.json"), json);

    run(&host, &cli).expect("resume run succeeds");

    // Stage 0 was cached, so no matrix.json is (re)written.
    assert!(
        !writes(&host)
            .iter()
            .any(|path| path.ends_with("matrix.json")),
        "cached Stage 0 must not rewrite its artifacts"
    );
    assert!(
        host.output()
            .iter()
            .any(|line| line.contains("cached (resume)")),
        "the cached-skip notice should be shown"
    );
}

#[test]
fn stage_selection_honors_only_and_from() {
    let only = Cli::parse_from(["uffs-bench", "--only-stage", "2"]);
    assert!(stage_selected(&only, 2));
    assert!(!stage_selected(&only, 1));
    assert!(!stage_selected(&only, 0));

    let from = Cli::parse_from(["uffs-bench", "--from-stage", "1"]);
    assert!(!stage_selected(&from, 0));
    assert!(stage_selected(&from, 1));
    assert!(stage_selected(&from, 3));

    let all = Cli::parse_from(["uffs-bench"]);
    assert!(stage_selected(&all, 0));
    assert!(stage_selected(&all, 3));
}
