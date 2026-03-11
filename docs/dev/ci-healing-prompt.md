# CI Healing Prompt (Fixes Only)

**Role:** Senior Rust engineer + CI healer.
**Goal:** Make the repository **CI-clean**: zero errors, zero warnings, and all tests pass. Achieve this by making *
*real code fixes**—no silencing, no skipping, no theatrics.

**Context:** Use the CI pipeline driver for baseline and final validation:
`rust-script scripts/ci/ci-pipeline.rs workflow-reset && rust-script scripts/ci/ci-pipeline.rs go - v`
The pipeline can take ~50 minutes. During fix cycles, you may run local `cargo clippy`/`cargo test`/`cargo check`/`cargo build` to iterate quickly. Do not modify the pipeline itself; treat pipeline output as the single source of truth for acceptance.

---

## Operate by these rules


> Verification policy: Baseline and final validation must use the CI pipeline commands above. Between those runs, you may use local `cargo clippy`/`cargo test`/`cargo check`/`cargo build` to iterate faster. Local checks are advisory; pipeline results decide acceptance.

1. **No suppression hacks:** Do **not** add blanket `#[allow(...)]`, disable lints, comment out failing tests, or hide
   problems behind cfg gates. If a targeted allow is truly necessary, keep it minimal, scoped, and justify it in code
   comments.
2. **Surgical, correct fixes:** Prefer minimal, idiomatic Rust changes that resolve root causes (ownership, types,
   semantics) rather than superficial workarounds.
3. **Preserve behavior & contracts:** Maintain public API and observable behavior unless the CI failures prove they’re
   wrong or inconsistent—then update docs and tests accordingly.
4. **Improve tests, don’t dodge them:** Strengthen tests to be deterministic and meaningful; never skip or relax them
   just to pass.
5. **Document & commit well:** Make small, atomic commits with clear messages (`fix: concise root cause`). Keep a
   running `<<YYY_MM_DD_HH_MM_>>CHANGELOG_HEALING.md` describing what failed, why, and how you fixed it. Location in the
   LOG directory we use to keep the app running logs

---

## Workflow (loop)

1. Kick off a full pipeline run to capture a baseline (save/trim logs).
2. Iterate locally: use targeted `cargo clippy`/`cargo test`/`cargo check`/`cargo build` to diagnose and fix quickly.
3. When green locally (or confident), re-run the full pipeline for validation.
4. If the pipeline passes (green), stop — you’re done. If it fails, loop back to step 2.

Notes:
- The pipeline is the source of truth for acceptance; local runs are for iteration speed only.
- Prefer mirroring pipeline flags/config when running locally to reduce drift.

- Stop criteria: Do not run extra confirmation passes after the first green pipeline unless new changes are pushed.


---

## How to fix typical failure classes

- **Compilation / Type / Borrow errors**
    - Resolve lifetime issues by restructuring ownership first (prefer moving ownership, splitting borrows, or using
      iterator adapters); only use `clone()` when it’s correct and intentional.
    - Disambiguate trait method calls with fully qualified syntax; add explicit types where inference misleads.
    - Tighten trait bounds (`where T: Trait`) or implement the required traits (`From/TryFrom/AsRef/Into`) instead of
      ad-hoc conversions.

- **Lints & warnings**
    - Remove unused imports/variables; handle `Result`/`Option` exhaustively.
    - Replace deprecated APIs; prefer `if let`/`match` over `unwrap()` in production paths, or use
      `expect("why this cannot fail")` with rationale.
    - Simplify control flow per lint recommendations (e.g., iterator idioms over index loops).

- **API breakages / dependency drift**
    - Update call sites to new signatures, feature flags, or modules per upstream changelogs.
    - Pin or bump versions in `Cargo.toml` with justification; prefer code updates over version downgrades unless the
      upgrade is impossible.
    - Keep features explicit and minimal; avoid enabling large, unrelated feature sets.

- **Edition/MSRV mismatches**
    - Align code to the current edition idioms (module paths, `TryFrom`, `dyn Trait`, etc.).
    - If the CI indicates an MSRV boundary, either refactor to fit the MSRV or pin dependencies compatible with it;
      document the choice.

- **Tests (unit/integration/doctests)**
    - Make tests deterministic: seed RNGs, eliminate sleep-based timing, and isolate filesystem/network/state (use temp
      dirs, mocks, or trait-based abstractions).
    - Fix race conditions (use channels, `Arc<Mutex/RwLock>`, or proper `async` awaits). Avoid shared mutable globals.
    - Keep doctests compiling; update examples or mark *truly* non-compiling snippets with `ignore` and a comment why.

- **Performance / allocation issues surfaced by CI**
    - Replace needless clones with borrowing; prefer `Cow` or iterators.
    - Use `&str` over `String` where possible; avoid intermediate allocations.

- **Unsafe code**
    - Remove or reduce `unsafe` where feasible; otherwise document invariants and wrap in safe abstractions with checks.

---

## Acceptance criteria

- A final pipeline run shows **zero warnings and zero errors**, and **all pipeline tests green**.
- Success is measured solely by the pipeline output; local runs are allowed mid-cycle but are not sufficient for acceptance.
- No broad lint suppression, no skipped tests, and no reduction in coverage.
- Changes are minimal, idiomatic, and justified in commit messages and `<<YYY_MM_DD_HH_MM_>>CHANGELOG_HEALING.md`.

---

## If blocked

If an upstream bug or unavoidable external constraint prevents a clean run, produce `ci-heal-report.md` with:

- A trimmed failing log,
- Root-cause analysis,
- The minimal viable fix or workaround (with code diffs),
- Any follow-up needed (issue links, PRs upstream).
