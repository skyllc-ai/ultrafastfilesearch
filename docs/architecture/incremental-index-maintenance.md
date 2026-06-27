<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 SKY, LLC.
-->

# Incremental Index Maintenance — Two-Tier Base + Delta (LSM-style)

**Status:** Phases 1–6 complete + WIN-validated — per-apply 1367 ms → ~200 ms (−85%): paths/trigram/ext/children all incremental/overlay-served, clone Arc-shared, apply cadence now debounce+max-wait (snappy + CPU-bounded). Phase 6 stripped the `IDXDELTA` dev instrumentation: the per-batch cost now lands as the `usn apply: batch applied` DEBUG summary, the git stamp graduated onto the `uffsd starting` banner, and the timing baseline became the cross-platform `apply_cost` Criterion bench (perf guard).
**Owner:** _(assign)_
**Branch:** `feat/incremental-index-maintenance`

---

## 1. Problem

Every live USN apply (`uffs_core::compact_loader::apply_usn_patch`) mutates the
record columns in place (O(changed)), then **rebuilds the derived structures
from scratch (O(total records))**.

### Measured baseline (Phase 0, build `629966bc2`, live C: = 3,889,117 records)

Captured by `scripts/windows/idx-delta-verify.rs` — mean over 12 applies
(`docs/architecture/baselines/` once committed):

| Step | Mean | Kind | Incremental target |
|------|-----:|------|--------------------|
| **`compute_path_lengths`** | **623 ms** | per-record path-len recompute | **#1 — only changed records + renamed subtree** |
| `TrigramIndex::build` | 378 ms | CSR inverted index | base + delta overlay |
| whole-body **clone** (Arc-swap) | 166 ms | deep copy in `shard.rs` | Arc-share the immutable base CSR |
| `ExtensionIndex::build` | 84 ms | CSR | base + delta overlay |
| per-change **loop** | 62 ms | O(changed) | already incremental |
| `ChildrenIndex::build` | 54 ms | CSR | base + delta overlay |
| **rebuild subtotal** | **1140 ms** | | |
| **full apply (clone+loop+rebuild)** | **≈ 1367 ms** | | **the number to beat** |

> **Baseline overturned the original assumption.** This doc first guessed
> *trigram* was the ~80 % win (~500 ms of ~600 ms). The measurement says the
> full apply is **~1.37 s** (not ~600 ms), and **`compute_path_lengths` (623 ms)
> is the single biggest cost — larger than trigram (378 ms)**. Instrumenting the
> *clone* separately (166 ms) was also load-bearing: the rebuild timing alone
> hid it. The phase order in §4 is sequenced from this data, not the guess.

So a single-file change pays a **~1.37 s** full apply. Consequences already
observed in production / the verify harness:

- **Apply backlog** when the apply interval drops below the rebuild cost
  (mitigated, not removed, by the apply-coalescing guard in `fix/usn-apply-coalesce`).
- **Churn CPU**: a continuously-active drive burns a bounded fraction of a core
  on rebuilds.
- **Freshness/CPU tradeoff**: the production apply interval is pinned at **30 s**
  precisely to keep rebuild churn down — i.e. we trade search freshness for CPU
  *because* each apply is O(n).

These CSR structures are **immutable / read-optimized**: inserting one record's
postings means shifting the flat `values`/`offsets` arrays — the same cost as a
rebuild. **You cannot cheaply mutate them in place.** This is fundamental, not a
missing optimization.

## 2. Goal

Turn apply from **O(total records)** into **O(changed records)** without
regressing search correctness or latency:

- Sub-second search freshness becomes cheap (apply interval can drop to ~1 s or
  event-driven).
- Churn CPU drops to ~proportional-to-changes.
- The existing full rebuild survives, but only as an **occasional compaction
  step**, not a per-apply tax. (This also speeds the save-tick path.)

**Non-goals:** changing the on-disk compact-cache format (the base CSR is still
what we serialize); changing search semantics/results; touching the
Windows-only I/O path.

## 3. Architecture — two-tier (base + delta + tombstones)

The Lucene-segment / LSM pattern:

