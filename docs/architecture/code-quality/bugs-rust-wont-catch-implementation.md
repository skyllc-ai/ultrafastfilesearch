<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.
-->

# Implementation Plan — Closing the "Bugs Rust Won't Catch" Gaps

**Branch:** `harden/bugs-rust-wont-catch`
**Companion audit:** [`bugs-rust-wont-catch-audit.md`](./bugs-rust-wont-catch-audit.md)
**Goal:** 100% coverage/mitigation of every bug category from the corrode.dev
article *Bugs Rust Won't Catch*, with regression guardrails so the gaps cannot
silently return.

---

## 0. How to use this document (read first)

This is a **step-by-step runbook for a junior engineer**. Work top-to-bottom.
Each unit of work is a **Work Item (WI)** with a stable ID like `WI-2.1`. For
every WI you will find:

- **Goal** — one sentence on what "done" looks like.
- **Files** — exact paths you will touch.
- **Steps** — numbered, copy-pasteable changes.
- **Acceptance criteria** — objective checks; all must pass.
- **Tests** — the test(s) you must add/update (we never reduce coverage).
- **Verify** — the exact commands to run.

### Working rules (non-negotiable — from `CLAUDE.md` + repo user rules)

1. **One WI = one commit.** Commit message: `fix(security): <root cause>` /
   `refactor(mft): <root cause>` etc. Small, atomic commits.
2. **No suppression hacks.** Do not add blanket `#[allow(...)]`, do not disable
   lints, do not comment out tests. A scoped `#[expect(lint, reason = "…")]` is
   allowed only when truly necessary and must carry a justification.
3. **Production code must stay lint-clean.** Every new private item needs a doc
   comment (`missing_docs_in_private_items` is denied). No `unwrap`/`expect`/
   `panic`/`indexing`/`as`-cast in prod code (all denied).
4. **Match surrounding style.** No verbose explanatory comments beyond repo norms.
5. **Update the tracking table** (§1) the moment a WI's status changes.
6. **Keep a healing log.** Append progress to
   `LOG/<YYYY_MM_DD_HH_MM>_CHANGELOG_HEALING.md` per the repo user rules.

### Verification policy (baseline & final use the CI pipeline)

- **Before you start** and **after each phase**, run the full pipeline as the
  source of truth:
  ```bash
  just go                       # safe validation: fmt + lint + test + coverage
  # or, equivalently:
  rust-script scripts/ci/ci-pipeline.rs go -v
  ```
- **Between** WIs, iterate faster with local advisory checks:
  ```bash
  just check                    # quick build+lint, no coverage
  cargo nextest run -p <crate>  # focused tests
  just lint-prod                # ultra-strict prod lint
  just lint-tests               # test-code lint
  ```
- A WI is **not** "Done" until `just go` is green **and** its acceptance
  criteria are met. Do **not** bump versions / deploy / push (that is the
  `just phase2-ship` lane and requires explicit maintainer approval).

### Phase ordering (do them in this order — later phases depend on earlier)

1. **Phase A — Security primitives** (WI-2.x, WI-1.x): shared secure-fs helpers;
   everything else reuses them.
2. **Phase B — Lints & guardrails** (WI-5.1, WI-G.1): turn on the missing lints
   and the regression grep-gate early so new code is born compliant.
3. **Phase C — Byte-boundary correctness** (WI-4.x).
4. **Phase D — Parser hardening & fuzzing** (WI-5.2, WI-5.3).
5. **Phase E — Errors, trust boundary, parity, identity** (WI-6.x, WI-8.x,
   WI-7.x, WI-3.x).

---

## 1. Progress tracking

**Status legend:** ⬜ Not started · 🟦 In progress · ✅ Done (acceptance met +
`just go` green) · ⛔ Blocked · 🟨 Deferred (tracked, see notes).

Update the **Status**, **Commit**, and **Verified** columns as you go. "Verified"
means the acceptance criteria were checked off *and* the pipeline was green.

### 1.1 Work-item tracker

