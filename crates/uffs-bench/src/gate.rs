// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Mode-aware confirmation gates and the data-driven card renderer.
//!
//! Every step declares a [`Card`]; [`confirm`] renders it according to the
//! active [`Mode`] and returns a [`Decision`]. The renderer is the single place
//! that formats a step, guaranteeing the *command shown equals the command run*
//! (the commands list is displayed verbatim and executed verbatim by the
//! caller). See `docs/benchmarks/robust-benchmark-flow-implementation-guide.md`
//! §4.

use alloc::collections::BTreeSet;

use crate::host::Host;

/// How aggressively the orchestrator confirms before mutating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Teach each step in full the first time, terse thereafter; prompt always.
    Guided,
    /// Terse prompt for every step; assume the operator already knows the flow.
    Interactive,
    /// Proceed through every step with no prompts (snapshot/restore still run).
    AutoPilot,
    /// Render every card but perform zero mutations.
    DryRun,
}

/// How much of a card to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLevel {
    /// The full teaching card.
    Full,
    /// A single-line summary.
    Terse,
    /// Render nothing.
    None,
}

/// The operator's (or mode's) choice for a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Run the step.
    Proceed,
    /// Dry-run: pretend to run the step, mutate nothing.
    ProceedNoop,
    /// Skip this step.
    Skip,
    /// Switch to autopilot and run this and all remaining steps.
    Autopilot,
    /// Go back to the previous step.
    Back,
    /// Abort the run now.
    Abort,
}

/// All data needed to render and confirm a single step.
#[derive(Debug, Clone)]
pub struct Card {
    /// Stable id (`"<stage>/<step-kind>"`); drives the seen-card dedupe.
    pub id: String,
    /// Stage banner, for example `"STAGE 2 · PARITY"`.
    pub stage: String,
    /// 1-based index of this step within its stage.
    pub step_num: u32,
    /// Total steps in this stage.
    pub step_total: u32,
    /// One-line step title.
    pub title: String,
    /// Short rationale shown on the full card.
    pub why: String,
    /// Exact commands, shown verbatim and run verbatim.
    pub commands: Vec<String>,
    /// Blast-radius resource ids touched by the step.
    pub resources: Vec<String>,
    /// Human-readable description of the backups taken before mutating.
    pub backups: Vec<String>,
    /// Rough time estimate, for example `"~25-60 s"`.
    pub est_time: String,
    /// What an abort/Ctrl-C restores.
    pub recovery: String,
    /// Long explanation shown when the operator presses `e`.
    pub long_why: String,
}

/// Outcome of running a step, rendered by [`done_panel`].
#[derive(Debug, Clone)]
pub struct StepResult {
    /// Process/operation exit code, if any.
    pub code: Option<i32>,
    /// One-line human summary.
    pub summary: String,
    /// Where the step wrote its artifact, if any.
    pub output_path: Option<String>,
}

/// Render `card` at the requested `level` through the host's console output.
pub fn show_card(host: &dyn Host, card: &Card, level: CardLevel) {
    match level {
        CardLevel::None => {}
        CardLevel::Terse => show_terse(host, card),
        CardLevel::Full => show_full(host, card),
    }
}

/// Render the single-line summary form of a card.
fn show_terse(host: &dyn Host, card: &Card) {
    host.out(&format!(
        "-> {} - step {}/{}: {} ({})  [y/s/a/b/q, e=explain, ?=help]",
        card.stage, card.step_num, card.step_total, card.title, card.est_time
    ));
}

/// Render the full teaching card.
fn show_full(host: &dyn Host, card: &Card) {
    host.out("");
    host.out(&format!(
        "+- {} - step {}/{}",
        card.stage, card.step_num, card.step_total
    ));
    host.out("|");
    host.out(&format!("| {}", card.title));
    host.out("|");
    host.out(&format!("| {}", card.why));
    if !card.commands.is_empty() {
        host.out("|");
        host.out("| commands (run verbatim):");
        for cmd in &card.commands {
            host.out(&format!("|   $ {cmd}"));
        }
    }
    if !card.resources.is_empty() {
        host.out("|");
        for res in &card.resources {
            host.out(&format!("| {res}"));
        }
    }
    if !card.backups.is_empty() {
        host.out("|");
        host.out(&format!("| backups: {}", card.backups.join(", ")));
    }
    host.out("|");
    host.out(&format!(
        "| est: {}   recovery: {}",
        card.est_time, card.recovery
    ));
    host.out("+- [Enter/y] proceed   [s] skip   [a] autopilot   [b] back   [q] quit   [e] explain   [?] help");
}

