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

/// Queue the 8 run results needed for the `--dry-run` code path.
///
/// `--dry-run` skips `teardown::baseline`, so the call order is:
///  1. `resolve::es_exe` — `where.exe es.exe` (returns full path)
///  2. `env::capture` hostname
///  3. `env::capture` cpu
///  4. `env::capture` `logical_cpus`
///  5. `env::capture` `total_ram`
///  6. `env::capture` uffs --version
///  7. `env::capture` `uffs_cpp` --version (needs "UFFS version:" line)
///  8. `env::capture` es -version
fn dry_run_host() -> MockHost {
    MockHost::new()
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         // 1: where.exe es.exe
        .with_run_result(stdout_of("myhost"))                 // 2: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          // 3: cpu
        .with_run_result(stdout_of("8"))                      // 4: logical_cpus
        .with_run_result(stdout_of("8589934592"))             // 5: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             // 6: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 7: uffs_cpp --version
        .with_run_result(stdout_of("1.1.0.30")) // 8: es -version
}

/// Queue the 9 run results needed for the autopilot (non-dry-run) path.
///
/// `teardown::baseline` fires `uffs daemon status` before the env probes:
///  1. `teardown::baseline` — `uffs daemon status`
///  2. `resolve::es_exe` — `where.exe es.exe` (returns full path)
///  3. `env::capture` hostname
///  4. `env::capture` cpu
///  5. `env::capture` `logical_cpus`
///  6. `env::capture` `total_ram`
///  7. `env::capture` uffs --version
///  8. `env::capture` `uffs_cpp` --version (needs "UFFS version:" line)
///  9. `env::capture` es -version
fn autopilot_host() -> MockHost {
    MockHost::new()
        .with_run_result(stdout_of("running"))                 // 1: uffs daemon status
        .with_run_result(stdout_of("C:\\bin\\es.exe"))         // 2: where.exe es.exe
        .with_run_result(stdout_of("myhost"))                 // 3: hostname
        .with_run_result(stdout_of("Name=Test CPU"))          // 4: cpu
        .with_run_result(stdout_of("8"))                      // 5: logical_cpus
        .with_run_result(stdout_of("8589934592"))             // 6: total_ram
        .with_run_result(stdout_of("uffs 0.0.0"))             // 7: uffs --version
        .with_run_result(stdout_of("\tUFFS version:\t1.0.0")) // 8: uffs_cpp --version
        .with_run_result(stdout_of("1.1.0.30")) // 9: es -version
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
