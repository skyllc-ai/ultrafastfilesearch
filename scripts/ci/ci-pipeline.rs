#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! ```
// =============================================================================
// scripts/ci/ci-pipeline.rs — LEGACY thin wrapper
// =============================================================================
//
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// Phase 7 of the dev-flow implementation plan promoted the CI pipeline
// driver to a proper Cargo workspace binary at `scripts/ci-pipeline/`.
// This file used to be a ~1750-line `rust-script` that compiled to an
// opaque cache under `~/.cache/cargo-rust-script/` — meaning local edits
// took effect only after an implicit Cargo rebuild on next invocation,
// and the IDE had no inline diagnostics for it.
//
// This thin shim remains so the muscle-memory invocation
//
//     rust-script scripts/ci/ci-pipeline.rs <subcommand>
//
// keeps working for one release cycle (through v0.5.73).  It just forwards
// every argument to the workspace binary:
//
//     cargo run -q --release -p uffs-ci-pipeline -- <subcommand>
//
// The in-repo justfile recipes have already been updated to call the new
// binary directly; this file exists purely for humans and out-of-tree
// scripts that still hard-code the old path.
//
// REMOVE-AFTER: v0.5.73.
// See: docs/architecture/dev-flow-implementation-plan.md § 7.
// =============================================================================

use std::process::{Command, exit};

fn main() {
    eprintln!(
        "[ci-pipeline] DEPRECATED: scripts/ci/ci-pipeline.rs is now a thin \
         wrapper. The implementation moved to the workspace binary \
         `uffs-ci-pipeline` (Phase 7 of dev-flow plan). Prefer \
         `cargo run -q --release -p uffs-ci-pipeline -- <subcommand>` or \
         the updated `just` recipes. This shim will be removed after v0.5.73."
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = Command::new("cargo")
        .args(["run", "-q", "--release", "-p", "uffs-ci-pipeline", "--"])
        .args(&args)
        .status()
        .expect("failed to spawn `cargo run -p uffs-ci-pipeline`");
    exit(status.code().unwrap_or(1));
}
