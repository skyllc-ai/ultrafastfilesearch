// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 0e plan gate + the staged orchestrator skeleton.
//!
//! [`run`] is the binary's single entry point (`main.rs` is a thin shim). It
//! resolves the bundle directory and `state.json`, captures the Stage 0a
//! environment fingerprint, runs the Stage 0c competitor preflight, negotiates
//! the Stage 0d matrix, then presents the Stage 0e plan gate through the
//! mode-aware [`confirm`] gate. Stages 1–3 dispatch to the live measurement
//! wrappers in [`crate::stages`]; resume and `--only-stage`/`--from-stage`
//! selection are honored throughout. The "no crumb left behind" [`RunGuard`]
//! teardown is created at the top level and threaded into the measurement
//! stages, which register their snapshot restores on it *before* mutating, then
//! drained once the staged loop returns.
//!
//! Every side effect flows through the [`Host`] seam, so the whole orchestrator
//! is unit-testable under the `MockHost` on any OS.

use alloc::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::bundle::{bundle_path, new_bundle};
use crate::cards::{
    assembly_card, dry_run_result, measurement_card, missing_tools_card, plan_card, report_scope,
    stage0_result,
};
use crate::cli::{Cli, Command};
use crate::env::{self, EnvFingerprint, EnvSpec, StateProbe, ToolProbe};
use crate::error::{BenchError, CrumbError, Result};
use crate::gate::{Decision, Mode, StepResult, confirm, done_panel};
use crate::host::Host;
use crate::matrix::{self, EVERYTHING_GUI_TOOL, Matrix, MatrixSpec};
use crate::preflight::{self, PreflightResult, PreflightSpec};
use crate::restore::RunGuard;
use crate::stages::{self, StageCfg};
use crate::state::{Decisions, State, Status, input_hash};
use crate::tooling::Disposition;
use crate::{competitors, report, resolve, teardown};

/// Suite version stamped into bundle names and `state.json`.
const SUITE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable id of the Stage 0 plan step in the resume engine.
pub(crate) const STAGE0_ID: &str = "stage0/plan";

/// Default readiness-poll attempts for a configured-but-cold competitor drive.
const PREFLIGHT_POLL_ATTEMPTS: u32 = 3;

/// Delay between competitor readiness-poll attempts, in milliseconds.
const PREFLIGHT_POLL_INTERVAL_MS: u64 = 1_000;

/// Number of measurement stages following Stage 0 (cross-tool, parity, full).
const MEASUREMENT_STAGES: u32 = 3;

/// Stage number of the read-only Stage 4 bundle assembly.
const ASSEMBLY_STAGE: u32 = 4;

/// Stable id of the Stage 4 assembly step in the resume engine.
pub(crate) const ASSEMBLY_ID: &str = "stage4/report";

/// Whether the staged run should continue or stop early (operator abort/back).
enum Flow {
    /// Keep running subsequent stages.
    Continue,
    /// Stop the run now (no error; the operator chose to stop).
    Stop,
}

/// The fixed name string for a launch [`Mode`] (feeds `state.json` + hashes).
const fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Guided => "guided",
        Mode::Interactive => "interactive",
        Mode::AutoPilot => "auto",
        Mode::DryRun => "dry-run",
    }
}

