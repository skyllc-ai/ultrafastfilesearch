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
`testlimit64`-style PowerShell loop (no separate binary required).

> **Critical: pages must be touched, not just reserved.**  `New-Object
> byte[] 1073741824` on its own only commits virtual address space
> (Private Bytes climbs) but leaves the pages non-resident (Working Set
> flat), because Windows backs zero-filled pages on first write via the
> demand-zero handler.  The kernel's `LowMemoryResourceNotification`
> fires on **physical-RAM pressure**, not commit-charge pressure, so a
> reserve-only allocator never trips the cascade.  Force commit by
> writing a non-zero byte to every page — `[Array]::Fill($arr, [byte]1)`
> is the fastest portable way (CLR-native; fills 1 GiB in milliseconds).
> If you have Sysinternals installed, `testlimit.exe -d 1024 -c N`
> bypasses .NET entirely and is the canonical tool for this.

```powershell
# Allocate 1 GiB chunks AND commit by touching every page; auto-break on Low.
$alloc = New-Object System.Collections.Generic.List[byte[]]
try {
  while ($true) {
    $arr = New-Object byte[] 1073741824
    [Array]::Fill($arr, [byte]1)            # ← commits every page; the line above only reserves
    $alloc.Add($arr)
    $free = (Get-Counter '\Memory\Available MBytes').CounterSamples[0].CookedValue
    Write-Host ("allocated {0} GiB; free = {1:N0} MiB" -f $alloc.Count, $free)
    if ($free -lt 512) {
      Write-Host "Low-memory zone reached — holding 10 s for the cascade to drain, then releasing."
      Start-Sleep -Seconds 10
      break
    }
    Start-Sleep -Milliseconds 500
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

The daemon log (`C:\Temp\uffsd-G1.log`) must show (note: tracing
target names like `cache.pressure` and `shard.transition` set the
filter routing but are **not** rendered in the log message text under
the default formatter — grep on the message text and field names
shown below):

```text
INFO Pressure transition observed level=Low
INFO Pressure cascade demoted one LRU Warm shard drive=C from="Warm" to="Parked" reason="pressure-cascade" last_query_at_ms=…
INFO Pressure cascade demoted one LRU Warm shard drive=G from="Warm" to="Parked" reason="pressure-cascade" last_query_at_ms=…
…
```

Grep cheat-sheet:

```powershell
Select-String -Path C:\Temp\uffsd-G1.log -Pattern 'Pressure transition|Pressure cascade demoted' |
    Select-Object -ExpandProperty Line
```

— one cascade line per Warm shard.  The `drive` field's order must
follow oldest-`last_query_at_ms`-first (LRU contract pinned by
`crate::index::tests::lifecycle_hooks::cascade_demote_one_step_picks_lru_warm_and_drains_in_order`
+ `pressure_subscriber_drains_warm_cascade_on_low_and_no_ops_on_high`).

When you Ctrl-C the allocator the kernel fires
`HighMemoryResourceNotification`.  The log must show:

```text
INFO Pressure transition observed level=High
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

