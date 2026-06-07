# UFFS benchmark suite — operator runbook

**Audience:** the person sitting at the Windows benchmark machine, about to run or publish a
benchmark. Not the person designing the methodology (see [`methodology.md`](methodology.md)) or
building the orchestrator (see [`robust-benchmark-flow-implementation-guide.md`](robust-benchmark-flow-implementation-guide.md)).

**TL;DR for an experienced operator:**

```powershell
# one-time setup already done? skip to "Run a benchmark cycle" below.
just bench-suite --drives C,D,E,F,M,S --keep-tools
```

---

## Prerequisites

### Machine requirements

| Requirement | Detail |
|-------------|--------|
| **OS** | Windows 10/11 (NTFS volumes required; MFT read needs elevation) |
| **Elevation** | Run PowerShell / Windows Terminal **as Administrator** |
| **Drives** | At least one populated NTFS volume; multi-drive gives more data points |
| **RAM** | ≥ 16 GB recommended (UFFS daemon caches ~181 MB per million records) |
| **Disk** | ≥ 2 GB free on the system drive for bundle artifacts and tool backups |

### Software prerequisites

```powershell
# 1. Rust toolchain (stable, MSVC target)
winget install Rustlang.Rustup
rustup update stable
rustup target add x86_64-pc-windows-msvc

# 2. just (task runner)
winget install Casey.Just

# 3. rust-script (required for Stage 1 cross-tool harness)
cargo install rust-script

# 4. Everything GUI engine 1.4.1.1032 (the daemon es.exe talks to over IPC).
#    es.exe is a thin wrapper — the engine version determines search behaviour
#    and performance. Install the GUI first:
#    https://www.voidtools.com/Everything-1.4.1.1032.x64-Setup.exe
#
#    Then fetch + SHA-256-verify the pinned ES CLI (1.1.0.30) automatically:
just bench-fetch-competitors --keep-tools
```

### UFFS binary — resolution cascade

The orchestrator resolves `uffs.exe` automatically using the same cascade as
all UFFS validation scripts. It does **not** auto-build; you control which
artifact is exercised:

| Priority | Location | How to get there |
|----------|----------|------------------|
| 1st | `%USERPROFILE%\bin\uffs.exe` | `just use` — installs the latest release or a dist build |
| 2nd | `target\release\uffs.exe` | `cargo build --release -p uffs -p uffsd` |
| 3rd | `uffs.exe` on PATH | any other install method |

If none of the above exist, the orchestrator surfaces the OS "executable not
found" error with the search path — clearer than a silent failure.

To pin a specific binary for a run:

```powershell
just bench-suite --bin C:\path\to\my\uffs.exe --drives C,D
```

---

## Run a benchmark cycle

### Option A — guided (recommended for first run or after a gap)

```powershell
just bench-suite --drives C,D,E,F,M,S
```

The orchestrator walks you through each stage with a gate card showing the exact
commands it will run, the resources it will touch, and the estimated time. Press
**`y`** to proceed, **`s`** to skip a stage, **`b`** to back up, **`q`** to abort cleanly.

On first run you will also be asked about `--keep-tools` (whether to retain the
downloaded `es.exe` after the run).

### Option B — autopilot (CI / unattended)

```powershell
just bench-suite-auto --drives C,D,E,F,M,S --keep-tools
```

No prompts. All stages run sequentially. Snapshot/restore and fingerprint
verification still run — the hygiene is never skipped.

### Option C — dry run (see what would happen, mutate nothing)

```powershell
just bench-suite-dry --drives C,D
```

Renders every gate card and the negotiated matrix. Nothing is written, no
daemon is touched, no tools are downloaded.

### Option D — preflight only (plan + sanity check, no measurement)

```powershell
just bench-preflight --drives C,D,E,F,M,S
```

Runs Stage 0 only: environment fingerprint, competitor preflight (is `es.exe`
reachable? does it respond to `--version`?), and matrix negotiation (which
drives support cross-tool comparison vs UFFS-only). No measurement stages run.
Use this to validate the machine state before committing to a full run.

---