| WI | Category | Description | Status | Commit | Verified |
|----|----------|-------------|:------:|--------|:--------:|
| WI-2.1 | 2 Perms | Add `create_new_secure_file` + `write_secret_file` helpers in `uffs-security::fs` | ⬜ | | |
| WI-2.2 | 2 Perms | `create_secure_dir`: per-component `0700` via `DirBuilderExt::mode` | ⬜ | | |
| WI-2.3 | 2 Perms | Keystore: write `key.bin` / DPAPI blob born `0600` (no chmod-after) | ⬜ | | |
| WI-2.4 | 2 Perms | `atomic_write`: temp born `0600` + randomised name (also feeds WI-1.2) | ⬜ | | |
| WI-1.1 | 1 TOCTOU | `secure_remove`: single fd (open once, `file.metadata()`) | ⬜ | | |
| WI-1.2 | 1 TOCTOU | Randomised, `create_new` temp in `atomic_write` + daemon `--out` export | ⬜ | | |
| WI-5.1 | 5 Panic | Enable `arithmetic_side_effects`; `overflow-checks=true` for `dist` | ⬜ | | |
| WI-G.1 | Guard | CI grep-gate script forbidding the anti-patterns from returning | ⬜ | | |
| WI-4.1 | 4 Bytes | Single instrumented UTF-16 decoder; per-index `lossy_name_count` stat + warn | ⬜ | | |
| WI-4.2 | 4 Bytes | Pass `OsString` (not `to_string_lossy`) to spawn argv / IPC paths | ⬜ | | |
| WI-4.3 | 4 Bytes | Strict-parse subprocess stdout used for decisions (PID/name) | ⬜ | | |
| WI-4.4 | 4 Bytes | **RFC + impl:** lossless name storage (binary/WTF-8 column) | 🟨 | | |
| WI-5.2 | 5 Panic | Replace parser arithmetic with `checked_*`; remove parser `indexing_slicing` allows → `.get()` | ⬜ | | |
| WI-5.3 | 5 Panic | In-tree malformed-input fuzz/regression tests (parsers + cache deserialize) | ⬜ | | |
| WI-6.1 | 6 Errors | `daemon_ctl` control writes: surface/log instead of bare `drop` | ⬜ | | |
| WI-6.2 | 6 Errors | Log dir-create failures (`log_init`, `mft/logging`) to stderr once | ⬜ | | |
| WI-6.3 | 6 Errors | Audit remaining `.ok()`/`let _ =`; add justification comments | ⬜ | | |
| WI-8.1 | 8 Trust | Broker: thread one process handle verify→`DuplicateHandle` (no PID re-open) | ⬜ | | |
| WI-8.2 | 8 Trust | Document daemon-nonce security property (depends on WI-2.2) | ⬜ | | |
| WI-7.1 | 7 Parity | Parity corpus: pathological names; assert vs Windows enumeration | ⬜ | | |
| WI-3.1 | 3 Identity | `paths_identical` (dev,inode) helper + invariant doc/test for scoping | ⬜ | | |

### 1.2 Category coverage rollup (fill as phases close)

| # | Category | Mitigation definition (acceptance) | WIs | Coverage |
|---|----------|------------------------------------|-----|:--------:|
| 1 | TOCTOU | No check→use on re-resolved paths; no predictable temp + `File::create` | 1.1, 1.2, 2.4 | 0% |
| 2 | Perms-after-create | Every secret/dir **born** with final perms; zero chmod-after on secrets | 2.1–2.4 | 0% |
| 3 | Path string identity | No safety decision on path strings; identity helper exists + tested | 3.1 | 0% |
| 4 | UTF-8 byte boundary | Zero **silent** lossy conversions; argv/IPC use `OsString`; lossless storage RFC landed | 4.1–4.4 | 0% |
| 5 | Panic = DoS | Missing lints on; parsers `.get()` + `checked_*`; fuzz tests green | 5.1–5.3 | 0% |
| 6 | Discarded errors | No bare `drop(write/flush)`; every intentional discard commented | 6.1–6.3 | 0% |
| 7 | Bug-for-bug parity | Parity test covers pathological names; runs in CI | 7.1 | 0% |
| 8 | Resolve before trust boundary | One process handle threads verify→grant; nonce property documented | 8.1, 8.2 | 0% |
| G | Regression guard | Grep-gate in CI blocks reintroduction of all anti-patterns | G.1 | 0% |

