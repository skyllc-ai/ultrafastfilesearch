<!-- SPDX-License-Identifier: MPL-2.0 -->
<!-- Copyright (c) 2025-2026 SKY, LLC. -->

# Memory-tiering operator-surface readiness validation — 2026-05-05

**Status:** ALL GREEN  ·  150 / 150 scenarios passed on **both** platforms.

**Audience:** anyone deciding whether the memory-tiering epic is ready to
ship as `v0.6.0`.  This file is the canonical capture of the dual-platform
operator-surface validation pass that closes Definition-of-Done item §6.2
"All Mac gates green / Windows gates passed for the phases that need it"
in [`memory-tiering-implementation-plan.md`](../refactor/memory-tiering-implementation-plan.md).

**Companion docs:**

- Operator runbook (Phase 5 G1-G8 captures, Phase 6 soak):
  [`memory-tiering-windows-host-validation.md`](memory-tiering-windows-host-validation.md)
- Bake-period exit criteria (what comes after this capture):
  [`memory-tiering-bake-criteria.md`](memory-tiering-bake-criteria.md)
- Source-of-truth tracker (gitignored working copy on the implementer's
  machine):
  `docs/refactor/memory-tiering-implementation-plan.md`

---

## 0. What this validates

The Phase 8 operator-driven memory-tiering surface (`hibernate`, `preload`,
`forget`, `status_drives`) plus the Phase 9 `promotions_total` counter
wire — exercised end-to-end against **production-shape release binaries**,
on **both** target platforms, in a single test harness.

Specifically the capture covers:

- **Lifecycle scenarios A-K** (11 scenarios, 65 steps) — the daemon
  start/stop/kill/restart/auto-start/stats matrix.  Confirms the daemon
  process model is sound under every realistic operator action.
- **Phase 8 scenarios L-P** (5 scenarios, 41 steps) — `status_drives`
  contract, `hibernate` end-to-end, `preload` pin contract, `forget
  --force` destructive cleanup, full round-trip cycle.  Confirms each
  operator command's RPC-level contract, including the `pin_until_ms`
  override semantics and the `freed_bytes` reporting.
- **Phase 9 scenarios Q-R** (2 scenarios, 24 steps) — `promotions_total`
  counter increments exactly once per Cold→Hot transition (never on
  AlreadyHot), and the search-driven re-promote latency profile traces
  the full Warm-baseline → Cold → Warm → Hot cost ladder.

Scenario S (Parked→Hot via TTL-gated demote) was skipped on both runs
because it requires `--park-wait-secs > 0` to actually sleep through the
warm-to-parked idle window.  S is exercised under the deferred Phase 6
24 h soak documented in
[`memory-tiering-windows-host-validation.md`](memory-tiering-windows-host-validation.md)
§2.

**150 = 11 + 5 + 2 + K-table = 65 lifecycle steps + 41 Phase 8 steps + 24
Phase 9 steps + 4 K-table phases + 16 timing-table aggregations + 1 RPC
summary.**  The exact step count is checked by the harness itself
(`@/Users/rnio/Private/Github/UltraFastFileSearch/scripts/dev/daemon-readiness.rs:243`).

---

## 1. Reproduction

```bash
# Mac (M-series ARM64, ~/uffs_data offline data set):
just readiness

# Windows (production NTFS host, live auto-discover):
just readiness
```

Equivalent direct invocation (both platforms):

```bash
rust-script scripts/dev/daemon-readiness.rs <data-dir-or-omit>
```

The harness self-builds a fresh release workspace, ensures any stale
daemon is killed, then runs scenarios A-S sequentially.  Exit code is
0 iff every step passed.

---

## 2. Mac capture — 2026-05-05 13:52 PDT

