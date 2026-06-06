// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 0e plan gate + the staged orchestrator skeleton.
//!
//! [`run`] is the binary's single entry point (`main.rs` is a thin shim). It
//! resolves the bundle directory and `state.json`, captures the Stage 0a
//! environment fingerprint, runs the Stage 0c competitor preflight, negotiates
//! the Stage 0d matrix, then presents the Stage 0e plan gate through the
//! mode-aware [`confirm`] gate. Stages 1–3 are measurement stubs in this phase
//! (the Windows-live harness wrappers land in P5/P6); they are wired here so
//! the resume engine and `--only-stage`/`--from-stage` selection are exercised
//! end to end. The "no crumb left behind" [`RunGuard`] teardown is created and
//! drained at the top level; measurement stages register their snapshots
//! through it once they gain real mutations in P5/P6.
//!
//! Every side effect flows through the [`Host`] seam, so the whole orchestrator
//! is unit-testable under the `MockHost` on any OS.

use alloc::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::bundle::{bundle_path, new_bundle};
use crate::cli::Cli;
use crate::env::{self, EnvFingerprint, EnvSpec, ToolProbe};
use crate::error::{CrumbError, Result};
use crate::gate::{Card, Decision, Mode, StepResult, confirm, done_panel};
use crate::host::Host;
use crate::matrix::{self, Matrix, MatrixSpec};
use crate::preflight::{self, PatternProbe, PreflightResult, PreflightSpec};
use crate::restore::RunGuard;
use crate::state::{Decisions, State, Status, input_hash};

/// Suite version stamped into bundle names and `state.json`.
const SUITE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable id of the Stage 0 plan step in the resume engine.
const STAGE0_ID: &str = "stage0/plan";

/// Default measurement patterns: display name + UFFS row-count argument
/// template (`{DRIVE}` is substituted per drive during preflight).
const DEFAULT_PATTERNS: [(&str, &[&str]); 2] = [
    ("all_dlls", &["{DRIVE}:\\", "*.dll", "--count"]),
    ("full_scan", &["{DRIVE}:\\", "*", "--count"]),
];

/// Default readiness-poll attempts for a configured-but-cold competitor drive.
const PREFLIGHT_POLL_ATTEMPTS: u32 = 3;

/// Delay between competitor readiness-poll attempts, in milliseconds.
const PREFLIGHT_POLL_INTERVAL_MS: u64 = 1_000;

/// Number of measurement stages following Stage 0 (cross-tool, parity, full).
const MEASUREMENT_STAGES: u32 = 3;

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

/// Build a version-probe for one tool id (Everything uses `es` + its own flag).
fn tool_probe(name: &str) -> ToolProbe {
    let (exe, args) = if name == matrix::EVERYTHING_TOOL {
        ("es".to_owned(), vec!["-get-everything-version".to_owned()])
    } else {
        (name.to_owned(), vec!["--version".to_owned()])
    };
    ToolProbe {
        name: name.to_owned(),
        exe,
        args,
    }
}

