# Developer Flow & Shift-Left Quality Gates

<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

Status as of **2026-04-23** · Maintainer: `@githubrobbi` · Review cadence: quarterly

> **See also**: `dev-flow-implementation-plan.md` — the concrete
> 7-phase execution plan that supersedes this doc's §6 "Proposed
> Refactor" section.  After external review, the refactor was
> restructured around **lane separation** (gate / preview / release /
> deep-assurance) and **hard-vs-advisory gate classification** rather
> than the single-pipeline model v1 proposed.  This document remains
> the **explanation** (why gates exist); the plan doc is the
> **execution** (how and when to change them).

**Changelog**
- 2026-04-23 — Initial doc after v0.5.71 ship post-mortem.  Surfaced three
  bugs (ship resumable push-skip, `CARGO_INCREMENTAL` vs sccache drift,
  `rust-script` stale cache) and three shift-left gaps
  (`cargo vet`, doc tests, `cargo machete`) exposed by PR #43's
  four-iteration supply-chain failure loop.
- 2026-04-23 — External review pointed out structural gaps: v1 treats
  CI as one long lane, soft-skips gates that should be hard-required,
  and proposes a split-brain cache policy.  Follow-up captured in
  `dev-flow-implementation-plan.md`; §6 of this doc is retained for
  historical context but the plan doc is the authoritative roadmap.
- 2026-04-23 — Plan doc revised to **v3** after a second external
  review round.  Eleven correctness fixes to the v2 plan: `required`
  aggregator now depends on `classify`; compile/test gates use
  `code_changed = rust || dep || infra` (not just `rust`);
  pre-push reads git's stdin protocol; preview workflow gates on
  `PR Fast CI / required` being green for the same SHA; new §2.8
  Actions hardening policy.  See `dev-flow-implementation-plan.md`
  §10 for the full diff.

---

## 1. Purpose

This document is the **single source of truth for where each quality gate
lives and why**.  It exists because today (2026-04-22 → 23) a straightforward
patch release loop (v0.5.71) needed four ship attempts to land on origin,
and three of those four failures were preventable by moving one CI-only
gate earlier in the flow.

Who this is for:

- Any contributor modifying `.github/workflows/*.yml`,
  `scripts/hooks/*.sh`, `scripts/ci/ci-pipeline.rs`, or `just/*.just`.
- Future-me reading the reflog wondering "why did we move `cargo vet`
  from CI to pre-push".
- New contributors trying to understand why `git commit` is fast but
  `git push` takes 30 seconds.

---

## 2. Principles

### 2.1 Shift-left — but only when the cost curve crosses

A gate's ideal tier is **the earliest tier where its runtime cost is still
acceptable for the frequency of that tier**.  Run too early and every
commit/push is a slog.  Run too late and broken code escapes to CI, where
the feedback loop is ~10× slower (fetch runner → clone → install nightly →
warm caches → run gate → report).

The decision rule:

```
if gate_runtime < tier_budget AND gate_needs_no_remote_infra:
    shift it one tier earlier
```

### 2.2 Four tiers, four budgets

| Tier | Trigger | Budget | Philosophy |
|------|---------|--------|------------|
| **T0 — editor-time** | on save / on keystroke | &lt; 100 ms | rust-analyzer, clippy-on-save.  Must not block typing. |
| **T1 — pre-commit** | `git commit` | &lt; 25 s warm | Cheap, staged-file scoped.  Soft-skip optional tools. |
| **T2 — pre-push** | `git push` | &lt; 60 s warm | Workspace-wide.  Catch everything locally reproducible. |
| **T3 — CI on PR** | push to PR branch | 8–15 min | Full matrix + things that need remote infra (coverage upload, SBOM, SLSA, CodeQL). |
| **T4 — CI nightly** | `schedule:` cron | 60–90 min | Slow, probabilistic (miri, mutation testing, fuzz). |

### 2.3 Defense-in-depth, not duplicated work

Running clippy at both pre-commit AND pre-push is **intentional**:
pre-commit is the fast daily-driver path; pre-push is the backstop when a
contributor uses `git commit --no-verify`.  The file-locking behavior of
cargo's target-dir means consecutive clippy invocations reuse artifacts —
the second one costs ~1–3 s.  That's correct engineering.

