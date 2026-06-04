<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.
-->

# "Bugs Rust Won't Catch" — UFFS Codebase Audit

**Audit date:** 2026-06-04
**Reference article:** *Bugs Rust Won't Catch* — Matthias Endler / corrode.dev
(<https://corrode.dev/blog/bugs-rust-wont-catch/>), derived from the April 2026
Canonical/uutils audit (44 CVEs).

## Purpose & scope

The article catalogues the classes of bug that survive Rust's borrow checker,
Clippy, and `cargo audit` because they live at the **boundary between the
program and the messy outside world** — paths, bytes, syscalls, and trust
boundaries. This document maps each of those categories onto the UFFS source
tree, points at concrete code, explains *why* each site is (or could become) a
problem **given UFFS's actual threat model**, and recommends fixes.

This is an analysis document, not a set of applied changes. Nothing in the code
was modified.

### UFFS threat model (so severities are honest)

UFFS is not `cp`/`rm` running as root, so most findings are **lower severity**
than the uutils CVEs. The relevant exposure surface is:

1. **A long-running daemon** (`uffs-daemon`) that parses cache files and serves
   IPC requests. The release profile sets `panic = "abort"` (Cargo.toml:645), so
   **any panic in any worker tears down the whole daemon for every client** — a
   panic is a clean denial-of-service.
2. **A privileged Windows broker** (`uffs-broker`) that runs as a Service, holds
   `SeBackupPrivilege`, opens raw volume handles, and `DuplicateHandle`s them
   into client processes after an identity check. This *is* a privilege/trust
   boundary.
3. **On-disk secrets:** `uffs-security::keystore` stores the AES-256 cache key
   (`key.bin` on Unix, DPAPI blob on Windows). The cache itself
   (`*_compact.uffs`) is an encrypted index of every filename on every volume —
   sensitive metadata.
4. **Untrusted-ish inputs:** raw MFT bytes (local, admin-read, semi-trusted) and
   cache/cursor files on disk (tamperable by anything with write access to the
   cache dir).

The biggest *correctness* issue (not security) is that the tool's **core job —
finding files by name — silently corrupts a class of real filenames**
(see §4).

## Summary table

| # | Article category | UFFS status | Worst concrete site | Sev* |
|---|------------------|-------------|---------------------|------|
| 1 | TOCTOU across two syscalls | **Present** | `secure_remove` metadata→open; predictable `.tmp` `File::create` | Med |
| 2 | Permissions set *after* creation | **Present (systemic)** | `key.bin` write→chmod; `create_secure_dir`; `atomic_write` | **High** |
| 3 | String equality on paths ≠ FS identity | Minor | drive-letter / `path_contains` scoping; no `canonicalize` | Low |
| 4 | UTF-8 conversion at byte boundaries | **Present (core correctness)** | `String::from_utf16_lossy` on every NTFS name | **High** |
| 5 | `panic!` as DoS | **Mitigated but gapped** | parser indexing on raw bytes + `panic="abort"` daemon; no `arithmetic_side_effects` | Med |
| 6 | Discarded errors | Mostly OK | broadly `drop(..)`/`let _ =` on cleanup; a few meaningful | Low |
| 7 | Bug-for-bug compatibility | N/A-ish | UFFS is not a reimplementation; parity vs Windows search semantics | Low |
| 8 | Resolve before crossing a trust boundary | Watch | broker PID→identity verification; non-crypto auth | Med |

*Severity is relative to UFFS's threat model, not absolute.

---

## 1. TOCTOU — don't trust a path across two syscalls

> *Article rule: anchor on a file descriptor, not a path; if you act on the same
> path twice, assume it's a TOCTOU bug.*

UFFS's high-level file ops all take `&Path` and re-resolve on each call. UFFS is
not privileged the way `install`/`cp` are, but the cache directory is a
shared-ish location and the broker is privileged, so the pattern still matters.

### 1.1 `secure_remove` — `metadata()` then `open()` (check → use)

`crates/uffs-security/src/fs.rs:250-288`

```rust
let meta = std::fs::metadata(path)?;        // (1) follows symlinks; checks len
// ...
let mut file = std::fs::OpenOptions::new().write(true).open(path)?; // (2) re-resolves
```

- The length used to drive the zero-overwrite loop comes from syscall (1); the
  bytes are written through syscall (2) which **re-resolves the name**. Between
  the two, the path component can be swapped for a symlink, so the zero-wipe can
  land on a different file than the one whose size was measured. `metadata` also
  *follows symlinks*, so the "file" being wiped may never have been the intended
  target at all.
- Severity Med: `secure_remove` exists specifically to destroy sensitive data
  (the threat actor it defends against is exactly someone with local access).
- **Fix:** open once with `OpenOptions` (anchored fd), then `file.metadata()` for
  the length and write through the *same* handle. Use `symlink_metadata` if you
  must stat by path so a symlink isn't silently followed.

### 1.2 Predictable `.tmp` + `File::create` (symlink-follow / truncate)

- `crates/uffs-security/src/fs.rs:212-226` (`atomic_write`):
  `let tmp_path = path.with_extension("uffs.tmp"); File::create(&tmp_path)?;`
- `crates/uffs-daemon/src/index/search.rs:686-689` (`--out` export): same
  predictable `target.with_extension("uffs.tmp")` + `File::create`.

`File::create` **follows symlinks and truncates**. The temp name is fully
predictable. Anyone able to write in the destination directory can pre-plant the
`.tmp` path as a symlink to an arbitrary file; the privileged/long-running writer
then truncates and overwrites the attacker's target with cache/export bytes
(article CVE-2026-35355 is this exact shape). For `atomic_write` the written
bytes are the *encrypted* cache, so it's data-destruction rather than
disclosure; for `--out` export it is plaintext search results to an
attacker-chosen file.

- **Fix:** `OpenOptions::new().write(true).create_new(true).mode(0o600).open()`
  on a *randomised* temp name in the same directory, then `rename`. `create_new`
  refuses to follow a dangling/existing symlink (article's recommended remedy).

### 1.3 Cache purge does it right (positive example)

`crates/uffs-core/src/compact_cache.rs:235-246` uses `symlink_metadata` (does not
follow the link) and deliberately refuses `remove_dir_all`, only `remove_dir` on
a known-empty dir. This is the defensive shape the article advocates and is worth
keeping as the in-tree reference pattern.

---

## 2. Set permissions at creation, not after  ← highest-value finding

> *Article rule: a file/dir is briefly world-accessible between create and chmod;
> an fd opened in that window keeps access forever. Use `OpenOptions::mode()` /
> `DirBuilderExt::mode()`.*

UFFS has this anti-pattern **systemically** in the very crate whose job is secure
storage, and it touches the crown-jewel key material.

### 2.1 Cache encryption key written then chmod'd (Unix)

`crates/uffs-security/src/keystore.rs:404-405`

```rust
std::fs::write(&key_path, key)?;                       // raw 32-byte AES key, umask perms
crate::fs::set_file_permissions_owner_only(&key_path)?; // chmod 0600 AFTER
```

- `key.bin` is the **plaintext AES-256 key** that decrypts the entire cache
  (every filename on every volume). It is created with default-umask permissions
  (commonly `0644`/`0664`) and only narrowed to `0600` afterward. Any local user
  who `open()`s it in that window keeps a readable fd even after the chmod.
- `std::fs::write` also creates/truncates by path and follows symlinks (ties to
  §1.2) — a pre-planted `key.bin` symlink redirects the key write.
- The Windows path (`dpapi_write_key`, keystore.rs:192-193) has the same
  write-then-chmod shape, but the blob is DPAPI-encrypted, so lower severity.
- Severity **High** (Unix): direct exposure window on the master key.
- **Fix:** create with `OpenOptions::new().write(true).create_new(true)
  .mode(0o600).open()` (Unix) so the key is *born* `0600`; never write the key by
  bare path.

### 2.2 `create_secure_dir` — `create_dir_all` then `set_permissions`

`crates/uffs-security/src/fs.rs:36-43`

```rust
std::fs::create_dir_all(path)?;                                  // default perms
std::fs::set_permissions(path, Permissions::from_mode(0o700))?;  // narrow AFTER
```

This is the article's textbook example verbatim. The cache/runtime directory
(which holds `key.bin`, the encrypted cache, the IPC socket, and the PID-file
nonce that gates daemon connections) exists with default perms for a window
before being narrowed to `0700`. `create_dir_all` also narrows *only the
leaf* — intermediate components it creates are left at the umask default.

- **Fix:** use `std::os::unix::fs::DirBuilderExt::mode(0o700)` with
  `DirBuilder::recursive(true)` so each created component is born `0700`.

### 2.3 `atomic_write` narrows perms after writing the data

`crates/uffs-security/src/fs.rs:217-223`: `File::create(tmp)` → `write_all(data)`
→ `sync_all` → `set_file_permissions_owner_only(tmp)` → `rename`. The temp file
holds the full payload at default perms for the entire write+sync, then is
narrowed just before the rename. For the encrypted cache this is metadata
exposure of the (encrypted) blob; combined with §1.2 the bigger issue is the
predictable name. **Fix:** create the temp with `.mode(0o600).create_new(true)`.

### 2.4 The codebase already knows the right pattern

`crates/uffs-security/src/runtime_dir.rs:300-303` and `619-622` *do* use
`OpenOptions::new().mode(0o600).create_new(true).open()`. The fix for §2.1–§2.3
is to make `keystore` / `fs::atomic_write` consistent with `runtime_dir`.

---