> **Note on WI-4.4 (🟨 Deferred-but-tracked):** literal *lossless* name handling
> requires a binary/WTF-8 name column that ripples through the Polars query
> engine, compact storage, and serialization. It is too large to land blind, so
> it ships as an RFC first (acceptance below). WI-4.1 makes the current loss
> **non-silent, measured, and tested** — that is the required mitigation; WI-4.4
> is the path to elimination and must not be silently dropped.

---

## Phase A — Security primitives (Categories 2 & 1)

All file paths below are relative to the repo root. The canonical "correct"
reference already in the tree is
`crates/uffs-security/src/runtime_dir.rs` (`UnixRuntimeDir::create_owner_only`,
lines ~298-310): `OpenOptions::new().read(true).write(true).create_new(true)
.mode(0o600).open(path)`. We are generalising that pattern.

### WI-2.1 — Add shared secure-create helpers in `uffs-security::fs`

**Goal:** one place that creates a file **born** with owner-only perms and that
**refuses to follow or reuse** an existing path (kills symlink pre-planting).

**Files:** `crates/uffs-security/src/fs.rs`

**Steps:**

1. At the top of `fs.rs`, the imports are currently `use std::io;` and
   `use std::path::Path;`. Leave them.
2. Add the following two public functions (place them just below the
   "Directory & File Permissions" section header, before `create_secure_dir`).
   Keep the existing Windows attribute helpers; reuse them for the Windows arm.

   ```rust
   /// Create a brand-new file with owner-only permissions, failing if the
   /// path already exists (including as a dangling symlink).
   ///
   /// On Unix the file is born `0o600` via `O_CREAT | O_EXCL` + `mode()`, so
   /// there is never a window where it is world-readable (cf.
   /// `set_permissions`-after-create). On Windows `create_new` likewise refuses
   /// to follow/replace an existing path; owner-only ACL is applied immediately.
   ///
   /// # Errors
   ///
   /// Returns [`io::ErrorKind::AlreadyExists`] if the path exists, or any other
   /// error from the underlying open.
   pub fn create_new_secure_file(path: &Path) -> io::Result<std::fs::File> {
       #[cfg(unix)]
       {
           use std::os::unix::fs::OpenOptionsExt as _;
           std::fs::OpenOptions::new()
               .write(true)
               .create_new(true)
               .mode(0o600)
               .open(path)
       }
       #[cfg(windows)]
       {
           let file = std::fs::OpenOptions::new()
               .write(true)
               .create_new(true)
               .open(path)?;
           // Apply owner-only ACL immediately (best-effort, same as elsewhere).
           if !win_set_owner_only_acl(path) {
               win_set_hidden(path);
           }
           Ok(file)
       }
   }

   /// Write `data` to a **new** secret file born with owner-only permissions.
   ///
   /// Refuses to overwrite an existing path; callers that intend to replace an
   /// existing secret must remove it first (so a symlink cannot be followed).
   ///
   /// # Errors
   ///
   /// Returns an error if creation, writing, or syncing fails.
   pub fn write_secret_file(path: &Path, data: &[u8]) -> io::Result<()> {
       use std::io::Write as _;
       let mut file = create_new_secure_file(path)?;
       file.write_all(data)?;
       file.sync_all()?;
       Ok(())
   }
   ```

**Acceptance criteria:**

- `cargo doc -p uffs-security` builds (doc comments present; `missing_docs`
  clean).
- `just lint-prod` is clean for `uffs-security`.

**Tests:** add to `crates/uffs-security/src/fs.rs` test module (or the existing
secure-fs test file):

- `create_new_secure_file_is_0600` (Unix): create a temp path, assert the
  returned file's `metadata().permissions().mode() & 0o777 == 0o600`.