## What the orchestrator does (stage map)

| Stage | Name | What it does | Mutates host? |
|-------|------|--------------|---------------|
| 0a | Env fingerprint | Captures OS, CPU, RAM, hostname, tool versions | No |
| 0b | Competitor preflight | Probes `es.exe` version + drive indexing state | No |
| 0c | Matrix negotiation | Decides cross-tool vs UFFS-only per drive | No |
| 0d | Gate card | Shows the full plan; waits for operator approval | No |
| 1 | Cross-tool | Shells to `cross-tool-benchmark.rs` (UFFS vs Everything, n rounds) | Yes (daemon restart) |
| 2 | Parity | Shells to `cold-parity-per-drive.ps1` (cold Rust vs warm C++) | Yes (daemon + cache) |
| 3 | Native full-suite | Times UFFS natively, emits CSV + TXT | Yes (daemon) |
| 4 | Assembly | Assembles `REPORT-DRAFT.md` from bundle artifacts | No |
| 5 | Teardown | Drains restore stack, diffs fingerprints, resets manifest, saves state | Restores host |

Every mutation in stages 1–3 is preceded by a snapshot registered on the
restore stack. On teardown (stage 5) every snapshot is replayed in reverse
(LIFO) order — daemon state, cache files, tool downloads. The host is always
returned to its as-found state.

---

## Bundle directory layout

The orchestrator writes everything to a timestamped bundle directory under
`LOG/bench/<timestamp>/` (override with `--bundle <path>`):

```
LOG/bench/20260607T140000Z/
├── state.json                  # resume/audit record (do not edit)
├── fingerprint-before.json     # pre-run host state snapshot
├── fingerprint-after.json      # post-run host state snapshot (written at teardown)
├── restore-manifest.json       # crash-recovery serialized undo list
├── env.json / env.md           # Stage 0a environment capture
├── preflight.json              # Stage 0b competitor probe results
├── matrix.json                 # Stage 0c negotiated drive × tool matrix
├── cross-tool-summary.csv      # Stage 1 output
├── parity.txt                  # Stage 2 output
├── full-suite.csv              # Stage 3 output
├── full-suite.txt              # Stage 3 human summary
├── REPORT-DRAFT.md             # Stage 4 assembled draft
├── backup/                     # R2 UFFS cache file backups (restored at teardown)
└── tools/                      # Downloaded competitor binaries (if --keep-tools)
```

---

## Resuming an interrupted run

If a run is interrupted mid-way (Ctrl-C, reboot, stage failure), resume from
where it left off:

```powershell
just bench-resume LOG\bench\20260607T140000Z
```

The orchestrator loads `state.json`, skips all steps marked `Done`, and
continues from the first `Pending` step. Pass the same flags as the original
run (`--drives`, `--rounds`, etc.) to avoid a plan-hash mismatch.

---

## Crash recovery (hard kill / power loss)

If the process was killed while a mutation was in progress (before teardown
ran), the host may be in a partially mutated state. Use the `restore`
subcommand to replay the persisted undo list:

```powershell
just bench-restore LOG\bench\20260607T140000Z
```

This replays every entry in `restore-manifest.json` in LIFO order. On success
the manifest is reset to a no-op sentinel. Exits non-zero if any undo fails —
inspect the output, fix manually, then re-run to confirm.

After restoring, verify the host is clean:

```powershell
just bench-suite-verify LOG\bench\20260607T140000Z
```

Diffs the current host state against `fingerprint-before.json`. Exits non-zero
if any difference remains; writes `fingerprint-after.json` for forensics.

---

## Selecting stages and drives

```powershell
# Run only Stage 3 (native full-suite) on drives C and D
just bench-suite --only-stage 3 --drives C,D

# Start from Stage 2 onwards (skip Stage 0 and 1)
just bench-suite --from-stage 2 --drives C,D,E

# Re-run Stage 0 even if already Done in state.json
just bench-suite --redo

# Re-run all stages from scratch (ignore cached Done state)
just bench-suite --force
```

---

## Publishing results

