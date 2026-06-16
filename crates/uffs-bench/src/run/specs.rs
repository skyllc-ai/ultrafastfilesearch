// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Spec-builder helpers: construct [`EnvSpec`], [`PreflightSpec`], and
//! [`MatrixSpec`] from the parsed CLI.  Extracted from `run/mod.rs` to keep
//! that file within the workspace file-size policy (≤ 800 LOC).

use std::path::{Path, PathBuf};

use super::{PREFLIGHT_POLL_ATTEMPTS, PREFLIGHT_POLL_INTERVAL_MS, mode_name};
use crate::cli::Cli;
use crate::env::{EnvSpec, StateProbe, ToolProbe};
use crate::host::Host;
use crate::matrix::{self, EVERYTHING_GUI_TOOL, MatrixSpec};
use crate::preflight::PreflightSpec;
use crate::resolve;
use crate::state::{Decisions, input_hash};

/// Build a version + state probe for one tool id.
pub(super) fn tool_probe(host: &dyn Host, name: &str) -> ToolProbe {
    let tasklist_state = || StateProbe {
        exe: "tasklist.exe".to_owned(),
        args: vec![
            "/FI".to_owned(),
            "IMAGENAME eq Everything.exe".to_owned(),
            "/NH".to_owned(),
            "/FO".to_owned(),
            "CSV".to_owned(),
        ],
        running_marker: "Everything.exe".to_owned(),
    };

    if name == matrix::EVERYTHING_TOOL {
        ToolProbe {
            name: name.to_owned(),
            exe: resolve::es_exe(host),
            display_exe: None,
            args: vec!["-version".to_owned()],
            version_line_prefix: None,
            daemon_error_markers: vec![],
            state_probe: Some(tasklist_state()),
        }
    } else if name == EVERYTHING_GUI_TOOL {
        ToolProbe {
            name: name.to_owned(),
            exe: resolve::es_exe(host),
            display_exe: Some(resolve::everything_exe(host)),
            args: vec!["-get-everything-version".to_owned()],
            version_line_prefix: None,
            daemon_error_markers: vec!["Error 8".to_owned(), "IPC window not found".to_owned()],
            state_probe: Some(tasklist_state()),
        }
    } else if name == "uffs_cpp" {
        ToolProbe {
            name: name.to_owned(),
            exe: resolve::uffs_cpp_exe(host),
            display_exe: None,
            args: vec!["--version".to_owned()],
            version_line_prefix: Some("UFFS version:".to_owned()),
            daemon_error_markers: vec![],
            state_probe: None,
        }
    } else if name == "uffs" {
        ToolProbe {
            name: name.to_owned(),
            exe: resolve::uffs_exe(host),
            display_exe: None,
            args: vec!["--version".to_owned()],
            version_line_prefix: Some("uffs ".to_owned()),
            daemon_error_markers: vec![],
            state_probe: None,
        }
    } else {
        ToolProbe {
            name: name.to_owned(),
            exe: name.to_owned(),
            display_exe: None,
            args: vec!["--version".to_owned()],
            version_line_prefix: None,
            daemon_error_markers: vec![],
            state_probe: None,
        }
    }
}

/// Resolve the read-only `Everything.ini` path from the host environment.
///
/// Uses `%APPDATA%\Everything\Everything.ini` when `APPDATA` is set (the
/// Windows install default), falling back to a bare relative name otherwise so
/// the preflight simply observes an absent ini on other hosts.
pub(crate) fn everything_ini_path(host: &dyn Host) -> PathBuf {
    host.env("APPDATA").map_or_else(
        || PathBuf::from("Everything.ini"),
        |appdata| {
            Path::new(&appdata)
                .join("Everything")
                .join("Everything.ini")
        },
    )
}

/// Build the persisted [`Decisions`] record from the parsed CLI.
pub(super) fn decisions_from_cli(cli: &Cli) -> Decisions {
    Decisions {
        mode: mode_name(cli.mode()).to_owned(),
        drives: cli
            .drives_or_default()
            .iter()
            .map(char::to_string)
            .collect(),
        tools: cli.tools_or_default(),
        rounds: cli.rounds,
        drop_cache: cli.drop_os_cache,
    }
}

/// Hash the plan-defining decisions into the Stage 0 resume `input_hash`.
pub(super) fn plan_input_hash(decisions: &Decisions) -> String {
    let drives = decisions.drives.join(",");
    let tools = decisions.tools.join(",");
    let rounds = decisions.rounds.to_string();
    let drop = if decisions.drop_cache { "drop" } else { "keep" };
    input_hash(&[&decisions.mode, &drives, &tools, &rounds, drop])
}

/// Build the Stage 0a [`EnvSpec`] (one version probe per requested tool).
pub(super) fn env_spec_from_cli(host: &dyn Host, cli: &Cli) -> EnvSpec {
    EnvSpec {
        tools: cli
            .tools_or_default()
            .iter()
            .map(|tool| tool_probe(host, tool))
            .collect(),
    }
}

/// Build the Stage 0c [`PreflightSpec`] from the CLI and captured env.
pub(super) fn preflight_spec_from_cli(
    host: &dyn Host,
    cli: &Cli,
    es_ram_budget_bytes: u64,
) -> PreflightSpec {
    PreflightSpec {
        ini_path: everything_ini_path(host),
        candidate_drives: cli.drives_or_default(),
        es_exe: resolve::es_exe(host),
        uffs_exe: resolve::uffs_exe(host),
        patterns: resolve::default_pattern_probes(),
        poll_attempts: PREFLIGHT_POLL_ATTEMPTS,
        poll_interval_ms: PREFLIGHT_POLL_INTERVAL_MS,
        es_ram_budget_bytes,
        es_instance_name: String::new(),
    }
}

/// Build the Stage 0d [`MatrixSpec`] from the CLI.
pub(super) fn matrix_spec_from_cli(cli: &Cli, es_ram_budget_bytes: u64) -> MatrixSpec {
    MatrixSpec {
        required_tools: cli.tools_or_default(),
        candidate_drives: cli.drives_or_default(),
        patterns: resolve::DEFAULT_PATTERNS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect(),
        es_ram_budget_bytes,
    }
}
