<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

# Memory-tiering Windows-host validation runbook

**Audience:** operator with shell access to a Windows box that has the
target dataset loaded under the daemon (the 7-drive reference box, or
any equivalent machine with at least 4 indexed NTFS volumes).
**Source-of-truth tracker:** the live progress doc lives in
`docs/refactor/memory-tiering-implementation-plan.md` (gitignored —
local working copy on the implementer's machine).  Use this runbook
when you want the **what to run on Windows** without the
implementation-side context.

The four Phase 5 operator gates that the all-Mac unit-test suite cannot
exercise are listed in §1 below.  §2 covers the deferred Phase 6
gate (24-h `min_tier = "Warm"` soak) since it shares the same
operator workflow and the same `uffsd.exe` process.  §3 is the
"what to capture" checklist for the PR description.

---

## 0. Prerequisites

* Windows 10 1709+ or Windows 11 (the `MEMORY_RESOURCE_NOTIFICATION`
  API has been stable since 1709; older Windows editions degrade to
  the never-fires path documented in `crate::cache::pressure`).
* The daemon binary built from the branch under test, copied to the
  host (or built locally with `cargo build --release -p uffs-daemon`).
* The seven NTFS volumes loaded against the daemon — confirm with:
  ```powershell
  uffs status --drives
  ```
  Expect `Ready` plus a per-drive table showing `[Hot]` / `[Warm]`
  markers.  If any drive shows `[Parked]` / `[Cold]` from the start
  the gate setup is wrong; bounce the daemon (`uffs daemon stop` →
  `uffs daemon start --drives C,D,E,F,G,M,S`).
* Task Manager → **Details** tab → enable the **I/O priority** column
  via column-header → *Select columns…* → check `I/O priority`.  This
  is required for gate **G3** (USN catch-up I/O priority capture).
* PowerShell 5.1 or PowerShell 7 — the snippets below use
  `Start-Process` / `Get-Counter` / `Wait-Event` which exist in both.

---

## 1. Phase 5 operator gates — 4 captures, ~75 minutes wall-clock

### G1 — Low-pressure stress: kernel notification → cache.pressure → cascade demote

**Duration:** ~5 min wall-clock.
**Plan reference:** `docs/refactor/memory-tiering-implementation-plan.md` §3 Phase 5 line "Stress test: spawn allocator that drains free RAM."

#### Setup

Open two terminals on the same Windows host.  In terminal A start the
daemon with structured logging:

```powershell
$env:RUST_LOG = "uffs_daemon=info,cache.pressure=info,shard.transition=info"
uffs daemon start --drives C,D,E,F,G,M,S 2>&1 | Tee-Object -FilePath C:\Temp\uffsd-G1.log
```

Wait until `uffs status` reports `Ready` for every drive.

#### Drive

In terminal B, allocate memory until the kernel publishes
`LowMemoryResourceNotification`.  The simplest portable driver is a
`testlimit64`-style PowerShell loop (no separate binary required):

```powershell
# Allocate 1 GiB chunks until the OS pushes back; release on Ctrl-C.
$alloc = New-Object System.Collections.Generic.List[byte[]]
try {
  while ($true) {
    $alloc.Add((New-Object byte[] 1073741824))
    Write-Host ("allocated {0} GiB; free = {1:N0} MiB" -f `
      $alloc.Count, ((Get-Counter '\Memory\Available MBytes').CounterSamples[0].CookedValue))
    Start-Sleep -Seconds 1
  }
} finally {
  $alloc.Clear()
  [GC]::Collect()
  Write-Host "released"
}
```

The kernel typically fires `LowMemoryResourceNotification` once
**Available MBytes** drops below ~256 MB on a 16 GB box (Windows
auto-tunes the threshold to ~32 MB on the lower end and
`PhysicalMemory / 64` on the upper end — see Microsoft Learn's
[CreateMemoryResourceNotification][win32-mem]).

#### Capture (this is what you paste into the PR)

The daemon log (`C:\Temp\uffsd-G1.log`) must show:

```text
INFO cache.pressure: Pressure transition observed level=Low
INFO shard.transition: Pressure cascade demoted one LRU Warm shard drive=… from=Warm to=Parked reason=pressure-cascade
INFO shard.transition: Pressure cascade demoted one LRU Warm shard drive=… from=Warm to=Parked reason=pressure-cascade
…
```

— one cascade line per Warm shard.  The `drive` field's order must
follow oldest-`last_query_at_ms`-first (LRU contract pinned by
`crate::index::tests::lifecycle_hooks::cascade_demote_one_step_picks_lru_warm_and_drains_in_order`
+ `pressure_subscriber_drains_warm_cascade_on_low_and_no_ops_on_high`).

When you Ctrl-C the allocator the kernel fires
`HighMemoryResourceNotification`.  The log must show:

```text
INFO cache.pressure: Pressure transition observed level=High
```

— and **no further** cascade lines after that timestamp.

[win32-mem]: https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification

---

### G2 — Working set drops in Task Manager within 5 s of demote

**Duration:** ~3 min wall-clock (re-uses the G1 terminal A log).
**Plan reference:** §3 Phase 5 line "Working set drops in Task Manager within 5 s of demote."

#### Drive

Re-run the G1 allocator loop with Task Manager → *Details* visible.

* Note `uffsd.exe` **Working Set (Memory)** before the allocator starts (`baseline_ws`).
* Press the kernel into firing `Low` (allocator runs).
* Within 5 s of the first `shard.transition` cascade line in
  `uffsd-G1.log`, `uffsd.exe` **Working Set** must drop below
  `baseline_ws / 2` (typical observation: 5341 MB → ~1 200 MB on the
  7-drive reference box).

#### Capture

Two screenshots in the PR description, both with
`Get-Date | Out-Host` visible at the top:

1. Task Manager → *Details* showing `uffsd.exe` `WorkingSet` at the
   peak (just before `Low` fires).
2. Task Manager → *Details* showing `uffsd.exe` `WorkingSet` ≤ 5 s
   after the first cascade line.

The drop is caused by the per-batch `EmptyWorkingSet` call wired in
Phase 5 task 5.4 (`crate::index::transitions::IndexManager::demote_idle_shards`
→ `WorkingSetTrim::trim`).  Mac/Linux ship a no-op stub.

---

### G3 — Background-IO priority during USN catch-up

**Duration:** ~6 min wall-clock (the USN refresh runs every 5 min by default).
**Plan reference:** §3 Phase 5 line "During USN catch-up, Task Manager I/O priority on `uffsd.exe` shows 'Low'."

#### Setup

Force a USN catch-up by reducing the cadence — easier than waiting
the default 5 min between ticks.  In terminal A:

```powershell
$env:UFFS_USN_REFRESH_INTERVAL_SECS = "30"
$env:RUST_LOG = "uffs_daemon=info,shard.refresh=info"
uffs daemon stop
uffs daemon start --drives C,D,E,F,G,M,S 2>&1 | Tee-Object -FilePath C:\Temp\uffsd-G3.log
```

The 30-second cadence guarantees a refresh tick within ~1 minute of
each Warm shard becoming visible to the controller.

#### Capture

Watch Task Manager → *Details* → the **I/O priority** column on
`uffsd.exe`.  During each `shard.refresh` window (the 30-second tick
between `INFO shard.refresh: USN refresh tick starting count=N` and
`INFO shard.refresh: USN refresh tick complete refreshed=N failed=0`),
`uffsd.exe` should show **Low** (or **Background**, depending on the
Task Manager localisation).  Outside the tick window the priority
returns to **Normal**.

The transition is driven by `crate::cache::background_io::BackgroundIoScope`
RAII guards wrapped around each per-letter `tokio::task::spawn_blocking`
closure in `crate::index::transitions::IndexManager::refresh_usn_for_warm_shards`
(Phase 5 task 5.7).  Mac/Linux: no-op (the trait stub returns
`Ok(())`).  Windows: `SetThreadPriority(GetCurrentThread(),
THREAD_MODE_BACKGROUND_BEGIN)` on enter,
`THREAD_MODE_BACKGROUND_END` on drop.

#### Capture for the PR

Screenshot of Task Manager *Details* with `uffsd.exe` showing
`I/O priority = Low` (or `Background`), timestamped against a log
line in `uffsd-G3.log` of the form:

```text
INFO shard.refresh: USN refresh tick starting count=… interval_secs=30
```

Reset before moving on:

```powershell
Remove-Item Env:\UFFS_USN_REFRESH_INTERVAL_SECS
uffs daemon stop
```

---

### G4 — 1-hour sustained-pressure soak: no OOM

**Duration:** 60 min wall-clock (one operator-attended setup; runs unattended).
**Plan reference:** §3 Phase 5 line "1-hour sustained-pressure test: no OOM."

#### Drive

Run the G1 allocator loop **continuously** for 60 minutes with the
daemon under it.  Replace the `Start-Sleep -Seconds 1` in the G1
snippet with a longer cadence so the allocator doesn't add new
chunks faster than the kernel can publish notifications:

```powershell
$env:RUST_LOG = "uffs_daemon=info,cache.pressure=info,shard.transition=info"
$start = Get-Date
$alloc = New-Object System.Collections.Generic.List[byte[]]
while ((Get-Date) -lt $start.AddMinutes(60)) {
  if ($alloc.Count -lt 12) { $alloc.Add((New-Object byte[] 1073741824)) }
  Start-Sleep -Seconds 5
}
$alloc.Clear()
[GC]::Collect()
```

Periodically the allocator drives free RAM under the threshold; the
daemon cascades; when the kernel fires `High` the cascade stops.
Repeat ad infinitum for 60 minutes.

#### Capture

Acceptance criteria:

* `uffsd.exe` is still running (`Get-Process uffsd | Select Id, WS`).
* No `OutOfMemoryError` lines in `uffsd-G4.log`.
* No `panic` lines.
* `uffs status --drives` returns within 1 s after the soak completes;
  the per-drive tier markers reflect a coherent state (some `[Parked]`,
  some `[Hot]`/`[Warm]` where the operator drove queries).

#### Capture for the PR

* `Get-Process uffsd | Select Id, WS, PM, NPM, VM, CPU, StartTime`
  output captured at 0 min, 30 min, and 60 min.
* The tail of `uffsd-G4.log` (last 200 lines) — should show
  alternating `level=Low` / `level=High` transitions and no fatal
  errors.

---

## 2. Phase 6 operator gate — 24-hour `min_tier = "Warm"` soak

**Duration:** 24 h wall-clock (set up and walk away).
**Plan reference:** `docs/refactor/memory-tiering-implementation-plan.md` §3 Phase 6 Windows gate.

#### Setup

Write a `daemon.toml` to the platform-default path
(`%LOCALAPPDATA%\uffs\daemon.toml`) with `min_tier = "Warm"` for `C:`:

```powershell
$cfgPath = "$env:LOCALAPPDATA\uffs\daemon.toml"
@'
[shards.per_drive."C:"]
min_tier = "WARM"
'@ | Set-Content -Encoding UTF8 -Path $cfgPath
uffs daemon stop
$env:RUST_LOG = "uffs_daemon=info,shard.ttl=debug,shard.transition=info"
uffs daemon start --drives C,D,E,F,G,M,S 2>&1 | Tee-Object -FilePath C:\Temp\uffsd-phase6-soak.log
```

#### Drive

**Do nothing for 24 h.**  No queries against any drive; let the
adaptive-TTL ladder drive demotions naturally.

#### Capture

Acceptance criteria (the Phase 6 contract under `[shards.per_drive]`):

1. **C never demotes below `Warm`.**  Grep the soak log:
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'shard.transition.*drive=C.*to=Parked' -List
   ```
   Expect **zero matches**.  Every `min-tier-clamp` event for C is
   logged at debug-level via `shard.ttl`:
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'shard.ttl.*drive=C.*reason="min-tier-clamp"' -Context 0,0 |
       Select-Object -First 5
   ```

2. **Other drives demote normally** (Warm → Parked at the configured
   `warm_ttl_base_secs`):
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'shard.transition.*drive=[DEFGMS].*from=Warm.*to=Parked' -List
   ```
   Expect at least one match per drive (D, E, F, G, M, S).

