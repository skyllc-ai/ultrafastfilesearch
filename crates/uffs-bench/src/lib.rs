// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! # uffs-bench: Robust Benchmark-Suite Orchestrator (Phase P1 spine)
//!
//! Workspace-only tool crate that drives the reproducible benchmark flow
//! described in `docs/benchmarks/robust-benchmark-flow-execution-plan.md`.
//!
//! ## Guiding principle — "no crumb left behind"
//!
//! Every host-state mutation follows the same cycle: **snapshot → mutate →
//! restore → verify**. The restore is registered *before* the mutation, so a
//! [`Drop`]-guarded [`restore::RestoreRegistry`] undoes it on early return or
//! panic just as reliably as on the happy path.
//!
//! ## Dependency-injection seam
//!
//! All side effects (filesystem, process spawning, clock, TTY, user input,
//! console output) flow through the [`host::Host`] trait. The real
//! [`host::SystemHost`] talks to the OS; the [`host::MockHost`] keeps an
//! in-memory filesystem, records every call, and replays scripted keypresses.
//! This lets the entire orchestrator — including the Windows-only measurement
//! stages added in later phases — be unit-tested on any OS in the same
//! `cargo nextest` lane as `just go`.
//!
//! ## P1 surface (this crate today)
//!
//! - [`error`] — [`error::BenchError`] / [`error::CrumbError`].
//! - [`host`] — the [`host::Host`] seam + `SystemHost` / `MockHost`.
//! - [`restore`] — the LIFO undo registry and its `Drop` guard.
//! - [`gate`] — modes, cards, and the mode-aware [`gate::confirm`] decision.
//! - [`state`] — the `state.json` model + resume engine (`input_hash`).
//! - [`tooling`] — acquired-tool keep/remove dispositions.
//! - [`fingerprint`] — host fingerprint capture + crumb diff.
//! - [`bundle`] — bundle directory creation + tool resolution.
//!
//! Measurement stages (`env`, `preflight`, `matrix`, `stages`, `report`) and
//! the `clap` CLI binary build on this foundation in phases P2–P9.
//!
//! ## Deferred to P2
//!
//! `Host::is_elevated` (from the design sketch) is intentionally **not** part
//! of the P1 trait: honest token-elevation detection needs platform-specific
//! `unsafe`, and no P1 component consumes it. It lands with `env.rs` (Stage
//! 0a), where the environment fingerprint actually requires it.

// Collection types are imported from `alloc` (not `std`) per the workspace
// `std_instead_of_alloc` lint; this brings the crate into scope for those
// paths.
extern crate alloc;

pub mod bundle;
pub mod error;
pub mod fingerprint;
pub mod gate;
pub mod host;
pub mod restore;
pub mod state;
pub mod tooling;

pub use error::{BenchError, CrumbError, Result};
pub use host::{Host, MockHost, ProcOutput, SystemHost};
