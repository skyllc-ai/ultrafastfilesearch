<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.

UFFS — Windows Clippy + Linux Native Cross-Check Plan
-->

# Windows Clippy + Linux Native Cross-Check Plan (v1)

> Sibling document to
> [`dev-flow-implementation-plan.md`](dev-flow-implementation-plan.md)
> (merge-time CI gates) and
> [`release-automation-plan.md`](release-automation-plan.md)
> (post-merge concerns).  This plan owns **making the super-strict
> lint flow actually strict on every target we ship to** —
> currently the strict `clippy` stack only runs natively on macOS
> and in Docker for Linux; the Windows gate is `cargo check` only
> (type-check, not lint), and no native macOS → Linux path exists.

## 0. TL;DR

| Aspect | Today | Target |
|---|---|---|
| Strict clippy flag stack | `prod_flags` + `test_flags` + `-D warnings` (see `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:28-30`) | **Unchanged** — zero flag churn; only the surface it runs on changes |
| macOS host gate | `lint-prod` + `lint-tests` + `lint-ci` — strict ✅ | **Unchanged** |
| Linux-in-Docker gate | `lint-ci-linux` — `-D warnings` ✅ | **Unchanged** (stays the authoritative Linux gate) |
| Linux native from macOS | Blocked (`zstd-sys` `build.rs` needs a C cross-compiler Apple `cc` can't provide) | **`cargo-zigbuild`** accelerator — handles C deps via `zig cc`; Docker path stays authoritative |
| Windows local (macOS → Win) | `cargo xwin check` only (type-check, no lint) — see `@/Users/rnio/Private/Github/UltraFastFileSearch/just/dev.just:52-55` | **`cargo xwin clippy`** with the same three-pass flag stack (prod / tests / CI) |
| Windows CI | `cargo check` on `windows-latest` — see `@/Users/rnio/Private/Github/UltraFastFileSearch/.github/workflows/pr-fast.yml:436-458` | **`cargo clippy -- -D warnings`** on `windows-latest` |
| Windows clippy backlog (prod: `--lib --bins`) | **93** errors (measured W0, 2026-04-24) | **0** after Phase W2 |
| Windows clippy backlog (tests: `--tests`) | **1,342** errors (measured W0) | **0** after Phases W3 – W5 |
| Windows clippy backlog (ci: `--all-targets -D warnings`) | **1,346** errors (measured W0) | **0** after W5 |
| Blanket `#[allow]` added | — | **0** (target for `#[expect(lint, reason)]` < 20 total, mostly cast-family FFI sites) |
| Gate integration model | 4-layer (IDE / pre-commit / pre-push / PR CI) | Unchanged model; **two new authoritative gates added at PR CI**, pre-push budget preserved |

**Central thesis**:

> *Every line we ship to every target we support must be held to
> the same clippy stack.  A lint gate that only covers the host
> platform is not a strict lint gate — it is a host-platform
> lint gate.*

## 1. Goals and non-goals

### 1.1 Goals (in scope for this plan)

- **G1**: Converge the Windows clippy backlog (baseline TBD in W0.3) to **zero** without a single blanket `#[allow]`.  The only acceptable suppression is `#[expect(lint, reason = "prose")]` on an individual item, and the target count for those is **< 10** across the whole Windows surface.
- **G2**: Upgrade `check-windows` (local) and `windows-check` (PR CI) from `cargo check` → `cargo clippy` with the full three-pass strict flag stack, then gate on it via the existing `required` aggregator.
- **G3**: Add a **native** macOS → Linux cross-check path via `cargo-zigbuild` that compiles `zstd-sys` (and any other C-dep crates) without Docker, giving developers a fast local Linux sweep.
- **G4**: Keep the existing `lint-ci-linux` Docker path as the authoritative Linux gate (it mirrors the CI image exactly).  Zigbuild is a developer accelerator, not a replacement.
- **G5**: Document per-phase acceptance criteria, escalation policy, and the "new Windows code must be lint-clean from day 1" rule.

### 1.2 Non-goals (out of scope — explicitly deferred)

- **NG1**: Running `cargo xwin clippy` in **pre-commit**.  Too expensive (~20–60 s warm) for the T1 sub-2-s budget.  Stays at pre-push + CI.
- **NG2**: Changing the clippy flag stack itself.  `common_flags` / `prod_flags` / `test_flags` at `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:28-30` stay exactly as defined.  Any flag tweak is a separate PR.
- **NG3**: ARM64 cross-check (macOS ARM, Linux ARM, Windows ARM).  The workspace only ships x86_64 today; ARM is out of scope until AFTER the amd64 cleanup lands.
- **NG4**: Cross-target `miri` / `cargo-udeps` / coverage.  Those stay single-target in Tier 2 nightly.
- **NG5**: Any behavioural change to Windows code beyond what a specific clippy rule requires.  Every fix is surgical; no drive-by refactors.
- **NG6**: Replacing Docker with zigbuild as the *authoritative* Linux gate.  Zigbuild uses a different C compiler than the CI's glibc; keeping Docker as authoritative eliminates that drift risk.

## 2. Problem characterisation

### 2.1 Item A — Windows gate is check-only, not clippy

**Current local gate** (`@/Users/rnio/Private/Github/UltraFastFileSearch/just/dev.just:52-55`):

```sh
cargo xwin check --workspace --all-targets --all-features --target x86_64-pc-windows-msvc
```

Catches type errors, missing imports, wrong `cfg` — *not* clippy lints.

**Current CI gate** (`@/Users/rnio/Private/Github/UltraFastFileSearch/.github/workflows/pr-fast.yml:436-458`):

```yaml
windows-check:
  name: Windows compile check
  runs-on: windows-latest
  needs: [classify, sanity]
  if: needs.classify.outputs.code == 'true'
  timeout-minutes: 25
  steps:
    - run: cargo check --workspace --all-targets --all-features --locked
```

Also check-only.  Native, so it catches real Windows compile issues (cfg mismatches, missing `windows` crate feature activations), but not strict lints.

**Scope of Windows-only code**: 346 `#[cfg(windows)]` / `cfg(target_os = "windows")` matches across 88 files.  Heaviest files:

- `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-mft/src/io/readers/mod.rs` (21 matches)
- `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-broker/src/broker.rs` (19 matches)
- `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-mft/src/platform/system.rs` (18 matches)
- `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-mft/src/io/readers/parallel/mod.rs` (12 matches)
- `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-mft/src/reader/index_read.rs` (12 matches)
- All of `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-mft/src/commands/windows/` (6 files)

### 2.2 Item B — macOS → Linux native is blocked by `zstd-sys`

- **`zstd-sys`** compiles a bundled C library via `build.rs` using `cc-rs`.
- From macOS, Apple's `cc` cannot produce Linux x86_64 ELF objects without an explicit cross-compiler.
- Today this is bypassed by running the Linux lint pass **inside Docker** (`@/Users/rnio/Private/Github/UltraFastFileSearch/just/test.just:82-107`).  It works but:
  - Requires a Docker runtime (Docker Desktop / colima / OrbStack / Podman).
  - Cold first-run is slow (~5–10 min to fetch the `rust:latest` image and compile from scratch).
  - Subsequent runs use a Docker volume cache (`uffs-linux-target`) — warm runs are ~60–90 s.
  - Blocks the `check-all-targets` recipe for anyone without Docker installed.

### 2.3 The hidden third gap (worth calling out)

Native CI Windows clippy has never been gated either.  Fixing Item A *locally* but leaving CI on `cargo check` leaves us one `git push --no-verify` away from drift.  This plan treats local + CI as a single unit — both upgrade in Phase W4.

## 3. Options analysis: macOS → Linux native

| Option | Handles `zstd-sys`? | Effort | Speed (warm) | Developer UX | Verdict |
|---|---|---|---|---|---|
| **A** — Keep Docker-only (`lint-ci-linux` as-is) | ✓ | 0 | 60–90 s | Requires Docker runtime | **Keep as authoritative fallback** |
| **B** — `cross` (cross-rs/cross) | ✓ (uses Docker under the hood) | Low | Similar to Docker | Simpler CLI than raw Docker | Strictly worse than A (still Docker) |
| **C** — Install `x86_64-linux-musl` C toolchain via Homebrew (`musl-cross` formula + `CC_x86_64_unknown_linux_musl` env) | Partial | Medium | Fast | Brittle; musl ≠ glibc (distinct lint profile, distinct platform cfgs) | **Reject** (produces musl, not the gnu target our CI uses) |
| **D** — **`cargo-zigbuild`** | ✓ (zig cc handles glibc + libc cross, including C deps) | Low | ~10–15 s overhead vs native | `brew install zig && cargo install cargo-zigbuild`; no Docker needed | **Adopt as accelerator** |
| **E** — Defer (accept no native option) | — | 0 | — | Docker required forever | **Reject** — gap stays open |

**Chosen**: **A + D in parallel**.  Keep Docker as the authoritative Linux gate (mirrors CI image exactly; runs `rust:latest`); add zigbuild as an optional developer accelerator for fast inner-loop Linux sweeps on macOS.

**Why `zig cc` works where Apple's `cc` doesn't**: Zig ships a hermetic, multi-platform C compiler with every supported libc bundled.  `cargo zigbuild --target x86_64-unknown-linux-gnu` sets `CC_x86_64_unknown_linux_gnu=zig cc -target x86_64-linux-gnu`, which `cc-rs` (the crate that drives `zstd-sys`'s `build.rs`) respects.  No external toolchain install beyond `brew install zig` + `cargo install cargo-zigbuild`.

## 4. Phased execution plan

### Phase W0 — Baseline + categorisation (½ day, solo, no code change)

- **W0.1** — Run `cargo xwin clippy --workspace --lib --bins --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ prod_flags }}` and capture full output to `_trash/w0-baseline/prod.txt` (gitignored).
- **W0.2** — Same with `--tests` + `test_flags`.
- **W0.3** — Same with `--all-targets` + `-D warnings`.  This is the count that matches the `windows-check` CI job's authoritative scope.
- **W0.4** — Bucket lints by rule (`awk -F'`' '/warning: .*`.*`/ {print $2}' | sort | uniq -c | sort -rn`) and fill the table below.
- **W0.5** — Tier each rule into T1 / T2 / T3 per § 4.1.  Tiers are rescoped if W0.3 reveals a rule we haven't seen before.

**Baseline results** (measured 2026-04-24 on `chore/misc-fixes-2026-04-24` @ `a83f3543e`, `x86_64-pc-windows-msvc`, `rustc 1.96.0-nightly`):

| Pass | Scope | Flag stack | Errors | Warm duration |
|---|---|---|---|---|
| prod | `--lib --bins` | `prod_flags` | **93** | 4 s |
| tests | `--tests` | `test_flags` | **1,342** | 6 s |
| ci | `--all-targets` | `-D warnings` | **1,346** | 7 s |

**Architectural signal**: Windows-only *production* code is nearly lint-clean (93 errors, ~15 rule families — mostly FFI-safety annotations and pedantic nits).  The backlog lives in **test + bench code** (1,342).  This is a significant re-shaping of the plan: we can land the prod gate in a week, then chip away at the test tail independently.

**Top rule distribution** (union across all three passes; showing top 30 of ~60 unique rules):

| Lint rule | Total | In prod? | Tier | Fix shape |
|---|---:|---|---|---|
| `clippy::cast_possible_truncation` | 278 | no | T2 | `as u32` → `u32::try_from(…)?` |
| `clippy::min_ident_chars` | 241 | yes (9) | T1 | rename `i` / `x` to descriptive (check `clippy.toml` config) |
| `clippy::indexing_slicing` | 188 | yes (2) | T2 | `a[i]` → `.get(i).ok_or(…)?` or justify `#[expect]` |
| `clippy::borrow_as_ptr` | 177 | yes (9) | T1 | `&x as *const _` → `std::ptr::from_ref(&x)` |
| `clippy::unseparated_literal_suffix` | 136 | yes (12) | T1 | `1u32` → `1_u32` |
| `clippy::std_instead_of_core` | 124 | no | T1 | `std::mem::size_of` → `core::mem::size_of` |
| `clippy::doc_markdown` | 95 | no | T1 | backtick bare identifiers in docs |
| `clippy::ref_patterns` | 83 | no | T3 | `ref x @` → `&x` destructure (pattern semantics) |
| `clippy::default_numeric_fallback` | 74 | no | T1 | `let x = 0` → `let x: u32 = 0` |
| `clippy::undocumented_unsafe_blocks` | 72 | yes (10) | T2 | write prose `SAFETY:` comments (FFI justification) |
| `clippy::missing_docs_in_private_items` | 70 | yes (3) | T1 | add rustdoc |
| `clippy::missing_errors_doc` | 59 | no | T1 | add `# Errors` rustdoc section |
| `clippy::semicolon_outside_block` | 57 | no | T1 | `{ …; }` → `{ … };` |
| `clippy::shadow_reuse` | 55 | no | T2 | rename shadowed binding |
| `clippy::cognitive_complexity` | 54 | yes (2) | T3 | structural split of complex fns |
| `clippy::cast_possible_wrap` | 48 | no | T2 | signed-wrap concern, per-site |
| `clippy::option_if_let_else` | 46 | no | T2 | reshape `Option` handling |
| `clippy::let_underscore_untyped` | 44 | yes (8) | T1 | annotate type |
| `clippy::let_underscore_must_use` | 44 | yes (8) | T1 | `drop(x)` or handle result |
| `clippy::wildcard_imports` | 40 | no | T1 | expand `use foo::*` |
| `clippy::items_after_statements` | 40 | no | T1 | hoist `fn` above first stmt |
| `clippy::as_pointer_underscore` | 39 | no | T2 | annotate target pointer type |
| `clippy::shadow_unrelated` | 38 | no | T2 | rename |
| `clippy::cast_sign_loss` | 38 | no | T2 | per-site cast family |
| `clippy::cast_precision_loss` | 38 | no | T2 | per-site cast family |
| `clippy::too_many_lines` | 36 | no | T3 | split fns (threshold=150 in `clippy.toml`) |
| `clippy::multiple_unsafe_ops_per_block` | 35 | yes (5) | T3 | split unsafe blocks with prose each |
| `clippy::manual_checked_ops` | 34 | no | T1 | use `checked_*` API |
| `clippy::float_arithmetic` | 32 | no | T3 | justify each use (often legit for perf) |
| `clippy::collapsible_if` | 28 | yes (2) | T1 | merge nested `if` |
| **top 30 total** | **~2,076** | 70 | — | **~88 % of the 1,346 authoritative CI count** (union counts > individual because many rules fire across multiple passes) |

Full per-rule and per-file distribution is captured in `_trash/w0-baseline/` (gitignored, not checked in).

**Tier split for the 93 prod lints** (the workstream we start with):

- **T1 (mechanical, ~70 of 93)**: `unseparated_literal_suffix` (12), `borrow_as_ptr` (9), `min_ident_chars` (9), `let_underscore_untyped` (8), `let_underscore_must_use` (8), `trivially_copy_pass_by_ref` (6), `print_stderr` (4), `missing_docs_in_private_items` (3), `indexing_slicing` (2), `collapsible_if` (2), `cognitive_complexity` (2), `print_stdout` (2), `uninlined_format_args` (1)
- **T2 (targeted, ~15 of 93)**: `undocumented_unsafe_blocks` (10), `indexing_slicing` (2), `cognitive_complexity` (2)
- **T3 (semantic, ~5 of 93)**: `multiple_unsafe_ops_per_block` (5)

**Acceptance** (W0 completion): per-rule breakdown table populated above.  Next: execute Phase W1 (infrastructure) then W2 (prod cleanup).

#### 4.1 Tier definitions (stable across phases)

- **T1 (mechanical)**: deterministic edits.  Rename, add `#[must_use]`, add rustdoc `# Errors` / `# Panics` sections.  Low risk; the lint tells you exactly what to do.
- **T2 (targeted)**: per-site reasoning required but change is local.  Cast conversions (`as u32` → `u32::try_from`), closure simplifications, scope tightening, iterator method choice.
- **T3 (semantic)**: touches design surface.  `significant_drop_tightening`, `significant_drop_in_scrutinee`, `await_holding_lock` — these expose real concurrency hazards and each fix needs a regression test.

### Phase W1 — Infrastructure: xwin clippy recipes (1 day)

- **W1.1** — Add three new recipes in `@/Users/rnio/Private/Github/UltraFastFileSearch/just/test.just` (sibling to `lint-prod` / `lint-tests` / `lint-ci`):

  ```just
  # Ultra-strict Windows production lint (cargo-xwin).  Mirrors lint-prod.
  lint-prod-windows:
      cargo xwin clippy --workspace --lib --bins --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ prod_flags }}

  lint-tests-windows:
      cargo xwin clippy --workspace --tests --all-features --target x86_64-pc-windows-msvc --no-deps -- {{ test_flags }}

  lint-ci-windows:
      cargo xwin clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc --no-deps -- -D warnings
  ```

- **W1.2** — Update `lint-all` + `check-all-targets` to call the new recipes.
- **W1.3** — Decide naming: either rename `check-windows` → `lint-windows` (canonical) with a deprecation redirect, or keep `check-windows` as the fast type-check entry-point and make `lint-ci-windows` the strict entry.  Default: keep both; `check-windows` stays as a cheap fallback.
- **W1.4** — Measure warm xwin clippy duration for each pass; record in this doc's § 5 table below.

**Acceptance** (2026-04-24, ✅ met): `just lint-prod-windows` + `just lint-tests-windows` + `just lint-ci-windows` run and report lint counts matching the W0.3 baseline (93 / 1,342 / 1,346 ±1 — the ±1 offset is the `error: could not compile` summary line cargo wraps each aborted crate in).  No semantic change.  Raw outputs captured in `_trash/w1-measurements/` (gitignored).

### Phase W2 — Prod gate (3–5 days) **← starts immediately after W1**

The W0 baseline reveals prod is only 93 errors across ~15 rule families — an order of magnitude smaller than the test backlog.  Fixing prod first gives us an early win: `just lint-prod-windows` becomes a green gate that blocks *new* Windows-gated production regressions while W3 – W5 chip away at the existing test backlog.

- **W2.1** — T1 mechanical (~70 of the 93): one PR per crate.  Conventional commits by scope:
  - `style(broker): unseparated_literal_suffix + borrow_as_ptr cleanup`
  - `refactor(mft): let_underscore_must_use + trivially_copy_pass_by_ref`
  - `docs(daemon): missing_docs_in_private_items on FFI bindings`
  - etc.
- **W2.2** — T2 targeted (~15 of 93): `undocumented_unsafe_blocks` (10) needs prose `SAFETY:` comments justifying each `unsafe` block's invariants (FFI signature, buffer lifetime, alignment — usually all three).  `indexing_slicing` (2) and `cognitive_complexity` (2) per-site.
- **W2.3** — T3 semantic (~5 of 93): `multiple_unsafe_ops_per_block` (5) splits `unsafe { read(); transmute(); }` into discrete `unsafe {}` blocks, each with its own SAFETY comment.  Touches FFI design surface; regression tests for each split.
- **W2.4** — **Wire `lint-prod-windows` into pre-push and PR CI**.  This is the first concrete authoritative-gate upgrade: new Windows production code is now lint-clean from day one.  W1.4 measurement shows prod pass is **2 s warm** — pre-push bundle gains negligible time (current 50 s bundle → ~52 s).  PR CI job adds a sub-minute step.

**Acceptance after W2**: `just lint-prod-windows` exits 0.  PR Fast CI runs Windows clippy on `--lib --bins`.  Production Windows regressions are now authoritatively gated.  Test backlog (~1,249 remaining) is UNBLOCKED and parallelisable.

### Phase W3 — Test T1 mechanical (1 week)

~900 of the remaining ~1,249 test lints are mechanical.  Phased by rule family, not by crate, because the rule-level PR is smaller and reviewable.

- **W3.1** — `min_ident_chars` (241 total, mostly tests): inspect `clippy.toml` for the `min-ident-chars-threshold` setting.  If tests legitimately use `i` / `x` in tight loops, we relax *in `clippy.toml`* (workspace-wide knob, reviewed), not via `#[expect]` spam.  Otherwise rename — 241 cases are tedious but mechanical.
- **W3.2** — `cast_possible_truncation` in tests (139): mostly `len as u32` style.  `u32::try_from(len).expect("test fixture fits in u32")` is the idiom; the `expect` is justified because tests author-controls the inputs.
- **W3.3** — `borrow_as_ptr` (168 in tests): `&x as *const _` → `std::ptr::from_ref(&x)` or `::from_mut(&mut x)`.  Deterministic.
- **W3.4** — `unseparated_literal_suffix` (124 in tests): `1u32` → `1_u32`.  `sed`-safe at the file level.
- **W3.5** — `std_instead_of_core` (124): workspace-wide decision — do we actually want `core` preference or is this a false-positive?  If we want it, mechanical fix; if we don't, relax in workspace.lints.
- **W3.6** — `doc_markdown` (95): backtick bare identifiers.  Mechanical.
- **W3.7** — `default_numeric_fallback` (74), `missing_docs_in_private_items` (67 test), `missing_errors_doc` (59), `semicolon_outside_block` (57), `let_underscore_*` (80), `wildcard_imports` (40), `items_after_statements` (40), `uninlined_format_args` (21), `collapsible_if` (26 test), etc.  One PR per rule family or per natural grouping.

**Acceptance after W3**: ≥ 65 % of remaining test backlog cleared.  Only T2 + T3 categories left (~350 lints).

### Phase W4 — Test T2 targeted (1–2 weeks)

- **W4.1** — `indexing_slicing` (186 in tests): every `a[i]` in test code is either (a) refactored to `.get(i).ok_or_else(|| …)` if the index may be bogus, or (b) wrapped in `#[expect(clippy::indexing_slicing, reason = "test fixture guarantees index is in range")]` with the invariant stated.  Per-site reasoning; no blanket allow.
- **W4.2** — `cast_possible_wrap` (48), `cast_sign_loss` (38), `cast_precision_loss` (38), `as_pointer_underscore` (39), `ptr_as_ptr` (17), `cast_lossless` (16), `unnecessary_cast` (12): the `cast_*` family is where **real bugs hide**.  Every `as` becomes `TryFrom::try_from` or a motivated `#[expect]` with an explicit invariant.  Each fix that changes a returned type gets a test update.
- **W4.3** — `option_if_let_else` (46), `shadow_reuse` (55), `shadow_unrelated` (38), `needless_pass_by_value` (12), `map_unwrap_or` (14), `manual_let_else` (14), `single_match_else` (16), `wildcard_enum_match_arm` (16), `trivially_copy_pass_by_ref` (12 test), `missing_const_for_fn` (18): mechanical-with-reading.
- **W4.4** — `undocumented_unsafe_blocks` (62 in tests): write prose `SAFETY:` comments.  Unlike prod, test SAFETY comments can be briefer ("test fixture; no concurrent access") but must still be present.

**Acceptance after W4**: ≥ 90 % of backlog cleared; only T3 remains (~150 lints).

### Phase W5 — Test T3 semantic + CI upgrade (1 week)

- **W5.1** — `cognitive_complexity` (52 in tests, threshold=30 in `clippy.toml`): many tests are inherently sequential setup → action → assertions.  Fix path: extract helper fns, move setup to fixtures, split multi-assertion tests.  Each extraction is reviewed — this is where test clarity improves most.
- **W5.2** — `too_many_lines` (36, threshold=150): same play as cognitive_complexity — extract helpers, split assertions into separate `#[test]` fns.
- **W5.3** — `ref_patterns` (83): `if let Some(ref x @ Foo(_)) = …` → explicit borrow destructure.  Pattern-semantics sensitive; each fix reviewed.
- **W5.4** — `multiple_unsafe_ops_per_block` (30 in tests), `float_arithmetic` (32): per-site motivation.  Float arithmetic in perf benches is legitimate; add `#[expect]` with reason.
- **W5.5** — **Upgrade `windows-check` in PR Fast CI** from `cargo check` → `cargo clippy --workspace --all-targets --all-features --locked --no-deps -- -D warnings`.  Rename job `windows-check` → `windows-lint` for clarity.  Keep `windows-latest` runner.
- **W5.6** — Upgrade local `check-windows` in pre-push to `lint-ci-windows`.  W1.4 measurement shows CI pass is **6 s warm** — pre-push bundle lands well under the 60 s target (current 50 s → ~56 s).  The original "if > 90 s, back off" budget gate is no longer needed; proceed directly to full-scope pre-push.
- **W5.7** — Final `#[expect(…, reason = …)]` audit.  Target: fewer than **20** total across the whole Windows surface (higher than the original 10 to accommodate cast-family FFI sites); each with prose `reason`.

**Acceptance after W5**: `just lint-ci-windows` exits 0.  PR Fast CI `windows-lint` runs clippy and passes.  `required` aggregator gates on it.  **No code can merge without passing Windows clippy on all targets.**

### Phase L1 — zigbuild accelerator (1 day, independent of W\*)

- **L1.1** — Document `brew install zig` + `cargo install cargo-zigbuild` in CONTRIBUTING.md and add to `install-dev-tools` recipe (`@/Users/rnio/Private/Github/UltraFastFileSearch/just/test.just:152-186`).
- **L1.2** — Add recipe in `just/test.just`:

  ```just
  # Native macOS → Linux clippy sweep via cargo-zigbuild.
  # Faster inner-loop alternative to `lint-ci-linux` (Docker).
  # Docker path remains authoritative — this is a dev accelerator.
  lint-ci-linux-zig:
      @printf "\033[0;34m🦎 Linux x86_64 clippy via cargo-zigbuild (native)...\033[0m\n"
      CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="zig cc -target x86_64-linux-gnu" \
      CC_x86_64_unknown_linux_gnu="zig cc -target x86_64-linux-gnu" \
      cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu --no-deps -- -D warnings
  ```

  *(Exact env-var form validated during L1.3; `cargo-zigbuild` may also be invoked directly as a wrapper — `cargo zigbuild clippy` — whichever pattern proves to drive `cc-rs` correctly for `zstd-sys`.)*
- **L1.3** — Prove `zstd-sys` compiles green via zigbuild.  If any other C-dep crate (e.g. `ring`, `openssl-sys`) trips, document and add explicit `CC_x86_64_unknown_linux_gnu` env overrides in the recipe.
- **L1.4** — Wire into `check-all-targets` (`just/dev.just:66-88`) as an **optional faster path** when `zig` is on `PATH`; Docker path still runs when `zig` is absent.

**Acceptance**: `just lint-ci-linux-zig` runs green on any macOS dev box with `zig` installed, in ≤ 30 s warm.  Docker path unchanged.

### Phase L2 — CI parity verification (½ day)

- **L2.1** — Confirm `lint-ci-linux` (Docker) output matches `pr-fast.yml`'s `clippy` job output exactly (same lints fire / don't fire).  If it drifts, pin the `rust:latest` tag to the same rustc the CI runner uses (source: `@/Users/rnio/Private/Github/UltraFastFileSearch/rust-toolchain.toml`).

**Acceptance**: a red PR's Docker run produces the same failing lints as the GitHub Actions run.

### Phase P1 — Plan codification (already in progress — this doc)

- **P1.1** — This doc lands on `main` with the W0 baseline table filled in.
- **P1.2** — Cross-reference from `@/Users/rnio/Private/Github/UltraFastFileSearch/docs/architecture/dev-flow-implementation-plan.md` § 1.3 (gates) and `@/Users/rnio/Private/Github/UltraFastFileSearch/docs/architecture/release-automation-plan.md` § 1 (non-goals list).
- **P1.3** — Update `@/Users/rnio/Private/Github/UltraFastFileSearch/CONTRIBUTING.md` "Architecture guardrails" to include "new Windows-gated code must pass `just lint-ci-windows` before PR".

## 5. Integration with the 4-layer quality gate model

| Layer | Current | After W4 + L1 |
|---|---|---|
| **IDE save** | rust-analyzer check-on-save (native only) | Unchanged (cross-target too expensive for on-save) |
| **pre-commit** (`lint-fast`, sub-2 s / ~25 s Rust) | fmt + native clippy trio + typos + reuse + file-size | **Unchanged** — xwin clippy too expensive here |
| **pre-push** (`lint-pre-push`, ≤ 60 s target) | fmt + native trio + rustdoc + doc-tests + tests + smoke + `check-windows` (8 s `check`) | **Upgrade**: `check-windows` → `lint-ci-windows` (+ 6 s warm, measured W1.4).  Bundle goes 50 s → ≈56 s — well under target.  Original W4.4 "> 90 s back off" gate no longer needed. |
| **PR CI** (`pr-fast.yml`) | Required: classify + file-size + fmt + sanity + clippy + docs + test-build + tests + security + `windows-check` (**`check`**) | **Upgrade** `windows-check` → `windows-lint` running `cargo clippy -- -D warnings`.  Linux clippy already covered by the existing `clippy` job on `ubuntu-22.04` (no change needed for Linux). |
| **Tier 2 nightly** (`tier-2.yml`) | Weekly `windows-latest` `cargo check` + coverage + udeps + miri | **Drop** the now-redundant windows check (PR-time authoritative post-W4.3); keep coverage / udeps / miri single-target. |

**Flag stack discipline**: all three targets (native host, Linux, Windows) use the exact same `common_flags` / `prod_flags` / `test_flags` from `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:28-30`.  Zero target-specific allowlists.  Any rule that truly diverges per-target (e.g. `cast_possible_wrap` on 32-bit targets) goes in `[workspace.lints]` in the root `Cargo.toml` where it is visible and review-gated.

**xwin clippy warm-runtime measurements** (filled by W1.4, 2026-04-24):

| Pass | Command | Warm duration | Lint count (matches W0) |
|---|---|---|---|
| prod | `just lint-prod-windows` | **2 s** | 93 ±1 ✅ |
| tests | `just lint-tests-windows` | **6 s** | 1,342 ±1 ✅ |
| ci | `just lint-ci-windows` | **6 s** | 1,346 ±1 ✅ |

**Pre-push budget implication**: all three passes fit well under the 60 s pre-push budget, even full-workspace (`lint-ci-windows` at 6 s warm).  The W5.6 "budget gate" conservatism can now be relaxed — there is room to run either `lint-prod-windows` (W2.4) or even `lint-ci-windows` (W5.6) in pre-push without exceeding 60 s.  **Decision now safe to make at W2.4 empirically, not at W5.6.**

## 6. Discipline enforcement (mapping to the four rules)

| Rule | How this plan honours it |
|---|---|
| **1. No suppression hacks** | Zero blanket `#[allow]`.  Only `#[expect(lint::name, reason = "prose")]` on individual items, with a target count of **< 20** total after W5 (revised up from 10 after W0 revealed the cast-family FFI site density).  Every suppression reviewed in the PR that introduces it. |
| **2. Surgical, correct fixes** | Phased by lint rule, not by file.  Each PR touches one rule family at a time; mechanical T1 fixes are deterministic, T2 + T3 fixes are per-site reasoned.  No "refactor-while-I'm-here" smuggling. |
| **3. Preserve behavior & contracts** | Public API unchanged except where a lint surfaces a real bug (typically `cast_*` truncations or `significant_drop_*` deadlocks).  Each API change gets a CHANGELOG entry + regression test. |
| **4. Improve tests, don't dodge** | T3 semantic fixes ship with new regression tests (deadlock / ordering / cast-boundary).  T1 + T2 fixes rely on the existing Windows test suite remaining green; `@/Users/rnio/Private/Github/UltraFastFileSearch/crates/uffs-daemon/tests/ipc_integration.rs` + `uffs-mft` unit tests already exercise the heaviest Windows surface area. |

## 7. Operational policy

### 7.1 New-code rule (starting Phase W1)

From the moment Phase W1 lands (xwin clippy recipes exist), **all new Windows-gated code must pass `just lint-ci-windows` before PR**.  Pre-push will enforce this once W4.4 lands; until then it's a contributor discipline item called out in CONTRIBUTING.md.

### 7.2 Triage cadence

- **Weekly** during W2 – W4: progress snapshot (remaining lint count by rule) posted as a comment on a tracking issue.
- **Daily** if a tier stalls: swap owner or reclassify the stalled rule into T3 for design review.

### 7.3 Abandonment criteria

If a single lint rule (typically `significant_drop_tightening`) accumulates > 3 working days of per-site reasoning without progress, **the decision is escalated to a design doc** — *not* a blanket `#[allow]`.  Each such rule either gets a small architectural fix (e.g. lock-granularity refactor) or a motivated `#[expect]` with a linked architectural ticket to remove it.

### 7.4 Calendar placement

| Week | Phases | Shape |
|---|---|---|
| 1 | P1 + W0 (✅ done) + W1 + L1 | Codify plan, infrastructure, zigbuild accelerator |
| 1 – 2 | **W2 — prod gate** (93 lints) | 3 PRs: T1 mechanical, T2 targeted (SAFETY comments), T3 (unsafe-block splits).  Ends with `lint-prod-windows` gating pre-push + CI. |
| 3 | W3 — test T1 mechanical (~900 lints) | One PR per rule family (~10 PRs, each 50–250 sites) |
| 4 – 5 | W4 — test T2 targeted (~350 lints) | One PR per rule family, cast-family gets a dedicated PR |
| 6 | W5 — test T3 semantic + CI upgrade | 2 PRs: semantic fixes, then CI flip |

**Total effort**: ~6 calendar weeks / ~12–15 focused working days.  Prod gate (W2) is the **early win** — it lands in week 1–2 and provides an authoritative "new Windows code must pass clippy" gate even while the test backlog is still being cleared.

## 8. Acceptance criteria (rollup)

- [ ] `just lint-prod-windows` + `just lint-tests-windows` + `just lint-ci-windows` exist and are green.
- [ ] `just lint-ci-linux-zig` exists and green; Docker path unchanged and still green.
- [ ] PR Fast CI `windows-lint` job runs `cargo clippy -- -D warnings` natively on `windows-latest`; `required` aggregator gates on it.
- [ ] Zero blanket `#[allow]` added in this workstream; fewer than 20 `#[expect(…, reason = …)]` across the Windows surface (budget revised from 10 after W0 revealed cast-family FFI site density).
- [ ] Every `#[expect]` has an inline justification comment referencing either the FFI signature it accommodates or an open ticket to remove it.
- [ ] CONTRIBUTING.md + `dev-flow-implementation-plan.md` + `release-automation-plan.md` cross-reference this plan.
- [ ] "New Windows code must be lint-clean from day 1" rule enforced at pre-push after W2 (prod) and at PR CI after W5 (full).

## 9. Cross-references

- **Strict lint flag stack source of truth**: `@/Users/rnio/Private/Github/UltraFastFileSearch/just/shared.just:28-30`
- **Current Linux Docker gate**: `@/Users/rnio/Private/Github/UltraFastFileSearch/just/test.just:82-107`
- **Current Windows xwin check gate (local)**: `@/Users/rnio/Private/Github/UltraFastFileSearch/just/dev.just:52-55`
- **Current Windows CI gate**: `@/Users/rnio/Private/Github/UltraFastFileSearch/.github/workflows/pr-fast.yml:436-458`
- **Pre-push hook runner**: `@/Users/rnio/Private/Github/UltraFastFileSearch/scripts/hooks/_lint_pre_push.sh`
- **Gate architecture**: `@/Users/rnio/Private/Github/UltraFastFileSearch/docs/architecture/dev-flow-implementation-plan.md`
- **Release architecture**: `@/Users/rnio/Private/Github/UltraFastFileSearch/docs/architecture/release-automation-plan.md`
- **Cargo-zigbuild upstream**: https://github.com/rust-cross/cargo-zigbuild
- **Cargo-xwin upstream**: https://github.com/rust-cross/cargo-xwin
