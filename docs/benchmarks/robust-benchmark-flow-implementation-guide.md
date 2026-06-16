# Robust Benchmark Flow — Implementation Guide

**Status:** BUILD GUIDE — the step-by-step counterpart to
[`robust-benchmark-flow-execution-plan.md`](robust-benchmark-flow-execution-plan.md)
(the *what/why*). This document is the *how to build it*, written so a junior
engineer can execute it phase by phase without prior context on the benchmark
suite.

**Audience:** the engineer implementing the flow. Read the execution plan once
for the big picture, then live in this document while coding.

**Golden rules while you build (from repo policy):**
- No suppression hacks; surgical, idiomatic fixes only.
- Every mutation gets a registered restore *before* it mutates (§P1 of the plan).
- Leave the tree green: run `just go` (or local `cargo nextest`/`cargo check`)
  after each phase. Pipeline results decide acceptance.
- Keep a healing log: `LOG/<<YYYY_MM_DD_HH_MM>>CHANGELOG_HEALING.md`.

---

## 0. How to use this guide

1. Work the phases **in order**, P1 → P9. They are dependency-ordered: the
   hygiene/UX spine (P1) lands before anything that touches a real machine.
2. Each phase below has the **same shape**:
   - **Goal** — one sentence.
   - **Files** — exactly what to create/edit.
   - **Build steps** — numbered, concrete.
   - **Definition of Done (DoD)** — the checklist that must be true to move on.
   - **How to test** — the command(s) that prove it.
3. After every phase: run that phase's test **and** `just go`. Commit small
   (`fix:`/`feat:` atomic commits), update the healing log.
4. Anything marked **[macOS/Linux OK]** is testable off-Windows (mock-based);
   anything marked **[Windows-live]** needs an elevated Windows box with the
   `uffs`/`es.exe` binaries present.

---

## 1. Prerequisites & dev environment

