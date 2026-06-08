// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

use std::path::PathBuf;

use super::{
    CellFeasibility, DrivePreflight, EsStatus, PatternProbe, PreflightResult, PreflightSpec,
    capture, parse_daemon_status_drives, parse_drives_from_ini, write,
};
use crate::host::{Call, MockHost, ProcOutput};

/// Build a scripted stdout-only process output.
fn stdout_of(stdout: &str) -> ProcOutput {
    ProcOutput {
        code: Some(0_i32),
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

/// A spec over the given candidate drives with one all-DLL pattern.
fn spec_for(drives: &[char], attempts: u32) -> PreflightSpec {
    PreflightSpec {
        ini_path: PathBuf::from("/Everything.ini"),
        candidate_drives: drives.to_vec(),
        es_exe: "es".to_owned(),
        uffs_exe: "uffs".to_owned(),
        patterns: vec![PatternProbe {
            name: "all_dlls".to_owned(),
            args: vec![
                "{DRIVE}:\\".to_owned(),
                "*.dll".to_owned(),
                "--count".to_owned(),
            ],
        }],
        poll_attempts: attempts,
        poll_interval_ms: 500,
        es_ram_budget_bytes: 0,
    }
}

/// Build an IPC-error output (what es.exe returns when Everything is not
/// running or not yet ready).
fn ipc_error_output() -> ProcOutput {
    ProcOutput {
        code: Some(1_i32),
        stdout: "Error 8: Everything IPC window not found. \
                 Please make sure Everything is running."
            .to_owned(),
        stderr: String::new(),
    }
}

#[test]
fn check_es_daemon_not_running_when_process_absent() {
    let host = MockHost::new()
        .with_run_result(ipc_error_output()) // es.exe -get-everything-version
        .with_run_result(stdout_of("")); // tasklist / pgrep: process NOT found
    let status = super::check_es_available(&host, "es.exe");
    assert_eq!(status, Some(EsStatus::DaemonNotRunning));
}

#[test]
fn check_es_daemon_starting_when_process_present() {
    let host = MockHost::new()
        .with_run_result(ipc_error_output()) // es.exe -get-everything-version
        .with_run_result(stdout_of("\"Everything.exe\",\"1234\"")); // tasklist: found
    let status = super::check_es_available(&host, "es.exe");
    assert_eq!(status, Some(EsStatus::DaemonStarting));
}

#[test]
fn parse_drives_reads_quoted_csv() {
    let ini = "foo=1\nntfs_volume_paths=\"C:\\\",\"d:\\\"\nbar=2\n";
    assert_eq!(parse_drives_from_ini(ini), vec!['C', 'D']);
}

#[test]
fn parse_drives_absent_key_is_empty() {
    assert_eq!(parse_drives_from_ini("a=1\nb=2\n"), Vec::<char>::new());
}

/// Fake `uffs daemon status` output with C (3 M records) and D (7 M records).
fn daemon_status_cd() -> &'static str {
    "Version:       0.0.0\n\
     Status:        Ready\n\
     Drives:\n\
       [Warm]   C: \u{2014}  3,000,000 records (live) \u{2014} 600 MB\n\
       [Warm]   D: \u{2014}  7,000,000 records (live) \u{2014} 1400 MB\n"
}

#[test]
fn parse_daemon_status_drives_extracts_counts() {
    let map = parse_daemon_status_drives(daemon_status_cd());
    assert_eq!(map.get(&'C').copied(), Some(3_000_000));
    assert_eq!(map.get(&'D').copied(), Some(7_000_000));
    assert_eq!(map.get(&'E'), None);
}

#[test]
fn capture_is_read_only_and_records_state() {
    // Call order:
    //  1. check_es_available: es -get-everything-version (daemon running)
    //  2. daemon_status_output: uffs daemon status (record counts)
    //  3. C: es result-count poll → loaded
    //  4. D: es result-count poll → not loaded
    //  5. feasibility estimate for (C, all_dlls)
    let host = MockHost::new()
        .with_file("/Everything.ini", b"ntfs_volume_paths=C:\\,D:\\".to_vec())
        .with_run_result(stdout_of("1.4.1.1032"))       // 1: es availability
        .with_run_result(stdout_of(daemon_status_cd())) // 2: uffs daemon status
        .with_run_result(stdout_of("1000"))             // 3: C es result-count
        .with_run_result(stdout_of("0"))                // 4: D es result-count
        .with_run_result(stdout_of("5000")); // 5: uffs estimate C/all_dlls
    let spec = spec_for(&['C', 'D'], 1);

    let result = capture(&host, &spec);

    assert_eq!(result.drives, vec![
        DrivePreflight {
            drive: 'C',
            configured: true,
            loaded: true,
            hot: true,
            record_count: 1000,
            uffs_record_count: 3_000_000,
            es_status: EsStatus::Loaded,
        },
        DrivePreflight {
            drive: 'D',
            configured: true,
            loaded: false,
            hot: false,
            record_count: 0,
            uffs_record_count: 7_000_000,
            es_status: EsStatus::StillIndexing,
        },
    ]);
    assert_eq!(result.cells, vec![CellFeasibility {
        drive: 'C',
        pattern: "all_dlls".to_owned(),
        est_rows: 5000,
        es_feasible: true,
    }]);
    // The whole stage must touch the ini read-only — never write or remove.
    assert!(host.calls().iter().all(|call| !matches!(
        call,
        Call::WriteFile(_) | Call::RemoveFile(_) | Call::Rename(_, _)
    )));
}

#[test]
fn configured_zero_drive_is_polled_with_backoff() {
    let host = MockHost::new()
        .with_file("/Everything.ini", b"ntfs_volume_paths=C:\\".to_vec())
        .with_run_result(stdout_of("1.4.1.1032"))    // 1: es availability
        .with_run_result(stdout_of(                  // 2: uffs daemon status
            "Status: Ready\n  [Warm]   C: \u{2014}  500,000 records (live) \u{2014} 50 MB\n"
        ))
        .with_run_result(stdout_of("0"))             // 3: C es poll 1
        .with_run_result(stdout_of("0"))             // 4: C es poll 2
        .with_run_result(stdout_of("7")); // 5: C es poll 3 → loaded
    let mut spec = spec_for(&['C'], 3);
    spec.patterns.clear();

    let result = capture(&host, &spec);

    assert_eq!(
        result.drives.first().map(|drive| drive.record_count),
        Some(7)
    );
    let sleeps = host
        .calls()
        .into_iter()
        .filter(|call| matches!(call, Call::Sleep(_)))
        .count();
    assert_eq!(sleeps, 2);
}

#[test]
fn unconfigured_drive_probed_once_without_sleep() {
    // E is absent from the first daemon status → warm_parked_drives fires
    // `preload E` then a second `daemon status`.  E stays absent after the
    // second status too (simulate it not warming up), so uffs_record_count=0.
    let host = MockHost::new()
        .with_file("/Everything.ini", b"ntfs_volume_paths=C:\\".to_vec())
        .with_run_result(stdout_of("1.4.1.1032"))    // 1: es availability
        .with_run_result(stdout_of(                  // 2: uffs daemon status (E absent)
            "Status: Ready\n  [Warm]   C: \u{2014}  100,000 records (live) \u{2014} 10 MB\n"
        ))
        .with_run_result(stdout_of("Promoted to Hot")) // 3: preload E
        .with_run_result(stdout_of(                  // 4: uffs daemon status re-check
            "Status: Ready\n  [Warm]   C: \u{2014}  100,000 records (live) \u{2014} 10 MB\n"
        ))
        .with_run_result(stdout_of("0")); // 5: E es result-count → not loaded
    let mut spec = spec_for(&['E'], 5);
    spec.patterns.clear();

    let result = capture(&host, &spec);

    assert_eq!(
        result.drives.first().map(|drive| drive.configured),
        Some(false)
    );
    // E absent from both status reads → uffs_record_count = 0.
    assert_eq!(
        result.drives.first().map(|drive| drive.uffs_record_count),
        Some(0)
    );
    let runs = host
        .calls()
        .into_iter()
        .filter(|call| matches!(call, Call::Run(_, _)))
        .count();
    // 1 es availability + 1 daemon status + 1 preload + 1 daemon status + 1 es
    // probe
    assert_eq!(runs, 5);
    assert!(
        !host
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Sleep(_)))
    );
}

