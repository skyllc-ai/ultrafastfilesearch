<!--
SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
SPDX-License-Identifier: MPL-2.0
-->
# UFFS Access Broker — follow-up work

**Status:** the broker's core path works end to end as of branch
`fix/broker-service-and-elevation` (2026-06-14). This document captures the
deliberate follow-ups that remain before the broker is production-grade for
**multi-drive, resident, at-scale** use. None of them block the basic
single-drive read; each is a scoped, root-cause piece of work.

> **Design reference:** `docs/dev/architecture/DAEMON_SERVICE_ARCHITECTURE.md`
> (intent: an elevated `LocalSystem` service vends duplicated NTFS volume
> handles to a non-elevated daemon, so the daemon reads the MFT with zero UAC)
> and `docs/dev/architecture/SECURITY_IMPLEMENTATION_PLAN.md` §S5 (broker
> hardening). Both live under gitignored `docs/dev/`.

---

## What works today (baseline)

The elevation model and handle plumbing are functional:

- The broker registers and starts as a service (`uffs-broker --install`) and
  runs in foreground for debugging (`uffs-broker --run`).
- Pipe security lets a **non-elevated** daemon connect (explicit SDDL:
  `D:(A;;GRGW;;;AU)S:(ML;;NW;;;LW)` — Authenticated-Users connect + low
  mandatory-integrity label), while the broker still verifies the client is
  `uffsd` + Authenticode before granting a handle.
- The daemon requests **one handle per drive** at load
  (`warm_up_broker_handles`), the broker `DuplicateHandle`s an elevated,
  overlapped volume handle into the daemon, and the daemon registers it.
- `uffs_mft::VolumeHandle::open` adopts a **duplicate** of the registered
  handle on every open (peek + `DuplicateHandle`), so the read pass, the
  overlapped/IOCP bulk read, and the cache-write pass all succeed without
  re-requesting.

Verified on a Windows 11 VM: a non-elevated `uffs "<query>"` spawns a
non-elevated daemon, which obtains a broker handle (PID-correlated:
`daemon_pid` == broker audit `pid`) and reads the MFT through it.

---

## Follow-up 1 — Windows Service dispatcher (run as `LocalSystem`)

**Priority: HIGH** (required for the advertised "install once, no future UAC").

### Problem
`uffs-broker` implements `--install` / `--uninstall` / `--run` but has **no
Windows Service control dispatcher**. When the Service Control Manager (SCM)
starts the registered service, it launches the binary with **no arguments**;
`run()` then falls through to `print_usage()` and exits 0. The service never
calls `StartServiceCtrlDispatcher` / reports `SERVICE_RUNNING`, so it never
actually runs as a service — only the foreground `--run` mode works.

### Evidence
`sc.exe start UffsAccessBroker` → service reaches `STOPPED`, exit code 0
(graceful, "never started"); `sc query` shows `WIN32_EXIT_CODE: 0` with no
running process.

### Fix approach
Implement the standard service entry point in `crates/uffs-broker`:
1. No-arg invocation → `StartServiceCtrlDispatcherW` with a
   `SERVICE_TABLE_ENTRYW` pointing at a `ServiceMain` callback.
2. `ServiceMain` → `RegisterServiceCtrlHandlerW` (handle `SERVICE_CONTROL_STOP`
   / `SHUTDOWN`), `SetServiceStatus(SERVICE_RUNNING)`, then run
   `serve_pipe_requests()` until a stop is signalled, then
   `SetServiceStatus(SERVICE_STOPPED)`.
3. Keep `--run` as the foreground/debug path (shares `serve_pipe_requests`).

Use the raw `windows` crate FFI (the crate already depends on it) or the
`windows-service` crate (idiomatic, but a new dependency → cargo-vet +
manifest-audit cost). Prefer raw FFI to avoid the dependency unless the
boilerplate proves error-prone.

### Effort / risk
Moderate. Unsafe FFI (`SetServiceStatus`, control handler), Windows-only,
untestable off-Windows — needs iterative validation on a real box.

---

## Follow-up 2 — USN journal read through the broker

**Priority: HIGH** (resident daemons rely on incremental refresh).

