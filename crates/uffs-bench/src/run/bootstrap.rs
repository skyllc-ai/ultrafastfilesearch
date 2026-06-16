// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Bundle directory resolution and state bootstrapping helpers for the
//! bench orchestrator's `run()` entry point.

use std::path::{Path, PathBuf};

use super::{SUITE_VERSION, decisions_from_cli};
use crate::bundle::{bundle_path, new_bundle};
use crate::cli::Cli;
use crate::competitors;
use crate::error::Result;
use crate::gate::Mode;
use crate::host::Host;
use crate::state::{Decisions, State};
use crate::tooling::Disposition;

/// Resolve the output bundle directory.
///
/// Uses `--bundle <dir>` verbatim when provided; otherwise creates a new
/// timestamped bundle under `--bundle-root` (or resumes the most-recent one
/// in dry-run mode so no empty directories are created).
pub(super) fn resolve_bundle_dir(host: &dyn Host, cli: &Cli, dry_run: bool) -> Result<PathBuf> {
    if let Some(dir) = &cli.bundle {
        return Ok(dir.clone());
    }
    if dry_run {
        Ok(bundle_path(host, &cli.bundle_root, SUITE_VERSION))
    } else {
        new_bundle(host, &cli.bundle_root, SUITE_VERSION)
    }
}

/// Disposition for tools the suite acquires (the `--keep-tools` toggle).
pub(super) const fn tool_disposition(cli: &Cli) -> Disposition {
    if cli.keep_tools {
        Disposition::Keep
    } else {
        Disposition::Remove
    }
}

/// Handle the `fetch-competitors` subcommand.
///
/// Resolves (or resumes) a bundle, fetches + SHA-256-verifies the pinned
/// competitor from `competitors.toml` into `<bundle>/tools/`, and records the
/// verified [`Acquisition`](crate::tooling::Acquisition) in `state.json`. A
/// dry-run acquires nothing.
///
/// # Errors
/// Returns an error if bundle/state I/O fails or provisioning fails (a
/// malformed manifest, a failed download, or a SHA-256 mismatch — all fail
/// closed).
pub(super) fn run_fetch_competitors(host: &dyn Host, cli: &Cli) -> Result<()> {
    if cli.mode() == Mode::DryRun {
        host.out("dry-run: competitor fetch acquires nothing");
        return Ok(());
    }
    let decisions = decisions_from_cli(cli);
    let bundle_dir = resolve_bundle_dir(host, cli, false)?;
    let state_path = bundle_dir.join("state.json");
    let mut state = load_or_new_state(host, cli, &state_path, &decisions)?;

    let manifest = competitors::load_manifest(host, Path::new(competitors::MANIFEST_PATH))?;
    let acquisition = competitors::fetch(host, &manifest, &bundle_dir, tool_disposition(cli))?;
    host.out(&format!(
        "fetched {} (Everything v{}) -> {} [sha256 verified, {:?}]",
        acquisition.name,
        manifest.everything.version,
        acquisition.path.display(),
        acquisition.disposition
    ));
    state.acquisitions.push(acquisition);
    state.save(host, &state_path)?;
    Ok(())
}

/// Load `state.json` when resuming an existing bundle, else start fresh.
pub(super) fn load_or_new_state(
    host: &dyn Host,
    cli: &Cli,
    state_path: &Path,
    decisions: &Decisions,
) -> Result<State> {
    if cli.bundle.is_some() && host.path_exists(state_path) {
        State::load(host, state_path)
    } else {
        Ok(State::new(host, SUITE_VERSION, decisions.clone()))
    }
}