#[test]
fn parked_drive_is_preloaded_and_count_populated() {
    // C is absent from the first status (parked) → preload fires, then
    // second status returns C as Warm with 500,000 records.
    let host = MockHost::new()
        .with_file("/Everything.ini", b"ntfs_volume_paths=C:\\".to_vec())
        .with_run_result(stdout_of("1.4.1.1032"))    // 1: es availability
        .with_run_result(stdout_of("Status: Ready\nDrives:\n")) // 2: status (C absent)
        .with_run_result(stdout_of("Promoted to Hot"))          // 3: preload C
        .with_run_result(stdout_of(                  // 4: status re-check (C now Warm)
            "Status: Ready\n  [Warm]   C: \u{2014}  500,000 records (live) \u{2014} 50 MB\n"
        ))
        .with_run_result(stdout_of("100")); // 5: es result-count → loaded
    let mut spec = spec_for(&['C'], 1);
    spec.patterns.clear();

    let result = capture(&host, &spec);

    assert_eq!(
        result.drives.first().map(|dp| dp.uffs_record_count),
        Some(500_000),
        "uffs_record_count must come from the post-preload status"
    );
    assert_eq!(result.drives.first().map(|dp| dp.loaded), Some(true));
}

#[test]
fn cell_above_ipc_ceiling_is_infeasible() {
    let host = MockHost::new()
        .with_file("/Everything.ini", b"ntfs_volume_paths=C:\\".to_vec())
        .with_run_result(stdout_of("1.4.1.1032"))    // 1: es availability
        .with_run_result(stdout_of(                  // 2: uffs daemon status
            "Status: Ready\n  [Warm]   C: \u{2014}  500,000 records (live) \u{2014} 50 MB\n"
        ))
        .with_run_result(stdout_of("100"))           // 3: C es result-count → loaded
        .with_run_result(stdout_of("200000")); // 4: estimate over the 150k ceiling
    let spec = spec_for(&['C'], 1);

    let result = capture(&host, &spec);

    assert_eq!(
        result.cells.first().map(|cell| cell.es_feasible),
        Some(false)
    );
}

#[test]
fn write_emits_preflight_json_and_round_trips() {
    let host = MockHost::new();
    let result = PreflightResult {
        drives: vec![DrivePreflight {
            drive: 'C',
            configured: true,
            loaded: true,
            hot: true,
            record_count: 42,
            uffs_record_count: 1_000_000,
            es_status: EsStatus::Loaded,
        }],
        cells: Vec::new(),
    };
    let dir = PathBuf::from("/bundle");

    write(&host, &result, &dir).expect("write preflight json");

    let path = dir.join("competitor-preflight.json");
    assert_eq!(host.calls(), vec![Call::WriteFile(path.clone())]);
    let json = host.file(&path).expect("preflight json written");
    let parsed: PreflightResult = serde_json::from_slice(&json).expect("valid json");
    assert_eq!(parsed, result);
}