- `create_new_secure_file_rejects_existing`: pre-create the path; assert the call
  returns `AlreadyExists`.
- `create_new_secure_file_rejects_symlink` (Unix): create a symlink at the path
  pointing elsewhere; assert the call errors and the symlink target is untouched.

**Verify:** `cargo nextest run -p uffs-security`

---

### WI-2.2 — `create_secure_dir`: born-`0700` per component

**Goal:** no window where the runtime/cache dir (holds key, cache, socket,
nonce) exists at default perms.

**Files:** `crates/uffs-security/src/fs.rs` (`create_secure_dir`, lines ~36-54).

**Steps:**

1. Replace the Unix arm body. Current:
   ```rust
   std::fs::create_dir_all(path)?;
   #[cfg(unix)]
   return {
       use std::os::unix::fs::PermissionsExt as _;
       std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
   };
   ```
   New (Unix arm creates every component already at `0700`):
   ```rust
   #[cfg(unix)]
   return {
       use std::os::unix::fs::DirBuilderExt as _;
       std::fs::DirBuilder::new()
           .recursive(true)
           .mode(0o700)
           .create(path)
   };
   ```
2. Keep the Windows arm as-is (it already calls `create_dir_all` then sets the
   ACL; leave `create_dir_all(path)?` *inside the Windows arm only*). Restructure
   so `create_dir_all` is no longer called unconditionally before the `cfg`
   branches — move it into the Windows `return { … }` block.
3. Note in the doc comment: `recursive(true)` makes `create` succeed if the dir
   already exists; existing components keep their current perms (we only
   guarantee birth perms for components we create).

**Acceptance criteria:**

- No `set_permissions` call remains in `create_secure_dir`.
- Unix: a freshly created nested dir reports mode `0700`.

**Tests:** `create_secure_dir_births_0700` (Unix): create `tmp/a/b/c`; assert each
created component's mode is `0700`. `create_secure_dir_idempotent`: calling twice
returns `Ok`.

**Verify:** `cargo nextest run -p uffs-security`

---

### WI-2.3 — Keystore writes the key born-`0600`

**Goal:** the AES key / DPAPI blob is never on disk at default perms.

**Files:** `crates/uffs-security/src/keystore.rs`
(`file_based_key` ~373-409; `dpapi_write_key` ~189-195).

**Steps:**

1. In `file_based_key`, replace:
   ```rust
   std::fs::write(&key_path, key)?;
   crate::fs::set_file_permissions_owner_only(&key_path)?;
   ```
   with:
   ```rust
   // Replace any stale key first so create_new can't follow a planted symlink.
   if key_path.exists() {
       let _ignore = std::fs::remove_file(&key_path);
   }
   crate::fs::write_secret_file(&key_path, &key)?;
   ```
   (The "wrong size" branch above already logs and falls through to regenerate;
   the `remove_file` makes the subsequent `create_new` deterministic.)
2. In `dpapi_write_key` (Windows), replace the `std::fs::write` +
   `set_file_permissions_owner_only` pair with the same
   `remove_file`-then-`write_secret_file(path, &encrypted)` shape.

**Acceptance criteria:**

- No `std::fs::write` for key material remains in `keystore.rs`.
- Unix: after first run, `key.bin` mode is `0600`; no observable window at a
  wider mode (covered structurally by `create_new` + `mode`).

**Tests:** `key_bin_is_0600_on_first_gen` (Unix, dev-mode/file-based path): point
the data dir at a temp dir (via the existing test seam, or
`dirs_next`), call `get_cache_key()`, assert the on-disk key file mode is
`0600`. Reuse/extend the existing keystore tests at lines ~439-450.

**Verify:** `cargo nextest run -p uffs-security`

---

### WI-2.4 + WI-1.2 — `atomic_write`: born-`0600` temp with a randomised name

**Goal:** remove both the perms-after-create window **and** the predictable,
symlink-pluggable temp name in the shared atomic-write primitive (and in the
daemon `--out` export that copies the pattern).

**Files:** `crates/uffs-security/src/fs.rs` (`atomic_write`, ~212-226);
`crates/uffs-daemon/src/index/search.rs` (`write_rows_to_file`, ~685-724).