### Problem
The USN journal open (`crates/uffs-mft/src/usn/windows.rs::open_volume_handle`)
opens its **own** volume handle via direct `CreateFileW`, which a non-elevated
daemon can't do. So the broker-backed daemon logs
`USN journal unavailable; full rebuild ... Access is denied` and falls back to
a **full MFT rebuild** on every refresh instead of a cheap incremental USN
update. Correct results, wrong cost.

### Fix approach
USN control codes (`FSCTL_QUERY_USN_JOURNAL`, `FSCTL_READ_USN_JOURNAL`) operate
on a **volume** handle — and the broker already supplies one. Route the USN
volume open through the same registry the MFT read uses:
`peek_broker_handle(drive)` + `DuplicateHandle`, falling back to direct
`CreateFileW` when no broker handle is registered (elevated path). Factor the
"adopt a registered broker handle, else open directly" logic so both
`VolumeHandle::open` and the USN path share it (DRY; today only `VolumeHandle`
has it).

### Effort / risk
Small–moderate. The handle-acquisition logic already exists; the work is
sharing it and threading it into `usn/windows.rs`.

---

## Follow-up 3 — `get_mft_extents` through the broker (fragmented MFTs)

**Priority: MEDIUM** (correctness on fragmented volumes).

### Problem
`VolumeHandle::get_mft_extents` opens `<drive>:\$MFT` directly to fetch the
MFT's retrieval pointers (`FSCTL_GET_RETRIEVAL_POINTERS`). On open failure it
**falls back to a single contiguous extent** derived from
`NTFS_VOLUME_DATA` (`mft_start_lcn` + `mft_valid_data_length`). That fallback
is correct for an **unfragmented** MFT but **silently incomplete for a
fragmented MFT** — it misses every fragment beyond the first, producing a
partial index with no error.

### Note / unknown
`$MFT` is opened with **zero desired access** (metadata only), which *may*
succeed for a non-elevated caller — in which case the real retrieval pointers
are read and the fallback never fires. The next VM test's log will confirm
whether the fallback is hit. Either way, relying on a silent
contiguous-assumption fallback for correctness is fragile.

### Fix approach
Two options:
1. **Broker an `$MFT` handle**: extend the broker to open `<drive>:\$MFT` (it's
   elevated) and vend it alongside the volume handle. Requires a protocol/
   registry extension to carry a second handle per drive.
2. **Bootstrap from the volume handle** (no extra handle): read FRS 0 (the
   `$MFT` record) from the known MFT location via the broker *volume* handle,
   parse its `$DATA` runlist, and derive the full extent map. Self-contained,
   no protocol change, but more reader work.

Prefer (2) if feasible — it keeps the broker protocol at one handle per drive.
Make the fallback **loud** (warn) in the interim so a partial index is never
silent.

### Effort / risk
Moderate. Option 2 touches the MFT bootstrap logic; needs a fragmented-MFT
test fixture to validate.

---

## Follow-up 4 — Authenticode verification caching (per-PID)

**Priority: MEDIUM** (multi-drive latency).

### Problem
`verify_authenticode` (broker side) shells out to PowerShell
(`Get-AuthenticodeSignature`) **per request**, costing hundreds of
milliseconds each. The daemon requests handles **sequentially, one per drive**
in `warm_up_broker_handles`, so an N-drive estate pays ~N × that cost up front
(e.g. 10 drives ≈ several seconds of signature checks before any load starts).

### Fix approach
Cache the verification result keyed by **client PID + exe path** (and ideally
the image's last-write time / a content hash, so PID reuse can't smuggle in a
different binary): verify once per client process, reuse for that process's
subsequent drive requests. Entry lifetime = the client process lifetime; the
broker already opens the client process handle once (WI-8.1), so it can detect
process identity reliably. Do **not** weaken the verification — only avoid
re-running it for an already-verified `(pid, exe, mtime)`.

Replacing the PowerShell spawn with a direct `WinVerifyTrust` FFI call is a
separate, larger optimization (removes the subprocess entirely) and could
fold in here.

### Effort / risk
Small (cache) to moderate (if also moving off PowerShell). Security-sensitive
— the cache key must make a substituted binary a cache miss.

---

## Follow-up 5 — Concurrent broker (multi-instance pipe + per-connection handling)

**Priority: MEDIUM** (scale; not needed for the common one-daemon flow).