| Need | Why | Install / check |
|---|---|---|
| **Rust toolchain** (repo's pinned stable) | the orchestrator **is** a Rust crate (`uffs-bench`) | `rustc --version` |
| `cargo-nextest` | runs the orchestrator's tests in the same lane as `just go` | `cargo install cargo-nextest` |
| `just` | recipe runner | `cargo install just` |
| `rust-script` | runs the existing **leaf** `.rs` harnesses (wrapped) | `cargo install rust-script` |
| Windows 10/11, **elevated** shell | MFT read + Everything restart (live stages only) | run terminal "as Administrator" |
| `pwsh` (PowerShell 7) | **only** to invoke the one *wrapped* legacy `.ps1` harness | `pwsh --version` |
| `uffs.exe` (+ `uffs.com`) in `~/bin` | tools under test | `just use-local` |
| `Everything` + `es.exe` | competitor (live only) | voidtools install; see §12 of plan |

**You can build and unit-test P1–P4 logic entirely on macOS/Linux** with plain
`cargo nextest` against the trait-based mock host (§7) — no MFT, no Windows, no
PowerShell. Only the *live* measurement stages (P5/P6) and the end-to-end run
(P9) require an elevated Windows box.

---

## 2. Deliverables map (files you will create / edit)

```
crates/uffs-bench/                 NEW workspace crate (the orchestrator)
├── Cargo.toml                     deps: serde, serde_json, toml, sha2, sysinfo, clap
├── src/
│   ├── main.rs                    CLI entry + mode dispatch              (P4→P9)
│   ├── host.rs                    `Host` trait + SystemHost / MockHost   (P1) ← DI seam
│   ├── restore.rs                 restore registry (LIFO) + Drop guard   (P1)
│   ├── gate.rs                    confirm / explain-card / done-panel    (P1)
│   ├── state.rs                   state file (serde) + resume engine     (P1) §5.2
│   ├── tooling.rs                 acquire + keep/remove dispositions     (P1) §5.1
│   ├── fingerprint.rs             host fingerprint + crumb diff          (P1)
│   ├── env.rs                     Stage 0a env capture                   (P2)
│   ├── preflight.rs               Stage 0c competitor preflight (RO)     (P3)
│   ├── matrix.rs                  Stage 0d drive/pattern negotiation     (P4)
│   ├── stages.rs                  Stage 1–3 wrappers (shell out)         (P5–P6)
│   └── report.rs                  Stage 4 assembly + report scaffold     (P7)
└── tests/                         nextest integration tests (MockHost, any OS)

scripts/windows/competitors.toml   pinned competitor versions + hashes    (P8)
just/bench_uffs.just               new recipes (delegate to `uffs-bench`)  (P4→P9)
```

**Reuse, do not rewrite** these leaf harnesses — the orchestrator *shells out* to
them: `scripts/windows/cross-tool-benchmark.rs` and `everything_capacity_probe.rs`
(rust-script), the legacy `cold-parity-per-drive.ps1` (via `pwsh`), and
`parity_check.rs`. Read-only helpers from the capacity probe are *lifted* into
`preflight.rs` (P3).

> ⚠️ **Naming collision to fix:** a recipe named **`bench-verify`** already
> exists in `just/bench_uffs.just` (it checks the launcher's delay-load import
> table — unrelated). Do **not** reuse that name. The plan §14 mention of
> `just bench-verify <bundle>` is renamed here to **`just bench-suite-verify`**.
> Update plan §14 to match when you start P9.

---

## 3. Execution modes (the operator-facing contract)

Four modes. **Guided** is the new one this guide adds, and it is the default
the *first* time someone runs the suite on a machine (§4.6).

| Mode | Flag | Prompts? | Teaches? | Mutates host? | Measures? | When to use |
|---|---|---|---|---|---|---|
| **Guided** | `-Guided` | yes (rich) | yes — full card per new step | yes | yes | First run; demos; onboarding. |
| **Interactive** | *(default after first run)* | yes (terse gate) | one line | yes | yes | You know the flow, still want control. |
| **AutoPilot** | `-AutoPilot` / `-Yes` | no | no | yes | yes | Trusted, repeat runs; CI-with-TTY. |
| **DryRun** | `-DryRun` | shows every card/gate | yes | **no** | **no** | Review the plan + negotiated matrix safely. |

> **Flag style:** flags are shown PowerShell-style (`-Guided`) for readability and
> continuity with §4's cards. The orchestrator is a Rust `clap` binary, so the
> **actual** CLI uses standard long flags: `--guided`, `--auto`, `--dry-run`,
> `--drives`, `--tools`, `--rounds`, `--bundle`, `--keep-tools`, etc. (§5, P4).

Hard rules (enforced in `gate::confirm`, §5):
- `-Guided` and `-AutoPilot` are mutually exclusive **at launch** — but inside a
  guided run the operator can press **`a`** to upgrade the *rest* of the run to
  autopilot once they trust it. This is the exact "ok now go on autopilot"
  hand-off the product asks for.
- Hygiene is **mode-independent**: every mode snapshots, restores, and verifies
  (§P1). Modes change *prompting and teaching*, never *cleanup*.
- Non-TTY + neither `-AutoPilot` nor `-DryRun` ⇒ **refuse to start** (plan §7.4).

---

## 4. Guided Mode — detailed design

> This is the "never blindside the user" experience. Goal: before **any**
> action, the operator sees *what* will happen, *why*, the **exact command**,
> *what each key does*, the **blast radius**, the **backup just taken**, the
> **time estimate**, and **how to undo**. After the action, they see exactly
> *what happened*. Once comfortable, one keystroke (`a`) flips to autopilot.

### 4.1 The explain-card (shown BEFORE a step)

Guided mode renders a full card; interactive mode renders only the 3-line
`Will run / Will mutate / Backups` block; autopilot renders nothing. Same data
source (§4.7), three verbosity levels.

```
╔══════════════════════════════════════════════════════════════════════════╗
║  STAGE 2 · PARITY  ·  step 1 of 3:  COLD cache purge + daemon warm-up      ║
╠══════════════════════════════════════════════════════════════════════════╣
║ WHY                                                                        ║
║   To measure a true COLD start we first drop UFFS's per-drive cache so the ║
║   daemon is forced to re-read the MFT from disk (not from a warm cache).   ║
║                                                                            ║
║ WHAT I'LL RUN  (verbatim — nothing hidden)                                 ║
║   1) uffs --daemon stop                                                      ║
║   2) del %LOCALAPPDATA%\uffs\cache\{C,D}_index.uffs  (+ _compact/_lock)    ║
║   3) uffs '*' --limit 1 --profile     # forces a full cold load            ║
║                                                                            ║
║ STATE I'LL CHANGE        →  R1 daemon (stopped→restarted)                  ║
║                             R2 UFFS cache files for C,D                    ║
║ BACKUP TAKEN FIRST       →  bundle\backup\uffs-cache\  (2 files, sha256 ✓) ║
║                             restore registered as LIFO slot #3             ║
║ ESTIMATED TIME           →  ~25–60 s (cold MFT read of C,D)                ║
║ IF SOMETHING GOES WRONG  →  Ctrl-C or 'q' restores the cache from backup   ║
║                             and restarts the daemon as it was.             ║
╠══════════════════════════════════════════════════════════════════════════╣
║  [Enter] do it   [c] show cmds only   [e] explain more   [s] skip          ║
║  [a] autopilot the rest   [b] back   [q] abort + restore   [?] help        ║
╚══════════════════════════════════════════════════════════════════════════╝
>
```

### 4.2 Keystrokes (guided mode)

| Key | Meaning |
|---|---|
| `Enter` | Run this step now. Afterwards print the **DONE panel** (§4.3). |
| `c` | Print just the raw command lines (easy to read/copy), then re-show the prompt. |
| `e` | **Explain more**: print the long-form rationale + a link to the relevant plan/methodology section. Re-show the prompt. |
| `s` | Skip this step. Mark it `SKIPPED(reason=operator)` in the bundle; auto-skip dependents with a stated reason. |
| `a` | **Autopilot the rest**: run all remaining steps with no prompts (hygiene still enforced). Confirms once: "Run remaining N steps unattended? [y/N]". |
| `b` | Back: re-display the previous step's card. Measurement already done is **not** silently undone; if the previous step mutated state, offer to restore-then-redo. |
| `q` | Abort now: drain the restore stack (`RestoreRegistry::drain`), write the partial bundle, exit non-zero. |
| `?` | Show the key legend + the current mode + where the bundle is being written. |

### 4.3 The DONE panel (shown AFTER a step)

The other half of "no blindside" — confirm reality matches the promise:

```
── DONE · Stage 2 step 1 (COLD purge + warm-up) ───────────────────────────
   ran:        uffs --daemon stop  →  exit 0
               purged 2 cache files (C_index.uffs, D_index.uffs)
               uffs '*' --limit 1 --profile  →  exit 0
   result:     daemon warm, 7,412,883 records loaded in 31.8 s (COLD)
   changed:    R1 daemon (now running), R2 cache (purged; restore armed)
   output →    bundle\full-suite.txt  (appended), bundle\run.log
   restore:    slot #3 will rebuild C,D cache from backup at teardown
   next:       Stage 2 step 2 of 3 — per-drive HOT vs C++ rounds
───────────────────────────────────────────────────────────────────────────
```

### 4.4 Progressive disclosure ("learn once, don't nag")

The per-drive / per-pattern loops repeat the same *kind* of step many times.
Showing the full card every iteration is noise. Rule:

- The **first** time a given card-id is shown this run → full card.
- Subsequent identical card-ids → the **terse one-liner** plus
  `(press e to re-explain, a for autopilot)`.
- `-Verbose` forces full cards every time; `-Brief` forces terse after step 1.

Track seen ids in a `[System.Collections.Generic.HashSet[string]]` on the
script scope. Card-id = `"<stage>/<step-kind>"` (not per-drive), so "HOT round"
is taught once, not once per drive.

### 4.5 First-run / onboarding detection (§4.6 implements the default)

- If `BundleRoot` has **no** prior bundle **and** no `~/.uffs/.bench-onboarded`
  marker exists ⇒ default mode = **Guided**.
- Otherwise default = **Interactive**.
- Explicit `-Guided` / `-Interactive` / `-AutoPilot` / `-DryRun` always win.
- After a *completed* guided run, write `~/.uffs/.bench-onboarded` (records date
  + suite version) so the next run defaults to Interactive. `-Guided` re-teaches
  any time.

### 4.6 Data-driven card model (so a junior dev fills data, not rendering)

Every step declares a **card** (`gate::Card`, §5); the library renders it
identically in all modes. You never hand-format a box — you fill the struct. The
snippet below is shown in shorthand for readability; the real type is the Rust
`Card` struct populated per step:

```rust
let card = Card {
    id:         "stage2/cold-purge".into(),        // stable; drives §4.4 dedupe
    stage:      "STAGE 2 · PARITY".into(),
    step_num:   1, step_total: 3,
    title:      "COLD cache purge + daemon warm-up".into(),
    why:        "Drop UFFS cache so the daemon re-reads the MFT from disk …".into(),
    commands:   vec![                              // EXACT, shown verbatim
        "uffs --daemon stop".into(),
        r"del %LOCALAPPDATA%\uffs\cache\{C,D}_index.uffs".into(),
        "uffs '*' --limit 1 --profile".into(),
    ],
    resources:  vec!["R1".into(), "R2".into()],    // blast radius (plan §6.1 ids)
    backups:    vec![r"bundle\backup\uffs-cache\ (2 files)".into()],
    est_time:   "~25–60 s".into(),
    recovery:   "Ctrl-C or 'q' restores cache from backup and restarts daemon.".into(),
    long_why:   "See methodology.md §Cold-vs-warm …".into(),  // shown on 'e'
};
match gate::confirm(&host, &mut mode, &mut seen, &card) {     // mode-aware (§5)
    Decision::Proceed => { let r = stage2_cold_purge(&host)?; gate::done_panel(&host, &card, &r); }
    other => handle(other),                                   // Skip|Back|Abort|…
}
```

`confirm()` returns a `Decision` (`Proceed | ProceedNoop | Skip | Autopilot |
Back | Abort`). The orchestrator (P4) is a flat list of
`confirm` → `invoke` → `done_panel` triples — easy to read, easy to extend with
a new step.

### 4.7 Transparency guarantees (acceptance for the mode)

A guided run is "transparent enough" iff, for every mutating step:
1. the **exact** command string shown == the command actually executed
   (assert in tests by capturing the invoked command via the `MockHost`, §7);
2. the resources listed == the resources whose restore got registered;
3. a DONE panel is printed with the real exit code and output path;
4. `e`/`c`/`?` never mutate anything;
5. choosing `a` runs the remainder with zero further prompts but identical
   snapshot/restore/verify behavior.

---

## 5. Crate spec — `crates/uffs-bench` (built in P1)

The orchestrator is a **Rust workspace crate**, not a script. Every side effect
goes through a `Host` trait so tests inject a `MockHost` (§7) and run in the same
`cargo nextest` lane as `just go`. Module-by-module public API:

```rust
// host.rs — the dependency-injection seam (the Rust analogue of $Deps)
pub trait Host {
    fn read_file(&self, p: &Path) -> io::Result<Vec<u8>>;
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<()>;
    fn remove_file(&self, p: &Path) -> io::Result<()>;
    fn run(&self, exe: &str, args: &[&str]) -> io::Result<ProcOutput>; // status+stdout+stderr
    fn now(&self) -> OffsetDateTime;
    fn is_tty(&self) -> bool;
    fn is_elevated(&self) -> bool;
    fn read_key(&self) -> io::Result<char>;          // single keypress for gates
}
pub struct SystemHost;        // real impl (#[cfg(windows)] specializations)
pub struct MockHost { /* in-memory FS + recorded calls + scripted keys */ }

// restore.rs — plan §6.2; the try/finally analogue is a Drop guard
pub struct RestoreRegistry { /* LIFO of boxed undo closures */ }
impl RestoreRegistry {
    pub fn register(&mut self, label: &str, undo: Box<dyn FnOnce(&dyn Host) -> Result<()>>);
    pub fn drain(&mut self, host: &dyn Host) -> Vec<CrumbError>;   // never panics
}   // Drop impl calls drain() so it runs on early return / panic

// fingerprint.rs — plan §13.2
pub struct HostFingerprint { ini_sha: String, cache: Vec<(String,String)>,
                             daemon_state: String, env: BTreeMap<String,String> }
pub fn capture(host: &dyn Host, cfg: &Config) -> HostFingerprint;
pub fn diff(before: &HostFingerprint, after: &HostFingerprint) -> Vec<String>; // [] == clean

// gate.rs — plan §7, this doc §4
pub enum Mode { Guided, Interactive, AutoPilot, DryRun }
pub enum CardLevel { Full, Terse, None }
pub enum Decision { Proceed, ProceedNoop, Skip, Autopilot, Back, Abort }
pub struct Card { id, stage, step_num, step_total, title, why,
                  commands: Vec<String>, resources: Vec<String>, backups: Vec<String>,
                  est_time: String, recovery: String, long_why: String }
pub fn confirm(host: &dyn Host, mode: &mut Mode,
               seen: &mut HashSet<String>, card: &Card) -> Decision;
pub fn show_card(host: &dyn Host, card: &Card, level: CardLevel);
pub fn done_panel(host: &dyn Host, card: &Card, result: &StepResult);

// bundle.rs
pub fn new_bundle(host: &dyn Host, root: &Path, version: &str) -> io::Result<PathBuf>;
pub fn resolve_tool(explicit: Option<&str>, home: &str, path_name: &str) -> ResolvedTool;
```

`confirm()` logic (the heart — identical semantics to the prior pseudocode):
```
DryRun       -> show_card(Full); return ProceedNoop      (caller mutates nothing)
AutoPilot    -> return Proceed
Guided       -> level = seen.contains(id) ? Terse : Full; show_card; loop read_key
Interactive  -> show_card(Terse); loop read_key
on 'a'       -> *mode = AutoPilot (after y/N confirm); return Proceed
record id in seen
```

### 5.1 Tooling lifecycle (acquire → keep/remove) — `tooling.rs`

Two classes of host resource, treated differently — this is the "keep or remove
when done" requirement:

- **Pre-existing** (UFFS caches, `Everything.ini`, the daemon): only ever
  snapshotted + restored via `restore.rs`. **Never deleted.**
- **Acquired by the suite** (a downloaded `es.exe`, an installed helper): tracked
  as an `Acquisition` carrying a `Disposition`.

```rust
pub struct Acquisition { name: String, path: PathBuf, source: String, sha256: String,
                         acquired_at: OffsetDateTime, disposition: Disposition }
pub enum Disposition { Keep, Remove }
```
Rules:
- Guided/Interactive: **prompt** per acquisition at teardown
  ("keep `es.exe` in `bundle/tools/`? [k]eep / [r]emove").
- AutoPilot: default **Remove** (override with `--keep-tools`); DryRun never acquires.
- Teardown removes only `Remove` acquisitions; everything kept is **logged** in
  `state.json` + the bundle, so nothing is silently left behind.
- Pre-existing resources are out of scope for removal — restore-only.

### 5.2 State file + resumability — `state.rs`

A single `bundle/state.json` (atomic write after every step) records every
decision and every completed step, so a re-run **skips work already done**.

```rust
pub struct State { suite_version: String, started_at: OffsetDateTime, updated_at: OffsetDateTime,
                   decisions: Decisions,            // mode, drives, tools, rounds, drop_cache…
                   acquisitions: Vec<Acquisition>,  // §5.1
                   steps: BTreeMap<StepId, StepRecord> }
pub struct StepRecord { status: Status, input_hash: String, outputs: Vec<String>,
                        started_at: OffsetDateTime, finished_at: Option<OffsetDateTime> }
pub enum Status { Pending, Done, Skipped, Failed }
```
Resume engine:
- On start, load `state.json` from `--bundle <dir>` (or auto-detect the newest);
  else start `New`.
- Per step, compute `input_hash` from the decisions it depends on. A `Done`
  record with the **same** `input_hash` ⇒ **skip** ("✓ cached"). Change a decision
  ⇒ dependents' hashes change ⇒ they re-run automatically.
- `--redo <step>` invalidates one step; `--force` invalidates all; both rewrite
  `state.json`.
- Completed result files stay in the bundle, so resuming never re-measures.
- `state.json` + kept tooling are **excluded** from the fingerprint diff (they are
  logged decisions, not crumbs).

---

## 6. Phase-by-phase build tasks

Each phase is independently reviewable and leaves the tree green. **[macOS/Linux
OK]** phases need no Windows.

### P1 — Hygiene + UX + state spine (`crates/uffs-bench`) [any OS]

- **Goal:** restore registry, gates, modes, card renderer, host fingerprint,
  **state file + resume engine**, and **tooling dispositions** — zero
  measurement — proven by `cargo nextest`.
- **Files:** new crate `crates/uffs-bench` with `host.rs, restore.rs, gate.rs,
  state.rs, tooling.rs, fingerprint.rs, bundle.rs` (§5) + `tests/`.
- **Build steps:**
  1. `cargo new --lib crates/uffs-bench`, add a `[[bin]]`, register it in the
     workspace `members`, inherit the workspace `[lints]`. Add deps with
     `cargo add serde serde_json toml sha2 sysinfo clap` (+ workspace `thiserror`).
     **Use cargo — do not hand-edit `Cargo.toml`.**
  2. Implement the §5 module API. Every side effect goes through `Host`;
     `SystemHost` is real, `MockHost` records calls + in-memory FS + scripted keys.
  3. Implement `confirm` exactly as the §5 pseudocode; `show_card`
     (Full/Terse/None) + `done_panel` from the §4.6 `Card`.
  4. Implement `RestoreRegistry` with a `Drop` guard so `drain()` runs on early
     return / panic.
  5. Implement `fingerprint::{capture,diff}`, the `state.rs` resume engine
     (§5.2), and `tooling.rs` dispositions (§5.1).
- **DoD:** the four plan §13.3 assertions pass as nextest tests — (a) every
  mutation registers a restore *before* mutating; (b) `drain()` empties the
  stack; (c) an injected panic still restores everything; (d) DryRun performs
  zero mutations. Plus: `a` flips to autopilot; SeenCards dedupe; **resume skips
  a `Done` step with a matching `input_hash`**; acquired tools are removed on
  teardown unless `Keep`, pre-existing resources are never removed.
- **How to test:** `cargo nextest run -p uffs-bench`.

### P2 — Stage 0a env fingerprint (`env.rs`) [logic any OS; full data Windows]

- **Goal:** emit `bundle/env.json` + rendered `bundle/env.md`.
- **Files:** `crates/uffs-bench/src/env.rs`; extend `tests/`.
- **Build steps:** collect the plan §8.1 table via `sysinfo` (CPU/RAM/disks,
  cross-platform) + tool versions through `Host::run("uffs",["--version"])` /
  `es.exe -get-everything-version` + `Host::is_elevated`. Render `env.md` as the
  §Test-environment markdown table.
- **DoD:** on Windows, `env.json` fields are non-empty vs a manual spot check; on
  any OS, the renderer turns a fixture `EnvFingerprint` into the expected table
  (golden test).
- **How to test:** golden test in `tests/` (fixture → `env.md`).

### P3 — Stage 0c competitor preflight (read-only) (`preflight.rs`) [Windows-live]

- **Goal:** answer "what is Everything actually holding?" **without writing**
  `Everything.ini`.
- **Files:** `crates/uffs-bench/src/preflight.rs`; extend `tests/`.
- **Build steps:**
  1. **Lift, don't fork:** move the read-only helpers out of
     `everything_capacity_probe.rs` — `parse_drives_from_ini` (ini → configured
     drives) and the `wait_for_index` poll — into `preflight.rs`. It **never**
     calls `isolate_drive_in_ini`/`fs::write`; all writes go through `Host` so a
     test can assert none occur.
  2. For each candidate drive run `es.exe "<D>:\" -get-result-count` via
     `Host::run`: `>0` ⇒ loaded+hot (record count); `0`/error ⇒ not loaded.
  3. Estimate per-(drive,pattern) result size via UFFS, compare to the Everything
     IPC ceiling ⇒ `es_feasible`.
  4. Write `bundle/competitor-preflight.json`.
- **DoD:** a nextest test asserts the preflight makes **zero**
  `Host::write_file`/`remove_file` against the ini; output JSON has
  `{configured,loaded,hot,record_count,es_feasible}` per drive.
- **How to test:** `cargo nextest run -p uffs-bench preflight` (MockHost) + a
  live run on the reference box.

### P4 — Stage 0d matrix + 0e plan gate + orchestrator skeleton (`matrix.rs`, `main.rs`) [any OS]

- **Goal:** compute capable/cross/UFFS-only cells, present the plan gate, and wire
  resume.
- **Files:** `crates/uffs-bench/src/{matrix.rs,main.rs}`; first `just` recipes.
- **Build steps:**
  1. `compute_matrix` from plan §8.4: inputs = required tools, candidate drives,
     preflight JSON; outputs = `cross_cells`, `uffs_only` (+reason), `excluded`
     (+reason). Write `bundle/matrix.json`. Render the 0e gate card (env summary +
     resolved binaries + preflight table + negotiated matrix + est runtime + full
     mutate-list).
  2. CLI via `clap`: `--guided/--auto/--dry-run`, `--drives/--tools/--rounds`,
     `--only-stage/--from-stage`, `--bundle <dir>` (resume), `--redo/--force`,
     `--bundle-root`, `--drop-os-cache`, `--keep-tools`.
  3. Skeleton: load-or-new bundle + `state.json`, run Stage 0, wrap execution in
     a restore stack so Stage 5 verify always runs (Drop guard). Stages 1–3 are
     stubs that print their cards. Resuming a completed Stage 0 skips straight to
     Stage 1.
  4. Recipes: `bench-suite`, `bench-suite-auto`, `bench-suite-dry`,
     `bench-preflight`, `bench-resume`.
- **DoD:** a fixture preflight where Everything has only C,D ⇒ matrix puts
  E/F/M/S into `uffs_only` with reason "es not loaded"; `--dry-run` walks all
  gates and mutates nothing; non-TTY without `--auto/--dry-run` refuses;
  **resume of a `Done` Stage 0 jumps to Stage 1**.
- **How to test:** `cargo nextest run -p uffs-bench matrix` + `just bench-suite-dry`.

### P5 — Stage 1 (cross-tool) + Stage 2 (parity) wrappers (`stages.rs`) [Windows-live]

- **Goal:** run the two existing leaf harnesses with snapshot/restore around them,
  resumably.
- **Files:** `crates/uffs-bench/src/stages.rs`.
- **Build steps:**
  1. Stage 1: snapshot R1 (daemon) + R5 (temp csv); register restores; `Host::run`
     → `rust-script cross-tool-benchmark.rs --skip-cold --drives <capable>
     --tools <required> --patterns <feasible> --rounds N --out
     bundle/cross-tool-summary.csv`. COLD sub-phase also snapshots+restores R2.
  2. Stage 2: snapshot R1/R2/R5; `Host::run` → `pwsh cold-parity-per-drive.ps1
     -Drives <capable> -OutputFile bundle/parity.txt` (+ a `-PurgeCacheFirst`
     pass); register a daemon-state restore around it.
  3. Each step records `Done` in `state.json` and carries a real §4.6 card so
     guided mode narrates it.
- **DoD:** bundle gains `cross-tool-summary.csv` + `parity.txt`; the post-run
  fingerprint diff is clean; restore rebuilds purged caches; **re-run skips the
  `Done` stage and resumes at the next**.
- **How to test:** `just bench-suite --only-stage 1` then `--only-stage 2` on the
  reference box; confirm `fingerprint-after == fingerprint-before` and that a
  second run reports the stage cached.

### P6 — Stage 3 full-suite (de-manualize §3) (`stages.rs`) [Windows-live]

- **Goal:** port the hand-pasted "SECTION 0 … Get-Stats" flow into Rust.
- **Files:** extend `crates/uffs-bench/src/stages.rs`.
- **Build steps:** port the flow that produced
  `raw/2026-04-v0.5.66_full-benchmark-suite.txt`: drive-accumulation scale sweep
  (repeated daemon starts), 30-round targeted-query latency per pattern, full-scan
  `*`→CSV export throughput (p50/10 rounds), daemon RSS sampling — all through
  `Host::run` + output parsing. Add a unit-tested `percentiles()` helper (don't
  inline). Emit `bundle/full-suite.txt` + `bundle/full-suite.csv`.
- **DoD:** re-running reproduces the existing raw log within noise; output CSV
  uses the common column convention (plan §11).
- **How to test:** `just bench-suite --only-stage 3`; diff summary vs the raw log;
  `percentiles()` unit test.

### P7 — Stage 4 assembly + report scaffold (`report.rs`) [any OS]

- **Goal:** assemble the bundle and scaffold a dated `REPORT-DRAFT.md`.
- **Files:** `crates/uffs-bench/src/report.rs` (pure bundle writes).
- **Build steps:** render `env.md` into the report's environment table; paste the
  §1/§2/§3 markdown tables; insert raw-log citations; write
  `bundle/REPORT-DRAFT.md` named `YYYY-MM-vX.Y.Z-<scope>.md`. **Never** auto-commit;
  promotion to `docs/benchmarks/` is a manual reviewed step.
- **DoD:** draft renders with real env + tables from a fixture bundle; nothing is
  written outside the bundle dir.
- **How to test:** golden test: fixture bundle → expected `REPORT-DRAFT.md` shape.

### P8 — Competitor pinning + fetch + version-drift cleanup (`tooling.rs`) [any OS]

- **Goal:** one pinned competitor version everywhere, fetched as a tracked
  acquisition.
- **Files:** `scripts/windows/competitors.toml`; recipe `bench-fetch-competitors`.
- **Build steps:** create `competitors.toml` (plan §12). `bench-fetch-competitors`
  downloads `es.exe` to `bundle/tools/`, verifies SHA-256, **fails closed** on
  mismatch, and records it as an `Acquisition` (§5.1) with the operator-chosen
  disposition — never installs system-wide, never vendors into the repo (open
  decision #1). **Resolve the drift:** README cites `1.1.0.30`, cross-tool header
  references `1.5.x`, the probe looks elsewhere — pick one, make every script/doc
  cite `competitors.toml`.
- **DoD:** a bad hash aborts the fetch; the acquisition appears in `state.json`
  with a disposition; `grep` shows a single es version across scripts/docs.
- **How to test:** unit test the hash check with a tampered fixture (expect abort).

### P9 — End-to-end + crash recovery + verify [Windows-live]

- **Goal:** the full chain on the reference machine, provably crumb-free and
  resumable.
- **Files:** recipes `bench-restore`, `bench-suite-verify` (note the rename from
  the colliding `bench-verify`); update plan §14 to match.
- **Build steps:** Stage 5 teardown drains the restore stack, runs the fingerprint
  diff, resolves tooling dispositions (§5.1), serializes `restore-manifest.json`,
  and finalizes `state.json`. `bench-restore <bundle>` re-applies the manifest
  after a hard kill; `bench-suite-verify <bundle>` re-runs the fingerprint diff.
- **DoD (acceptance, plan §17):** a clean `just bench-suite` run yields a
  self-describing bundle with zero hand-typed env; cross-tool cells auto-limited
  to capable drives; `fingerprint-after == fingerprint-before`; a simulated
  mid-run kill is fully recovered by `just bench-restore`; **a restart resumes
  from `state.json`, skipping completed stages**.
- **How to test:** full run on the reference box; kill it mid-Stage-2 and prove
  `bench-restore` restores the cache+ini+daemon and a restart resumes;
  `bench-suite-verify` reports clean.

---

## 7. Testing strategy (the safety net)

**Framework:** `cargo nextest` — the **same** lane as `just go`/CI, so the
"no-crumbs" self-test is actually enforced (a separate Pester ecosystem would
never run in the pipeline). Tests live in `crates/uffs-bench/tests/` + `#[cfg(test)]`
modules. Recipe `just bench-test` → `cargo nextest run -p uffs-bench`.

**The DI seam is the `Host` trait** (§5). `SystemHost` is real; `MockHost`
records every call, backs an in-memory FS, and replays scripted keypresses — so
**no real MFT/daemon/ini/network is touched on any OS**:

```rust
let host = MockHost::new()
    .with_file("Everything.ini", b"...")
    .with_keys(['\n', 'a']);                 // Enter, then autopilot
run_stage(&host, &mut state, &card)?;
assert!(host.writes_to("Everything.ini").is_empty());      // preflight is read-only
assert_eq!(host.run_calls()[0].cmdline, card.commands[0]); // shown == executed
```

**Required tests (map to the DoDs above):**
1. spine (`restore`/`gate`/`state`/`tooling`) — restore LIFO order;
   register-before-mutate; panic-still-restores (Drop guard); DryRun
   zero-mutation; `a`→autopilot; SeenCards dedupe; fingerprint detects a tampered
   ini; **resume skips a `Done` step**; acquired tool removed unless `Keep`,
   pre-existing never removed.
2. gate/card — render Full/Terse/None per mode; **the exact command shown ==
   command dispatched** (assert via `host.run_calls()`); `e`/`c`/`?` produce no
   `run` calls; DONE panel prints the real exit code.
3. matrix — C,D-only preflight ⇒ E/F/M/S land in `uffs_only` with the right
   reason; IPC-infeasible pattern excludes `es` for that cell.
4. preflight — **zero** `write_file`/`remove_file` against the ini.
5. competitors fetch — tampered hash ⇒ abort.

**CI:** all run cross-platform under `cargo nextest` (no MFT). Gate live-only
cases with `#[ignore]`; run them on elevated Windows via
`cargo nextest run -p uffs-bench -- --ignored` (matches the repo convention).
Wire `just bench-test` into the test lane; **ask the user** before adding it to
the default `just go` gate (open decision).

---

## 8. `just` recipes to add (final surface)

| Recipe | Delegates to | Phase |
|---|---|---|
| `bench-suite` | `uffs-bench` (guided/interactive auto-default) | P4 |
| `bench-suite-auto` | `uffs-bench --auto` | P4 |
| `bench-suite-dry` | `uffs-bench --dry-run` | P4 |
| `bench-preflight` | `uffs-bench --only-stage 0` | P4 |
| `bench-resume <bundle>` | `uffs-bench --bundle <dir>` (skip `Done` steps) | P4 |
| `bench-fetch-competitors` | `uffs-bench fetch-competitors` (download + verify SHA-256) | P8 |
| `bench-restore <bundle>` | `uffs-bench restore --bundle <dir>` (re-apply `restore-manifest.json`) | P9 |
| `bench-suite-verify <bundle>` | `uffs-bench verify --bundle <dir>` (was `bench-verify` — **renamed**) | P9 |
| `bench-test` | `cargo nextest run -p uffs-bench` | P1 |

---

## 9. Handoff checklist (tick before calling it done)

- [ ] P1–P9 each green via `cargo nextest run -p uffs-bench` **and** `just go`.
- [ ] `just bench-suite-dry` walks the full plan, mutates nothing.
- [ ] First run on a fresh box defaults to **Guided**; every step shows a card +
      DONE panel; `a` cleanly switches to autopilot.
- [ ] `state.json` written after every step; `just bench-resume` skips completed
      steps on a re-run.
- [ ] Acquired tools honor Keep/Remove; pre-existing resources are never deleted;
      dispositions logged in `state.json`.
- [ ] Live run: bundle is complete + self-describing; `fingerprint-after ==
      fingerprint-before`.
- [ ] Mid-run kill recovered by `just bench-restore`; a restart resumes from
      `state.json`.
- [ ] One pinned competitor version across all scripts/docs.
- [ ] Plan §14 updated for the `bench-verify` → `bench-suite-verify` rename;
      plan reflects the Rust-orchestrator decision.
- [ ] Healing log written; competitor-redistribution decision (plan §16 #1)
      recorded in `state.json`.

---

*Build P1 first. Do not touch a real machine until the §13.3 self-tests are green
under `cargo nextest`.*
