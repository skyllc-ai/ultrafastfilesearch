<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.
-->

# RFC — Lossless Filename Storage (WI-4.4)

**Status:** 🟨 Proposed — awaiting maintainer sign-off before implementation.
**Companion:** [`bugs-rust-wont-catch-audit.md`](../code-quality/bugs-rust-wont-catch-audit.md) §4,
[`bugs-rust-wont-catch-implementation.md`](../code-quality/bugs-rust-wont-catch-implementation.md) WI-4.1 / WI-4.4.

## 1. Problem

UFFS's core job is finding files by name, yet a class of *real* NTFS names is
silently corrupted on the way into the index. NTFS stores names as UTF-16 code
units with **no well-formedness guarantee** — unpaired surrogates are legal on
disk. UFFS decodes every name with (effectively) `String::from_utf16_lossy`,
which replaces each unpaired surrogate with U+FFFD. The result:

- A file literally named with an unpaired surrogate is stored as a different
  name (`…\u{FFFD}…`), so it is **not findable by its true name** and a
  round-trip "find → open" can fail.
- The loss was **silent** until WI-4.1.

WI-4.1 (landed) makes the loss **non-silent and measured**: one instrumented
decoder counts U+FFFD substitutions into `stats.lossy_name_count` and warns at
index-build time. This RFC is the path to **elimination** — store names
losslessly so such files are findable and openable.

## 2. Why this is an RFC, not a patch

The name column is a Polars **UTF-8 `String`** column
(`uffs-polars::columns`). Deserialization *requires* valid UTF-8 (see
`crates/uffs-mft/src/index/storage/deserialize.rs:379`,
`String::from_utf8(names_bytes…).map_err(|_| "Invalid UTF-8 in names")`). WTF-8
— the only encoding that can represent unpaired surrogates as bytes — is **not
valid UTF-8**, so holding it requires moving off the `String` column. That
ripples through:

- the DataFrame **schema** and every filter/sort/aggregate that reads `name`;
- **compact storage** layout + (de)serialization (`compact*.rs`, `deserialize.rs`);
- the **trigram / case-fold** search path (case-folding is defined on Unicode
  scalar values, not on lone surrogates);
- every **output formatter** + escaping rule (terminals, CSV, JSON, MCP);
- the **on-disk cache format** (a version bump + rebuild path).

Landing that blind is unacceptable. Hence: design first, sign-off, then
implement behind a cache-format version bump.

## 3. Options considered

### Option A — Binary/WTF-8 `name` column (full replacement)
Replace the UTF-8 `String` column with a `Binary` column holding WTF-8 bytes.
- **Pro:** single source of truth; every name lossless.
- **Con:** maximal blast radius — touches *every* `name` read, all formatters,
  case-fold, trigram, and forces a UTF-8↔bytes boundary at every display site.
  Polars string kernels (contains/lower/etc.) no longer apply directly.

### Option B — UTF-8 column + sidecar "raw name" for the rare lossy rows (**recommended**)
Keep the existing UTF-8 `name` column (display/search stays exactly as today
for the >99.99% of names that are valid UTF-16), and add an **optional sidecar**
keyed by `record_index` that stores the original WTF-8 bytes **only for rows
where `lossy_name_count > 0`**.
- **Pro:** zero change to the hot path / formatters / Polars kernels for normal
  names; the sidecar is tiny (only the handful of pathological rows); exact-name
  lookup and open can consult the sidecar when the UTF-8 name contains U+FFFD.
- **Pro:** migration is additive — a new optional cache section, version-bumped,
  absent in old caches (which simply rebuild).
- **Con:** two code paths for the rare case; exact-match search must check the
  sidecar when the query itself contains a surrogate (rare).

**Recommendation: Option B.** It eliminates the *loss* (the bytes are retained
and the file is findable/openable via the sidecar) while keeping the common
path — and Polars string acceleration — untouched. Option A is a much larger,
riskier rewrite for a case that affects a vanishingly small fraction of names.

## 4. Design (Option B)

1. **Decoder (done, WI-4.1):** `decode_name_u16` already returns
   `(String, replacement_count)`. When `count > 0`, also retain the original
   `&[u16]` → WTF-8 bytes for that record.
2. **Sidecar:** `raw_names: HashMap<u32 /*record_index*/, Vec<u8> /*WTF-8*/>`
   on the index, persisted as a new optional compact-cache section.
3. **Cache format:** bump the compact-cache version; add the optional section
   after the existing name blob. Old caches lack it → `deserialize` treats it
   as empty and the daemon rebuilds from MFT on next load (existing rebuild path).
4. **Search:**
   - Substring / glob / regex on the UTF-8 `name` column: unchanged.
   - **Exact-name** lookup: if the query string contains U+FFFD (or the caller
     opts into "raw" matching), additionally match against the sidecar's WTF-8
     bytes (encode the query to WTF-8 for comparison).
5. **Case-fold / trigram:** unchanged — they continue to operate on the UTF-8
   column. Lone-surrogate rows simply don't participate in case-fold (documented;
   they're matched exactly via the sidecar).
6. **Output:** formatters keep emitting the UTF-8 `name` (with U+FFFD) for
   display by default; a `--raw-name` / structured-output flag can surface the
   WTF-8 bytes (hex/escaped) for the rare rows. Define escaping precisely so a
   surrogate-bearing name can't break CSV/JSON framing.
7. **`lossy_name_count`:** stays as the health metric; with the sidecar present,
   "lossy" means "stored with a sidecar entry", not "lost".

## 5. Acceptance (implementation phase)

- A file whose name contains an unpaired surrogate is **findable by its exact
  name** and round-trips to a working open.
- Old caches load (rebuild) without error after the version bump.
- Normal-name search/sort/aggregate performance is **unchanged** (the sidecar is
  consulted only for surrogate-bearing queries/rows). Measured perf delta
  recorded against the `perf-phase2-measurement-plan.md` baseline.
- A Windows integration test creates an unpaired-surrogate file and
  finds/opens it via UFFS; `stats.lossy_name_count` reflects sidecar usage.

## 6. Out of scope / follow-ups

- DOS-namespace (8.3) name handling is unchanged.
- Normalisation (NFC/NFD) is a separate concern; this RFC is byte-faithful
  retention, not normalisation.

## 7. Sign-off

Implementation does **not** begin until a maintainer approves the chosen option
(B) and the cache-format version-bump plan. Until then WI-4.4 stays 🟨 in the
tracker; WI-4.1 (measured, non-silent loss) is the in-place mitigation.