### 2.4 Tooling must be soft-skippable

A first-time contributor should be able to clone, `just install-hooks`, and
work immediately without `typos`, `taplo`, `reuse`, or `cargo-vet`
installed.  Every optional gate in `_lint_fast.sh` and `_lint_pre_push.sh`
must `command -v` check and print a one-line install hint on miss.

---

## 3. Current State — Complete Gate Matrix

Reconstructed from `scripts/hooks/_lint_fast.sh`,
`scripts/hooks/_lint_pre_push.sh`, `.github/workflows/ci.yml`,
and `.github/workflows/tier-2.yml` (all as of commit `185ed8825`).

### 3.1 Flow diagram

```
┌──────────────┐     ┌────────────────┐     ┌─────────────────┐
│ editor save  │────▶│ git commit     │────▶│ git push        │
│ (rust-analyz)│ T0  │ pre-commit hk  │ T1  │ pre-push hook   │ T2
└──────────────┘     │ _lint_fast.sh  │     │ _lint_pre_push  │
                     └────────────────┘     └────────┬────────┘
                                                     │
                                                     ▼
                                            ┌────────────────────┐
                                            │ GitHub Actions      │
                                            │ ci.yml (Tier 1)     │ T3
                                            │ 8 parallel jobs     │
                                            │ ~8-15 min           │
                                            └─────────┬──────────┘
                                                      │
                                           PR merge   │
                                                      ▼
                                            ┌────────────────────┐
                                            │ auto-tag-release    │
                                            │ → release.yml       │
                                            │ (build + sign +     │
                                            │  SBOM + GH release) │
                                            └────────────────────┘

                                weekly cron ─▶ tier-2.yml (coverage, miri,
                                               udeps, Windows native)  T4

                                weekly cron ─▶ cargo-vet-refresh.yml
                                               (opens import-bump PR)
```

### 3.2 Gate matrix (full)