> **Per-thread, not per-process.**  `BackgroundIoScope` calls
> `SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)`
> on the **calling thread only** — specifically, on the
> `tokio::task::spawn_blocking` worker that runs the refresh closure
> for one drive.  Windows lowers that thread's **I/O priority** to
> `IoPriorityVeryLow` and **memory priority** to lowest; scheduling
> priority is left unchanged at Normal (Base Priority = 8) by
> design.  The **process-level** I/O priority (the column shown in
> PE's main window or Task Manager Details) stays **Normal**,
> because the daemon's main thread + IPC accept loop are not in
> background mode.  **Do not look at the process-row I/O Priority
> column — it will (correctly) always show Normal.**

Three capture paths, in increasing operator effort:

> **What changes when `THREAD_MODE_BACKGROUND_BEGIN` fires.**  The
> Win32 documentation says "the system lowers the resource scheduling
> priorities of the thread", but the **only** properties that actually
> change in observable Windows APIs are:
>
> * **I/O priority** — drops from `IoPriorityNormal` to
>   `IoPriorityVeryLow`.  Read via
>   `NtQueryInformationThread(ThreadIoPriority)` or PE's I/O Priority
>   column.
> * **Memory priority** — drops to `MEMORY_PRIORITY_LOWEST`.  Read
>   via `GetThreadInformation(ThreadMemoryPriority)`.
>
> The thread's **scheduling priority** (`GetThreadPriority`, the
> `THREAD_PRIORITY_*` enum) **stays at `THREAD_PRIORITY_NORMAL` /
> Base Priority = 8**, by design — background mode is meant to
> deprioritise I/O and memory without slowing CPU work when the
> system is idle.  This means **`.NET ProcessThread.PriorityLevel`
> and `Get-CimInstance Win32_Thread` will NOT show any change
> during a tick** (verified empirically: 60 s of 10 Hz polling
> across two ticks captured zero threads at lowered scheduling
> priority).  Only the I/O Priority column or the
> `NtQueryInformationThread` syscall surface the actual change.

**(a) Debug-log grep — text-only, definitive (preferred for PR
evidence):**

Clear the `RUST_LOG` env var (it overrides `--log-level`), restart
with `--log-level debug`, wait 2-3 ticks, then grep for
`BackgroundIoPriority`.  Empty grep = `SetThreadPriority` succeeded
on every per-drive worker; the wiring + the unit test
(`refresh_usn_for_warm_shards_wraps_each_closure_in_background_io_scope`)
complete the proof:

```powershell
uffs daemon stop
Remove-Item Env:\RUST_LOG -ErrorAction SilentlyContinue
$env:UFFS_USN_REFRESH_INTERVAL_SECS = "30"
uffs daemon start --log-level debug --log-file C:\Temp\uffsd-G3-debug.log

# Re-warm so refresh has work to do.
uffs "*" --ext rs --drive C --limit 5 > $null

Start-Sleep -Seconds 90

# Empty grep = success (begin() returned Ok on every closure).
"=== BackgroundIoPriority diagnostic (empty = good) ==="
Select-String -Path C:\Temp\uffsd-G3-debug.log -Pattern 'BackgroundIoPriority' |
    Select-Object -ExpandProperty Line

# Tick lines confirm the controller fired during the same window.
"=== Tick lines ==="
Select-String -Path C:\Temp\uffsd-G3-debug.log -Pattern 'USN refresh tick' |
    Select-Object -ExpandProperty Line
```

If the `BackgroundIoPriority` grep prints any `begin failed` lines,
that's a real bug — file a follow-up issue with the error string and
stop.  G3 stays 🟡 in that case.

**(b) Process Explorer Threads tab (GUI screenshot, redundant
verification):**

1. Right-click `uffsd.exe` → **Properties** → **Threads** tab.
2. Right-click any column header → **Select Columns** → **Threads**
   sub-tab → enable **I/O Priority**.  (Base Priority is *not*
   useful here — it stays at 8 in background mode.)
3. During a tick window (between the
   `USN refresh tick starting count=N` and
   `USN refresh tick complete refreshed=N` log lines), screenshot.
   Expect ≥ 1 thread with **I/O Priority = Very Low**.  Outside the
   tick, all threads show `I/O Priority = Normal`.

**(c) Sysinternals `accesschk.exe -p -t <pid>`** (if installed):
Dumps per-thread info including I/O priority — grep the output for
`I/O Priority: VeryLow` during a tick window.

> **What does NOT work for this gate:**
>
> * `Get-CimInstance Win32_Thread` — the WMI provider returns NULL
>   for `PriorityCurrent` / `BasePriority` on modern Windows.
> * `(Get-Process uffsd).Threads | Where PriorityLevel -in 'Idle',
>   'Lowest', 'BelowNormal'` — reads scheduling priority via
>   `GetThreadPriority`, which doesn't change in background mode.
> * Task Manager "Details" tab I/O Priority column — process-level
>   only; the daemon's main thread is at Normal so this column
>   stays Normal.

