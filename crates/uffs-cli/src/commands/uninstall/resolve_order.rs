// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Resolution-order analysis for `uffs --uninstall` (task U-10 of
//! `docs/dev/architecture/UFFS-Uninstall-Implementation-Plan.md`).
//!
//! Pure: given the discovered copies of a binary stem and the ordered list of
//! directories the OS searches for an executable (design §5.1: the running
//! image's dir, the system dirs, the current dir, then PATH in order), return
//! the copies sorted by which one a bare `uffs <stem>` would actually run, with
//! the first reachable copy marked ACTIVE and the rest SHADOWED. Building the
//! search-dir list is the caller's (impure) job; ordering is pure here.

use std::path::{Path, PathBuf};

use crate::commands::update::model::{Channel, Scope};

/// Standing of a discovered copy in the OS executable search order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionState {
    /// The copy a bare `uffs <stem>` resolves to (first reachable on the path).
    Active,
    /// A copy that exists but is shadowed by an earlier one, or is not on the
    /// search path at all.
    Shadowed,
}

/// A discovered copy of one binary stem, before ordering.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// Logical stem (e.g. `uffs`), without the platform `.exe` suffix.
    pub(crate) stem: String,
    /// On-disk version, if it could be read.
    pub(crate) version: Option<String>,
    /// Channel that placed the copy.
    pub(crate) channel: Channel,
    /// Install scope of the copy's root.
    pub(crate) scope: Scope,
    /// The directory the copy lives in.
    pub(crate) dir: PathBuf,
}

/// A copy after ordering, tagged with its resolution standing.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedBinary {
    /// Active or shadowed.
    pub(crate) state: ResolutionState,
    /// On-disk version, if it could be read.
    pub(crate) version: Option<String>,
    /// Channel that placed the copy.
    pub(crate) channel: Channel,
    /// Install scope of the copy's root.
    pub(crate) scope: Scope,
    /// The directory the copy lives in.
    pub(crate) dir: PathBuf,
    /// Whether the copy's dir is on the executable search path at all.
    pub(crate) on_search_path: bool,
}

/// All discovered copies of one stem, ordered by resolution precedence.
#[derive(Debug, Clone)]
pub(crate) struct StemResolution {
    /// Logical stem (e.g. `uffs`).
    pub(crate) stem: String,
    /// The copies, ACTIVE first when one is reachable, then shadowed.
    pub(crate) copies: Vec<ResolvedBinary>,
}

