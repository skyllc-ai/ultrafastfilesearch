# UFFS — Rust Codebase Lint Cleanup Spec for Intent by Augment

> **Tool:** This spec is designed for **[Intent by Augment](https://www.augmentcode.com/)** — a spec-driven development app and agent orchestration platform (public beta). Use this document as the driving spec; let Intent's parallel agents handle implementation while you review every change for correctness.

---

## 0. Repo Context (Read-Only Reference)

| Property | Value |
|---|---|
| **Project** | UFFS — Ultra Fast File Search (NTFS MFT reader + Polars DataFrames) |
| **Workspace members** | `uffs-polars`, `uffs-mft`, `uffs-core`, `uffs-cli`, `uffs-tui`, `uffs-gui`, `uffs-diag` |
| **Toolchain** | Pinned nightly (`rust-toolchain.toml` — `channel = "nightly-2025-12-15"`) |
| **Edition** | 2024 |
| **MSRV** | 1.85 |
| **Feature flags** | `zstd` (default in `uffs-mft`; only workspace-level feature) |
| **Existing lint config** | `[workspace.lints.clippy]`, `[workspace.lints.rust]`, `[workspace.lints.rustdoc]` in root `Cargo.toml` — extensive deny/warn set already defined |
| **No `clippy.toml`** | Confirmed absent (one exists in `vendor/errno/` — irrelevant, vendored) |
| **`.cargo/config.toml` present** | Yes — sets `sccache` wrapper, custom `target-dir`, per-target `rustflags` and linkers. **Do not modify** (see §3.4) |
| **Vendored code** | `vendor/errno`, `vendor/fs4`, `vendor/mft-reader-rs`, `vendor/stacker`, `vendor/winapi-util` |
| **Workflow tool** | `just` (justfile at repo root); also `rust-script scripts/ci/ci-pipeline.rs` |

---

## 1. Objective

Systematically remove **all blanket lint/clippy suppression attributes** from this Rust workspace and fix the underlying issues they were hiding. The end state: clean, idiomatic Rust that compiles and tests **warning-free** without relying on suppressions.

---

## 2. Definitions — What Is a "Blanket Allow"?

The following are **all considered blanket allows** and must be removed unless explicitly exempted in §3.2:

| Pattern | Example |
|---|---|
| **Crate-root inner attributes** | `#![allow(unused)]`, `#![allow(clippy::all)]` |
| **Module / file-level attributes** | `#[allow(dead_code)]` on a `mod` block |
| **Group / multi-lint allows** | `#[allow(unused)]`, `#[allow(clippy::all)]`, `#[allow(clippy::pedantic)]` |
| **Comma-separated multi-lint** | `#[allow(dead_code, unused_imports)]` — each lint counts separately |
| **`cfg_attr`-wrapped allows** | `#[cfg_attr(not(test), allow(unused))]` |
| **Macro-generated allows** | Any `allow` emitted by a `macro_rules!` or proc-macro **in this repo** (see §6.9 for detection) |

**Not considered blanket** (and therefore acceptable to keep):

- A **single-lint** `#[expect(lint_name)]` (preferred) or `#[allow(lint_name)]` on **one item** (fn, struct, field, etc.) with a `// Reason: ...` comment explaining why.

> **Prefer `#[expect(...)]` over `#[allow(...)]`** for any intentional suppression. `#[expect]` will fail if the lint stops triggering, preventing stale suppressions. This repo uses nightly, so `#[expect]` is fully available. **Fallback:** if a specific lint does not support `#[expect]` (compiler error), use `#[allow(lint_name)]` with a `// Reason: #[expect] unsupported for this lint` comment.

---

## 3. Scope

### 3.1 Workspace Scope

Apply to the **entire workspace** — all `.rs` files reachable from any workspace member, including:

- `crates/*/src/`, `crates/*/tests/`, `crates/*/benches/`
- All `build.rs` files
- Binary crate entry points (`crates/uffs-cli/src/main.rs`, `crates/uffs-tui/src/main.rs`, etc.)

**Discover members programmatically** — do not hard-code paths:

```bash
cargo metadata --no-deps --format-version 1 | jq -r '.packages[].manifest_path' | xargs -I{} dirname {}
```

### 3.2 Generated & Vendored Code — Exception Policy

**Generated code checked into the repo:**

- Modify the **generator inputs/config** or **build script**, then **regenerate** and commit the regenerated file. Do not hand-edit generated output.
- If the generator cannot be configured to suppress the lint: wrap the `include!` / `mod generated { ... }` at the **wrapper boundary** with a narrow `#[expect(lint_name)]` and a comment: `// Generated code — suppression required; generator does not support fixing this`.

**Generated code NOT checked in** (built at compile time by `build.rs`):

- Fix in the build script or generator config. If impossible, add a narrow `#[expect]` at the `include!` / `mod` wrapper boundary only.

**Vendored third-party code** (`vendor/errno`, `vendor/fs4`, `vendor/mft-reader-rs`, `vendor/stacker`, `vendor/winapi-util`):

- Do **not** refactor. These are excluded from the workspace already. If any workspace crate wraps vendored code via `mod` or `include!`, isolate with a module boundary and `#[expect]` only at that boundary, with a `// Vendored — suppression required` comment.

### 3.3 Toolchain & Edition — Pinned (Do Not Change)

Use the repo's existing `rust-toolchain.toml` (`channel = "nightly-2025-12-15"`). **Do not change:**

- The toolchain channel (pinned nightly — recent nightlies have Polars-incompatible breakages)
- The edition (`2024`)
- The MSRV (`1.85`)
- The `components` or `targets` lists

If an MSRV/edition bump is desirable, file it as a separate, explicitly approved task — not part of this cleanup.

### 3.4 No Cheating via Configuration

**Do not modify lint configuration to reduce warnings. Only code fixes count.** Specifically, do not change:

- `[workspace.lints]` or `[lints]` tables in any `Cargo.toml`
- `clippy.toml` (do not create one)
- `.cargo/config.toml` (exists; do not modify — it configures sccache, target-dir, and per-target linkers/rustflags)
- CI scripts (`.github/workflows/*.yml`), `justfile`, `scripts/ci/ci-pipeline.rs`, or any file that passes `-A` / `-W` flags or sets `RUSTFLAGS` to suppress lints
- Do not add `#![allow(...)]` via `cfg_attr` or feature-gated lint suppression

**Exception:** the existing `multiple_crate_versions = "allow"` in `[workspace.lints.clippy]` may remain (justified — Polars/Tokio ecosystem brings unavoidable version conflicts).

---

## 4. Numeric Type Guidance

The following rules replace any prior "use smaller types" advice:

- **Use `usize`** for indexing, `.len()`, allocation sizes, and slice operations (memory-internal).
- **Use fixed-width integers** (`u32`, `i64`, etc.) for serialization, protocol fields, FFI boundaries, and persistent storage.
- **Prefer `From` / `TryFrom` / `*_into()` methods** for type conversions. Use `as` **only** for proven-safe widenings and document any lossy casts with a comment.
- **When converting to floats**, prefer `f64::from(x)` for integer widenings; avoid chained `as` casts like `x as u64 as f64`.
- **Do not blindly narrow types** (e.g., `usize` → `u16`) — this can cause panics on large inputs or break indexing. Only narrow when the domain provably constrains the range, and use `TryFrom` with error handling.

---

## 5. Approach — Staged Audit (Not Big-Bang)

### Phase 0: Inventory & Baseline Report (Before Any Code Changes)

Produce a report (`LOG/<YYYY_MM_DD_HH_MM>_UTC_INVENTORY.md`) containing:

1. **Total count** of `allow` / `expect` attributes across the workspace.
2. **Breakdown by individual lint** — top 20 most common suppressed lints (split comma-separated lists into individual counts).
3. **Breakdown by scope** — crate-root (`#![]`) vs. module-level vs. item-level.
4. **Breakdown by form** — plain `allow`/`expect` vs. `cfg_attr`-wrapped vs. inner attributes.
5. **Per-crate counts** — ranked by suppression count.

This report enables multi-agent partitioning and prevents missed areas.

**Inventory script** (no new dependencies required):

```bash
#!/usr/bin/env bash
set -euo pipefail

# Discover all workspace member directories
MEMBERS=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[].manifest_path' \
  | xargs -I{} dirname {})

# Directories to exclude from scanning
# vendor/ is excluded from workspace
EXCLUDES="--exclude-dir=target --exclude-dir=vendor"

echo "=== Total allow/expect attributes ==="
echo "$MEMBERS" | xargs -I{} grep -rn $EXCLUDES \
  '#\[allow\|#!\[allow\|#\[expect\|#!\[expect\|cfg_attr.*allow\|cfg_attr.*expect' \
  --include='*.rs' {} 2>/dev/null | wc -l

echo ""
echo "=== Top 20 suppressed lints (comma-separated split) ==="
echo "$MEMBERS" | xargs -I{} grep -rnoP \
  '(?:allow|expect)\(([^)]+)\)' --include='*.rs' {} $EXCLUDES 2>/dev/null \
  | sed 's/.*(\(.*\))/\1/' \
  | tr ',' '\n' \
  | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' \
  | sort | uniq -c | sort -rn | head -20

echo ""
echo "=== Per-crate counts ==="
for dir in $MEMBERS; do
  count=$(grep -rn $EXCLUDES '#\[allow\|#!\[allow\|#\[expect\|#!\[expect' \
    --include='*.rs' "$dir" 2>/dev/null | wc -l | tr -d ' ')
  echo "$dir: $count"
done | sort -t: -k2 -rn

echo ""
echo "=== Crate-root inner attributes (#![allow/expect]) ==="
echo "$MEMBERS" | xargs -I{} grep -rn $EXCLUDES \
  '#!\[allow\|#!\[expect' --include='*.rs' {} 2>/dev/null || true

echo ""
echo "=== cfg_attr-wrapped suppressions ==="
echo "$MEMBERS" | xargs -I{} grep -rn $EXCLUDES \
  'cfg_attr.*allow\|cfg_attr.*expect' --include='*.rs' {} 2>/dev/null || true
```

> **Note:** counts are approximate for multi-line attributes. That is acceptable for the inventory — the goal is triage, not perfection.

### Phase A: Crate-Root `#![allow(...)]`

**Delete** all crate-root inner `#![allow(...)]` attributes. If suppression is genuinely required for a specific lint on the entire crate, it must be moved to the **narrowest item** using `#[expect(lint_name)]` with a reason — or documented as an exemption for generated/vendored wrappers (§3.2). Run the full validation suite (§8) until green before proceeding.

### Phase B: Module-Level Blanket Allows

Remove `#[allow(...)]` on `mod` blocks and file-level attributes. Fix until green.

### Phase C: Item-Level Allows

Remove remaining item-level `#[allow(...)]`. For each:

- Fix the underlying issue, **or**
- Convert to `#[expect(lint_name)]` with a `// Reason:` comment if suppression is genuinely needed.

### Phase D: `cfg_attr` and Macro-Generated Allows

Address `cfg_attr(..., allow(...))` and macro-generated suppressions. Fix or justify. See §6.9 for macro detection instructions.

---

## 6. Rules (Non-Negotiable)

### 6.1 No Suppression Hacks

Do NOT add blanket `#[allow(...)]`, disable lints, comment out failing tests, or introduce new `cfg`/feature gates **solely to hide warnings**. Existing `cfg` structure is fine; test-only code may live under `#[cfg(test)]`. See also §3.4 (no cheating via config).

### 6.2 Prefer `#[expect]` Over `#[allow]`

If a lint must be suppressed intentionally, use `#[expect(lint_name)]` with a reason comment. This ensures the suppression fails loudly if the lint stops triggering. Use `#[allow]` only if `#[expect]` produces a compiler error for that specific lint (and note this in the reason comment).

### 6.3 Surgical, Correct Fixes

Prefer minimal, idiomatic Rust changes that resolve root causes (ownership, types, semantics) rather than superficial workarounds.

### 6.4 Diff Hygiene

- **No opportunistic refactors** unrelated to removing suppressions.
- **Keep diffs minimal.** No renames or reformatting beyond what `rustfmt` produces.
- **Any behavior change** must be covered by tests.

### 6.5 Public API & Dead Code Policy

Determine API stability mechanically:

- **`publish = false`** in a crate's `Cargo.toml` → treat as internal; more freedom to remove/change `pub` items (but still prefer deprecation for widely-used internal APIs).
- **Published crate** (or intended to be published) → treat all `pub` items as stable. No removals — only deprecations.
- **Binary-only workspace members** (e.g., `uffs-cli`, `uffs-tui`, `uffs-gui`) → public API constraints are looser (no external consumers), but still avoid unnecessary churn.
- **Facade crate** (`uffs-polars`) → its `pub` API is consumed by all other workspace crates. Treat changes with care — breakage cascades across the workspace.

When deprecating, use the **crate version from `Cargo.toml`** for the `since` field:

```rust
#[deprecated(since = "0.2.202", note = "Use XYZ instead")]
```

Dead code that is clearly internal (`pub(crate)` or private) may be deleted outright.

### 6.6 Dependency Policy

- Do **not** add new direct dependencies in any `Cargo.toml` (including dev-dependencies) unless strictly necessary and no standard-library or existing-dep solution exists. Any addition must be justified in the changelog.
- Do **not** run `cargo update`. Lockfile changes should only come from `Cargo.toml` edits.
- Run all validation with `--locked` to prevent accidental lock churn:

```bash
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build --workspace --release --locked
```

> **Note:** The repo uses `sccache` as `rustc-wrapper` via `.cargo/config.toml`. If `sccache` is not installed, either install it (`cargo install sccache`) or temporarily unset `RUSTC_WRAPPER=""` in your shell.

### 6.7 Preserve Behavior & Contracts

Maintain observable behavior unless lint failures prove it's wrong or inconsistent — then update docs and tests accordingly.

### 6.8 Improve Tests, Don't Dodge Them

Strengthen tests to be deterministic and meaningful. Never skip or relax a test just to pass.

### 6.9 Macro-Generated Allows — Detection & Fix Protocol

To find allows emitted by macros in this repo:

1. **Search macro sources** for `allow` / `expect` emissions:
   ```bash
   rg 'allow\(|expect\(' --include='*.rs' -g '*macro*' -g '*proc_macro*' .
   rg 'quote!.*allow' --include='*.rs' .
   ```
2. **Check expanded output** for allows that don't appear in source:
   ```bash
   cargo expand --lib -p <crate_name> 2>/dev/null | grep '#\[allow\|#\[expect'
   ```
3. **If the allow only appears post-expansion:** fix the **macro definition**, not call sites.
4. **If a proc-macro intentionally emits an allow:** add a comment at the emission site explaining why, and document it in the changelog.
5. **Stop rule:** if a macro-generated allow originates from an **external** crate (not this repo), do not attempt to fix it. Document it in the remaining-allows report (§10) and move on.

---

## 7. Multi-Agent Partitioning (Intent by Augment)

When running with Intent's parallel agents:

- **Partition by crate** — one agent per workspace member. Suggested groupings for this repo:

  | Agent | Crates | Notes |
  |---|---|---|
  | A | `uffs-polars` | Facade crate — changes here affect all downstream crates |
  | B | `uffs-mft` | Largest crate; Windows-only I/O code with `#[cfg(windows)]` |
  | C | `uffs-core` | Query engine; platform-agnostic |
  | D | `uffs-cli`, `uffs-tui` | Binary crates; thinnest layers |
  | E | `uffs-gui`, `uffs-diag` | GUI placeholder + diagnostic tools |

- Do **not** partition by lint category across the entire repo (this causes conflicts and duplicated work).
- Each agent works on its **own branch**, keeping changes localized to its assigned crate(s).
- **No agent may modify** `Cargo.toml` lint tables, `rust-toolchain.toml`, `.cargo/config.toml`, `justfile`, or shared workspace config (§3.4).
- **Agent A (`uffs-polars`) should run first** — it is a dependency of all other crates, so its changes must be stable before downstream agents begin.
- **Integration step:** rebase/merge all agent branches, then run the full workspace validation suite (§8) on the merged result.
- Merge conflicts in changelog files are avoided by the per-agent file convention (§9).

---

## 8. Validation Commands (Full Surface Area)

Run **all five** after every batch of fixes. All must pass before moving on.

```bash
# 1. Formatting check
cargo fmt --check

# 2. Clippy — workspace-wide, all targets, all features, locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# 3. Tests — workspace-wide, all features, locked
cargo test --workspace --all-features --locked

# 4. Doc tests (compiled separately from unit tests)
cargo test --workspace --doc --locked

# 5. Release build — locked
cargo build --workspace --release --locked
```

**Equivalent `just` commands** (preferred — use these if the justfile is available):

```bash
just fmt          # Format + check
just lint-prod    # Ultra-strict production clippy
just lint-tests   # Pragmatic test clippy (allows unwrap/expect)
just test         # nextest runner
just build        # Release build
```

**Feature flag coverage for this repo:**

The only workspace-level feature is `zstd` (default in `uffs-mft`). `--all-features` covers it. No special feature matrix is needed.

**Suppression count check** (run after each phase to track progress):

```bash
rg --include='*.rs' -c '(#\[allow|#!\[allow|#\[expect|#!\[expect|cfg_attr.*allow|cfg_attr.*expect)' \
  --glob='!target/**' --glob='!vendor/**' . | awk -F: '{s+=$2} END {print "Remaining suppressions:", s}'
```

---

## 9. Changelog & Commit Workflow

### Commit Messages

Small, atomic commits: `fix(<crate>): concise root cause description`

Examples for this repo:
- `fix(uffs-mft): remove blanket #![allow(unused)] from lib.rs`
- `fix(uffs-core): replace allow(dead_code) with expect + reason on FastPathResolver`
- `fix(uffs-polars): convert as-cast to From for column index`

### Changelog Files (Conflict-Safe for Parallel Agents)

Each agent (or work session) creates its **own** changelog file, named by the crate(s) it owns:

```
LOG/<crate_name>_<YYYY_MM_DD_HH_MM>_UTC_CHANGELOG_HEALING.md
```

**All timestamps in UTC.**

Each entry records:

- **What failed** — the lint / warning / error (with `rustc` / clippy error code if applicable)
- **Why** — root cause
- **How you fixed it** — the change (with file + line references)

At the end of the task, produce a **summary** file:

```
LOG/<YYYY_MM_DD>_UTC_CHANGELOG_HEALING_SUMMARY.md
```

All changelog files **must** be included in the final commit/push.

---

## 10. Definition of Done

The task is complete **only** when all of the following are true:

| # | Criterion | How to verify |
|---|---|---|
| 1 | **Zero warnings** | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` exits 0 |
| 2 | **All tests pass** | `cargo test --workspace --all-features --locked` exits 0 |
| 3 | **Release builds** | `cargo build --workspace --release --locked` exits 0 |
| 4 | **Formatting clean** | `cargo fmt --check` exits 0 |
| 5 | **Remaining suppressions listed** | `LOG/<date>_UTC_REMAINING_ALLOWS.md` lists **every** remaining `allow`/`expect` attribute — including inner attributes (`#![]`), `cfg_attr`-wrapped, and macro-generated — with: file, line, lint, form, and justification |
| 6 | **Before/after delta** | The summary changelog includes total suppression count before and after (from inventory) |
| 7 | **Inventory report exists** | `LOG/<timestamp>_UTC_INVENTORY.md` from Phase 0 is committed |
| 8 | **Changelogs committed** | All `LOG/*_CHANGELOG_HEALING*.md` files are in the final push |
| 9 | **Types correct at origin** | No unnecessary `usize` → `as f64` chains; conversions use `From`/`TryFrom` |
| 10 | **No new direct dependencies** | `git diff Cargo.toml crates/*/Cargo.toml` shows no new `[dependencies]` entries (or any additions are justified in changelog) |
| 11 | **No lockfile churn** | `cargo update` was not run; `Cargo.lock` changes only reflect `Cargo.toml` edits |
| 12 | **Lint config unchanged** | `git diff` shows no changes to `[workspace.lints]`, `[lints]`, `clippy.toml`, `.cargo/config.toml`, `justfile`, or CI lint flags |
| 13 | **Workspace membership unchanged** | `git diff Cargo.toml` shows no unexpected workspace-member changes |

---

## Quick Reference — Validation Loop

```bash
# Raw cargo commands (always work)
cargo fmt --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --doc --locked
cargo build --workspace --release --locked
rg --include='*.rs' -c '(#\[allow|#!\[allow|#\[expect|#!\[expect|cfg_attr.*allow)' \
  --glob='!target/**' --glob='!vendor/**' . | awk -F: '{s+=$2} END {print "Remaining:", s}'

# Or via justfile (preferred)
just fmt && just lint-prod && just lint-tests && just test && just build
```
