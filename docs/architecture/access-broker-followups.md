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

**Also fix the retry storm (observed 2026-06-14):** once a drive loads, the
per-shard journal loop polls the USN journal every ~500 ms and, when the open
is access-denied, logs `Journal poll failed; retrying next tick` **forever** —
flooding the log and burning cycles. Until the USN open is brokered, the loop
should detect a persistent access-denied and **back off / disable** for that
drive (e.g. exponential backoff to a long ceiling, or mark USN-unavailable and
rely on periodic full rebuilds) instead of retrying at 500 ms indefinitely.

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
There are **two** stacked fixes here; do both, the second first if forced to
choose:

**(a) Move off the PowerShell subprocess to `WinVerifyTrust` (the bigger win).**
Shelling out to `powershell.exe Get-AuthenticodeSignature` spins up an entire
PowerShell runtime per request — hundreds of ms, plus the process-spawn tax and
a dependency on PowerShell being present and on PATH. A world-class Windows
implementation never does this for a service hot path. Replace it with a direct
`WinVerifyTrust` FFI call (the `windows` crate already exposes
`Win32::Security::WinTrust`): build a `WINTRUST_FILE_INFO` for the client image,
call `WinVerifyTrust(INVALID_HANDLE_VALUE, &WINTRUST_ACTION_GENERIC_VERIFY_V2,
&data)`, and map the `TRUST_E_*` / `CERT_E_*` HRESULTs to the same
accept/reject policy the PowerShell path uses today (accept `NotSigned` for dev,
reject `HashMismatch`). In-process, no subprocess, ~single-digit ms, no external
dependency. This alone removes the dominant startup latency.

**(b) Cache the result keyed by client PID + exe path** (and ideally the image's
last-write time / a content hash, so PID reuse can't smuggle in a different
binary): verify once per client process, reuse for that process's subsequent
drive requests. Entry lifetime = the client process lifetime; the broker already
opens the client process handle once (WI-8.1), so it can detect process identity
reliably. Do **not** weaken the verification — only avoid re-running it for an
already-verified `(pid, exe, mtime)`.

With (a) in place each check is cheap enough that (b) is a smaller win, but
together they take an N-drive estate from ~N × hundreds-of-ms down to a single
fast verify per client process.

### Effort / risk
Moderate. The `WinVerifyTrust` FFI is unsafe and Windows-only (untestable
off-Windows; needs validation on a real box against signed *and* unsigned
images). The cache is small but security-sensitive — the key must make a
substituted binary a cache miss.

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
**Preferred — go async with tokio's named pipes.** The community-standard way to
write a scalable Windows pipe server in Rust is
[`tokio::net::windows::named_pipe`] (`ServerOptions` + `NamedPipeServer`), not a
hand-rolled serial thread loop over raw `CreateNamedPipeW`. The idiomatic shape:

1. `ServerOptions::new().max_instances(N).create(PIPE_NAME)` for the first
   listener, applying the same SDDL/security attributes as today.
2. `server.connect().await`; **immediately** build the *next* listening instance
   (`ServerOptions…create`) before handling the current connection, so there's
   always an instance waiting (this is the documented tokio accept-loop pattern
   and removes the "between connections we're deaf" window).
3. `tokio::spawn` the per-connection handler (read request, verify identity,
   `DuplicateHandle`, write response). The connected `NamedPipeServer` is
   `Send`, so it moves into the task cleanly — no raw-`HANDLE` `Send` newtype
   needed on the connection itself.
4. Share the rate-limit / audit state behind `Arc<Mutex<…>>` (or an actor task
   owning it); the per-connection tasks take clones of the `Arc`.

This makes multi-instance concurrency natural, keeps the I/O off the parsing,
and aligns the broker with the daemon's existing async runtime. Raw-FFI
multi-instance + a worker thread pool is the fallback if pulling tokio into the
broker is unwanted, but it reintroduces the `HANDLE`-`Send` problem (below) that
the async path avoids.

**Wrap raw handles in a `Send`-safe RAII type.** Independent of the async move:
the broker (and the daemon-side registry) pass volume/process handles around as
bare `u64` / `HANDLE`, which are neither `Send` nor self-closing — every call
site re-implements "reconstruct `HANDLE` from `u64`" and is responsible for not
leaking it. Introduce a small newtype, e.g. `struct OwnedHandle(HANDLE)` with
`unsafe impl Send` (documented SAFETY: a Win32 kernel handle is process-wide and
safe to move between threads), `Drop` calling `CloseHandle`, and `as_raw()` /
`into_raw()` accessors. Thread *that* through the registry and the broker
instead of raw integers. This removes the scattered
`with_exposed_provenance_mut` reconstructions, makes ownership/lifetime explicit,
and is what a Rust master would reach for before adding concurrency.

Pair the whole follow-up with Follow-up 4 so each concurrent connection isn't
independently paying the Authenticode cost.

[`tokio::net::windows::named_pipe`]: https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/index.html