Grep the log for the matching tick window:

```powershell
Select-String -Path C:\Temp\uffsd-G3.log -Pattern 'USN refresh tick' |
    Select-Object -ExpandProperty Line
```

The transition is driven by `crate::cache::background_io::BackgroundIoScope`
RAII guards wrapped around each per-letter `tokio::task::spawn_blocking`
closure in `crate::index::transitions::IndexManager::refresh_usn_for_warm_shards`
(Phase 5 task 5.7).  Mac/Linux: no-op (the trait stub returns
`Ok(())`).  Windows: `SetThreadPriority(GetCurrentThread(),
THREAD_MODE_BACKGROUND_BEGIN)` on enter,
`THREAD_MODE_BACKGROUND_END` on drop.

##### Diagnostic if priority is not dropping

`BackgroundIoScope::begin()` failures are logged at **debug** level
under target `shard.refresh` and otherwise swallowed.  If `(b)` or
`(c)` show the threads stuck at Normal priority during a tick,
restart the daemon with `RUST_LOG=...,shard.refresh=debug` and
grep:

```powershell
Select-String -Path C:\Temp\uffsd-G3.log -Pattern 'BackgroundIoPriority' |
    Select-Object -ExpandProperty Line
```

A non-empty match on `begin failed` indicates a real bug (e.g., the
process token is missing `SeIncreaseBasePriorityPrivilege`, or an AV
product is intercepting `SetThreadPriority`); empty grep + threads
at Normal would mean the scope guard is not actually running.

#### Capture for the PR

**Preferred (path (a) above):** the empty `BackgroundIoPriority`
grep + populated `USN refresh tick` grep from a `--log-level debug`
run.  Two short text blocks, no GUI required.  This is sufficient
proof because:

1. The unit test
   `refresh_usn_for_warm_shards_wraps_each_closure_in_background_io_scope`
   in `crates/uffs-daemon/src/index/tests/usn_refresh.rs` already pins
   that `begin()` and `end()` are called exactly once per Warm shard
   per refresh tick.
2. The empty debug grep proves `SetThreadPriority` returned `Ok` for
   every per-drive worker (the only failure path is `tracing::debug!`
   target `shard.refresh` line `BackgroundIoPriority::begin failed`).
3. The populated tick grep proves the controller fired during the
   same observation window.

**Optional (path (b) above):** screenshot of PE Threads tab with the
**I/O Priority** column showing `Very Low` on a worker thread
during a tick.  Useful if a reviewer wants visual confirmation but
not required for sign-off.

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

Run an **adaptive** allocator continuously for 60 min.  The kernel's
`LowMemoryResourceNotification` threshold scales with physical RAM
(roughly 1.5% of total — ~1 GB on 64 GB hosts, ~512 MB on 32 GB), so
the allocator must drive sysAvailable down to that threshold to fire
Low; the only correct stop condition is "sysAvailable about to hit a
safety floor", **not** a hardcoded GiB cap based on guessed reserves
(an empirical lesson: a `TotalRAM − 26 GiB` cap stopped the allocator
at 38 GiB on a 64 GB host with sysAvailable still at 7 GB, never
firing Low).  The version below has only one stop condition — the
256 MB safety floor — so it grows as much as the host has headroom
for, then holds at the target while the daemon cascades:

```powershell
# Set up daemon log routing FIRST (before allocator), in a fresh
# shell so RUST_LOG doesn't carry over from a previous gate.
Remove-Item Env:\RUST_LOG -ErrorAction SilentlyContinue
$env:RUST_LOG = "uffs_daemon=info,cache.pressure=info,shard.transition=info"
uffs daemon stop
uffs daemon start --log-file C:\Temp\uffsd-G4.log

# In a second shell, run the adaptive allocator.
$targetAvailMB = 1024  # squeeze sysAvailable down to ~1 GB to fire Low
$safetyAvailMB = 256   # NEVER drop below this — allocator pauses growth
$rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()

$start = Get-Date
$end   = $start.AddMinutes(60)
$alloc = New-Object System.Collections.Generic.List[byte[]]

Write-Host "Adaptive allocator (incompressible random fill; safety floor only)."
Write-Host "  target=$targetAvailMB MB  safety floor=$safetyAvailMB MB"
Write-Host "  start=$($start.ToString('HH:mm:ss'))  end=$($end.ToString('HH:mm:ss'))"

while ((Get-Date) -lt $end) {
    $avail = [math]::Round((Get-Counter '\Memory\Available MBytes').CounterSamples[0].CookedValue)
    $action = if ($avail -lt $safetyAvailMB) {
        "hold(safety)"
    } elseif ($avail -gt $targetAvailMB) {
        $arr = New-Object byte[] 1073741824
        $rng.GetBytes($arr)                  # incompressible — defeats Memory Compression
        $alloc.Add($arr)
        "+1 GiB"
    } else {
        "hold(target)"
    }
    Write-Host "$(Get-Date -Format 'HH:mm:ss')  $($action.PadRight(13))  alloc=$($alloc.Count) GiB  sysAvailable=$avail MB"
    Start-Sleep -Seconds 5
}

$alloc.Clear()
[GC]::Collect()
[GC]::WaitForPendingFinalizers()
```

> **Why cryptographic random bytes, not `[Array]::Fill($arr, [byte]1)`.**
> Windows 10/11 enables **Memory Compression** by default: pages
> with low entropy (uniform fill, mostly-zeros, repetitive patterns)
> are compressed in-place to a small fraction of their size and
> held in the **Compression Store** under the System process.
> Verified empirically: a `byte 1` fill on a 64 GB host let the
> allocator reach 70 GiB held while `sysAvailable` stayed at 4-7 GB
> the whole time — the OS had compressed ~50 GiB into ~10 GiB of
> physical memory, so kernel-Low never fired.  `RandomNumberGenerator.GetBytes`
> produces ~8 bits/byte entropy which the compressor cannot shrink,
> so each +1 GiB allocation drops `sysAvailable` by ~1 GiB linearly
> as expected.  The crypto RNG path is slower (~150 MB/s vs ~3 GB/s
> for the uniform fill) so each chunk takes ~7 s instead of <1 s,
> but the kernel-Low signal arrives reliably.

State machine (only three states, no GiB cap):

* **Climb:** `sysAvailable > target` → add 1 GiB of random bytes.
* **Hold(target):** `safety <= sysAvailable <= target` → hold steady.
  The daemon's cascade runs here; as it frees memory, sysAvailable
  rises above target and the allocator climbs again, driving the
  next Low event.
* **Hold(safety):** `sysAvailable < safety` → stop adding.  Prevents
  OOM-killing other apps when the daemon hasn't cascaded fast enough.

The allocator will grow to whatever GiB count is required to push
sysAvailable down to the target on this specific host.  With
incompressible fill that's typically (TotalRAM − DaemonRSS − ~6 GB
OS overhead): on a 64 GB box with the daemon at ~16 GiB, expect
the allocator to reach ~42-46 GiB held when `sysAvailable` first
crosses the target.  Each `hold(target)` window is the daemon's
chance to cascade; each return to `+1 GiB` is the next Low trigger.
Over 60 min you should see the daemon log emit a
`level=Low ... level=High` pair every 30-90 s.

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