```
DriveCompactIndex
├── records / names                 (mutated in place — already O(changed))
├── frs_to_compact                  (mutated in place — already O(changed))
├── trigram:  TrigramIndex   (BASE)  ─┐
├── children: ChildrenIndex  (BASE)   │ immutable CSR, rebuilt only at compaction
├── ext_index:ExtensionIndex (BASE)  ─┘
└── delta: Option<IndexDelta>        (NEW — small mutable overlay)
        ├── trigram:  HashMap<u64 (packed trigram), Vec<u32 idx>>
        ├── ext:      HashMap<u16 (ext_id),         Vec<u32 idx>>
        ├── children: HashMap<u32 (parent idx),     Vec<u32 idx>>
        └── tombstones: FxHashSet<u32 idx>          (records whose BASE postings are stale)
```

- **Base layer** — the current immutable CSR indexes. Built at cold-load and at
  compaction; never mutated between.
- **Delta layer** — per-index mutable overlays holding postings for records
  created/renamed *since the last compaction*.
- **Tombstones** — record indices whose **base** postings are stale (deleted, or
  renamed and re-added to the delta with a new name). Search subtracts them.

### 3.1 Semantics by operation

| USN op | records/names | tombstone (base idx) | delta postings |
|--------|---------------|----------------------|----------------|
| **create** | append new record (new idx) | — (idx not in base) | add new idx → trigram/ext/children |
| **delete** | mark record removed | tombstone the mapped base idx; if idx was a recent create, drop it from delta instead | remove from delta if present |
| **rename** | update name/ext/parent in place | tombstone the base idx (old-name base postings now stale) | add the same idx → trigram/ext/children **with the new name** |

Key invariant: **a record index appears in search results iff** it is
`(in base AND not tombstoned) OR (in delta)`. A renamed record is *both*
tombstoned-in-base (old name suppressed) *and* present-in-delta (new name found)
— same idx, no data duplication.

### 3.2 Search integration (the hot path — highest risk)

Every read that consults a base index must consult `base ∪ delta` and subtract
tombstones. Wrap each at a single choke point on `DriveCompactIndex`:

| Base call (today) | New delta-aware accessor | Callers to migrate |
|-------------------|--------------------------|--------------------|
| `self.trigram.search(needle, fold) -> Option<Vec<u32>>` | `self.trigram_search(needle) -> Option<Vec<u32>>` | `search/tree.rs`, `search/query/mod.rs`, `search/query/prefix_search.rs` |
| `self.children.get(idx) -> &[u32]` | `self.children_of(idx) -> SmallVec/Cow<[u32]>` | `FastPathResolver`, directory listing, tree search |
| `self.ext_index.get(ext_id) -> &[u32]` | `self.records_with_ext(ext_id) -> Cow<[u32]>` | `--ext` filter dispatch |

- **Trigram** intersects posting lists across the needle's trigrams. For each
  trigram `t`, the effective posting list is `base.get_posting(t) ∪ delta.trigram[t]`
  (sorted-merge, dedup). Intersect across trigrams as today; **filter tombstones
  on the final result** (cheap — one `FxHashSet` lookup per surviving idx).
- **Ext / children** return `base.get(k)` filtered through tombstones, with
  `delta[k]` appended. When the delta is empty (`delta == None`), every accessor
  is a zero-overhead passthrough to the base — *no regression for the common,
  freshly-compacted case.*

### 3.3 Compaction

Fold the delta back into a fresh base CSR (this **is** today's
`apply_usn_patch` rebuild path, reused verbatim) when any trigger fires:

- delta record count `> COMPACT_THRESHOLD_RECORDS` (start at 50 000), **or**
- delta record count `> COMPACT_THRESHOLD_FRACTION` of base (start at 5 %), **or**
- the save tick fires (we already pay a rebuild there — fold the delta in then).

After compaction: new base, `delta = None`, tombstones cleared. Compaction runs
on the existing background `spawn_blocking` applier path, never on a query.

## 4. Phases (each is independently shippable + reversible)