/// Build a version + state probe for one tool id.
///
/// - `everything`     → `es.exe -version` (ES CLI version, no IPC), state via
///   `tasklist`
/// - `everything_gui` → `es.exe -get-everything-version` (daemon IPC version,
///   `daemon_error_markers` turn IPC errors into `"not running"`), display exe
///   = `Everything.exe`, state via `tasklist`
/// - `uffs`           → `uffs --version`, state via `uffs daemon status`
/// - `uffs_cpp`       → `uffs.com --version` (resolved via `~/bin` cascade), no
///   daemon state
/// - anything else    → `<exe> --version`, no daemon state
fn tool_probe(host: &dyn Host, name: &str) -> ToolProbe {
    let tasklist_state = || StateProbe {
        exe: "tasklist.exe".to_owned(),
        args: vec![
            "/FI".to_owned(),
            "IMAGENAME eq Everything.exe".to_owned(),
            "/NH".to_owned(),
            "/FO".to_owned(),
            "CSV".to_owned(),
        ],
        // tasklist CSV output contains the image name when the process is
        // present; if the filter matches nothing it prints a "no tasks are
        // running" message that does NOT contain this string.
        running_marker: "Everything.exe".to_owned(),
    };

    if name == matrix::EVERYTHING_TOOL {
        ToolProbe {
            name: name.to_owned(),
            exe: resolve::es_exe(host),
            display_exe: None,
            // Use `-version` (ES CLI version, no IPC/daemon required) rather
            // than `-get-everything-version` which exits 0 but prints an IPC
            // error when the Everything daemon is not running.
            args: vec!["-version".to_owned()],
            version_line_prefix: None,
            daemon_error_markers: vec![],
            state_probe: Some(tasklist_state()),
        }
    } else if name == EVERYTHING_GUI_TOOL {
        ToolProbe {
            name: name.to_owned(),
            // Version is queried via es.exe IPC; display path is Everything.exe.
            exe: resolve::es_exe(host),
            display_exe: Some(resolve::everything_exe(host)),
            // `-get-everything-version` queries the running daemon via IPC and
            // returns the GUI version. When the daemon is not running it exits
            // 0 but prints an IPC error — daemon_error_markers turns this into
            // "not running" in the version field.
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
            // `uffs --version` prints e.g. `uffs 0.5.117`; strip the prefix.
            args: vec!["--version".to_owned()],
            version_line_prefix: Some("uffs ".to_owned()),
            daemon_error_markers: vec![],
            // The daemon (uttfd) is started/restarted by the bench itself, so
            // its pre-run state is irrelevant — report n/a.
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

/// Normalized action a gate [`Decision`] maps to for one stage step.
enum Act {
    /// Run the step's effect (operator pressed proceed, or autopilot).
    Run,
    /// Render only, mutate nothing (dry-run).
    Noop,
    /// Skip this step.
    Skip,
    /// Stop the run (back / abort / quit).
    Stop,
}

/// Map a gate [`Decision`] to the [`Act`] the orchestrator performs.
const fn act_of(decision: Decision) -> Act {
    match decision {
        Decision::Proceed | Decision::Autopilot => Act::Run,
        Decision::ProceedNoop => Act::Noop,
        Decision::Skip => Act::Skip,
        Decision::Back | Decision::Abort => Act::Stop,
    }
}

/// Build the persisted [`Decisions`] record from the parsed CLI.
fn decisions_from_cli(cli: &Cli) -> Decisions {
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
fn plan_input_hash(decisions: &Decisions) -> String {
    let drives = decisions.drives.join(",");
    let tools = decisions.tools.join(",");
    let rounds = decisions.rounds.to_string();
    let drop = if decisions.drop_cache { "drop" } else { "keep" };
    input_hash(&[&decisions.mode, &drives, &tools, &rounds, drop])
}

/// Build the Stage 0a [`EnvSpec`] (one version probe per requested tool).
fn env_spec_from_cli(host: &dyn Host, cli: &Cli) -> EnvSpec {
    EnvSpec {
        tools: cli
            .tools_or_default()
            .iter()
            .map(|tool| tool_probe(host, tool))
            .collect(),
    }
}

/// Build the Stage 0c [`PreflightSpec`] from the CLI and host environment.
fn preflight_spec_from_cli(host: &dyn Host, cli: &Cli) -> PreflightSpec {
    PreflightSpec {
        ini_path: everything_ini_path(host),
        candidate_drives: cli.drives_or_default(),
        es_exe: resolve::es_exe(host),
        uffs_exe: resolve::uffs_exe(host),
        patterns: resolve::default_pattern_probes(),
        poll_attempts: PREFLIGHT_POLL_ATTEMPTS,
        poll_interval_ms: PREFLIGHT_POLL_INTERVAL_MS,
    }
}

/// Build the Stage 0d [`MatrixSpec`] from the CLI.
fn matrix_spec_from_cli(cli: &Cli) -> MatrixSpec {
    MatrixSpec {
        required_tools: cli.tools_or_default(),
        candidate_drives: cli.drives_or_default(),
        patterns: resolve::DEFAULT_PATTERNS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect(),
    }
}

/// Whether `stage` is selected by the `--only-stage` / `--from-stage` filters.
const fn stage_selected(cli: &Cli, stage: u32) -> bool {
    match (cli.only_stage, cli.from_stage) {
        (Some(only), _) => stage == only,
        (None, Some(from)) => stage >= from,
        (None, None) => true,
    }
}

/// Resume-engine step id for a measurement stage.
pub(crate) fn stage_step_id(stage: u32) -> String {
    format!("stage{stage}/measure")
}

/// Operator-facing banner for a measurement stage.
pub(crate) fn stage_banner(stage: u32) -> String {
    let label = match stage {
        1 => "CROSS-TOOL",
        2 => "PARITY",
        _ => "FULL SUITE",
    };
    format!("STAGE {stage}: {label}")
}

/// Artifact paths Stage 0 writes into the bundle (for the state record).
fn stage0_outputs(bundle_dir: &Path) -> Vec<String> {
    [
        "env.json",
        "env.md",
        "competitor-preflight.json",
        "matrix.json",
    ]
    .iter()
    .map(|name| bundle_dir.join(name).display().to_string())
    .collect()
}

/// Report any restore failures collected at teardown ("crumbs left behind").
fn report_crumbs(host: &dyn Host, crumbs: &[CrumbError]) {
    if crumbs.is_empty() {
        return;
    }
    host.out("WARNING: some restores failed (crumbs left behind):");
    for crumb in crumbs {
        host.out(&format!("  - {crumb}"));
    }
}

/// Read-only Stage 0 capture, shared by the plan gate and the measurement
/// stages.
///
/// Computed at most once per run (only when Stage 0 or a measurement stage will
/// actually execute), so a fully-cached resume performs no probes.
struct Capture {
    /// Stage 0a environment fingerprint.
    fp: EnvFingerprint,
    /// Stage 0c competitor preflight.
    preflight: PreflightResult,
    /// Stage 0d negotiated matrix (its `capable_drives` feed Stage 1).
    matrix: Matrix,
}

/// Mutable per-run gate state threaded through every staged confirm.
///
/// Bundles the (mutable) confirmation [`Mode`] with the set of card ids already
/// taught this run, so a single `&mut Session` flows through the staged loop
/// instead of two parallel out-parameters.
struct Session {
    /// Confirmation mode (an interactive `a` keypress upgrades it in place).
    mode: Mode,
    /// Card ids already shown in full this run (guided-mode teach-once).
    seen: BTreeSet<String>,
}

/// Coordinates Stage 0 capture and the staged measurement loop over a [`Host`].
struct Orchestrator<'a> {
    /// Host seam every side effect flows through.
    host: &'a dyn Host,
    /// Parsed command-line configuration for this run.
    cli: &'a Cli,
    /// Resolved bundle directory artifacts are written into.
    bundle_dir: PathBuf,
}

impl Orchestrator<'_> {
    /// Persist the Stage 0 artifacts (env, preflight, matrix) into the bundle.
    fn write_stage0(
        &self,
        fp: &EnvFingerprint,
        preflight: &PreflightResult,
        matrix: &Matrix,
    ) -> Result<()> {
        env::write(self.host, fp, &self.bundle_dir)?;
        preflight::write(self.host, preflight, &self.bundle_dir)?;
        matrix::write(self.host, matrix, &self.bundle_dir)?;
        Ok(())
    }

    /// Run the read-only Stage 0 probes (env + preflight + matrix).
    ///
    /// Computed at most once per run and shared by the plan gate and the
    /// measurement stages (whose [`StageCfg`] draws its cross-tool-capable
    /// drive subset from the negotiated matrix), so no probe runs twice.
    ///
    /// When one or more tools cannot be found (version = `"unknown"`), the
    /// operator is shown an install hint per missing tool and asked whether to
    /// proceed with the remaining tools or quit. If fewer than two tools are
    /// available after the gate, the run is aborted.
    ///
    /// # Errors
    /// Returns [`BenchError::MissingTools`] if fewer than 2 tools are available
    /// after the operator's decision, or if the operator chooses to abort.
    fn capture(&self, session: &mut Session) -> Result<Capture> {
        let fp = env::capture(self.host, &env_spec_from_cli(self.host, self.cli));
        self.host.out(&env::render_md(&fp));
        let missing: Vec<&str> = fp
            .tools
            .iter()
            .filter(|tv| tv.version == "unknown")
            .map(|tv| tv.name.as_str())
            .collect();
        if !missing.is_empty() {
            let available = fp.tools.len() - missing.len();
            if available < 2 {
                return Err(BenchError::MissingTools(format!(
                    "only {available} tool(s) available — need at least 2 to run a meaningful \
                     benchmark. Install the missing tools and re-run."
                )));
            }
            let card = missing_tools_card(&missing);
            if matches!(
                confirm(self.host, &mut session.mode, &mut session.seen, &card),
                Decision::Back | Decision::Abort
            ) {
                return Err(BenchError::MissingTools(
                    "operator chose to abort — install missing tools and re-run".to_owned(),
                ));
            }
        }
        let preflight =
            preflight::capture(self.host, &preflight_spec_from_cli(self.host, self.cli));
        let matrix = matrix::compute_matrix(&matrix_spec_from_cli(self.cli), &preflight);
        Ok(Capture {
            fp,
            preflight,
            matrix,
        })
    }

    /// Build the [`StageCfg`] shared by every measurement stage from the CLI
    /// and the negotiated matrix.
    fn stage_cfg(&self, cap: &Capture) -> StageCfg {
        StageCfg {
            bundle_dir: self.bundle_dir.clone(),
            capable_drives: cap.matrix.capable_drives.clone(),
            drives: self.cli.drives_or_default(),
            tools: self.cli.tools_or_default(),
            rounds: self.cli.rounds,
            drop_cache: self.cli.drop_os_cache,
            patterns: resolve::default_pattern_probes(),
            uffs_exe: resolve::uffs_exe(self.host),
        }
    }

    /// Render the captured Stage 0 plan and gate it.
    fn run_stage0(
        &self,
        state: &mut State,
        session: &mut Session,
        cap: &Capture,
        hash: &str,
    ) -> Result<Flow> {
        self.host.out(&matrix::render_md(&cap.matrix));

        let card = plan_card(&self.bundle_dir);
        match act_of(confirm(
            self.host,
            &mut session.mode,
            &mut session.seen,
            &card,
        )) {
            Act::Run => {
                self.write_stage0(&cap.fp, &cap.preflight, &cap.matrix)?;
                state.set_step(
                    self.host,
                    STAGE0_ID,
                    Status::Done,
                    hash,
                    stage0_outputs(&self.bundle_dir),
                );
                done_panel(self.host, &card, &stage0_result(&self.bundle_dir));
                Ok(Flow::Continue)
            }
            Act::Noop => {
                done_panel(self.host, &card, &dry_run_result());
                Ok(Flow::Continue)
            }
            Act::Skip => {
                state.set_step(self.host, STAGE0_ID, Status::Skipped, hash, Vec::new());
                Ok(Flow::Continue)
            }
            Act::Stop => Ok(Flow::Stop),
        }
    }

    /// Plan, gate, and (on proceed) run one measurement stage through
    /// [`stages::run_stage`], threading the [`RunGuard`] so the stage registers
    /// its snapshot restores *before* mutating.
    ///
    /// # Errors
    /// Returns an error if the wrapped harness cannot be spawned or a snapshot
    /// / artifact write fails (see [`stages::run_stage`]).
    fn run_measurement(
        &self,
        state: &mut State,
        session: &mut Session,
        guard: &mut RunGuard<'_>,
        stage_num: u32,
        cap: &Capture,
        hash: &str,
    ) -> Result<Flow> {
        let cfg = self.stage_cfg(cap);
        let plan = stages::plan(stage_num, &cfg);
        let card = measurement_card(stage_num, &plan);
        let id = stage_step_id(stage_num);
        match act_of(confirm(
            self.host,
            &mut session.mode,
            &mut session.seen,
            &card,
        )) {
            Act::Run => {
                let result = stages::run_stage(self.host, guard, stage_num, &cfg)?;
                let outputs = result.output_path.clone().into_iter().collect();
                done_panel(self.host, &card, &result);
                state.set_step(self.host, id, Status::Done, hash, outputs);
                Ok(Flow::Continue)
            }
            Act::Noop => {
                done_panel(self.host, &card, &dry_run_result());
                Ok(Flow::Continue)
            }
            Act::Skip => {
                state.set_step(self.host, id, Status::Skipped, hash, Vec::new());
                Ok(Flow::Continue)
            }
            Act::Stop => Ok(Flow::Stop),
        }
    }

    /// Gate and (on proceed) run the Stage 4 bundle assembly.
    ///
    /// Read-only with respect to host state — it only reads the bundle's
    /// artifacts and writes the draft back into the bundle — so it takes no
    /// [`RunGuard`].
    ///
    /// # Errors
    /// Returns an error if the draft cannot be written into the bundle (see
    /// [`report::assemble`]).
    fn run_assembly(&self, state: &mut State, session: &mut Session, hash: &str) -> Result<Flow> {
        let card = assembly_card(&self.bundle_dir);
        match act_of(confirm(
            self.host,
            &mut session.mode,
            &mut session.seen,
            &card,
        )) {
            Act::Run => {
                let path = report::assemble(
                    self.host,
                    &self.bundle_dir,
                    SUITE_VERSION,
                    &report_scope(self.cli),
                )?;
                let display = path.display().to_string();
                let result = StepResult {
                    code: Some(0_i32),
                    summary: format!("Assembled {}.", report::REPORT_DRAFT),
                    output_path: Some(display.clone()),
                };
                done_panel(self.host, &card, &result);
                state.set_step(self.host, ASSEMBLY_ID, Status::Done, hash, vec![display]);
                Ok(Flow::Continue)
            }
            Act::Noop => {
                done_panel(self.host, &card, &dry_run_result());
                Ok(Flow::Continue)
            }
            Act::Skip => {
                state.set_step(self.host, ASSEMBLY_ID, Status::Skipped, hash, Vec::new());
                Ok(Flow::Continue)
            }
            Act::Stop => Ok(Flow::Stop),
        }
    }

    /// Run the selected stages in order, honoring resume and stage selection.
    ///
    /// The read-only Stage 0 [`Capture`] is computed at most once, and only
    /// when Stage 0 or a measurement stage will actually run, so a
    /// fully-cached resume performs no probes.
    fn execute(
        &self,
        state: &mut State,
        session: &mut Session,
        guard: &mut RunGuard<'_>,
        hash: &str,
    ) -> Result<()> {
        let stage0_selected = stage_selected(self.cli, 0);
        let stage0_skip = state.should_skip(STAGE0_ID, hash);
        let measure_live = (1..=MEASUREMENT_STAGES).any(|stage| {
            stage_selected(self.cli, stage) && !state.should_skip(&stage_step_id(stage), hash)
        });
        let capture = ((stage0_selected && !stage0_skip) || measure_live)
            .then(|| self.capture(session))
            .transpose()?;

        if stage0_selected {
            let flow = if stage0_skip {
                self.host.out("-> STAGE 0: PLAN cached (resume) - skipping");
                Flow::Continue
            } else if let Some(cap) = capture.as_ref() {
                self.run_stage0(state, session, cap, hash)?
            } else {
                Flow::Continue
            };
            if matches!(flow, Flow::Stop) {
                return Ok(());
            }
        }
        for stage in 1..=MEASUREMENT_STAGES {
            if !stage_selected(self.cli, stage) {
                continue;
            }
            if state.should_skip(&stage_step_id(stage), hash) {
                self.host.out(&format!(
                    "-> {} cached (resume) - skipping",
                    stage_banner(stage)
                ));
                continue;
            }
            let Some(cap) = capture.as_ref() else {
                continue;
            };
            if matches!(
                self.run_measurement(state, session, guard, stage, cap, hash)?,
                Flow::Stop
            ) {
                return Ok(());
            }
        }
        if stage_selected(self.cli, ASSEMBLY_STAGE) {
            if state.should_skip(ASSEMBLY_ID, hash) {
                self.host
                    .out("-> STAGE 4: ASSEMBLY cached (resume) - skipping");
            } else if matches!(self.run_assembly(state, session, hash)?, Flow::Stop) {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Resolve the bundle directory: resume an explicit `--bundle`, else mint a new
/// timestamped one (a dry-run computes the path without creating it).
fn resolve_bundle_dir(host: &dyn Host, cli: &Cli, dry_run: bool) -> Result<PathBuf> {
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
const fn tool_disposition(cli: &Cli) -> Disposition {
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
fn run_fetch_competitors(host: &dyn Host, cli: &Cli) -> Result<()> {
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
fn load_or_new_state(
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

/// Entry point for the `uffs-bench` binary (`main.rs` is a thin shim).
///
/// Resolves the bundle and `state.json`, then runs Stage 0 (the read-only plan
/// gate) followed by the selected measurement stages, draining the [`RunGuard`]
/// teardown afterwards. An operator abort/back stops the run gracefully.
///
/// # Errors
/// Returns an error if bundle creation, state load/save, or a Stage 0 artifact
/// write fails. An operator abort/back is **not** an error (returns `Ok`).
pub fn run(host: &dyn Host, cli: &Cli) -> Result<()> {
    match cli.command {
        Some(Command::FetchCompetitors) => return run_fetch_competitors(host, cli),
        Some(Command::Restore) => return teardown::restore(host, cli),
        Some(Command::Verify) => return teardown::verify(host, cli),
        None => {}
    }

    let mode = cli.mode();
    let dry_run = mode == Mode::DryRun;
    let decisions = decisions_from_cli(cli);
    let hash = plan_input_hash(&decisions);

    let bundle_dir = resolve_bundle_dir(host, cli, dry_run)?;
    let state_path = bundle_dir.join("state.json");
    let mut state = load_or_new_state(host, cli, &state_path, &decisions)?;
    if cli.force {
        state.invalidate_all();
    } else if cli.redo {
        state.invalidate(STAGE0_ID);
    }

    if !dry_run {
        teardown::baseline(host, cli, &bundle_dir)?;
    }

    let orchestrator = Orchestrator {
        host,
        cli,
        bundle_dir: bundle_dir.clone(),
    };
    let mut session = Session {
        mode,
        seen: BTreeSet::new(),
    };
    let mut guard = RunGuard::new(host);

    orchestrator.execute(&mut state, &mut session, &mut guard, &hash)?;

    if dry_run {
        report_crumbs(host, &guard.finish());
        return Ok(());
    }
    teardown::finalize(host, cli, &bundle_dir, guard, &mut state, &state_path)
}

#[cfg(test)]
mod tests;
