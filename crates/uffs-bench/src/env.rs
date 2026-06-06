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

/// A tool whose version string should be probed for the fingerprint.
#[derive(Debug, Clone)]
pub struct ToolProbe {
    /// Display name (for example `"uffs"`).
    pub name: String,
    /// Executable to invoke.
    pub exe: String,
    /// Arguments that make the tool print its version.
    pub args: Vec<String>,
}

/// A resolved tool name → version pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVersion {
    /// Display name of the tool.
    pub name: String,
    /// Reported version string (`"unknown"` if the probe produced nothing).
    pub version: String,
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

/// Probe one tool's version, preferring stdout then stderr (`"unknown"` on
/// failure or empty output — many tools print their banner to stderr).
fn probe_tool(host: &dyn Host, tool: &ToolProbe) -> ToolVersion {
    let arg_refs: Vec<&str> = tool.args.iter().map(String::as_str).collect();
    let version = host.run(&tool.exe, &arg_refs).ok().map_or_else(
        || "unknown".to_owned(),
        |out| {
            first_nonempty(&out.stdout)
                .or_else(|| first_nonempty(&out.stderr))
                .unwrap_or_else(|| "unknown".to_owned())
        },
    );
    ToolVersion {
        name: tool.name.clone(),
        version,
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
    let total_ram = probe_value(host, &ram_probe);
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
        tools,
    }
}

/// Render a captured fingerprint as the report's "Test environment" markdown.
///
/// A pure function of its input (no host access), so it is covered by a golden
/// test.
#[must_use]
pub fn render_md(fp: &EnvFingerprint) -> String {
    let elevated = if fp.elevated { "yes" } else { "no" };
    let tools = if fp.tools.is_empty() {
        "_None probed._".to_owned()
    } else {
        fp.tools
            .iter()
            .map(|tool| format!("- **{}:** {}", tool.name, tool.version))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "## Test environment\n\n\
         - **Captured:** {}\n\
         - **OS / arch:** {} / {}\n\
         - **Host:** {}\n\
         - **Elevated:** {elevated}\n\
         - **CPU:** {} ({} logical)\n\
         - **RAM:** {}\n\
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{DateTime, Utc};

    use super::{
        EnvFingerprint, EnvSpec, ToolProbe, ToolVersion, bytes_to_gib, capture, clean_value,
        first_nonempty, probe_tool, render_md, write,
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
    fn probe_tool_falls_back_to_stderr() {
        let host = MockHost::new().with_run_result(ProcOutput {
            code: Some(0_i32),
            stdout: String::new(),
            stderr: "banner 9.9\n".to_owned(),
        });
        let tool = ToolProbe {
            name: "x".to_owned(),
            exe: "x".to_owned(),
            args: Vec::new(),
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
                args: vec!["--version".to_owned()],
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
            version: "uffs 1.2.3".to_owned(),
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
            tools: vec![ToolVersion {
                name: "uffs".to_owned(),
                version: "1.2.3".to_owned(),
            }],
        }
    }

    #[test]
    fn render_md_matches_golden() {
        let expected = "## Test environment\n\n\
- **Captured:** 2023-11-14 22:13:20 UTC\n\
- **OS / arch:** windows / x86_64\n\
- **Host:** box\n\
- **Elevated:** yes\n\
- **CPU:** Test CPU (8 logical)\n\
- **RAM:** 16.0 GiB\n\
\n### Tool versions\n\n\
- **uffs:** 1.2.3\n";
        assert_eq!(render_md(&sample_fp()), expected);
    }

    #[test]
    fn render_md_reports_no_tools() {
        let mut fp = sample_fp();
        fp.tools.clear();
        assert!(render_md(&fp).contains("_None probed._"));
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
}
