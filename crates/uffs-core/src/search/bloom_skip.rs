// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Phase 4 Commit F — bloom-based per-drive search skip.
//!
//! ## Why
//!
//! A search dispatched to N drives normally fan-outs to all of them
//! and lets each scan its records.  When the user filters by
//! extension (`--ext toml`, `*.toml` ext-glob, or
//! `>.*\.(toml|md)$` regex-alternation — the latter two get
//! promoted to extension filters by `super::dispatch`'s safety
//! nets), the bloom filter holds an authoritative answer to
//! "does this drive contain *any* record whose extension is in the
//! filter set?".  A bloom miss means **definitely no** records
//! match, so the drive can be skipped entirely:
//!
//! * **Warm shard:** skip the trigram / record scan (CPU saved).
//! * **Parked shard:** skip the promote-from-disk (RAM saved — the 1 GB body
//!   never re-enters resident memory).
//!
//! This is the architectural payoff of Phase 4 — Parked shards stay
//! parked unless a query genuinely needs them.
//!
//! ## Why only ext filters
//!
//! The bloom inserts every record's *whole, folded basename* plus
//! every interned *whole extension*.  It does **not** insert
//! substrings or trigrams.
//!
//! For an exact-basename query the bloom is authoritative — but the
//! daemon's search is substring-by-default, so a query for `Cargo`
//! must still match a record named `Cargo.toml`.  Probing
//! `bloom.contains("Cargo")` would miss (the bloom has
//! `cargo.toml`, not `cargo`), producing a false negative.
//!
//! For an ext filter the bloom *is* authoritative: extensions are
//! inserted whole, lowercase, dot-stripped — exactly the form the
//! [`super::filters::SearchFilters::extensions`] vector carries.
//! Probing `bloom.contains("toml")` is bit-exact and a miss truly
//! means "no `.toml` records on this drive".
//!
//! The substring-search path is left untouched — it pays full cost
//! on every drive it touches, but never produces false negatives.
//! Users who want Phase-4 search-skip savings on a name search
//! type `*.toml` (which the dispatch safety nets promote to
//! `pattern = "*"` + `extensions = ["toml"]`), at which point the
//! bloom pre-check applies cleanly.
//!
//! ## Telemetry
//!
//! Every decision (skip or keep) is emitted as a
//! `shard.bloom.decision { drive, match, terms, source }` tracing
//! event so operators can audit which drives a workload exercised
//! and at what false-positive rate.

use crate::bloom::Bloom;

/// Decision outcome for a single drive's bloom probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BloomDecision {
    /// The drive's bloom is authoritative and missed every term —
    /// the caller should skip this drive entirely (no records can
    /// possibly match).
    Skip,
    /// The drive's bloom hit at least one term, OR no bloom was
    /// available, OR the query isn't bloom-checkable — the caller
    /// must include this drive in the search.
    Keep,
}

impl BloomDecision {
    /// `true` iff the drive should be excluded from the active
    /// search subset.
    #[must_use]
    pub const fn skip(self) -> bool {
        matches!(self, Self::Skip)
    }

    /// `true` iff the bloom pre-check returned a `Keep` decision —
    /// either authoritatively (any term hit) or because the check
    /// didn't apply.
    #[must_use]
    pub const fn keep(self) -> bool {
        matches!(self, Self::Keep)
    }
}

/// Probe `bloom` for every term in `ext_terms` and decide whether
/// the drive can be skipped.
///
/// Returns:
/// * [`BloomDecision::Skip`] iff `bloom` is `Some`, `ext_terms` is non-empty,
///   AND no term hits the filter.  All three conditions must hold — anything
///   weaker risks a false negative.
/// * [`BloomDecision::Keep`] otherwise (no bloom, no ext filter, or any single
///   term hit).
///
/// The bloom term encoding mirrors
/// [`crate::compact::DriveCompactIndex::build_bloom`]:
/// extensions are inserted lowercase, dot-stripped, as raw bytes
/// (e.g. `"toml"`, not `".toml"` or `".TOML"`).  Callers that hold
/// extensions in [`crate::search::filters::SearchFilters::extensions`]
/// already match this encoding (see
/// [`crate::search::filters::SearchFilters::from_params`]).
#[must_use]
pub fn decide_for_ext_filter(bloom: Option<&Bloom>, ext_terms: &[String]) -> BloomDecision {
    if ext_terms.is_empty() {
        return BloomDecision::Keep;
    }
    let Some(bloom_ref) = bloom else {
        return BloomDecision::Keep;
    };
    let any_hit = ext_terms
        .iter()
        .any(|term| bloom_ref.contains(term.as_bytes()));
    if any_hit {
        BloomDecision::Keep
    } else {
        BloomDecision::Skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bloom containing the given byte strings.
    fn bloom_with(items: &[&[u8]]) -> Bloom {
        let mut bloom = Bloom::with_capacity_and_fpr(items.len().max(1), 0.01);
        for item in items {
            bloom.insert(item);
        }
        bloom
    }

    /// Empty `ext_terms` always keeps the drive — no filter, no
    /// pre-check, regardless of bloom contents.
    #[test]
    fn empty_terms_always_keeps() {
        let bloom = bloom_with(&[b"toml"]);
        assert_eq!(
            decide_for_ext_filter(Some(&bloom), &[]),
            BloomDecision::Keep
        );
        assert_eq!(decide_for_ext_filter(None, &[]), BloomDecision::Keep);
    }

    /// `None` bloom always keeps — pre-Phase-4 caches load with
    /// `bloom = None`, and the daemon must not skip them.
    #[test]
    fn missing_bloom_always_keeps() {
        let terms = vec!["toml".to_owned()];
        assert_eq!(decide_for_ext_filter(None, &terms), BloomDecision::Keep);
    }

    /// Single-term hit returns Keep (the drive must be searched).
    #[test]
    fn single_term_hit_keeps() {
        let bloom = bloom_with(&[b"toml"]);
        let terms = vec!["toml".to_owned()];
        assert_eq!(
            decide_for_ext_filter(Some(&bloom), &terms),
            BloomDecision::Keep
        );
    }

    /// Single-term miss returns Skip — the bloom is authoritative.
    #[test]
    fn single_term_miss_skips() {
        let bloom = bloom_with(&[b"jpg", b"png"]);
        let terms = vec!["toml".to_owned()];
        assert_eq!(
            decide_for_ext_filter(Some(&bloom), &terms),
            BloomDecision::Skip
        );
    }

    /// Multi-term: any hit keeps; all-miss skips.
    #[test]
    fn multi_term_keep_on_any_hit() {
        let bloom = bloom_with(&[b"jpg"]);
        let terms = vec!["toml".to_owned(), "jpg".to_owned(), "rs".to_owned()];
        assert_eq!(
            decide_for_ext_filter(Some(&bloom), &terms),
            BloomDecision::Keep,
            "any hit must keep the drive",
        );
    }

    #[test]
    fn multi_term_skip_when_all_miss() {
        let bloom = bloom_with(&[b"jpg"]);
        let terms = vec!["toml".to_owned(), "rs".to_owned(), "md".to_owned()];
        assert_eq!(
            decide_for_ext_filter(Some(&bloom), &terms),
            BloomDecision::Skip,
        );
    }

    /// Skip / Keep helpers reflect the variant correctly.
    #[test]
    fn decision_helpers_match_variant() {
        assert!(BloomDecision::Skip.skip());
        assert!(!BloomDecision::Skip.keep());
        assert!(!BloomDecision::Keep.skip());
        assert!(BloomDecision::Keep.keep());
    }
}