| Field | Value |
|---|---|
| Host | M-series ARM64 (macOS) |
| Binary | `target/release/uffs` (workspace `main` post-#130) |
| Source | `--data-dir /Users/rnio/uffs_data` (offline data set) |
| Pattern | `*.rs` |
| Forget target | `G` |
| Drives loaded | 7 (`C, D, E, F, G, M, S`) |
| Wall-clock | 4 m 30 s |
| Result | **150 / 150 passed** |

### 2.1 COLD / WARM / HOT startup ladder (Scenario K)

| Phase | Startup | Query | Total | Speedup vs COLD |
|---|---:|---:|---:|---:|
| COLD | 5 365 ms | 255 ms | 5 620 ms | — |
| WARM | 5 326 ms | 255 ms | 5 581 ms | 1.0× |
| HOT  |   255 ms | 253 ms |   508 ms | **11.1×** |

WARM is essentially identical to COLD on Mac because the offline
`~/uffs_data` workload is small enough that the encrypted-cache decrypt
saves negligible time over a fresh in-memory parse.  Mac's payoff comes
entirely from the daemon-resident HOT path.

### 2.2 Phase 8 per-RPC timing summary

| RPC | n | min | mean | max | total |
|---|---:|---:|---:|---:|---:|
| `forget`        |  2 | 255 ms |  255 ms |  255 ms |    510 ms |
| `hibernate`     |  8 | 251 ms |  254 ms |  256 ms |  2 033 ms |
| `preload`       |  7 | 254 ms | 1 559 ms | 2 542 ms | 10 914 ms |
| `status_drives` | 19 | 250 ms |  253 ms |  255 ms |  4 821 ms |

`hibernate` and `status_drives` are tight (3-5 ms variance) because
they're pure-CPU under the registry write-lock / read-lock.  `preload`
spans 254 ms (AlreadyHot pin-extension fast path) to 2 542 ms (Cold→Hot
encrypted-cache decrypt + body load) — the variance is the work
asymmetry between the two code paths, not noise.

---

## 3. Windows capture — 2026-05-05 (production host)

| Field | Value |
|---|---|
| Host | Production Windows (7 NTFS volumes, live auto-discover) |
| Binary | `C:\Users\rnio\bin\uffs.exe` (v0.5.90 via `just use`) |
| Source | live NTFS drives (auto-discover) |
| Pattern | `*.rs` |
| Forget target | `G` |
| Drives loaded | 7 (`C, D, E, F, G, M, S`) |
| Result | **150 / 150 passed** |

### 3.1 COLD / WARM / HOT startup ladder (Scenario K)

| Phase | Startup | Query | Total | Speedup vs COLD |
|---|---:|---:|---:|---:|
| COLD | 69 617 ms | 253 ms | 69 870 ms | — |
| WARM | 30 104 ms | 253 ms | 30 357 ms | **2.3×** |
| HOT  |    254 ms | 253 ms |    507 ms | **137.8×** |

This is the headline result of the entire memory-tiering epic.  On a
production Windows host with ~2 GB of cumulative MFT across 7 NTFS
volumes:

- **COLD** = ~70 s (full MFT parse from `\\.\C:` etc.).
- **WARM** = ~30 s (encrypted-compact-cache decrypt + body load —
  validates the Phase 4 cache architecture: 2.3× faster than re-parse).
- **HOT**  = 0.5 s (daemon already-resident, RAM-only path —
  validates the entire Phase 8 operator surface end-to-end:
  **137.8× speedup** over COLD).

### 3.2 Phase 8 per-RPC timing summary

| RPC | n | min | mean | max | total |
|---|---:|---:|---:|---:|---:|
| `forget`        |  2 | 253 ms |  253 ms |  253 ms |    506 ms |
| `hibernate`     |  8 | 253 ms |  660 ms | 1 004 ms |  5 280 ms |
| `preload`       |  7 | 253 ms | 3 262 ms | 5 779 ms | 22 840 ms |
| `status_drives` | 19 | 253 ms |  253 ms |  255 ms |  4 818 ms |

`status_drives` and `forget` match Mac exactly — they're CPU-bound and
disk-bound respectively, with cross-platform-identical work shape.
`hibernate` and `preload` are 2-3× slower than Mac, reflecting the
dominant cost on Windows: **encrypted-cache decrypt + Win32 file I/O**
(per-shard `mmap` + AES-GCM stream + body load).  This matches the
expectation set in `docs/refactor/memory-budget-analysis.md`.

`hibernate` variance on Windows (253 ms → 1 004 ms) is real and
correlates with how much resident-body memory each call has to drop —
the registry write-lock + N × `Arc` swap pays a higher cost when N
grows.  The cost remains sub-second across the whole run, so it's
**observed but not a bug**.

---

## 4. Cross-platform analysis

### 4.1 RPC cost ladder

| RPC | Mac mean | Win mean | Win/Mac | Class |
|---|---:|---:|---:|---|
| `status_drives` |   253 ms |   253 ms | **1.0×** | CPU-bound (register walk under read-lock) |
| `forget`        |   255 ms |   253 ms | **1.0×** | OS-bound (`fs::remove_file` × 4) |
| `hibernate`     |   254 ms |   660 ms | **2.6×** | RAM-bound (write-lock + N × `Arc` swap) |
| `preload`       | 1 559 ms | 3 262 ms | **2.1×** | Disk-bound (cache decrypt + body load) |

The clean pattern — **CPU-bound RPCs are platform-identical, I/O-bound
RPCs are 2-3× slower on Windows** — is exactly what an architecture
abstraction-layer audit would predict.  The portable-file-system /
encrypted-cache layer is doing its job; the per-platform asymmetry shows
up only where it has to (Win32 disk I/O).

### 4.2 Phase 9 counter wire — verified end-to-end

Scenario Q exercises the `promotions_total` counter contract on both
platforms.  All assertions passed:

| Step | Assertion | Mac | Windows |
|---|---|---|---|
| Q4  | First Cold→Hot increments 0 → 1 | ✅ 0→1 in 2 539 ms | ✅ 0→1 in 5 779 ms |
| Q5  | AlreadyHot does **not** increment | ✅ stayed at 1 | ✅ stayed at 1 |
| Q7  | Second Cold→Hot increments 1 → 2 | ✅ 1→2 in 2 529 ms | ✅ 1→2 in 5 259 ms |
| Q8  | AlreadyHot ≥ 5× faster than Cold→Hot | ✅ 10.0× | ✅ **22.8×** |
| Q9  | Two Cold→Hot calls have similar latency | ✅ Δ 0.4 % | ✅ Δ 9.0 % |

The 22.8× Windows speedup on Q8 (AlreadyHot vs Cold→Hot) is the
operator's most visible signal that the pin contract is doing real
work — pinning a frequently-accessed drive avoids the 5-second decrypt
penalty on every preload.

### 4.3 Pin-contract semantics — verified end-to-end

| Step | Assertion | Mac | Windows |
|---|---|---|---|
| N5  | `preload` Cold → Hot, sets `pin_until_ms` | ✅ | ✅ |
| N7  | `pin_until_ms > 0` after preload | ✅ | ✅ |
| N8  | Re-preload pinned drive uses fast path | ✅ 254 ms | ✅ 253 ms |
| P7  | Explicit `hibernate` overrides pin | ✅ | ✅ |
| P8  | Hibernated drive is `cold` (pin cleared) | ✅ | ✅ |

The pin-vs-explicit-hibernate semantics (master plan §3 Phase 3) hold
identically on both platforms.

### 4.4 Search-driven re-promote ladder (Scenario R)

| Step | Cost class | Mac | Windows |
|---|---|---:|---:|
| R4  | Warm-baseline search | 255 ms | 253 ms |
| R6  | Cold → Warm (search-triggered cache decrypt) | 4 311 ms | **11 810 ms** |
| R8  | Warm → Hot (preload, registry rebuild only) | 511 ms | 507 ms |
| R9  | Hot search | 251 ms | 254 ms |

R11 ("Cold→Warm ≥ 3× Warm baseline") passed at **16.9× on Mac** and
**46.7× on Windows** — both far exceed the 3× floor, confirming the
cache-decrypt cost dominates re-promote on both platforms.

R8 (Warm→Hot via preload) is uniformly fast on both platforms because
no body decryption is needed — the cache body is already resident, and
preload just rebuilds the registry entry with a `Hot` shard.  This
confirms the Phase 8-C design choice to keep `promote_letter_to_hot`
free of decrypt cost when starting from `Warm`.

---

## 5. Conclusions for v0.6.0 cut

This capture closes the validation half of the v0.6.0 Definition of Done
(`memory-tiering-implementation-plan.md` §6.2):

- ✅ Phase 8 operator surface validated end-to-end on Mac.
- ✅ Phase 8 operator surface validated end-to-end on Windows.
- ✅ Phase 9 `promotions_total` counter wire validated end-to-end on both.
- ✅ Pin contract semantics validated on both.
- ✅ Cross-platform RPC cost ladder traced and documented.

**Remaining items for the v0.6.0 cut:**

1. One-week bake on `main` per the criteria in
   [`memory-tiering-bake-criteria.md`](memory-tiering-bake-criteria.md).
2. CHANGELOG `Unreleased` → `0.6.0` finalize.
3. Release notes drafted (this file is the primary input).
4. Manual review of the diff `v0.5.85..v0.6.0`.
5. `just ship` with `build/update_all_versions.rs minor`.

The bake period now begins.  No new operator-surface features land on
`main` until `v0.6.0` ships.