/// Resolve the read-only `Everything.ini` path from the host environment.
///
/// Uses `%APPDATA%\Everything\Everything.ini` when `APPDATA` is set (the
/// Windows install default), falling back to a bare relative name otherwise so
/// the preflight simply observes an absent ini on other hosts.
fn everything_ini_path(host: &dyn Host) -> PathBuf {
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
fn env_spec_from_cli(cli: &Cli) -> EnvSpec {
    EnvSpec {
        tools: cli
            .tools_or_default()
            .iter()
            .map(|tool| tool_probe(tool))
            .collect(),
    }
}

/// Build the Stage 0c [`PreflightSpec`] from the CLI and host environment.
fn preflight_spec_from_cli(host: &dyn Host, cli: &Cli) -> PreflightSpec {
    let patterns = DEFAULT_PATTERNS
        .iter()
        .map(|(name, args)| PatternProbe {
            name: (*name).to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        })
        .collect();
    PreflightSpec {
        ini_path: everything_ini_path(host),
        candidate_drives: cli.drives_or_default(),
        es_exe: "es".to_owned(),
        uffs_exe: "uffs".to_owned(),
        patterns,
        poll_attempts: PREFLIGHT_POLL_ATTEMPTS,
        poll_interval_ms: PREFLIGHT_POLL_INTERVAL_MS,
    }
}

/// Build the Stage 0d [`MatrixSpec`] from the CLI.
fn matrix_spec_from_cli(cli: &Cli) -> MatrixSpec {
    MatrixSpec {
        required_tools: cli.tools_or_default(),
        candidate_drives: cli.drives_or_default(),
        patterns: DEFAULT_PATTERNS
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
fn stage_step_id(stage: u32) -> String {
    format!("stage{stage}/measure")
}

/// Operator-facing banner for a measurement stage.
fn stage_banner(stage: u32) -> String {
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

/// Build the Stage 0e plan-gate [`Card`].
fn plan_card(bundle_dir: &Path) -> Card {
    Card {
        id: STAGE0_ID.to_owned(),
        stage: "STAGE 0: PLAN".to_owned(),
        step_num: 1,
        step_total: 1,
        title: "Confirm environment, competitor preflight, and negotiated matrix".to_owned(),
        why: "Lock the apples-to-apples plan before any measurement runs.".to_owned(),
        commands: Vec::new(),
        resources: vec![bundle_dir.display().to_string()],
        backups: Vec::new(),
        est_time: "~5-20 s".to_owned(),
        recovery: "Read-only: nothing is mutated, so an abort restores nothing.".to_owned(),
        long_why: "The plan above is derived entirely from read-only probes; \
                    proceeding writes the Stage 0 artifacts into the bundle and \
                    unlocks the measurement stages."
            .to_owned(),
    }
}

/// Build a measurement-stage [`Card`] (skeleton: the harness lands in P5/P6).
fn measurement_card(stage: u32) -> Card {
    let banner = stage_banner(stage);
    let title = format!("{banner} measurements");
    Card {
        id: stage_step_id(stage),
        stage: banner,
        step_num: 1,
        step_total: 1,
        title,
        why: "Time the negotiated cells for each participating tool.".to_owned(),
        commands: Vec::new(),
        resources: Vec::new(),
        backups: Vec::new(),
        est_time: "~1-5 min".to_owned(),
        recovery: "Any competitor config touched is restored at teardown.".to_owned(),
        long_why: "Measurement harness wrappers (snapshot -> mutate -> measure \
                    -> restore) are implemented in phases P5/P6; this skeleton \
                    wires the gate, resume, and stage selection."
            .to_owned(),
    }
}

/// The [`StepResult`] for a completed Stage 0.
fn stage0_result(bundle_dir: &Path) -> StepResult {
    StepResult {
        code: Some(0_i32),
        summary: "Plan locked; Stage 0 artifacts written.".to_owned(),
        output_path: Some(bundle_dir.join("matrix.json").display().to_string()),
    }
}

/// The [`StepResult`] used when a step is dry-run (rendered, not executed).
fn dry_run_result() -> StepResult {
    StepResult {
        code: None,
        summary: "Dry-run: rendered only, nothing mutated.".to_owned(),
        output_path: None,
    }
}

/// The placeholder [`StepResult`] for a skeleton measurement stage.
fn stub_result(stage: u32) -> StepResult {
    StepResult {
        code: Some(0_i32),
        summary: format!("Stage {stage} harness lands in P5/P6 (skeleton no-op)."),
        output_path: None,
    }
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

    /// Capture Stage 0 (env + preflight + matrix), render the plan, and gate
    /// it.
    fn run_stage0(
        &self,
        state: &mut State,
        mode: &mut Mode,
        seen: &mut BTreeSet<String>,
        hash: &str,
    ) -> Result<Flow> {
        let fp = env::capture(self.host, &env_spec_from_cli(self.cli));
        let preflight =
            preflight::capture(self.host, &preflight_spec_from_cli(self.host, self.cli));
        let matrix = matrix::compute_matrix(&matrix_spec_from_cli(self.cli), &preflight);

        self.host.out(&env::render_md(&fp));
        self.host.out(&matrix::render_md(&matrix));

        let card = plan_card(&self.bundle_dir);
        match act_of(confirm(self.host, mode, seen, &card)) {
            Act::Run => {
                self.write_stage0(&fp, &preflight, &matrix)?;
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

    /// Gate one measurement stage (skeleton: no timing harness yet).
    fn run_measurement(
        &self,
        state: &mut State,
        mode: &mut Mode,
        seen: &mut BTreeSet<String>,
        stage_num: u32,
        hash: &str,
    ) -> Flow {
        let card = measurement_card(stage_num);
        let id = stage_step_id(stage_num);
        match act_of(confirm(self.host, mode, seen, &card)) {
            Act::Run => {
                done_panel(self.host, &card, &stub_result(stage_num));
                state.set_step(self.host, id, Status::Done, hash, Vec::new());
                Flow::Continue
            }
            Act::Noop => {
                done_panel(self.host, &card, &dry_run_result());
                Flow::Continue
            }
            Act::Skip => {
                state.set_step(self.host, id, Status::Skipped, hash, Vec::new());
                Flow::Continue
            }
            Act::Stop => Flow::Stop,
        }
    }

    /// Run the selected stages in order, honoring resume and stage selection.
    fn execute(
        &self,
        state: &mut State,
        mode: &mut Mode,
        seen: &mut BTreeSet<String>,
        hash: &str,
    ) -> Result<()> {
        if stage_selected(self.cli, 0) {
            if state.should_skip(STAGE0_ID, hash) {
                self.host.out("-> STAGE 0: PLAN cached (resume) - skipping");
            } else if matches!(self.run_stage0(state, mode, seen, hash)?, Flow::Stop) {
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
            if matches!(
                self.run_measurement(state, mode, seen, stage, hash),
                Flow::Stop
            ) {
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
    let mut mode = cli.mode();
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

    let orchestrator = Orchestrator {
        host,
        cli,
        bundle_dir,
    };
    let mut seen = BTreeSet::new();
    let guard = RunGuard::new(host);

    orchestrator.execute(&mut state, &mut mode, &mut seen, &hash)?;

    report_crumbs(host, &guard.finish());

    if !dry_run {
        state.save(host, &state_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{STAGE0_ID, decisions_from_cli, plan_input_hash, run, stage_selected};
    use crate::cli::Cli;
    use crate::host::{Call, MockHost};
    use crate::state::{State, Status};

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
        let host = MockHost::new();
        let cli = Cli::parse_from(["uffs-bench", "--dry-run", "--drives", "C"]);

        run(&host, &cli).expect("dry run succeeds");

        assert!(
            host.calls().iter().all(|call| !is_mutation(call)),
            "dry-run must perform zero filesystem mutations"
        );
    }

    #[test]
    fn autopilot_writes_stage0_artifacts_and_saves_state() {
        let host = MockHost::new();
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
}