### Effort / risk
Moderate. Bringing tokio into the broker (or a worker-thread model) plus the
handle-ownership refactor; the rate-limiter and audit paths must stay correct
under parallelism. The `OwnedHandle` wrapper is a low-risk, high-clarity
refactor that can land independently and first.

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

## Follow-up 8 — `$UpCase` read on the overlapped broker handle

**Priority: LOW–MEDIUM** (graceful fallback today; correctness on non-standard
case tables).

### Problem
Reading `$UpCase` (the NTFS uppercase-mapping table, used for
case-insensitive name comparison) does a **synchronous `SetFilePointerEx`
seek** on the volume handle. The broker handle is opened `FILE_FLAG_OVERLAPPED`,
and overlapped handles don't maintain a synchronous file pointer — so the seek
fails. Observed 2026-06-14:

```
WARN $UpCase live read failed — falling back to compiled-in default table
     error=$UpCase: seek to offset 3221235712 failed:
           The parameter is incorrect. (0x80070057)
```

It falls back to the **compiled-in default Unicode case table**, which is
correct for standard NTFS volumes, so search results are unaffected today. But
a volume with a customised `$UpCase` would silently use the wrong table, and
the warning is noise on every broker-backed load.

### Fix approach
Either (a) read `$UpCase` through a **non-overlapped** handle (the broker would
need to vend one, or this specific read uses a synchronous duplicate with the
overlapped flag cleared — not directly possible via `DuplicateHandle`, so more
likely a second broker handle or a synchronous re-open), or (b) issue the
`$UpCase` read using the **overlapped offset mechanism** (offset in the
`OVERLAPPED` struct + `GetOverlappedResult`) instead of `SetFilePointerEx`,
matching how the MFT bulk read already addresses the overlapped handle. Option
(b) keeps the single-handle model and is preferred.

### Effort / risk
Small–moderate. Localised to the `$UpCase` read path; needs a volume with a
non-default `$UpCase` to fully validate the correctness angle.

---

## Follow-up 9 — Gate broker warm-up on `!is_elevated()` (self-inflicted regression)

**Priority: HIGH** (trivial fix; removes a regression introduced while fixing the
probe-race).

### Problem
While solving the `ERROR_PIPE_BUSY` starvation, `warm_up_broker_handles` was
changed to run **unconditionally** — for *any* daemon, elevated or not. That was
correct for killing the racy `broker_available()` probe, but it means an
**elevated** daemon with no broker now makes a futile broker pipe-open attempt
**per drive**, each failing fast and logging a `WARN`. On a 7-drive elevated
load that's 7 failed `OpenOptions::open` calls + 7 WARNs. Microseconds of cost,
but real log noise and conceptually wrong: **an elevated daemon never needs the
broker** — it can open volumes directly.

### Evidence
The MFT *read* path is unaffected when elevated (`VolumeHandle::open` finds the
registry empty via a cheap mutex check and falls through to direct
`CreateFileW`, full-speed IOCP as before). The regression is confined to the
warm-up probe: an elevated daemon that previously did nothing broker-related now
emits per-drive "opening broker pipe" → failure WARNs.

### Fix approach
Gate the warm-up on elevation, not on a pipe probe:

```rust
// in load_live_drives_if_windows, before warm_up_broker_handles(...)
if uffs_mft::is_elevated() {
    // Elevated daemons open volumes directly; the broker is only for the
    // non-elevated path. Skip the futile probe entirely.
} else {
    warm_up_broker_handles(&drives);
}
```

`is_elevated()` is cheap (a token query), involves **no pipe interaction**, and
has **none** of the race that made the old `broker_available()` probe dangerous —
it never touches the broker's single pipe instance. This restores the
pre-broker behavior for elevated daemons while keeping the non-elevated path
exactly as it works today.

### Effort / risk
Trivial (a few lines, one import). Validate that a non-elevated daemon still
warms up (broker present) and an elevated daemon emits no broker WARNs.

---

## Suggested sequencing

1. **Land the basic read** (current branch) — confirm one drive loads and the
   daemon stays resident. ✅ (verified 2026-06-14)
2. **Quick regression cleanup:** Follow-up 9 (`!is_elevated()` warm-up gate) —
   tiny, removes log noise, do it first.
3. **Correctness tier:** Follow-up 2 (USN through broker) and Follow-up 3
   (`get_mft_extents` / fragmented MFTs).
4. **Productionization tier:** Follow-up 1 (service dispatcher) so it runs
   without `--run`.
5. **Performance / scale tier:** Follow-up 4 (`WinVerifyTrust` + Authenticode
   cache) + Follow-up 5 (async multi-instance broker + `OwnedHandle` refactor),
   then Follow-up 6.
6. **Verify / opportunistic:** Follow-up 7 and Follow-up 8 if the tests surface
   them.

Each should be its own atomic commit with a `fix:`/`feat:` message naming the
root cause, cross-compiled clean for `x86_64-pc-windows-msvc` and host, and
validated on a real Windows box (the broker path can't be exercised off
Windows).