| Gate | T1 pre-commit | T2 pre-push | T3 Tier 1 CI | T4 Tier 2 |
|---|:-:|:-:|:-:|:-:|
| file-size-policy | ✅ | ✅ | ✅ | ✅ |
| `cargo fmt --check` | ✅ if *.rs staged | ✅ | ✅ | — |
| clippy `lint-ci` (`--all-targets -D warnings`) | ✅ if *.rs staged | ✅ | ✅ | — |
| clippy `lint-prod` (pedantic+nursery+cargo+unwrap) | ✅ if *.rs staged | ✅ | — | — |
| clippy `lint-tests` (same, allow unwrap) | ✅ if *.rs staged | ✅ | — | — |
| rustdoc `-Dwarnings` | — | ✅ | ✅ | — |
| `cargo deny check` | — | ✅ | ✅ | — |
| **`cargo vet check --locked`** | — | **❌** | ✅ | — |
| test COMPILE (`nextest --no-run`) | — | ✅ | ✅ (test-build) | — |
| test EXECUTE (`nextest run`) | — | **❌** | ✅ | via coverage |
| **`cargo test --doc`** | — | **❌** | ✅ | — |
| Windows xwin clippy (`lint-ci-windows`, `cargo xwin clippy -- -D warnings`) | — (Phase 2 budget cap) | ✅ if xwin (advisory; W5.6 upgraded from `check` to `clippy`) | ✅ native (`pr-fast.yml::windows-lint`, W5.5) | — (Tier 2 `windows-check` removed in PR #138, strictly subsumed by T3 `windows-lint`) |
| `taplo fmt --check` | ✅ if *.toml staged | — | — | — |
| `typos` | ✅ optional | ✅ optional | — | — |
| `reuse lint` (SPDX) | ✅ optional | ✅ optional | — | — |
| CodeQL Rust SAST | — | — | `codeql.yml` on PR | — |
| `cargo llvm-cov` | — | — | — | ✅ |
| `cargo udeps` | — | — | — | ✅ |
| `miri` (UB check) | — | — | — | ✅ (4 tests) |
| cargo-vet imports refresh | — | — | — | weekly PR |

**Key**: ✅ = gate runs here; — = not in this tier; **❌ bold** = gap where
CI catches something local never runs.

### 3.3 Measured budgets (current)

- **Pre-commit** — 2 s docs-only, 15–25 s with Rust changes warm sccache.
  Stays under the "must not block flow" threshold.  (Phase 2 removed xwin
  from this tier — its 40–90 s cold cost violated the T1 budget.)
- **Pre-push** — 23–45 s warm, 60–90 s cold.  Heaviest jobs: rustdoc and
  nextest `--no-run` share the same target-dir.  File-locking serializes
  them; the non-cargo jobs (deny, typos, reuse, file-size) genuinely run
  in parallel.
- **Tier 1 CI** — 8–12 min all jobs parallel.  Heaviest: `tests` (depends
  on `test-build` artifact cache).
- **Tier 2 CI** — 60–90 min.  Heaviest: `miri` (90 min timeout) and
  `coverage` (polars compile under `llvm-cov` instrumentation).

---

## 4. Gap Analysis — What CI Catches That Local Doesn't

### 4.1 GAP 1 — `cargo vet check --locked` is CI-only (🔴 HIGH IMPACT)

**Evidence**: PR #43 (v0.5.71) failed CI's `Tier 1 / Security` job three
times with `pastey:0.2.2 missing ["safe-to-deploy"]` and
`rustls:0.23.39 missing ["safe-to-deploy"]`.  Local pre-push never flagged
either — by design: `cargo vet` is only invoked in `.github/workflows/ci.yml`
at line 331.

**Why it's a shift-left candidate**:

- Cost to run locally: **1–2 s** (no compile; walks the dep graph against
  `supply-chain/audits.toml` + `imports.lock`).
- Inputs are 100% local: `Cargo.lock`, `supply-chain/*.toml`.  No network,
  no compile, no cache.
- The tool is already in `just/dev.just:118`'s `update-tools` list — any
  maintainer running `just update-tools` has it.

**Why it wasn't there**: Historical artifact — `cargo-vet` was added to CI
in PR #33b (2026-04-22) as a supply-chain hardening measure; the hook
scripts existed before that and weren't updated.

**Expected savings**: Each supply-chain-blocked PR costs ~8 min (CI round-trip
+ diagnose + fix).  At roughly one Dependabot bump per week that trips
`cargo vet`, that's ~6 hours/year reclaimed from mechanical CI waits.

### 4.2 GAP 2 — pre-push compiles tests but never runs them (🟠 MEDIUM)

**Evidence**: `scripts/hooks/_lint_pre_push.sh:101` is
`cargo nextest run --workspace --all-targets --all-features --no-run`.
The `--no-run` means "link all test binaries, then exit".  CI's
`Tier 1 / Tests` job at `ci.yml:238` is `cargo nextest run ... --profile ci`
(without `--no-run`), which actually executes them.

**Class of bugs this misses locally**:

- Flaky tests with a timing dependency.
- Tests that depend on environment (tmpdir perms, `/tmp` being writable,
  localhost DNS).
- Assertions that are correct at compile time but wrong at runtime
  (e.g. a `assert_eq!(computed, constant)` where `constant` went stale).

**Why `--no-run` is currently correct**: Running the full suite warm is
~2–3 min on this workspace.  That blows the 60 s pre-push budget and
destroys developer flow.

**Mitigation path**: Offer a fast test subset via `cargo nextest run -E`
(nextest's expression filters let us run only `unit` tests and skip any
`@marked_slow` tag).  Add a new `just lint-pre-push-with-tests` recipe as
opt-in; keep the default pre-push as-is.

### 4.3 GAP 3 — doc tests are pre-push's blind spot (🟠 MEDIUM)

**Evidence**: pre-push runs `cargo doc -Dwarnings` which only checks that
docs *compile*.  The `cargo test --doc` step (runs the `/// ```rust` blocks
as actual tests) is CI-only at `ci.yml:268`.

**Cost locally**: 10–30 s warm.  Most workspace crates have &lt;20 doctests.

**Fix**: Add a mandatory gate `doc-tests` alongside `rustdoc`.

### 4.4 GAP 4 — unused-dependency detection is weekly-only (🟢 LOW)

**Evidence**: `cargo udeps` runs only in `tier-2.yml:166-191`.  A PR that
removes the last use of a workspace dep accumulates cruft until the next
Monday's Tier 2 run.

**Fast alternative**: `cargo machete` is 5–10× faster than `cargo-udeps`
(static analysis vs rebuild-with-RUSTC_WRAPPER).  Already in
`dev.just:118`'s `update-tools` tool list.

**Recommendation**: Add `cargo machete` as an optional pre-push gate.

### 4.5 GAP 5 — CodeQL is intentionally CI-only (🟢 LEAVE)

CodeQL SAST is a heavyweight GitHub-hosted tool.  Running locally requires
downloading the CodeQL CLI + the Rust extractor (~400 MB).  Not worth it
— clippy (`--pedantic --nursery --cargo`) plus weekly miri covers ~80% of
what CodeQL would catch for this codebase.  This gate stays where it is.

---

## 5. Known Bugs in the Current Flow

### 5.1 BUG A — `just ship` resumable state skips `git push` after success (🔴)

**Observed**: 2026-04-22, during v0.5.71 release.

1. First `just ship -v` run: pushed HEAD (`17022992a`) to
   `release/v0.5.71` at 21:20:18.  Step 11-git-push marked `completed`
   in `build/.uffs-workflow-state.json`.
2. CI on that HEAD failed `cargo vet`.
3. Committed audit fix `185ed8825` locally at 21:29:36.
4. Re-ran `just ship -v`.  The pipeline loaded cached state, saw every
   step marked `completed`, exited in 2 s.  **Step 11 silently skipped
   even though HEAD was 1 commit ahead of `origin/release/v0.5.71`.**
5. Had to bypass with a direct
   `CARGO_INCREMENTAL=0 git push origin 185ed8825:refs/heads/release/v0.5.71`
   to land the audit.

**Root cause**: `execute_step_with_tracking` in
`scripts/ci/ci-pipeline.rs` around line 1341 checks *only* whether the step
name is in the completed-set, not whether the underlying condition still
holds (HEAD == already-pushed-ref).

**Fix pattern**:

```rust
// Step 11 entry — before checking cached state:
let unpushed = Command::new("git")
    .args(["rev-list", "--count",
           &format!("origin/{}..HEAD", release_branch)])
    .output()
    .await?;
let n: u64 = std::str::from_utf8(&unpushed.stdout)?.trim().parse()?;
if n > 0 {
    state.invalidate_step(STEP_GIT_PUSH);
}
```

### 5.2 BUG B — `CARGO_INCREMENTAL` vs sccache drift (🟠)

**Observed**: same release, same day.  pre-push hook failed with
`sccache: incremental compilation is prohibited: Unset CARGO_INCREMENTAL
to continue.`

**Root cause**: two unrelated configs set conflicting policies:

- `.cargo/config.toml:24` — `rustc-wrapper = "sccache"` (applies globally).
- `just/shared.just:15` — `export CARGO_INCREMENTAL := "1"` (exported into
  every `just` recipe's environment).

sccache refuses to wrap rustc when `CARGO_INCREMENTAL=1` because
incremental artifacts are non-cacheable (sccache source:
`src/compiler/rust.rs`).  When `just ship` spawned `git push`, the
pre-push hook inherited `CARGO_INCREMENTAL=1` from the just env, and
every cargo-invoking gate (rustdoc, deny, nextest) died.

**Status**: Worked around in commit `420e82387` by explicitly pairing
`CARGO_INCREMENTAL=0` with `RUSTC_WRAPPER=sccache` in the pipeline's
`global_env` so every subprocess (including `git`) inherits the correct
setting.  The underlying drift between `shared.just` and
`.cargo/config.toml` is **not yet root-caused** — see Proposed Refactor
§ 6.4.

### 5.3 BUG C — `rust-script` serves stale binary cache (🟡)

**Observed**: same release.  My ci-pipeline.rs fix appeared to have no
effect through several attempts because rust-script's binary cache under
`~/Library/Caches/rust-script/binaries/release/` had 10+ stale compiled
versions of `ci-pipeline.rs` and was picking up a pre-fix one.

**Detection**: Added a
`[ci-pipeline][sccache-fix]` diagnostic eprintln to the source.  If the
line appeared in output, the binary was fresh.  If absent, stale cache.

**Fix**: Two options documented under § 6.5.

---

## 6. Proposed Refactor — Detailed

Five phases, ordered by risk × reward.  Each phase is a single atomic
commit.

### 6.1 Phase 1 — shift `cargo vet` to pre-push (Commit 1)

**Goal**: Close Gap 1.  Never again submit a PR that fails `cargo vet`.

**Files touched**: `scripts/hooks/_lint_pre_push.sh` only.

**Diff**:

```diff
@@ -100,6 +100,11 @@ spawn "deny"         cargo deny check --hide-inclusion-graph
 spawn "tests"        cargo nextest run --workspace --all-targets --all-features --no-run --hide-progress-bar
 spawn "file-size"    bash scripts/ci/check_file_size_policy.sh

+# Supply-chain audit: every crate-version must be covered by an import,
+# an entry in supply-chain/audits.toml, or a time-limited exemption in
+# supply-chain/config.toml.  Soft-skip when cargo-vet is absent (new
+# contributors are hinted to install via `just install-dev-tools`).
+if command -v cargo-vet >/dev/null 2>&1; then
+    spawn "vet" cargo vet check --locked
+fi
+
 # Cross-platform: Windows compile-check via cargo-xwin.
 if command -v cargo-xwin >/dev/null 2>&1; then
     spawn "check-windows" just check-windows
@@ -146,6 +151,7 @@ fi
 # ── Optional-tool hint ─────────────────────────────────────────────────
 missing=()
 command -v typos >/dev/null 2>&1 || missing+=("typos-cli")
+command -v cargo-vet >/dev/null 2>&1 || missing+=("cargo-vet")
 command -v reuse >/dev/null 2>&1 || missing+=("reuse (pipx install reuse)")
```

**Test plan**:

1. `cargo vet check --locked` — confirm passes on current HEAD.
2. `just lint-pre-push` — the new `vet` gate appears in the per-job grid.
3. Introduce a deliberate failure (e.g. transient exemption removal),
   confirm pre-push aborts.

**Runtime cost**: 1–2 s.  Pre-push budget stays under 60 s warm.

**Risk**: Zero.  Soft-skip pattern matches `typos` / `reuse`.

### 6.2 Phase 2 — add doc tests to pre-push (Commit 2)

**Goal**: Close Gap 3.  Never merge a PR with broken `/// ```rust` blocks.

**Diff**:

```diff
@@ -95,6 +95,8 @@ spawn "lint-ci"      just lint-ci
 spawn "lint-prod"    just lint-prod
 spawn "lint-tests"   just lint-tests
 spawn "fmt"          cargo fmt --all -- --check
 spawn "rustdoc"      env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --all-features --no-deps
+spawn "doc-tests"    env RUSTDOCFLAGS=-Dwarnings cargo test --doc --workspace --all-features
 spawn "deny"         cargo deny check --hide-inclusion-graph
```

**Test plan**: Add a deliberately-failing doctest, confirm pre-push aborts
with `doc-tests` in the failed list.

**Runtime cost**: 10–30 s warm.  Total pre-push budget moves from 45 s to
~60 s — still within the flow threshold.

**Risk**: Low.  Pre-push may need 1 tick more time.  Nextest timing under
`--no-run` + cargo-doc overlap should amortize.

### 6.3 Phase 3 — fix ship resumable push-skip bug (Commit 3)

**Goal**: Close Bug A.  `just ship` re-run after a local commit always
pushes the new HEAD.

**Files touched**: `scripts/ci/ci-pipeline.rs`.

**Patch target** (around line 1341, at start of step 11 execution):

```rust
// Invariant: a push step is only "complete" if origin is at HEAD.
// If the developer has committed locally since the prior push (e.g.
// to fix a CI-detected issue such as a cargo-vet audit), the cached
// state would otherwise skip the push silently.  Re-run unconditionally
// whenever HEAD is ahead of the target remote ref.
let n_unpushed = count_unpushed_commits(&release_branch).await?;
if n_unpushed > 0 {
    println!(
        "↻ {} unpushed commit(s) on HEAD — re-running step 11",
        n_unpushed.to_string().yellow()
    );
    state.invalidate_step(STEP_GIT_PUSH);
}

execute_step_with_tracking(state, STEP_GIT_PUSH, || async {
    // ... existing push logic
}).await?;
```

Plus one helper:

```rust
async fn count_unpushed_commits(remote_branch: &str) -> Result<u64> {
    let out = Command::new("git")
        .args(["rev-list", "--count",
               &format!("origin/{}..HEAD", remote_branch)])
        .output()
        .await?;
    if !out.status.success() {
        // Remote branch doesn't exist yet — push creates it.
        return Ok(1);
    }
    Ok(std::str::from_utf8(&out.stdout)?.trim().parse().unwrap_or(1))
}
```

**Test plan**: reproduce the v0.5.71 sequence.  Run ship, let it push,
then add a commit, re-run ship.  Before fix: exits in 2 s without
pushing.  After fix: re-runs step 11, push lands new HEAD.

**Risk**: Low.  The `git rev-list` check is read-only; the invalidation is
targeted to step 11 only.

### 6.4 Phase 4 — resolve `CARGO_INCREMENTAL` vs sccache drift (Commit 4)

**Goal**: Close Bug B at root.  One authority decides the local cache
policy; no more surprises when a pipeline inherits the "wrong" one.

**Options compared**:

| Option | Change | Pros | Cons |
|---|---|---|---|
| **A** — flip `shared.just` to `CARGO_INCREMENTAL=0` | `just/shared.just:15` | sccache covers nearly everything incremental would; one source of truth; matches CI env | First cold compile slightly slower (~5–10 %) |
| **B** — remove `rustc-wrapper` from `.cargo/config.toml`, enable sccache only in `ci-pipeline.rs` | `.cargo/config.toml:24` | Keeps local `cargo build` feeling snappy with incremental | sccache benefit lost for daily `cargo build`/test; defeats its purpose |
| **C** — keep the pipeline-level pairing (status quo) | none | Zero code change | Drift remains; the sccache server env-caching bug lurks |

**Recommendation**: **Option A**.  At UFFS workspace size (polars-heavy,
~700 dependent crates resolved), sccache-warm is consistently faster than
incremental-warm because incremental only helps crates you just edited,
while sccache hits across the entire graph.  Benchmarked on this machine:

- sccache-warm, `CARGO_INCREMENTAL=0`, `cargo build --workspace`:
  ~42 s after touching one file in `uffs-core`.
- sccache-off, `CARGO_INCREMENTAL=1`: ~68 s same scenario.

**Diff**:

```diff
--- a/just/shared.just
+++ b/just/shared.just
@@ -12,7 +12,11 @@ export TERM := "xterm-256color"
 export COLORTERM := "truecolor"
 export CARGO_TERM_COLOR := "always"
-export CARGO_INCREMENTAL := "1"
+# Disabled because `.cargo/config.toml` sets `rustc-wrapper = "sccache"`
+# globally.  sccache refuses to wrap rustc when CARGO_INCREMENTAL=1
+# because incremental artifacts are non-cacheable.  sccache-warm is
+# consistently faster than incremental-warm at this workspace size
+# (see docs/architecture/dev-flow.md § 6.4).
+export CARGO_INCREMENTAL := "0"
 export CARGO_NET_RETRY := "3"
 export CARGO_HTTP_TIMEOUT := "30"
 export CARGO_HTTP_MULTIPLEXING := "true"
```

And simplify `scripts/ci/ci-pipeline.rs` (remove the now-redundant
workaround):

```diff
-        // sccache refuses to wrap rustc when CARGO_INCREMENTAL=1 ...
-        let sccache_available = !no_sccache && command_exists("sccache");
-        if sccache_available {
-            global_env.push(("RUSTC_WRAPPER".into(), "sccache".into()));
-            global_env.push(("CARGO_INCREMENTAL".into(), "0".into()));
-            if verbose { eprintln!("[ci-pipeline][sccache-fix] ..."); }
-        }
+        // sccache integration: CARGO_INCREMENTAL=0 is enforced by
+        // just/shared.just (see docs/architecture/dev-flow.md § 6.4).
+        let sccache_available = !no_sccache && command_exists("sccache");
+        if sccache_available {
+            global_env.push(("RUSTC_WRAPPER".into(), "sccache".into()));
+        }
```

**Test plan**:

1. Fresh clone + `cargo build --workspace` cold — record time.
2. Touch one file + rebuild — record time.  Should match the ~42 s above.
3. `just ship -v` through to push.  Pre-push hook all green.
4. Run a direct `git push` from shell with no env overrides.  Pre-push
   hook all green (proof that the drift is gone).

**Risk**: **Medium**.  Any `just` recipe that silently assumed
incremental compilation may feel slightly different.  Rollback is a
one-line revert.

### 6.5 Phase 5 — eliminate `rust-script` stale cache (Commit 5, optional)

**Goal**: Close Bug C.  Make `ci-pipeline.rs` changes take effect on the
next `just ship` unconditionally.

**Two approaches**:

#### 6.5.A — add `rust-script --clear-cache` to `ship-fresh`

Trivial one-line addition to `just/workflow.just:79`:

```diff
 ship-fresh *ARGS:
     @printf "..."
+    @rust-script --clear-cache >/dev/null 2>&1 || true
     rust-script scripts/ci/ci-pipeline.rs ship --fresh {{ ARGS }}
```

Pros: zero restructuring.
Cons: blows 3.7 GB of rust-script cache on every ship-fresh — other
scripts recompile too.  Catastrophic if multiple scripts.

#### 6.5.B — promote `ci-pipeline.rs` to a real cargo binary (RECOMMENDED)

Move `scripts/ci/ci-pipeline.rs` → `scripts/ci-pipeline/src/main.rs`
with a dedicated `scripts/ci-pipeline/Cargo.toml`.  Add to workspace
`members` so `cargo build -p ci-pipeline` compiles it like any other
crate.

```diff
 # Cargo.toml (workspace)
 [workspace]
 members = [
     "crates/uffs-broker",
     ...
+    "scripts/ci-pipeline",
 ]
```

Then `just/workflow.just`:

```diff
 ship *ARGS:
-    rust-script scripts/ci/ci-pipeline.rs ship {{ ARGS }}
+    cargo run --release -p ci-pipeline -- ship {{ ARGS }}
```

Pros: standard cargo fingerprinting catches source changes; no cache
ambiguity; ci-pipeline can use workspace dependencies cleanly;
`cargo check` in IDE picks it up.
Cons: Changes the ship invocation string (muscle memory), requires one
compile during `just ship` if source changed (seconds).

**Recommendation**: 6.5.B.  Cost: ~30 min of restructuring + updating the
docstrings.  Eliminates an entire class of Heisenbugs.

---

## 7. Post-Refactor Budget Targets

| Tier | Before | After | Change |
|------|--------|-------|--------|
| T1 pre-commit | 15–25 s | 15–25 s | none |
| T2 pre-push | 23–45 s | 35–60 s | +12–15 s (`cargo vet` + doc tests) |
| T3 Tier 1 CI | 8–12 min | 8–12 min | none |
| T4 Tier 2 CI | 60–90 min | 60–90 min | none |

Post-refactor pre-push budget is still within the "developer flow"
threshold (&lt; 60 s ideal, &lt; 90 s tolerable).  The additional work is
strictly more caught-locally bugs.

---

## 8. Non-Goals (intentionally NOT changing)

- **Not running the full test suite at pre-push.**  2–3 min is too
  long.  Keep `--no-run` plus add `--doc` (Phase 2).
- **Not shifting CodeQL left.**  Wrong layer; 400 MB download.
- **Not shifting coverage / miri / udeps left.**  Minutes-scale.
  `cargo machete` is a fast alternative for udeps's subset, deferred to
  a future Phase 6 if needed.
- **Not duplicating CI gates at pre-push out of paranoia.**  Clippy at
  both pre-commit AND pre-push is intentional defense-in-depth for
  `--no-verify` bypass; other duplication would be noise.
- **Not removing the existing exemption-based `cargo vet` entries.**
  388 exemptions is the established posture for a small-team repo;
  wholesale auditing every transitive dep is a 100+ hour project.
  Keep the current mix: real audits for new crates (per the "no
  suppression hacks" rule applied to new deps), exemptions grandfathered
  for established ecosystem crates.

---

## 9. Industry Comparison

Surveyed pre-commit / pre-push posture of public Rust repos (sourced
from each repo's `.pre-commit-config.yaml` / `Makefile.toml` /
`scripts/hooks/`):

| Repo | pre-commit | pre-push | Notes |
|---|---|---|---|
| **tokio** | fmt + clippy + typos | none | Heavy CI; devs run `cargo test` manually. |
| **rustls** | fmt only | none | Minimal local; heavy CI. |
| **polars** | ruff + black + minimal Rust | none | Mixed Python/Rust project. |
| **clap** | fmt + clippy | none | Heavy CI matrix. |
| **bytes** | fmt | none | Tiny; CI-heavy. |
| **UFFS (current, post-W5/L1)** | clippy trio + file-size + typos + reuse + taplo | clippy trio + rustdoc + doctests + deny + test-compile + smoke + **xwin clippy (`lint-ci-windows`)** + cargo-vet (when dep-changed) + commit-subjects + file-size + typos + reuse | Deeper local gates than any above; pre-push now strict-lints Windows-gated code via cargo-xwin clippy. |
| **UFFS (initial baseline, 2025)** | clippy trio + xwin + file-size + typos + reuse + taplo | clippy trio + rustdoc + deny + test-compile + xwin + file-size + typos + reuse | Pre-Phase-2 baseline kept here for historical reference — xwin lived at pre-commit and was a compile-only check. |

**Takeaway**: UFFS is already ahead of typical Rust OSS posture on local
coverage.  The shift-left work here is about closing two specific holes,
not adopting a new philosophy.

---

## 10. Execution Plan (commits, ranked)

| # | Title | Phase | Effort | Risk | ROI |
|---|---|---|---|---|---|
| 1 | `shift-left(pre-push): cargo vet check --locked` | § 6.1 | 5 min | zero | **highest** — prevents today's exact failure class |
| 2 | `shift-left(pre-push): cargo test --doc` | § 6.2 | 5 min | low | high |
| 3 | `fix(ship): invalidate git-push step when HEAD is ahead of origin` | § 6.3 | 30 min | low | high — removes v0.5.71-class pain |
| 4 | `fix(shared.just): CARGO_INCREMENTAL=0 to match sccache config` | § 6.4 | 15 min + validation | medium | medium — root-causes Bug B |
| 5 | `refactor(scripts): promote ci-pipeline.rs to a workspace binary` | § 6.5.B | 90 min | medium | medium — eliminates Heisenbug class |

Commits 1 + 2 + 3 together form the "fast pass" — ~45 min of
surgery, addresses the three highest-pain items from the v0.5.71
retrospective.

---

## 11. Open Questions

- **Should `cargo machete` join pre-push?**  ~200 ms cost, catches
  Dependabot-orphan deps.  Defer until after Commits 1–3 land and we see
  whether the existing pre-push is already painful enough.
- **Should pre-push grow a `--fast` mode?**  `just lint-pre-push-fast`
  that skips the clippy trio on the grounds that pre-commit already ran
  them?  Probably not — `--no-verify` bypass makes the backstop
  worthwhile.
- **Windows pre-push parity.**  The Unix `scripts/hooks/pre-push` + shell
  script pattern doesn't extend cleanly to Windows.  Today Windows
  contributors implicitly rely on CI.  Acceptable while maintainer is
  the primary Windows user, reassess if contributor base grows.

---

## 12. References

- `.github/workflows/ci.yml` — Tier 1 CI (PR path).
- `.github/workflows/tier-2.yml` — Tier 2 nightly.
- `.github/workflows/codeql.yml` — CodeQL Rust SAST.
- `.github/workflows/cargo-vet-refresh.yml` — weekly supply-chain refresh.
- `scripts/hooks/_lint_fast.sh` — pre-commit gate bundle.
- `scripts/hooks/_lint_pre_push.sh` — pre-push gate bundle.
- `scripts/ci/ci-pipeline.rs` — ship pipeline (Phase 1 validation +
  Phase 2 release PR open).
- `just/shared.just` — shared env / flag exports across all just recipes.
- `.cargo/config.toml` — `rustc-wrapper = "sccache"` declaration.
- `docs/architecture/security/supply-chain-posture.md` — supply-chain
  posture (complementary doc).

---

## 13. Revision history

- **2026-04-23** — Initial doc; captures v0.5.71 ship post-mortem.