> Each phase keeps the **full-rebuild path as the oracle** (see §7). A phase is
> "done" only when the oracle test passes and the baseline (§8) shows no search
> regression.

Order is by **measured cost** (§1), biggest lever first, cheapest/riskiest last.
Cumulative "apply after this phase" assumes a small change batch on the 3.89M
baseline (clone+loop are constant-ish; each phase removes one rebuild term).

- **Phase 0 — scaffolding (✅ done on this branch):**
  - Design doc; build-id stamp + per-step `IDXDELTA-TIMING` (§9); WIN rig +
    baseline (§8, §10). **Done:** baseline captured (≈1367 ms).
  - **Still in Phase 0 (next):** `IndexDelta` struct + `delta: Option<IndexDelta>`
    field on `DriveCompactIndex` (unused, `None` everywhere → zero behavior
    change) + the oracle harness (§7). Gate for every phase below.

- **Phase 1 — incremental `compute_path_lengths` (623 ms → ~O(changed); the #1 win):**
  This is *not* a base+delta overlay — `path_len` is a per-`CompactRecord`
  field (`= parent.path_len + 1 separator + name_len`), so it is updated
  surgically. Approach (§5.5):
  - **create / file-rename:** recompute just that record's `path_len` from its
    (unchanged) parent's `path_len` + new `name_len` — O(1).
  - **directory rename:** `Δ = new_dir_path_len − old_dir_path_len`; walk the
    renamed dir's subtree via the (still-fresh) children CSR and add `Δ` to each
    descendant's `path_len` — O(subtree), cheap arithmetic, no string walk.
  - **delete:** record is tombstoned; `path_len` irrelevant.
  - Children + trigram + ext **still full-rebuild** this phase (keeps the diff
    small and gives a valid children CSR for the subtree walk).
  - **Acceptance:** oracle passes (path resolution identical to a full rebuild);
    `paths_us` drops from ~623 ms to sub-ms for small batches; apply ≈ 744 ms.

- **Phase 2 — trigram delta (378 ms; base + delta overlay):**
  `IndexDelta.trigram` + tombstones + `DriveCompactIndex::trigram_search` (§3.2,
  §5.1–5.3); apply stops rebuilding trigram; migrate the 3 trigram callers;
  compaction folds the delta. **Acceptance:** oracle passes; trigram search
  within baseline + ε; apply ≈ 366 ms.

- **Phase 3 — shrink the clone (166 ms; Arc-share the base CSR):**
  Hold the immutable base indexes as `Arc<TrigramIndex>` / `Arc<ChildrenIndex>` /
  `Arc<ExtensionIndex>` on `DriveCompactIndex` so the per-apply whole-body clone
  copies records + names + the small delta, **not** the large inverted indexes
  (pointer-clone the Arcs). **Acceptance:** `clone_us` drops materially; oracle
  unaffected (pure representation change). Best done after Phase 2 makes trigram
  a shareable base.

- **Phase 4 — extension + children delta (84 + 54 ms):** same overlay shape for
  `ext_index` → `records_with_ext` and `children` → `children_of`. **Children is
  the highest-care** index — it feeds `FastPathResolver` *and* the Phase-1 subtree
  walk; exercise the path-resolver oracle heavily and keep the children full
  rebuild until its delta + the Phase-1 walk are reconciled.

- **Phase 5 — unify + retire per-apply rebuild + re-tune:** apply is now O(changed)
  end-to-end; the full rebuild runs only at compaction. Re-evaluate the production
  apply-interval default (candidate: 30 s → ~2 s or event-driven). Remove the dead
  per-apply rebuild branch.

- **Phase 6 — cleanup (done):** grep-removed every `IDXDELTA` dev marker; kept
  the build.rs git stamp (folded into the `uffsd starting` banner) and the
  per-apply timing (now the `usn apply: batch applied` DEBUG summary); folded the
  baseline into the committed `apply_cost` perf bench; retargeted
  `idx-delta-verify.rs` onto the graduated logs (§9, §10).

## 5. Detailed implementation guidelines (junior-dev executable)

### 5.1 New types (`crates/uffs-core/src/compact/delta.rs`, new file)

```rust
/// Mutable overlay over the immutable base CSR indexes. `None` on
/// DriveCompactIndex means "freshly compacted — pure base, zero overhead".
#[derive(Debug, Default, Clone)]
pub struct IndexDelta {
    /// packed-trigram -> sorted, deduped record indices added since compaction.
    pub trigram: rustc_hash::FxHashMap<u64, Vec<u32>>,
    /// ext_id -> record indices added since compaction.
    pub ext: rustc_hash::FxHashMap<u16, Vec<u32>>,
    /// parent record idx -> child record indices added since compaction.
    pub children: rustc_hash::FxHashMap<u32, Vec<u32>>,
    /// record indices whose BASE postings are stale (deleted / renamed-away).
    pub tombstones: rustc_hash::FxHashSet<u32>,
    /// running count of distinct records touched (compaction trigger input).
    pub touched_records: u32,
}
```

- All postings kept **sorted + deduped** on insert (binary-search insert) so the
  base∪delta merge is a linear sorted-merge.
- Provide: `add_record(idx, trigrams: &[u64], ext_id, parent_idx)`,
  `tombstone(idx)`, `is_tombstoned(idx)`, `len()` (for compaction trigger).

### 5.2 `DriveCompactIndex` accessors (single choke point)

Implement on `DriveCompactIndex` (in `compact.rs`), each a passthrough when
`self.delta.is_none()`:

```rust
pub fn trigram_search(&self, needle: &str) -> Option<Vec<u32>> {
    let base = self.trigram.search(needle, self.fold)?;        // existing logic
    let Some(delta) = &self.delta else { return Some(base); }; // fast path
    // merge per-trigram postings from delta, re-intersect, filter tombstones
    // (helper: merge_and_filter — see delta.rs)
    Some(self.merge_trigram(needle, base, delta))
}
```

> **Correctness note for trigram:** because trigram search is an **AND
> intersection** across the needle's trigrams, a delta record only survives if it
> is in the delta posting for *every* trigram of the needle. Since `add_record`
> inserts the idx into all of the record's name-trigrams, this holds. Tombstone
> filtering is applied to the final intersected set, never per-list (a base idx
> may legitimately appear in some lists; only the final membership matters).

### 5.3 `apply_usn_patch` changes (`compact_loader.rs`)

Today (per phase, replace the rebuild for the migrated index):

```rust
// BEFORE (per apply):
drive.trigram = TrigramIndex::build(&drive.records, &drive.names, drive.fold); // ~500ms

// AFTER (per apply):
let delta = drive.delta.get_or_insert_with(IndexDelta::default);
for &idx in &created_or_renamed_idxs {
    delta.add_record(idx, &trigrams_for(idx), ext_of(idx), parent_of(idx));
}
for &idx in &deleted_or_renamed_old {
    delta.tombstone(idx);
}
if delta.len() > COMPACT_THRESHOLD { compact(drive); }       // occasional full rebuild
```

Keep `compact(drive)` = the *current* full rebuild (children+trigram+ext+
path-lengths), then `drive.delta = None`.

### 5.4 Serialization

The compact-cache (`compact_cache.rs`) serializes **base only**. Before a disk
save, **compact first** (fold delta → base), then serialize. So the on-disk
format is unchanged and always delta-free. (Cold load → `delta = None`.)

### 5.5 Phase 1 — incremental `compute_path_lengths` (the #1 lever)

`compute_path_lengths` today (`compact.rs`) builds a parent→children adjacency
and BFS-recomputes **every** record's `path_len` where
`path_len = parent.path_len + 1 (separator) + name_char_count`. That O(n) BFS is
the 623 ms. The incremental version only touches what changed.

**Inputs.** `apply_usn_patch`'s per-change loop already knows each touched
record's compact idx and disposition. Collect them into a small list as the loop
runs (no extra pass): `Vec<(u32 idx, PathOp)>` where
`PathOp = { Created, FileRenamed, DirRenamed, Deleted }`. The directory bit comes
from `CompactRecord::flags` (`FILE_ATTRIBUTE_DIRECTORY`).

**New fn** (e.g. `compact.rs::update_path_lengths_incremental`):

```rust
pub(crate) fn update_path_lengths_incremental(
    records: &mut [CompactRecord],
    names: &[u8],
    drive_letter: DriveLetter,
    children: &ChildrenIndex,          // the freshly-rebuilt CSR (Phase 1 keeps it)
    changed: &[(u32, PathOp)],
) {
    for &(idx, op) in changed {
        match op {
            PathOp::Deleted => {}      // tombstoned; path_len irrelevant
            PathOp::Created | PathOp::FileRenamed => {
                // parent is unchanged → its path_len is valid. O(1).
                set_path_len_from_parent(records, names, drive_letter, idx);
            }
            PathOp::DirRenamed => {
                let old = records[idx as usize].path_len;
                set_path_len_from_parent(records, names, drive_letter, idx);
                let delta = i32::from(records[idx as usize].path_len) - i32::from(old);
                if delta != 0 {
                    // every descendant's path runs *through* this dir, so its
                    // path_len shifts by exactly `delta`.  DFS/BFS the subtree
                    // via the children CSR; pure arithmetic, no name walk.
                    shift_subtree_path_len(records, children, idx, delta);
                }
            }
        }
    }
}
```

- `set_path_len_from_parent`: `path_len = parent.path_len + 1 + name_char_count`
  (root/drive cases identical to the BFS seed in `compute_path_lengths`).
- `shift_subtree_path_len`: stack/queue over `children.get(idx)` recursively,
  `rec.path_len = (rec.path_len as i32 + delta) as u16` (saturating).

**Wiring** (`compact_loader/rebuild.rs`): in Phase 1 keep the children/trigram/ext
full rebuilds, but **replace the `compute_path_lengths(...)` call with
`update_path_lengths_incremental(..., changed)`**. Children must be rebuilt
*before* the path update so the subtree walk sees current adjacency. Gate behind
a `changed.len() < FULL_RECOMPUTE_THRESHOLD` fallback to the full BFS for
pathological huge batches (and for the cold-load path, which still calls the full
`compute_path_lengths`).

**Edge cases the oracle (§7) must cover:** rename a directory with a deep subtree
(Δ propagation); FRS-reuse (create into a just-deleted slot); a file whose parent
was itself renamed in the same batch (process parents before children — sort
`changed` by depth, or rely on the BFS order the children CSR already gives);
case-only rename (`name_char_count` unchanged → Δ = 0, no subtree walk).

## 6. Risk register

| Risk | Mitigation |
|------|------------|
| Search correctness drift (base∪delta ≠ truth) | Oracle test (§7) is mandatory per phase; property-based over random op sequences. |
| Hot-path latency regression (delta merge cost) | Passthrough when `delta == None`; baseline timing gate (§8); keep delta small via compaction threshold. |
| Tombstone leak (memory grows on churny drive) | Compaction threshold bounds delta+tombstone size; `touched_records` trigger. |
| Rename edge cases (FRS reuse, case-only rename) | Dedicated oracle scenarios; reuse the USN net-state resolution already in `uffs-mft::usn`. |
| Path resolver fed stale children (Phase 3) | Path-resolver-specific oracle; Phase 3 isolated + last. |

## 7. Oracle test harness (the core correctness guarantee)

**Invariant:** for any sequence of USN ops, the two-tier index must be
**observationally identical** to a freshly-rebuilt full index.

Location: `crates/uffs-core/src/compact/delta_oracle_tests.rs`.

```
fn oracle(ops: &[Op]) {
    let mut incremental = base_index();   // two-tier (delta path)
    let mut rebuilt     = base_index();   // control (full rebuild every apply)
    for op in ops {
        apply_incremental(&mut incremental, op);   // delta path
        apply_full_rebuild(&mut rebuilt, op);      // O(n) control
        for q in QUERY_BATTERY {                    // name / --ext / prefix / tree / path-resolve
            assert_eq!(sorted(incremental.query(q)), sorted(rebuilt.query(q)),
                       "divergence after {op:?} on query {q:?}");
        }
    }
    // After a forced compaction, the base CSR must be byte-identical to a
    // from-scratch rebuild of the same record set.
    incremental.compact();
    assert_eq!(incremental.trigram, rebuilt.trigram);      // byte-identical
    assert_eq!(incremental.children, rebuilt.children);
    assert_eq!(incremental.ext_index, rebuilt.ext_index);
}
```

- **Query battery:** exact-name, substring (trigram), `--ext`, prefix, tree/glob,
  and **path resolution** (FastPathResolver) — one assertion per query type.
- **Op generation:** both hand-written regression scenarios (create→rename→delete,
  FRS reuse, case-only rename, delete-then-recreate-into-same-dir) **and** a
  `proptest`/seeded-random generator over `{create, delete, rename}` with a small
  name alphabet (so trigrams collide and intersections are exercised).
- Runs cross-platform (no live MFT — synthetic records), so it gates every PR.

## 8. Baseline + timing-regression detection

- Add an env-gated micro-benchmark (`cargo bench` or a `#[ignore]` timing test)
  that, on a synthetic N-record drive, measures: **apply latency**, **trigram /
  ext / children search latency** at delta sizes `{0, 1k, 10k, 50k}`, and
  **compaction latency**.
- Capture a **baseline JSON** (`docs/architecture/baselines/incremental-index-<date>.json`)
  committed at the end of Phase 0 (pure-base numbers) and refreshed per phase.
- **Committed perf guard (landed):** the cross-platform `apply_cost` Criterion
  bench (`crates/uffs-core/benches/apply_cost.rs`) applies representative batches
  (`creates/256`, `creates/4000`, `mixed/4000`, `deletes/4000`) to a ~500k-record
  fixture and times the apply alone (clone excluded via `iter_batched`). Paired
  with `overlay_read.rs` (search-under-churn), this is the regression guard the
  `IDXDELTA-TIMING` baseline graduated into. The §10 WIN rig is the live
  confirmation under real USN churn.

## 9. Dev instrumentation — `IDXDELTA` marker (removed in Phase 6)

During the build-out, all temporary logging/timing carried the literal token
`IDXDELTA` so it could be grep-and-removed in one pass. Phase 6 did exactly that
(`grep -rn IDXDELTA crates/ scripts/` → zero hits). Two pieces graduated into
permanent facilities instead of being deleted:

- **Build identifier** — the `git=<sha>` build stamp (emitted by
  `uffs-daemon/build.rs` as `UFFS_GIT_SHA`) folded into the existing
  `uffsd starting` INFO banner, so every field log still pins which binary
  produced it (the wrong-build trap is closed permanently, not just for the dev
  flow).
- **Per-apply timing** — the per-step `IDXDELTA-TIMING apply` lines became the
  single `usn apply: batch applied` DEBUG summary in `compact_loader/rebuild.rs`
  (`changes / created / deleted / renamed / skipped / records / ext_index_entries
  / compacted / apply_us`). The whole-body clone timing was dropped (the clone is
  now Arc-shared and cheap — Phase 3).

The per-search timing + compaction-event lines were never needed beyond the
overlay bring-up and were removed outright.

## 10. Dev test-script — `scripts/windows/idx-delta-verify.rs`

Modeled on `scripts/windows/usn-verify.rs` (same `~/bin/uffs.exe` resolution,
`~/idxtest` scratch, `_run/` artifact dir, daemon-restart-with-logging pattern).
What it adds beyond usn-verify:

1. **Build confirmation** — read the `git=` stamp off the `uffsd starting` banner
   and assert it equals repo HEAD (fail fast on a stale binary).
2. **Churn generator** — create / rename / delete in escalating bursts (1 000,
   10 000, 100 000 files) so the delta grows and the 100k burst crosses the 50k
   compaction threshold, capturing each `usn apply: batch applied` DEBUG line to
   `_run/idx-timing.log` (perf) and a per-burst freshness probe (correctness).
3. **Freshness probe** — after a burst, measure wall-clock from file-op to
   search-visible (should be ≈ apply cadence, no backlog).
4. **Per-apply summary** — parse the captured apply lines into `_run/baseline.txt`:
   applies fired, changes coalesced, mean/max `apply_us`, and compaction count.
   The cross-platform `apply_cost` Criterion bench is the committed perf guard;
   this on-box summary is the live confirmation under real USN churn.
5. **Mutate smoke** — rename + delete on unique sentinel names, asserting the new
   name appears, the deleted name leaves, and the old name is gone (the live
   analogue of the §7 oracle).

Output: one shareable `~/idxtest/_run/` dir, exactly like the USN flow — so we can
"push → pull on WIN → run → share `_run/`" each iteration.

## 11. Tracking

| Phase | Item | Status | PR | Notes |
|-------|------|--------|----|----|
| 0 | Design doc (+ measured baseline + data-driven re-order) | ✅ done | `2e57d6013`, this | |
| 0 | Dev markers + build-id stamp (§9) | ✅ done | `629966bc2` | `IDXDELTA` |
| 0 | Per-step apply timing (clone/loop/rebuild) | ✅ done | `629966bc2` | µs integers |
| 0 | `idx-delta-verify.rs` WIN rig + baseline (§8, §10) | ✅ done | `629966bc2` | ≈1367 ms |
| 0 | `IndexDelta` type | ✅ done | `61dfde09d` | `compact/delta.rs`, unit-tested; posting/tombstone overlay |
| 0 | `delta: Option<IndexDelta>` field on `DriveCompactIndex` | ✅ done | `1cf72d589` | wired with `trigram_search` (Phase 2a) so each of ~20 ctor sites was touched once |
| 0 | Oracle harness (§7) | ✅ done | `9806bc339`, `b7c688e09` | path-len oracle + trigram base+delta oracle (overlay ≡ compacted rebuild) |
| **1** | **Incremental `compute_path_lengths` (§5.5)** | ✅ done | `9806bc339` | 623 ms → ~O(changed); WIN-validated 0.005 ms; oracle byte-identical incl. dir-rename subtree Δ |
| **2a** | **`trigram_search` base+delta choke point (plumbing)** | ✅ done | `1cf72d589` | zero-behavior-change; field + 3 caller migration; rename-visibility unit-tested |
| **2b** | **Apply populates trigram delta; no per-tick rebuild** | ✅ done | `b7c688e09` | 338 ms → ~0 (compaction at 50k touched); end-to-end oracle; awaiting WIN timing |
| — | *Decompose `compact.rs` 1363 → 385* (refactor) | ✅ done | `c3728b0c1` | 5 submodules; off file-size exception list |
| **3** | **Shrink clone — Arc-share base CSR indexes** | ✅ done | `33e754b04` | 166 → 78 ms (WIN); records/names/delta still copied |
| **4a** | **Extension delta (`records_with_ext`)** | ✅ done | `42ff96b94` | 58 → ~0 ms (WIN); Cow overlay, records-validated (no ext tombstone) |
| **4b** | **Children delta (`for_each_child` / `children_of`)** | ✅ done | `abe9ff115` | 60 → ~0 ms (WIN); apply reordered (delta before paths); move/create/delete + same-batch-create oracles |
| — | *Overlay read-cost microbench* | ✅ done | `1a7eff444` | churn overhead measured: small (tree walk 0.7→2.1 ms); `for_each_child` lever ready |
| **5** | **Apply cadence: debounce + max-wait (snappy + CPU-bounded)** | ✅ done | this | `ApplyTrigger` 30 s rate-limit → 250 ms debounce / 2 s max-wait; cadences evaluated every poll; full apply now ~200 ms (WIN, −85% from 1367) |
| **6** | **Remove `IDXDELTA` dev helpers; graduate baseline → perf test** | ✅ done | this | `grep -rn IDXDELTA crates/ scripts/` → 0; git stamp folded into `uffsd starting`; per-apply → `usn apply: batch applied` DEBUG; `apply_cost` bench is the committed perf guard; WIN rig retargeted |

**Done-definition (whole project):** apply is O(changes); oracle green; no search
latency regression vs baseline; production apply interval reduced; all `IDXDELTA`
dev scaffolding removed. **All met.**