**Steps (atomic_write):**

1. Build a unique temp name in the **same directory** as `path` (same-FS rename
   stays atomic). `uffs-security` already depends on `rand` (used by keystore).
   ```rust
   use rand::Rng as _;
   let suffix: u64 = rand::rng().random();
   let file_name = path.file_name().unwrap_or_default();
   let tmp_name = format!("{}.{:016x}.uffs.tmp", file_name.to_string_lossy(), suffix);
   let tmp_path = path.with_file_name(tmp_name);
   ```
   (Note: `unwrap_or_default` here is on `Option`, not `Result`; it is *not* an
   `unwrap()` lint violation. Confirm with `just lint-prod`.)
2. Replace `let mut file = std::fs::File::create(&tmp_path)?;` with
   `let mut file = create_new_secure_file(&tmp_path)?;`.
3. Remove the now-redundant `set_file_permissions_owner_only(&tmp_path)?;` line
   (the temp is already `0600`).
4. On any error after creation, remove the temp before returning (add an
   error-cleanup path mirroring the daemon code).

**Steps (daemon `--out` export, search.rs):**

1. Replace the predictable `target.with_extension("uffs.tmp")` + `File::create`
   with the same randomised-name + `uffs_security::fs::create_new_secure_file`
   approach. The crate already depends on `uffs-security`.
2. Keep the existing error-cleanup `remove_file(&tmp_path)` and the final
   `rename`.

**Acceptance criteria:**

- No `File::create` on a deterministic, caller-derived temp path remains in
  either file.
- The grep-gate (WI-G.1) for `with_extension("uffs.tmp")` + `File::create`
  reports zero hits.

**Tests:**

- `atomic_write_sets_0600` (Unix): write via `atomic_write`, assert final file
  mode `0600`.
- `atomic_write_concurrent` : spawn N threads writing the same target; assert no
  panic and the final content equals one of the writers' payloads (randomised
  temps must not collide).
- Daemon: extend an existing `write_rows_to_file` test to assert the produced
  file exists with expected content and that a pre-planted symlink at a *guessed*
  temp name is not followed (best-effort; randomised name makes the guess fail).

**Verify:** `cargo nextest run -p uffs-security && cargo nextest run -p uffs-daemon -- write_rows`

---

### WI-1.1 — `secure_remove`: anchor on a single fd

**Goal:** the size used to zero-overwrite and the bytes written go through the
**same** handle; do not re-resolve the path or follow symlinks.

**Files:** `crates/uffs-security/src/fs.rs` (`secure_remove`, ~244-289).

**Steps:**

1. Replace the initial `std::fs::metadata(path)` check + later
   `OpenOptions::new().write(true).open(path)` with a single open, then read the
   length from the open file:
   ```rust
   // Open first (anchor on the fd); NotFound is a no-op as before.
   let mut file = match std::fs::OpenOptions::new().write(true).read(true).open(path) {
       Ok(f) => f,
       Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
       Err(err) => return Err(err),
   };
   let file_len = file.metadata()?.len();
   ```
2. Keep the Windows `win_clear_readonly(path)?` call **before** the open (a
   read-only file can't be opened for write); document that this is a path-based
   attribute clear that precedes the anchor — acceptable because it only toggles
   an attribute, not content.
3. The zero-fill loop and `remove_file(path)` at the end stay. (The final
   `remove_file` is by path; that is inherent to unlink and acceptable.)

**Acceptance criteria:**

- Only **one** `open` of `path` for writing; no `std::fs::metadata(path)` stat
  before it.

**Tests:** `secure_remove_zeroes_then_unlinks`: create a file with known bytes,
`secure_remove`, assert it no longer exists. `secure_remove_absent_is_ok`: call on
a missing path → `Ok`. (Symlink-follow assertion: create a symlink to a sentinel
file, `secure_remove(symlink)`; assert sentinel **content** preserved — only the
link removed — or document the chosen semantics.)

**Verify:** `cargo nextest run -p uffs-security`

---
