// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 0a — environment fingerprint capture and rendering.
//!
//! [`capture`] records the machine and tooling a benchmark run executed on, so
//! the report needs **zero** hand-typed environment data. Everything flows
//! through the [`Host`] seam (process probes + env vars), so the whole stage is
//! deterministic under the `MockHost` on any OS. [`render_md`] is a pure
//! function from a captured [`EnvFingerprint`] to the report's
//! "Test environment" markdown — exercised by a golden test.

use std::env;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{BenchError, Result};
use crate::host::Host;

/// A lightweight probe that determines whether a tool's background
/// process/daemon is currently running.
#[derive(Debug, Clone)]
pub struct StateProbe {
    /// Executable to invoke.
    pub exe: String,
    /// Arguments to pass.
    pub args: Vec<String>,
    /// If this substring is present in combined stdout+stderr the state is
    /// `"running"`, otherwise `"stopped"`.
    pub running_marker: String,
}

/// A tool whose version string should be probed for the fingerprint.
#[derive(Debug, Clone)]
pub struct ToolProbe {
    /// Display name (for example `"uffs"`).
    pub name: String,
    /// Executable to invoke for version/state probes.
    pub exe: String,
    /// Path shown in the report. Defaults to `exe` when `None`. Useful when
    /// the version is queried via a helper binary (e.g. `es.exe` probing the
    /// Everything daemon version) but the report should display the primary
    /// binary path (e.g. `Everything.exe`).
    pub display_exe: Option<String>,
    /// Arguments that make the tool print its version.
    pub args: Vec<String>,
    /// When `Some`, select the first output line *containing* this substring
    /// and trim up to and including the substring (plus surrounding whitespace)
    /// rather than taking the first non-empty line. Useful for tools whose
    /// first output line is a banner URL rather than a version number.
    pub version_line_prefix: Option<String>,
    /// If any of these substrings appear in the combined stdout+stderr output,
    /// the version is reported as `"not running"` instead of the raw error
    /// text. Useful for daemons (e.g. Everything) that exit 0 but print an IPC
    /// error when their background process is absent.
    pub daemon_error_markers: Vec<String>,
    /// Optional probe to determine whether the tool's daemon/process is active.
    /// `None` means the tool has no background process (renders as `"n/a"`).
    pub state_probe: Option<StateProbe>,
}

/// A resolved tool name → version + state triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVersion {
    /// Display name of the tool.
    pub name: String,
    /// Resolved executable path (or bare name when found via PATH).
    pub exe: String,
    /// Reported version string (`"unknown"` if the probe produced nothing).
    pub version: String,
    /// Daemon/process state: `"running"`, `"stopped"`, `"n/a"` (no daemon),
    /// or `"unknown"` if the state probe failed unexpectedly.
    pub state: String,
}

/// Inputs that scope an environment capture.
#[derive(Debug, Clone, Default)]
pub struct EnvSpec {
    /// Tools to version-probe, in display order.
    pub tools: Vec<ToolProbe>,
}

/// The captured environment, serialized to `bundle/env.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvFingerprint {
    /// When the capture ran.
    pub captured_at: DateTime<Utc>,
    /// Target OS (`std::env::consts::OS`).
    pub os: String,
    /// Target architecture (`std::env::consts::ARCH`).
    pub arch: String,
    /// Reported host name (`"unknown"` if undiscoverable).
    pub hostname: String,
    /// Whether the run is elevated (MFT reads require it on Windows).
    pub elevated: bool,
    /// CPU model string (best-effort).
    pub cpu: String,
    /// Logical CPU count (best-effort, as a string).
    pub logical_cpus: String,
    /// Total physical RAM (best-effort, human-readable).
    pub total_ram: String,
    /// Total physical RAM in bytes (`0` when the probe fails).
    pub ram_bytes: u64,
    /// Probed tool versions.
    pub tools: Vec<ToolVersion>,
}

/// A platform system-info probe and how to interpret its output.
struct Probe {
    /// Executable to run.
    exe: &'static str,
    /// Arguments.
    args: &'static [&'static str],
    /// Whether the cleaned value is a byte count to render as GiB.
    ram: bool,
}