### Problem
The broker serves a **single** pipe instance (`max_instances = 1`) in a
**serial** loop: create instance → wait for client → handle → disconnect →
repeat. Concurrent clients queue (`PIPE_WAIT`) or hit `ERROR_PIPE_BUSY`. The
normal flow (one daemon requesting drives sequentially) is fine, but genuinely
concurrent load (multiple daemons, or a future parallelized warm-up)
serializes — and combined with the per-request Authenticode spawn (Follow-up
4), throughput is poor at scale.

> Historical note: an earlier symptom of the single-instance design was that a
> daemon's `GetFileAttributesW` *availability probe* consumed the only pipe
> instance and starved the real request (`ERROR_PIPE_BUSY`). That probe has
> been removed; the broker now sees only real requests. This follow-up is
> about *concurrency capacity*, not that (resolved) starvation.

### Fix approach
Restructure `serve_pipe_requests` so multiple instances listen at once:
- `CreateNamedPipeW` with `PIPE_UNLIMITED_INSTANCES` (or a tuned cap).
- After `ConnectNamedPipe` returns for one client, **immediately create the
  next listening instance** before handling the current connection, and handle
  each connection on its own thread (or async task).
- Share the rate-limit map behind `Arc<Mutex<…>>`; `handle_one_connection`
  takes a shared reference.
- Mind `HANDLE` `Send`-ness when moving a connected pipe handle into a worker
  thread (wrap in a `Send` newtype with a documented SAFETY note, or use an
  IOCP/overlapped accept model).

Pair with Follow-up 4 so each concurrent connection isn't independently paying
the Authenticode cost.

### Effort / risk
Moderate. Concurrency + Windows handle `Send` semantics; the rate-limiter and
audit paths must stay correct under parallelism.

---

## Follow-up 6 — Client-side pipe probe still connects (`broker_pipe_present`)

**Priority: LOW** (cosmetic; works due to timing).

### Problem
`uffs-client`'s `broker_pipe_present` (gates the non-elevated daemon spawn)
uses `GetFileAttributesW`, which **connects to** the pipe — the broker logs a
rejected `uffs.exe` connection on every search. It happens ~1 s before the
daemon's real request, so the single-instance broker recovers in time and it's
currently harmless, but it's wasteful and fragile under load.

### Fix approach
Switch the client probe to a **non-connecting** check (e.g. `WaitNamedPipeW`
with a short timeout), or accept a brief false-negative and rely on the spawn's
fallback. Re-evaluate after Follow-up 5 (a multi-instance broker tolerates the
probe connection cleanly, possibly making this moot).

### Effort / risk
Small.

---

## Follow-up 7 — Volume-data FSCTL on the adopted overlapped handle

**Priority: LOW–MEDIUM** (verify under test; correctness if it misbehaves).

### Problem
`get_ntfs_volume_data` issues `FSCTL_GET_NTFS_VOLUME_DATA` with a **NULL**
`OVERLAPPED`. The broker handle is opened `FILE_FLAG_OVERLAPPED`; per the Win32
docs, a synchronous `DeviceIoControl` (NULL overlapped) on an overlapped handle
is technically undefined, though this particular FSCTL completes synchronously
and tends to work in practice. The "simple-first" choice was deliberate.

### Fix approach
If VM testing shows a `NotNtfs` / volume-data failure on the broker-backed
path, add an overlapped-safe variant: pass a real `OVERLAPPED` with an event
and `GetOverlappedResult`, used by `from_broker_handle`. Otherwise leave as-is
and note the dependence on the FSCTL's synchronous completion.

### Effort / risk
Small (~15 lines) if needed.

---

## Suggested sequencing

1. **Land the basic read** (current branch) — confirm one drive loads and the
   daemon stays resident.
2. **Correctness tier:** Follow-up 2 (USN through broker) and Follow-up 3
   (`get_mft_extents` / fragmented MFTs).
3. **Productionization tier:** Follow-up 1 (service dispatcher) so it runs
   without `--run`.
4. **Performance / scale tier:** Follow-up 4 (Authenticode cache) + Follow-up 5
   (concurrent broker), then Follow-up 6.
5. **Verify:** Follow-up 7 only if the test surfaces it.

Each should be its own atomic commit with a `fix:`/`feat:` message naming the
root cause, cross-compiled clean for `x86_64-pc-windows-msvc` and host, and
validated on a real Windows box (the broker path can't be exercised off
Windows).