> **Distinguishing pressure-cascade from TTL idle-demote.**
> Every demote — whether TTL-driven (`demote_idle_shards`) or
> pressure-driven (`cascade_demote_one_step`) — flows through the
> low-level `Registry::demote_letter` primitive, which
> unconditionally emits a `reason="demote"` event from
> `cache/registry.rs`.  The pressure-cascade path additionally
> emits a `reason="pressure-cascade"` event from
> `index/transitions.rs:cascade_demote_one_step` after calling the
> primitive.  Result: every cascade demote produces TWO log lines
> per shard (low-level `demote` event, then high-level
> `pressure-cascade` event), separated by the
> `WorkingSetTrim::trim()` syscall duration (typically 6-22 ms,
> but up to ~1 s on the first cascade demote when the daemon's
> working set is still large).  TTL idle-demotes produce ONE log
> line per shard.
>
> To reliably distinguish the two paths, grep for orphan
> `reason="demote"` events (no paired `reason="pressure-cascade"`
> within the same second on the same drive):
>
> ```powershell
> # Cascade-demote events (the goal during a memory-pressure soak):
> Select-String -Path C:\Temp\uffsd-G4.log -Pattern 'reason="pressure-cascade"' |
>     Select-Object -ExpandProperty Line
>
> # TRUE TTL-driven idle demotes are demote events with NO
> # paired pressure-cascade event for the same drive within ~1 s.
> # On a clean G4 run with `UFFS_WARM_TO_PARKED_IDLE_SECS=3600`
> # this should be empty.
> $log = Get-Content C:\Temp\uffsd-G4.log
> $demote = $log | Select-String 'reason="demote"' | ForEach-Object { $_.Line }
> $cascade = $log | Select-String 'reason="pressure-cascade".*drive=([A-Z])' |
>     ForEach-Object { $_.Line }
> "demote events: $($demote.Count); cascade events: $($cascade.Count)"
> # If demote == cascade: every demote was cascade-driven. Good.
> # If demote >  cascade: the difference is real TTL idle-demotes.
> ```
>
> A future cleanup may pass the reason through `demote_letter` so
> the primitive emits a single canonical event with the correct
> reason; until then, the pair-match grep above is the operator's
> source of truth.

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

**Note on grep patterns**: tracing target names (`shard.transition`,
`shard.ttl`) are NOT rendered in the log line text under the default
formatter.  In addition, two different conventions coexist in the
source:

* `cache/registry.rs` idle-demote / promote / usn-refresh events use
  field name `letter=` and lowercase state names (`to=parked`).
* `index/transitions.rs` cascade and `shard.ttl` events use field
  name `drive=` and quoted+capitalized state names (`to="Parked"`).

The patterns below handle both.  Future operators: if you change the
daemon's tracing fields, update these patterns in the same commit.

1. **C never demotes below `Warm`.**  Grep the soak log:
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern '(letter|drive)=C\b.*to="?[Pp]arked"?' -List
   ```
   Expect **zero matches**.  Every `min-tier-clamp` event for C is
   logged at debug-level via `shard.ttl` with the descriptive
   message `"Demote target clamped by per-drive min_tier"`:
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'Demote target clamped.*drive=C' -Context 0,0 |
       Select-Object -First 5
   ```

2. **Other drives demote normally** (Warm → Parked at the configured
   `warm_ttl_base_secs`):
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern '(letter|drive)=[DEFGMS]\b.*from="?[Ww]arm"?.*to="?[Pp]arked"?' -List
   ```
   Expect at least one match per drive (D, E, F, G, M, S).

3. **Different TTLs for high-rate vs low-rate drives.**  After the
   soak, drive a synthetic 5-min load against C, leave the others
   idle, and grep for the per-drive `chosen_ttl_sec` field on the
   `shard.ttl` debug events (descriptive message text
   `"Adaptive idle-demote evaluation produced demote target"`):
   ```powershell
   Select-String -Path C:\Temp\uffsd-phase6-soak.log -Pattern 'chosen_ttl_sec' |
       Select-Object -Last 30 -ExpandProperty Line
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