/// The three fixed system probes (cpu, logical-cpu-count, ram) for this target.
const fn sys_probes() -> [Probe; 3] {
    #[cfg(windows)]
    {
        [
            Probe {
                exe: "wmic",
                args: &["cpu", "get", "Name", "/value"],
                ram: false,
            },
            Probe {
                exe: "wmic",
                args: &["cpu", "get", "NumberOfLogicalProcessors", "/value"],
                ram: false,
            },
            Probe {
                exe: "wmic",
                args: &["ComputerSystem", "get", "TotalPhysicalMemory", "/value"],
                ram: true,
            },
        ]
    }
    #[cfg(target_os = "macos")]
    {
        [
            Probe {
                exe: "sysctl",
                args: &["-n", "machdep.cpu.brand_string"],
                ram: false,
            },
            Probe {
                exe: "sysctl",
                args: &["-n", "hw.logicalcpu"],
                ram: false,
            },
            Probe {
                exe: "sysctl",
                args: &["-n", "hw.memsize"],
                ram: true,
            },
        ]
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        [
            Probe {
                exe: "sh",
                args: &["-c", "grep -m1 'model name' /proc/cpuinfo | cut -d: -f2"],
                ram: false,
            },
            Probe {
                exe: "nproc",
                args: &[],
                ram: false,
            },
            Probe {
                exe: "sh",
                args: &["-c", "free -b | awk '/Mem:/ {print $2}'"],
                ram: true,
            },
        ]
    }
}