/// Compare two paths for equality, case-insensitively (Windows file systems are
/// case-insensitive and PATH entries vary in case).
fn paths_equal_ignore_case(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

/// Rank of `dir` within `search_dirs` (lower = earlier). Directories not on the
/// search path return `None` (sorted after all reachable copies).
fn rank_of(dir: &Path, search_dirs: &[PathBuf]) -> Option<usize> {
    search_dirs
        .iter()
        .position(|candidate| paths_equal_ignore_case(candidate, dir))
}

/// Order `candidates` (all the same stem) by search precedence and tag the
/// first reachable copy ACTIVE, the rest SHADOWED. Stable: off-path copies (and
/// ties) fall back to a path-string compare so output is deterministic.
pub(crate) fn resolve_stem(
    candidates: Vec<Candidate>,
    search_dirs: &[PathBuf],
) -> Vec<ResolvedBinary> {
    let mut ranked: Vec<(Option<usize>, Candidate)> = candidates
        .into_iter()
        .map(|candidate| (rank_of(&candidate.dir, search_dirs), candidate))
        .collect();
    ranked.sort_by(|left, right| match (left.0, right.0) {
        (Some(rank_l), Some(rank_r)) => rank_l.cmp(&rank_r),
        (Some(_), None) => core::cmp::Ordering::Less,
        (None, Some(_)) => core::cmp::Ordering::Greater,
        (None, None) => left.1.dir.cmp(&right.1.dir),
    });
    let mut active_assigned = false;
    ranked
        .into_iter()
        .map(|(rank, candidate)| {
            let on_search_path = rank.is_some();
            let state = if on_search_path && !active_assigned {
                active_assigned = true;
                ResolutionState::Active
            } else {
                ResolutionState::Shadowed
            };
            ResolvedBinary {
                state,
                version: candidate.version,
                channel: candidate.channel,
                scope: candidate.scope,
                dir: candidate.dir,
                on_search_path,
            }
        })
        .collect()
}

/// Group `candidates` by stem (sorted) and resolve each group. Only a handful
/// of binary stems exist, so the per-stem filter is trivial and avoids pulling
/// in a map type.
pub(crate) fn group_and_resolve(
    candidates: &[Candidate],
    search_dirs: &[PathBuf],
) -> Vec<StemResolution> {
    let mut stems: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.stem.clone())
        .collect();
    stems.sort_unstable();
    stems.dedup();
    stems
        .into_iter()
        .map(|stem| {
            let group: Vec<Candidate> = candidates
                .iter()
                .filter(|candidate| candidate.stem == stem)
                .cloned()
                .collect();
            StemResolution {
                stem,
                copies: resolve_stem(group, search_dirs),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Candidate, ResolutionState, group_and_resolve, resolve_stem};
    use crate::commands::update::model::{Channel, Scope};

    fn candidate(stem: &str, dir: &str) -> Candidate {
        Candidate {
            stem: stem.to_owned(),
            version: None,
            channel: Channel::Unmanaged,
            scope: Scope::User,
            dir: PathBuf::from(dir),
        }
    }

    #[test]
    fn first_on_path_is_active_rest_shadowed() {
        let candidates = vec![
            candidate("uffs", r"C:\src\target\release"),
            candidate("uffs", r"C:\Users\me\bin"),
        ];
        let search = vec![
            PathBuf::from(r"C:\Users\me\bin"),
            PathBuf::from(r"C:\src\target\release"),
        ];
        let out = resolve_stem(candidates, &search);
        let first = out.first().expect("a first copy");
        let second = out.get(1).expect("a second copy");
        assert_eq!(first.state, ResolutionState::Active);
        assert_eq!(first.dir, PathBuf::from(r"C:\Users\me\bin"));
        assert_eq!(second.state, ResolutionState::Shadowed);
    }

    #[test]
    fn off_path_copies_sort_last_and_are_shadowed() {
        let candidates = vec![
            candidate("uffs", r"C:\Downloads"),
            candidate("uffs", r"C:\Users\me\bin"),
        ];
        let search = vec![PathBuf::from(r"C:\Users\me\bin")];
        let out = resolve_stem(candidates, &search);
        let first = out.first().expect("a first copy");
        let second = out.get(1).expect("a second copy");
        assert_eq!(first.dir, PathBuf::from(r"C:\Users\me\bin"));
        assert_eq!(first.state, ResolutionState::Active);
        assert!(first.on_search_path);
        assert_eq!(second.state, ResolutionState::Shadowed);
        assert!(!second.on_search_path);
    }

    #[test]
    fn case_insensitive_path_match() {
        let out = resolve_stem(vec![candidate("uffs", r"C:\Users\Me\Bin")], &[
            PathBuf::from(r"c:\users\me\bin"),
        ]);
        assert_eq!(out.first().expect("a copy").state, ResolutionState::Active);
    }

    #[test]
    fn no_active_when_nothing_on_path() {
        let out = resolve_stem(vec![candidate("uffs", r"C:\Downloads")], &[]);
        assert_eq!(
            out.first().expect("a copy").state,
            ResolutionState::Shadowed
        );
    }

    #[test]
    fn empty_input_empty_output() {
        assert!(resolve_stem(Vec::new(), &[]).is_empty());
    }

    #[test]
    fn group_and_resolve_groups_by_stem_sorted() {
        let candidates = vec![
            candidate("uffsd", r"C:\bin"),
            candidate("uffs", r"C:\bin"),
            candidate("uffs", r"C:\other"),
        ];
        let groups = group_and_resolve(&candidates, &[PathBuf::from(r"C:\bin")]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups.first().expect("group").stem, "uffs");
        assert_eq!(groups.get(1).expect("group").stem, "uffsd");
        assert_eq!(groups.first().expect("group").copies.len(), 2);
    }
}
