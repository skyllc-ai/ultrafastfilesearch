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

/// The `Status:        Ready` output the daemon emits when fully loaded.
///
/// Includes a `[Warm]` line for `C:` so the preflight `warm_parked_drives`
/// step skips the preload call for the `--drives C` test spec.
const DAEMON_READY_STATUS: &str = "Version:       0.0.0\n\
    Daemon PID:    1\n\
    Status:        Ready\n\
    Drives:\n\
      [Warm]   C: \u{2014}  3,000,000 records (live) \u{2014} 300 MB\n";

/// Queue the run results needed for the `--dry-run` code path.
///
/// `capture()` first kills and restarts the daemon with no drive restrictions
/// so it self-discovers all drives, then env-probes, then `ensure_daemon_ready`
/// before first preflight.  In dry-run mode the gated restart fires as
/// `ProceedNoop` so the kill/start `--drive C` are skipped; no second-pass
/// preflight is run.
///
///  1. `capture()` initial kill  -- `uffs --daemon kill`
///  2. `capture()` initial start -- `uffs --daemon start` (no drives)
///  3. `resolve::es_exe`         -- `where.exe es.exe` (for `everything`)
///  4. `resolve::es_exe`         -- `where.exe es.exe` (for `everything_gui`)
///  5. `resolve::everything_exe` -- `where.exe Everything.exe`
///  6. `env::capture` hostname
///  7. `env::capture` cpu
///  8. `env::capture` `logical_cpus`
///  9. `env::capture` `total_ram`
/// 10. `env::capture` uffs --version
/// 11. `env::capture` `uffs_cpp` --version
/// 12. tasklist (`everything` state probe -- stopped)
/// 13. `env::capture` es -version
/// 14. tasklist (`everything_gui` state probe -- stopped)
/// 15. `env::capture` es -get-everything-version
/// 16. `ensure_daemon_ready`     -- `uffs --daemon status` -> Ready
/// 17. `preflight`  -- `es -get-everything-version` (availability)
/// 18. `preflight`  -- `uffs --daemon status` (record counts for C)
/// 19. `preflight`  -- `es -get-result-count C:` -> loaded
fn dry_run_host() -> MockHost {
    let evr = "C:\\Program Files (x86)\\Everything\\Everything.exe";
    MockHost::new()
        .with_run_result(stdout_of(""))                       //  1: initial daemon kill
        .with_run_result(stdout_of(""))                       //  2: initial daemon start (all drives)
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  3: where.exe es.exe
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  4: where.exe es.exe
        .with_run_result(stdout_of(evr))                      //  5: where.exe Everything.exe
        .with_run_result(stdout_of("myhost"))                 //  6: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          //  7: cpu
        .with_run_result(stdout_of("8"))                      //  8: logical_cpus
        .with_run_result(stdout_of("8589934592"))             //  9: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             // 10: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 11: uffs_cpp --version
        .with_run_result(stdout_of(""))                       // 12: tasklist (everything stopped)
        .with_run_result(stdout_of("1.1.0.30"))               // 13: es -version
        .with_run_result(stdout_of(""))                       // 14: tasklist (everything_gui stopped)
        .with_run_result(stdout_of("1.4.1.1032"))             // 15: es -get-everything-version
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 16: ensure_daemon_ready (all-drives)
        .with_run_result(stdout_of("1.4.1.1032"))             // 17: preflight es availability
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 18: preflight daemon status
        .with_run_result(stdout_of("1000")) // 19: es result-count C
}

/// Queue the run results needed for the autopilot (non-dry-run) path.
///
/// `teardown::baseline` fires `uffs --daemon status` first.  Then `capture()`
/// kills+restarts with no drives (self-discover all), env-probes,
/// `ensure_daemon_ready`, first preflight.  After the matrix the gated
/// UFFS restart kills+starts with `--drive C`; a second-pass preflight
/// follows.
///
///  1. `teardown::baseline`      -- `uffs --daemon status`
///  2. `capture()` initial kill  -- `uffs --daemon kill`
///  3. `capture()` initial start -- `uffs --daemon start` (no drives)
///  4. `resolve::es_exe`         -- `where.exe es.exe` (for `everything`)
///  5. `resolve::es_exe`         -- `where.exe es.exe` (for `everything_gui`)
///  6. `resolve::everything_exe` -- `where.exe Everything.exe`
///  7. `env::capture` hostname
///  8. `env::capture` cpu
///  9. `env::capture` `logical_cpus`
/// 10. `env::capture` `total_ram`
/// 11. `env::capture` uffs --version
/// 12. `env::capture` `uffs_cpp` --version
/// 13. tasklist (`everything` state probe -- stopped)
/// 14. `env::capture` es -version
/// 15. tasklist (`everything_gui` state probe -- stopped)
/// 16. `env::capture` es -get-everything-version
/// 17. `ensure_daemon_ready`     -- `uffs --daemon status` -> Ready
/// 18. `preflight`  -- `es -get-everything-version` (availability)
/// 19. `preflight`  -- `uffs --daemon status` (record counts for C)
/// 20. `preflight`  -- `es -get-result-count C:` -> loaded
/// 21. `uffs --daemon kill`   (gated restart: autopilot -> proceed)
/// 22. `uffs --daemon start --drive C`
/// 23. `ensure_daemon_ready` poll -- `uffs --daemon status` -> Ready
/// 24. second-pass preflight  -- `es -get-everything-version`
/// 25. second-pass preflight  -- `uffs --daemon status`
/// 26. second-pass preflight  -- `es -get-result-count C:`
fn autopilot_host() -> MockHost {
    let evr = "C:\\Program Files (x86)\\Everything\\Everything.exe";
    MockHost::new()
        .with_run_result(stdout_of("running"))                 //  1: teardown daemon status
        .with_run_result(stdout_of(""))                       //  2: initial daemon kill
        .with_run_result(stdout_of(""))                       //  3: initial daemon start (all drives)
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  4: where.exe es.exe
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         //  5: where.exe es.exe
        .with_run_result(stdout_of(evr))                      //  6: where.exe Everything.exe
        .with_run_result(stdout_of("myhost"))                 //  7: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          //  8: cpu
        .with_run_result(stdout_of("8"))                      //  9: logical_cpus
        .with_run_result(stdout_of("8589934592"))             // 10: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             // 11: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 12: uffs_cpp --version
        .with_run_result(stdout_of(""))                       // 13: tasklist (everything stopped)
        .with_run_result(stdout_of("1.1.0.30"))               // 14: es -version
        .with_run_result(stdout_of(""))                       // 15: tasklist (everything_gui stopped)
        .with_run_result(stdout_of("1.4.1.1032"))             // 16: es -get-everything-version
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 17: ensure_daemon_ready (all-drives)
        .with_run_result(stdout_of("1.4.1.1032"))             // 18: preflight es availability
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 19: preflight daemon status
        .with_run_result(stdout_of("1000"))                   // 20: es result-count C
        .with_run_result(stdout_of(""))                       // 21: daemon kill (gated restart)
        .with_run_result(stdout_of(""))                       // 22: daemon start --drive C
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 23: ensure_daemon_ready poll
        .with_run_result(stdout_of("1.4.1.1032"))             // 24: 2nd-pass preflight es avail
        .with_run_result(stdout_of(DAEMON_READY_STATUS))      // 25: 2nd-pass preflight status
        .with_run_result(stdout_of("1000")) // 26: 2nd-pass es result-count C
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