3. **Different TTLs for high-rate vs low-rate drives.**  After the
   soak, drive a synthetic 5-min load against C, leave the others
   idle, and grep `shard.ttl` for the per-drive `chosen_ttl_sec`
   field:
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'shard.ttl' |
       Select-String -Pattern 'chosen_ttl_sec' |
       Select-Object -Last 30
   ```
   The C row's `chosen_ttl_sec` should exceed the others' by the
   `60·log2(rate)` bonus from the §5.2 formula
   (`crate::cache::policy::hot_ttl`).

#### Reset

```powershell
Remove-Item $cfgPath
uffs daemon stop
```

---

## 3. PR-attachment checklist

Before opening the Phase 5 / Phase 6 acceptance PR, paste the
following into the description so the reviewer can sign off without
re-running the soak:

* **G1** capture — log excerpt showing `Low` → cascade chain →
  `High` with the LRU-ordered `drive=` field.
* **G2** capture — pair of Task Manager screenshots (peak / +5 s).
* **G3** capture — Task Manager screenshot showing
  `I/O priority = Low` during a `shard.refresh` tick window;
  matching log line.
* **G4** capture — `Get-Process uffsd` table at 0 / 30 / 60 min plus
  the soak-log tail.
* **Phase 6 24-h soak** capture — three grep results from §2 above
  (`drive=C…to=Parked` empty, peer-drive demotes present, different
  `chosen_ttl_sec` after synthetic load).

After all five captures land, update the implementation-plan §5.1
row for the corresponding phase to 🟢 with the date the PR landed.
