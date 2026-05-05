<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

# Memory-tiering v0.6.0 bake-period exit criteria

**Audience:** the implementer maintaining `main` during the bake period
between dual-platform readiness validation
([`memory-tiering-readiness-validation-2026-05-05.md`](memory-tiering-readiness-validation-2026-05-05.md))
and the v0.6.0 release cut.

**Reference:**
[`memory-tiering-implementation-plan.md`](../refactor/memory-tiering-implementation-plan.md)
§6.2 — *"Per epic (v0.6.0 release): topic branch baked one week with no
regressions."*

This file makes "baked one week with no regressions" **operationally
concrete** so the v0.6.0 cut decision is a checklist, not a judgment
call.

---

## 0. What "bake" means

The bake period is **observational, not active**.  No new operator-surface
features land on `main` until v0.6.0 ships.  The implementer:

1. Runs the dual-platform readiness pass once per day on a representative
   workload.
2. Watches the daemon's tracing logs for unexpected events.
3. Records each day's result in the bake log (§4 below).
4. Triggers the v0.6.0 cut iff §3 exit criteria are met.

What **is** allowed to land on `main` during the bake:

- Pure documentation changes.
- Recipe / tooling improvements that do **not** change daemon behavior
  (e.g., the `just use` resilience improvement is fine; a tracing-format
  change is **not**).
- Bug fixes for regressions surfaced during the bake — but the regression
  must be characterized first (§2 failure protocol) and the fix must be
  reviewed under PR-fast.

What is **not** allowed:

- New operator-surface RPCs.
- Changes to the cache format, registry shape, or pin contract.
- Changes to default tier values.
- Anything that would invalidate the readiness capture in
  [`memory-tiering-readiness-validation-2026-05-05.md`](memory-tiering-readiness-validation-2026-05-05.md).

---

## 1. Daily bake checks

Run these every weekday during the bake period.  ~10 minutes wall-clock.

### 1.1 Mac side (M-series ARM64)

```bash
just readiness 2>&1 | tee ~/uffs_bake/$(date +%Y-%m-%d)-mac.log
```

**Pass criteria:**

- Exit code 0.
- Final line: `══ ALL GOOD ══  150/150 steps passed`.
- Phase 8 RPC summary table is present and within ±25 % of the reference
  capture (per [`memory-tiering-readiness-validation-2026-05-05.md`](memory-tiering-readiness-validation-2026-05-05.md)
  §2.2):
  - `forget` mean ≤ 320 ms
  - `hibernate` mean ≤ 320 ms
  - `preload` mean ≤ 1 950 ms
  - `status_drives` mean ≤ 320 ms

### 1.2 Windows side (production NTFS host)

```powershell
just readiness *>&1 | Tee-Object -FilePath ~/uffs_bake/$(Get-Date -Format yyyy-MM-dd)-win.log
```

**Pass criteria:**

- Exit code 0.
- Final line: `══ ALL GOOD ══  150/150 steps passed`.
- Phase 8 RPC summary within ±25 % of the reference capture
  ([`memory-tiering-readiness-validation-2026-05-05.md`](memory-tiering-readiness-validation-2026-05-05.md)
  §3.2):
  - `forget` mean ≤ 320 ms
  - `hibernate` mean ≤ 825 ms (variance is structural — see §3.2 of the
    capture; the ceiling is on the **mean**, not the max)
  - `preload` mean ≤ 4 080 ms
  - `status_drives` mean ≤ 320 ms
- COLD/WARM/HOT speedup ratio: HOT ≥ 100× COLD (the production-host
  payoff that justifies the operator surface).

### 1.3 Tracing-log audit (both platforms, daily)

Inspect the daemon log captured during the readiness run for these
red flags:

```bash
# Mac (or any Unix-like host)
grep -E '\b(panic|OutOfMemoryError|FATAL|abort)\b' ~/uffs_bake/$(date +%Y-%m-%d)-mac.log
grep -E 'BackgroundIoPriority.*begin failed' ~/uffs_bake/$(date +%Y-%m-%d)-mac.log
```

```powershell
Select-String -Path ~/uffs_bake/$(Get-Date -Format yyyy-MM-dd)-win.log `
    -Pattern '\b(panic|OutOfMemoryError|FATAL|abort)\b'
Select-String -Path ~/uffs_bake/$(Get-Date -Format yyyy-MM-dd)-win.log `
    -Pattern 'BackgroundIoPriority.*begin failed'
```

**Pass criteria:** every grep returns no matches (empty output).

### 1.4 Process-stability snapshot (Windows only, daily)

The daemon is restarted by every readiness run, but the bake should also
catch slow leaks in the long-running daemon path.  Once per week,
capture a 24 h `uffsd.exe` Working-Set trajectory:

```powershell
1..24 | ForEach-Object {
    Get-Process uffsd | Select-Object Id, WS, PM, NPM, VM, CPU,
        @{Name='ts'; Expression={ Get-Date -Format 'HH:mm:ss' }}
    Start-Sleep -Seconds 3600
} | Export-Csv -Path ~/uffs_bake/uffsd-ws-trace-$(Get-Date -Format yyyy-MM-dd).csv -NoTypeInformation
```

**Pass criteria:** Working Set at hour 24 ≤ 1.5× Working Set at hour 1
(allows for normal cache fill-up but flags a runaway leak).

---

## 2. Failure protocol

If **any** daily check fails:

1. **Stop the bake clock.**  The 7-day countdown does not advance until
   the regression is fixed and one full pass-day is observed.
2. **Characterize the regression.**  Capture:
   - The failing scenario / step.
   - Full daemon log for the run.
   - `git rev-parse HEAD` of `main` at the time of failure.
   - Time of last clean run.
3. **Open a regression PR.**  Title: `fix(daemon): bake-day-N regression
   in scenario X`.  Link to the failing log.
4. **Fix + merge under PR-fast.**  No bypass.
5. **Restart the bake clock at day 1** the day after merge — the fix must
   itself bake.  This is non-negotiable: a regression discovered on
   bake-day 5 means a 6 + 1 = 7 day total bake, not a 5 + 2 = 7 day total.

If the failure is a **flake** (transient, not reproducible on rerun):

1. Re-run the failing check immediately.
2. If the rerun passes, log the flake under §4 with `flake: <reason>`
   and continue the bake (clock does not reset).
3. If the same flake appears on **two non-consecutive days**, treat as a
   real regression and reset.

---

## 3. Exit criteria for the v0.6.0 cut

All of the following must be true to trigger `just ship` with the
minor-bump invocation:

- [ ] **7 consecutive bake-days** with all daily checks (§1) green.
      "Consecutive" excludes weekends — 7 weekdays with at least one
      Mac run and one Windows run each.
- [ ] **At least one 24 h Working-Set trace** (§1.4) captured and within
      pass criteria.
- [ ] **CHANGELOG `Unreleased` section finalized** — every shipped PR
      since v0.5.85 listed under the right heading
      (`Added` / `Changed` / `Fixed`).
- [ ] **Release notes drafted** in the v0.6.0 section, summarizing the
      memory-tiering epic in operator-facing terms.  Primary input:
      [`memory-tiering-readiness-validation-2026-05-05.md`](memory-tiering-readiness-validation-2026-05-05.md).
- [ ] **Manual diff review** — `git log --oneline v0.5.85..main` walked
      end-to-end with no surprise commits, no commits with empty / TODO
      bodies, no commits that bypass branch protection.
- [ ] **No open critical issues** in `gh issue list --label critical`.
- [ ] **CI green on `main`** at the cut commit (no flaking PR-fast lanes).

When all boxes are checked:

```bash
build/update_all_versions.rs minor   # 0.5.x → 0.6.0
just ship
```

---

## 4. Bake log

Append one row per bake-day.  Times are PDT.  All-green days
record the wall-clock of each readiness run and a one-word `notes`
field; failure days record the failure summary and link the regression
PR.

| Day | Date       | Mac result   | Mac wall | Win result   | Win wall | Notes |
|----:|------------|--------------|---------:|--------------|---------:|-------|
| 0   | 2026-05-05 | 150/150 ✅ |    4m 30s | 150/150 ✅ |     ~12m | Reference capture — pre-bake validation. |
|  1  |            |              |          |              |          |       |
|  2  |            |              |          |              |          |       |
|  3  |            |              |          |              |          |       |
|  4  |            |              |          |              |          |       |
|  5  |            |              |          |              |          |       |
|  6  |            |              |          |              |          |       |
|  7  |            |              |          |              |          |       |

**Cut day:** the day after Day 7 if all rows are green.

---

## 5. Why the ±25 % tolerance

The reference RPC means in §1.1 / §1.2 are wall-clock measurements on
specific hosts (Mac M-series, production Windows host).  Day-to-day
variation will come from:

- Background OS activity (Spotlight reindex on Mac, Windows Update on
  the Win host).
- Disk-cache state at run time.
- Per-drive `last_query_at_ms` history influencing `status_drives`
  ordering / parsing.

±25 % is wide enough to absorb this noise without masking real
regressions.  Anything wider would tolerate a real performance regression
of 25-50 %, which would be operator-visible by the time it landed in a
release.  Anything tighter would produce daily flake rather than a
useful signal.

If the daily means trend monotonically upward over 4-5 days *within* the
±25 % window, treat as a soft regression and characterize before the
ceiling is hit.