After a successful run, `LOG/bench/<timestamp>/REPORT-DRAFT.md` contains a
pre-filled report scaffold. To promote it to a canonical benchmark report:

1. **Review the draft** — fill in the `<!-- TODO -->` placeholders, add the
   §Known regressions section, and verify every table number against
   `full-suite.csv` and `cross-tool-summary.csv`.

2. **Copy raw artifacts** into the tree:
   ```powershell
   copy LOG\bench\<timestamp>\full-suite.csv         docs\benchmarks\raw\<date>-vX.Y.Z_full-suite.csv
   copy LOG\bench\<timestamp>\cross-tool-summary.csv docs\benchmarks\raw\<date>-vX.Y.Z_cross-tool.csv
   copy LOG\bench\<timestamp>\parity.txt             docs\benchmarks\raw\<date>-vX.Y.Z_parity.txt
   ```

3. **Generate charts** from the CSV (see existing chart scripts in
   `scripts/windows/`) and commit them under
   `docs/benchmarks/charts/<date>-vX.Y.Z/`.

4. **Move the current canonical report** to `docs/benchmarks/archive/`
   (verbatim — no edits).

5. **Commit the new report** as `docs/benchmarks/<date>-vX.Y.Z-<scope>.md`
   and update `docs/benchmarks/README.md` to point at it.

6. **Update `competitors.toml`** if the competitor version changed:
   `scripts/windows/competitors.toml`.

---

## Flags reference

| Flag | Default | Description |
|------|---------|-------------|
| `--bin <path>` | auto-cascade | Override the `uffs.exe` path (skips cascade) |
| `--drives <C,D,…>` | `C` | Comma-separated NTFS drive letters to benchmark |
| `--tools <uffs,es,…>` | `uffs,everything` | Tool IDs for cross-tool stage |
| `--rounds <n>` | `10` | Measurement rounds per cell (30+ for publishable results) |
| `--only-stage <n>` | — | Run exactly stage N (0–4) and stop |
| `--from-stage <n>` | — | Skip stages before N |
| `--bundle <dir>` | auto-timestamped | Override bundle directory |
| `--redo` | false | Re-run Stage 0 even if already Done |
| `--force` | false | Invalidate all Done steps and re-run everything |
| `--keep-tools` | false | Retain downloaded competitor binaries after teardown |
| `--drop-os-cache` | false | Purge UFFS cache files before Stage 2 (cold parity) |
| `--auto` | false | Autopilot — no interactive prompts |
| `--dry-run` | false | Render plan only, mutate nothing |
| `--guided` | true (default) | Full gate cards on first visit, terse thereafter |

---

## Troubleshooting

**`es.exe` not found / preflight fails**
Run `just bench-fetch-competitors --keep-tools` to download and SHA-256-verify
the pinned competitor binary into the bundle.

**`uffs daemon` won't start (elevation error)**
The orchestrator requires an elevated shell. Re-run from an elevated PowerShell
or Windows Terminal.

**Stage 1 fails with `rust-script not found`**
Install it: `cargo install rust-script`.

**Fingerprint diff after teardown shows daemon still running**
The daemon restore undo (Stage 5) logs a crumb warning. Run
`uffs daemon stop` manually, then `just bench-suite-verify <bundle>` to
confirm the host is clean.

**`restore-manifest.json` replay fails for a cache file**
The cache backup in `<bundle>/backup/` may have been corrupted or the
destination path changed. Copy the backup file back manually:
```powershell
copy <bundle>\backup\C_index.uffs $env:LOCALAPPDATA\uffs\cache\C_index.uffs
```

**Run completed but `REPORT-DRAFT.md` is missing**
Stage 4 (assembly) was skipped or failed. Resume with:
```powershell
just bench-resume <bundle> --only-stage 4
```

---

## See also

- [`methodology.md`](methodology.md) — the fairness doctrine every published number must satisfy
- [`README.md`](README.md) — benchmark hub and current canonical report
- [`robust-benchmark-flow-execution-plan.md`](robust-benchmark-flow-execution-plan.md) — orchestrator design doc
- `just bench-help` — quick recipe reference in the terminal