/// Render the DONE panel after a step has run.
pub fn done_panel(host: &dyn Host, card: &Card, result: &StepResult) {
    let status = match result.code {
        Some(0_i32) => "OK".to_owned(),
        Some(code) => format!("exit {code}"),
        None => "terminated by signal".to_owned(),
    };
    host.out(&format!(
        "[done] {} - {} [{status}]",
        card.stage, card.title
    ));
    host.out(&format!("       {}", result.summary));
    if let Some(path) = &result.output_path {
        host.out(&format!("       output: {path}"));
    }
}

/// Confirm a step according to the active [`Mode`], returning a [`Decision`].
///
/// `seen` tracks card ids already taught this run so guided mode shows the full
/// card once and the terse form thereafter. Pressing `a` upgrades `*mode` to
/// [`Mode::AutoPilot`] in place. On a non-interactive host an interactive mode
/// fails closed with [`Decision::Abort`].
#[must_use]
pub fn confirm(
    host: &dyn Host,
    mode: &mut Mode,
    seen: &mut BTreeSet<String>,
    card: &Card,
) -> Decision {
    match *mode {
        Mode::DryRun => {
            show_card(host, card, CardLevel::Full);
            Decision::ProceedNoop
        }
        Mode::AutoPilot => Decision::Proceed,
        Mode::Guided | Mode::Interactive => prompt(host, mode, seen, card),
    }
}

/// Render a card and loop on keypresses until a terminal decision is made.
fn prompt(host: &dyn Host, mode: &mut Mode, seen: &mut BTreeSet<String>, card: &Card) -> Decision {
    let level = if *mode == Mode::Guided && !seen.contains(&card.id) {
        CardLevel::Full
    } else {
        CardLevel::Terse
    };
    show_card(host, card, level);
    seen.insert(card.id.clone());

    if !host.is_tty() {
        host.out("  (no TTY: cannot confirm interactively - aborting)");
        return Decision::Abort;
    }

    loop {
        let Ok(key) = host.read_key() else {
            return Decision::Abort;
        };
        if let Some(decision) = interpret_key(host, mode, card, key) {
            let echo = match key {
                '\n' | '\r' => "[Enter] → proceeding",
                'y' | 'Y' => "[y] → proceeding",
                'a' | 'A' => "[a] → autopilot",
                's' | 'S' => "[s] → skipping",
                'b' | 'B' => "[b] → going back",
                'q' | 'Q' => "[q] → aborting",
                _ => "",
            };
            if !echo.is_empty() {
                host.out(&format!("   {echo}"));
            }
            return decision;
        }
    }
}

/// Map a keypress to a [`Decision`], or `None` to keep prompting.
///
/// `e`/`?`/unrecognized keys render help and return `None` so the caller loops;
/// they never mutate anything (transparency guarantee §4.7).
fn interpret_key(host: &dyn Host, mode: &mut Mode, card: &Card, key: char) -> Option<Decision> {
    match key.to_ascii_lowercase() {
        'y' | '\n' | '\r' => Some(Decision::Proceed),
        's' => Some(Decision::Skip),
        'a' => {
            *mode = Mode::AutoPilot;
            Some(Decision::Autopilot)
        }
        'b' => Some(Decision::Back),
        'q' => Some(Decision::Abort),
        'e' => {
            host.out(&format!("  {}", card.long_why));
            None
        }
        '?' => {
            host.out("  keys: y=proceed  s=skip  a=autopilot  b=back  q=quit  e=explain  ?=help");
            None
        }
        _ => {
            host.out("  (unrecognized key; press ? for help)");
            None
        }
    }
}
