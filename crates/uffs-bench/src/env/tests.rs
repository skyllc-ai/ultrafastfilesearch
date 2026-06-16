// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::{
    EnvFingerprint, EnvSpec, StateProbe, ToolProbe, ToolVersion, backfill_everything_gui_version,
    bytes_to_gib, capture, clean_value, first_nonempty, probe_tool, render_md, write,
};
use crate::host::{Call, MockHost, ProcOutput};

/// A fixed, deterministic capture instant (2023-11-14 22:13:20 UTC).
fn fixed_time() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
}

/// Build a scripted process output with the given stdout (empty stderr).
fn stdout_of(stdout: &str) -> ProcOutput {
    ProcOutput {
        code: Some(0_i32),
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

#[test]
fn bytes_to_gib_uses_integer_math() {
    assert_eq!(bytes_to_gib(17_179_869_184), "16.0 GiB");
    assert_eq!(bytes_to_gib(1_610_612_736), "1.5 GiB");
    assert_eq!(bytes_to_gib(0), "0.0 GiB");
}

#[test]
fn clean_value_handles_key_value_plain_and_empty() {
    assert_eq!(clean_value("\n\nName=My CPU\n"), "My CPU");
    assert_eq!(clean_value("plain value\n"), "plain value");
    assert_eq!(clean_value("   \n  \n"), "unknown");
}

#[test]
fn first_nonempty_skips_blank_leading_lines() {
    assert_eq!(first_nonempty("\n  \nhi\nthere"), Some("hi".to_owned()));
    assert_eq!(first_nonempty("   \n\t\n"), None);
}

#[test]
fn probe_tool_version_line_prefix_extracts_correct_line() {
    let banner = "Ultra Fast File Search   https://example.com\n\
                  \n\
                  based on SwiftSearch\n\
                  \n\
                  \tUFFS version:\t1.0.0\n\
                  \tBuild for:\tx86_64\n";
    let host = MockHost::new().with_run_result(ProcOutput {
        code: Some(0_i32),
        stdout: banner.to_owned(),
        stderr: String::new(),
    });
    let tool = ToolProbe {
        name: "uffs_cpp".to_owned(),
        exe: "uffs.com".to_owned(),
        display_exe: None,
        args: vec!["--version".to_owned()],
        version_line_prefix: Some("UFFS version:".to_owned()),
        daemon_error_markers: vec![],
        state_probe: None,
    };
    assert_eq!(probe_tool(&host, &tool).version, "1.0.0");
}

#[test]
fn probe_tool_daemon_error_markers_returns_not_running() {
    let ipc_error =
        "Error 8: Everything IPC window not found. Please make sure Everything is running.\n";
    let host = MockHost::new().with_run_result(ProcOutput {
        code: Some(0_i32),
        stdout: ipc_error.to_owned(),
        stderr: String::new(),
    });
    let tool = ToolProbe {
        name: "everything".to_owned(),
        exe: "es.exe".to_owned(),
        display_exe: None,
        args: vec!["-get-everything-version".to_owned()],
        version_line_prefix: None,
        daemon_error_markers: vec!["Error 8".to_owned(), "IPC window not found".to_owned()],
        state_probe: None,
    };
    assert_eq!(probe_tool(&host, &tool).version, "not running");
}

#[test]
fn probe_tool_falls_back_to_stderr() {
    let host = MockHost::new().with_run_result(ProcOutput {
        code: Some(0_i32),
        stdout: String::new(),
        stderr: "banner 9.9\n".to_owned(),
    });
    let tool = ToolProbe {
        name: "x".to_owned(),
        exe: "x".to_owned(),
        display_exe: None,
        args: Vec::new(),
        version_line_prefix: None,
        daemon_error_markers: vec![],
        state_probe: None,
    };
    assert_eq!(probe_tool(&host, &tool).version, "banner 9.9");
}

#[test]
fn capture_reads_probes_in_documented_order() {
    let host = MockHost::new()
        .with_now(fixed_time())
        .with_elevated(true)
        .with_run_result(stdout_of("myhost"))
        .with_run_result(stdout_of("Name=Test CPU"))
        .with_run_result(stdout_of("8"))
        .with_run_result(stdout_of("17179869184"))
        .with_run_result(stdout_of("uffs 1.2.3"));
    let spec = EnvSpec {
        tools: vec![ToolProbe {
            name: "uffs".to_owned(),
            exe: "uffs".to_owned(),
            display_exe: None,
            args: vec!["--version".to_owned()],
            version_line_prefix: None,
            daemon_error_markers: vec![],
            state_probe: None,
        }],
    };

    let fp = capture(&host, &spec);

    assert_eq!(fp.captured_at, fixed_time());
    assert_eq!(fp.hostname, "myhost");
    assert_eq!(fp.cpu, "Test CPU");
    assert_eq!(fp.logical_cpus, "8");
    assert_eq!(fp.total_ram, "16.0 GiB");
    assert!(fp.elevated);
    assert_eq!(fp.os, std::env::consts::OS);
    assert_eq!(fp.arch, std::env::consts::ARCH);
    assert_eq!(fp.tools, vec![ToolVersion {
        name: "uffs".to_owned(),
        exe: "uffs".to_owned(),
        version: "uffs 1.2.3".to_owned(),
        state: "n/a".to_owned(),
    }]);
}

/// Build a fully-populated fingerprint for render/write tests.
fn sample_fp() -> EnvFingerprint {
    EnvFingerprint {
        captured_at: fixed_time(),
        os: "windows".to_owned(),
        arch: "x86_64".to_owned(),
        hostname: "box".to_owned(),
        elevated: true,
        cpu: "Test CPU".to_owned(),
        logical_cpus: "8".to_owned(),
        total_ram: "16.0 GiB".to_owned(),
        ram_bytes: 17_179_869_184,
        tools: vec![ToolVersion {
            name: "uffs".to_owned(),
            exe: "uffs.exe".to_owned(),
            version: "1.2.3".to_owned(),
            state: "running".to_owned(),
        }],
    }
}

#[test]
fn render_md_matches_golden() {
    let expected = "## Test environment\n\n\
| Field | Value |\n\
|-------|-------|\n\
| Captured | 2023-11-14 22:13:20 UTC |\n\
| OS / arch | windows / x86_64 |\n\
| Host | box |\n\
| Elevated | yes |\n\
| CPU | Test CPU (8 logical) |\n\
| RAM | 16.0 GiB |\n\
\n### Tool versions\n\n\
| Tool | Version | Path       |\n\
|------|---------|------------|\n\
| uffs | 1.2.3   | `uffs.exe` |\n";
    assert_eq!(render_md(&sample_fp()), expected);
}

#[test]
fn render_md_reports_no_tools() {
    let mut fp = sample_fp();
    fp.tools.clear();
    assert!(render_md(&fp).contains("_None probed._"));
}

/// A fingerprint whose `everything_gui` row carries the pre-launch
/// `"ipc unavailable"` placeholder.
fn fp_with_gui_placeholder() -> EnvFingerprint {
    let mut fp = sample_fp();
    fp.tools.push(ToolVersion {
        name: "everything_gui".to_owned(),
        exe: "Everything.exe".to_owned(),
        version: "ipc unavailable".to_owned(),
        state: "running".to_owned(),
    });
    fp
}

fn gui_version(fp: &EnvFingerprint) -> &str {
    fp.tools
        .iter()
        .find(|tv| tv.name == "everything_gui")
        .map(|tv| tv.version.as_str())
        .expect("everything_gui row")
}

#[test]
fn backfill_overwrites_gui_version_when_instance_up() {
    let mut fp = fp_with_gui_placeholder();
    let host = MockHost::new().with_run_result(stdout_of("1.4.1.1024\n"));
    let returned = backfill_everything_gui_version(&host, &mut fp, "es.exe", "uffs-bench");
    assert_eq!(returned.as_deref(), Some("1.4.1.1024"));
    assert_eq!(gui_version(&fp), "1.4.1.1024");
}

#[test]
fn backfill_keeps_placeholder_on_ipc_error() {
    let mut fp = fp_with_gui_placeholder();
    let host = MockHost::new().with_run_result(ipc_error_output());
    let returned = backfill_everything_gui_version(&host, &mut fp, "es.exe", "uffs-bench");
    assert_eq!(returned, None);
    assert_eq!(gui_version(&fp), "ipc unavailable");
}

fn ipc_error_output() -> ProcOutput {
    ProcOutput {
        code: Some(1),
        stdout: "Error 8: Everything IPC window not found.".to_owned(),
        stderr: String::new(),
    }
}

fn tasklist_found() -> ProcOutput {
    stdout_of("\"Everything.exe\",\"1234\"")
}

fn tasklist_empty() -> ProcOutput {
    stdout_of("")
}

fn gui_tool_probe() -> ToolProbe {
    ToolProbe {
        name: "everything_gui".to_owned(),
        exe: "es.exe".to_owned(),
        display_exe: Some("Everything.exe".to_owned()),
        args: vec!["-get-everything-version".to_owned()],
        version_line_prefix: None,
        daemon_error_markers: vec!["Error 8".to_owned()],
        state_probe: Some(StateProbe {
            exe: "tasklist.exe".to_owned(),
            args: vec!["/FI".to_owned(), "IMAGENAME eq Everything.exe".to_owned()],
            running_marker: "Everything.exe".to_owned(),
        }),
    }
}

#[test]
fn ipc_error_with_process_running_reports_ipc_unavailable() {
    let host = MockHost::new()
        .with_run_result(tasklist_found())  // state probe: process running
        .with_run_result(ipc_error_output()); // version probe: IPC error
    let tv = probe_tool(&host, &gui_tool_probe());
    assert_eq!(tv.state, "running");
    assert_eq!(
        tv.version, "ipc unavailable",
        "process is up but es.exe cannot see the private instance"
    );
}

#[test]
fn ipc_error_with_process_absent_reports_not_running() {
    let host = MockHost::new()
        .with_run_result(tasklist_empty())  // state probe: no process
        .with_run_result(ipc_error_output()); // version probe: IPC error
    let tv = probe_tool(&host, &gui_tool_probe());
    assert_eq!(tv.state, "stopped");
    assert_eq!(tv.version, "not running", "process is genuinely absent");
}

#[test]
fn write_emits_json_then_md_and_round_trips() {
    let host = MockHost::new();
    let fp = sample_fp();
    let dir = PathBuf::from("/bundle");

    write(&host, &fp, &dir).expect("write env artifacts");

    assert_eq!(host.calls(), vec![
        Call::WriteFile(dir.join("env.json")),
        Call::WriteFile(dir.join("env.md")),
    ]);
    let json = host.file(&dir.join("env.json")).expect("env.json written");
    let parsed: EnvFingerprint = serde_json::from_slice(&json).expect("valid json");
    assert_eq!(parsed, fp);
    let md = host.file(&dir.join("env.md")).expect("env.md written");
    assert_eq!(md, render_md(&fp).into_bytes());
}