/// The first trimmed, non-empty line of `text`, if any.
fn first_nonempty(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Extract the meaningful value from a probe's stdout.
///
/// Takes the first non-empty line and, for `key=value` (wmic) output, the part
/// after the first `=`; returns `"unknown"` when nothing usable was printed.
fn clean_value(stdout: &str) -> String {
    first_nonempty(stdout).map_or_else(
        || "unknown".to_owned(),
        |text| {
            text.split_once('=')
                .map_or(text.as_str(), |(_, value)| value)
                .trim()
                .to_owned()
        },
    )
}

/// Render a byte count as `"<int>.<tenths> GiB"` using integer math only.
fn bytes_to_gib(bytes: u64) -> String {
    const GIB: u64 = 1_073_741_824;
    let whole = bytes / GIB;
    let tenths = (bytes % GIB) * 10 / GIB;
    format!("{whole}.{tenths} GiB")
}

/// Run one system probe and clean its output (`"unknown"` on failure).
fn probe_value(host: &dyn Host, probe: &Probe) -> String {
    let Ok(out) = host.run(probe.exe, probe.args) else {
        return "unknown".to_owned();
    };
    let value = clean_value(&out.stdout);
    if probe.ram {
        value.parse::<u64>().map_or(value, bytes_to_gib)
    } else {
        value
    }
}

/// Run the RAM probe and return the raw byte count (`0` on failure).
fn probe_ram_bytes(host: &dyn Host, probe: &Probe) -> u64 {
    host.run(probe.exe, probe.args)
        .ok()
        .and_then(|out| clean_value(&out.stdout).parse::<u64>().ok())
        .unwrap_or(0)
}

/// Run a [`StateProbe`] and return `"running"` or `"stopped"`.
fn probe_state(host: &dyn Host, sp: &StateProbe) -> String {
    let arg_refs: Vec<&str> = sp.args.iter().map(String::as_str).collect();
    host.run(&sp.exe, &arg_refs).ok().map_or_else(
        || "stopped".to_owned(),
        |out| {
            let combined = format!("{} {}", out.stdout, out.stderr);
            if combined.contains(sp.running_marker.as_str()) {
                "running".to_owned()
            } else {
                "stopped".to_owned()
            }
        },
    )
}

/// Probe one tool's version, preferring stdout then stderr (`"unknown"` on
/// failure or empty output — many tools print their banner to stderr).
fn probe_tool(host: &dyn Host, tool: &ToolProbe) -> ToolVersion {
    // State is probed first so the daemon-error-marker path can reuse the
    // result without issuing a second tasklist/pgrep call.
    let state = tool
        .state_probe
        .as_ref()
        .map_or_else(|| "n/a".to_owned(), |sp| probe_state(host, sp));

    let arg_refs: Vec<&str> = tool.args.iter().map(String::as_str).collect();
    let version = host.run(&tool.exe, &arg_refs).ok().map_or_else(
        || "unknown".to_owned(),
        |out| {
            let combined = format!("{} {}", out.stdout, out.stderr);
            if tool
                .daemon_error_markers
                .iter()
                .any(|marker| combined.contains(marker.as_str()))
            {
                // The IPC channel reported an error — es.exe cannot talk to
                // the instance.  If the process is actually running (e.g. a
                // private instance launched by the bench) report the version
                // as "ipc unavailable" so the operator knows the binary
                // exists but is not the default instance.  Only fall back to
                // "not running" when the process is genuinely absent.
                return if state == "running" {
                    "ipc unavailable".to_owned()
                } else {
                    "not running".to_owned()
                };
            }
            let text = if out.stdout.is_empty() {
                &out.stderr
            } else {
                &out.stdout
            };
            tool.version_line_prefix.as_ref().map_or_else(
                || first_nonempty(text).unwrap_or_else(|| "unknown".to_owned()),
                |prefix| {
                    text.lines()
                        .find(|line| line.contains(prefix.as_str()))
                        .and_then(|line| line.split_once(prefix.as_str()))
                        .map(|(_, after)| after.trim().to_owned())
                        .filter(|ver| !ver.is_empty())
                        .unwrap_or_else(|| "unknown".to_owned())
                },
            )
        },
    );
    let exe = tool.display_exe.as_deref().unwrap_or(&tool.exe).to_owned();
    ToolVersion {
        name: tool.name.clone(),
        exe,
        version,
        state,
    }
}

/// Probe the host name (`"unknown"` on failure).
fn capture_hostname(host: &dyn Host) -> String {
    host.run("hostname", &[]).ok().map_or_else(
        || "unknown".to_owned(),
        |out| first_nonempty(&out.stdout).unwrap_or_else(|| "unknown".to_owned()),
    )
}

/// Capture an [`EnvFingerprint`] for the given [`EnvSpec`].
///
/// Host probes run in a fixed, documented order so the capture is fully
/// deterministic under the `MockHost`: `hostname`, then the three system probes
/// (CPU, logical-CPU count, RAM), then each tool in `spec.tools` order. The OS
/// and architecture come from compile-time constants and the elevation flag
/// from the host seam, so neither spawns a process.
#[must_use]
pub fn capture(host: &dyn Host, spec: &EnvSpec) -> EnvFingerprint {
    let captured_at = host.now();
    let hostname = capture_hostname(host);
    let [model_probe, cores_probe, ram_probe] = sys_probes();
    let cpu = probe_value(host, &model_probe);
    let logical_cpus = probe_value(host, &cores_probe);
    let ram_bytes = probe_ram_bytes(host, &ram_probe);
    let total_ram = if ram_bytes > 0 {
        bytes_to_gib(ram_bytes)
    } else {
        probe_value(host, &ram_probe)
    };
    let elevated = host.is_elevated();
    let tools = spec
        .tools
        .iter()
        .map(|tool| probe_tool(host, tool))
        .collect();
    EnvFingerprint {
        captured_at,
        os: env::consts::OS.to_owned(),
        arch: env::consts::ARCH.to_owned(),
        hostname,
        elevated,
        cpu,
        logical_cpus,
        total_ram,
        ram_bytes,
        tools,
    }
}

/// Render the tool-versions GFM table for `fp`.
///
/// Missing tools (version = `"unknown"`) appear with `⚠️ not found` in the
/// Version cell and their install URL in the Path cell. Used both in the
/// terminal (printed before the missing-tool gate) and embedded in
/// [`render_md`] for the report file.
#[must_use]
pub(crate) fn render_tool_table(fp: &EnvFingerprint) -> String {
    if fp.tools.is_empty() {
        return "_None probed._".to_owned();
    }
    // Compute column widths for a padded GFM table (3 visible columns).
    // `state` is retained on ToolVersion for downstream logic but is not
    // shown in the report — the table is read by humans who care about
    // which version is installed, not pre-run daemon status.
    let w_name = fp
        .tools
        .iter()
        .map(|tv| tv.name.len())
        .max()
        .unwrap_or(0)
        .max("Tool".len());
    let w_ver = fp
        .tools
        .iter()
        .map(|tv| {
            if tv.version == "unknown" {
                "⚠️ not found".len()
            } else {
                tv.version.len()
            }
        })
        .max()
        .unwrap_or(0)
        .max("Version".len());
    let w_path = fp
        .tools
        .iter()
        .map(|tv| {
            if tv.version == "unknown" {
                tool_install_hint(&tv.name).len()
            } else {
                // backtick-wrapped: exe + 2 chars
                tv.exe.len() + 2
            }
        })
        .max()
        .unwrap_or(0)
        .max("Path".len());
    let sep = format!(
        "|{}|{}|{}|",
        "-".repeat(w_name + 2),
        "-".repeat(w_ver + 2),
        "-".repeat(w_path + 2),
    );
    let header = format!(
        "| {:<w_name$} | {:<w_ver$} | {:<w_path$} |",
        "Tool", "Version", "Path",
    );
    let rows: Vec<String> = fp
        .tools
        .iter()
        .map(|tv| {
            let (ver_cell, path_cell) = if tv.version == "unknown" {
                (
                    "\u{26a0}\u{fe0f} not found".to_owned(),
                    tool_install_hint(&tv.name).to_owned(),
                )
            } else {
                (tv.version.clone(), format!("`{}`", tv.exe))
            };
            format!(
                "| {:<w_name$} | {:<w_ver$} | {:<w_path$} |",
                tv.name, ver_cell, path_cell,
            )
        })
        .collect();
    format!("{header}\n{sep}\n{}", rows.join("\n"))
}

/// Backfill the `everything_gui` tool version once the private Everything
/// instance is up.
///
/// The Stage 0a env capture runs *before* the bench launches its private
/// `-instance` Everything, so `es.exe -get-everything-version` could only
/// report `"ipc unavailable"`. Once the instance is loaded, this re-probes it
/// over IPC and overwrites the `everything_gui` row with the real running
/// version — the actual indexer the bench measures against.
///
/// Returns the discovered version on success (so the caller can announce it),
/// or `None` if the row is absent or the probe fails / returns a non-version
/// string.
#[must_use]
pub fn backfill_everything_gui_version(
    host: &dyn Host,
    fp: &mut EnvFingerprint,
    es_exe: &str,
    instance: &str,
) -> Option<String> {
    let tool = fp.tools.iter_mut().find(|tv| tv.name == "everything_gui")?;
    let out = host
        .run(es_exe, &["-instance", instance, "-get-everything-version"])
        .ok()?;
    let version = out.stdout.lines().next().unwrap_or_default().trim();
    // Accept only a real version string (starts with a digit) — never an IPC
    // error banner like "Error 8: Everything IPC window not found".
    version
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
        .then(|| {
            version.clone_into(&mut tool.version);
            version.to_owned()
        })
}

/// Render a captured fingerprint as the report's "Test environment" markdown.
///
/// A pure function of its input (no host access), so it is covered by a golden
/// test.
#[must_use]
pub fn render_md(fp: &EnvFingerprint) -> String {
    let elevated = if fp.elevated { "yes" } else { "no" };
    let tools = render_tool_table(fp);
    format!(
        "## Test environment\n\n\
         | Field | Value |\n\
         |-------|-------|\n\
         | Captured | {} |\n\
         | OS / arch | {} / {} |\n\
         | Host | {} |\n\
         | Elevated | {elevated} |\n\
         | CPU | {} ({} logical) |\n\
         | RAM | {} |\n\
         \n### Tool versions\n\n\
         {tools}\n",
        fp.captured_at.format("%Y-%m-%d %H:%M:%S UTC"),
        fp.os,
        fp.arch,
        fp.hostname,
        fp.cpu,
        fp.logical_cpus,
        fp.total_ram,
    )
}

/// Serialize `fp` to `bundle_dir/env.json` and render `bundle_dir/env.md`.
///
/// # Errors
/// Returns an error if serialization fails or either file cannot be written.
pub fn write(host: &dyn Host, fp: &EnvFingerprint, bundle_dir: &Path) -> Result<()> {
    let json = serde_json::to_vec_pretty(fp)?;
    let json_path = bundle_dir.join("env.json");
    host.write_file(&json_path, &json)
        .map_err(|err| BenchError::io(&json_path, err))?;
    let md_path = bundle_dir.join("env.md");
    host.write_file(&md_path, render_md(fp).as_bytes())
        .map_err(|err| BenchError::io(&md_path, err))?;
    Ok(())
}

/// Return a human-readable install hint for a tool id, or a generic message
/// if the tool is unknown.
///
/// Used by the missing-tool soft gate to tell the operator where to get the
/// binary before they decide whether to proceed with the remaining tools.
pub(crate) fn tool_install_hint(name: &str) -> &'static str {
    match name {
        "uffs" => "https://github.com/skyllc-ai/UltraFastFileSearch/releases",
        "uffs_cpp" => {
            "https://github.com/githubrobbi/Ultra-Fast-File-Search-legacy-cpp/releases/download/v1.0.0/uffs.com"
        }
        "everything" => "https://www.voidtools.com/downloads/#cli",
        "everything_gui" => "https://www.voidtools.com/support/everything/installing_everything/",
        _ => "Ensure the binary is on PATH or in ~/bin and re-run",
    }
}

#[cfg(test)]
mod tests;
