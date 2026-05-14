// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Duplicate verification — Stage C of the duplicate analytics pipeline.
//!
//! After candidate groups are identified (Stage A+B), this module optionally
//! verifies that members are truly identical by reading file content.
//!
//! # Architecture
//!
//! ```text
//! DuplicateAccumulator.finalize()  →  DuplicateResult (candidates)
//!         ↓
//! DuplicateVerifier.verify()       →  DuplicateResult (verified)
//! ```
//!
//! File I/O is abstracted via the [`FileReader`] trait so that:
//! - The daemon provides a real reader using resolved file paths.
//! - Tests use a mock reader returning controlled byte content.

use std::io;

use super::duplicates::{DuplicateGroup, DuplicateResult};
use super::spec::DuplicateVerify;

// ── File reader trait ────────────────────────────────────────────────────

/// Abstraction over file I/O for duplicate verification.
///
/// Each member in a [`DuplicateGroup`] is identified by `(record_idx,
/// drive_ordinal)`. The implementor resolves this to a file path and reads the
/// requested bytes.
///
/// # Sealed-trait decision (Phase 3b §3.7)
///
/// **Kept open** (not sealed).  This trait is a deliberate
/// dependency-injection seam: the daemon supplies a production
/// reader that resolves `(record_idx, drive_ordinal)` against the
/// live `MftIndex` + drive registry, and the test suite supplies an
/// in-memory mock reader.  Sealing the trait would force the mock
/// implementations to live inside `uffs-core` (or behind a feature
/// flag), undermining the test-isolation rationale that motivated
/// the abstraction in the first place.  External crates may
/// implement `FileReader` to plug in alternate I/O strategies
/// (e.g. async readers, network-backed readers) without changes to
/// `uffs-core`.
pub trait FileReader {
    /// Read the first `count` bytes of the file identified by `(record_idx,
    /// drive_ordinal)`.
    ///
    /// Returns fewer bytes if the file is shorter than `count`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be read (permissions, moved, etc.).
    fn read_first_bytes(
        &self,
        record_idx: usize,
        drive_ordinal: u8,
        count: u32,
    ) -> io::Result<Vec<u8>>;

    /// Read the entire file identified by `(record_idx, drive_ordinal)`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be read (permissions, moved, etc.).
    fn read_all(&self, record_idx: usize, drive_ordinal: u8) -> io::Result<Vec<u8>>;
}

// ── Verification budget ──────────────────────────────────────────────────

/// Budget controlling how much I/O verification may perform.
///
/// Verification stops once the budget is exhausted. Remaining groups
/// stay unverified (their `verified` field remains `false`).
#[derive(Debug, Clone, Copy)]
pub struct VerificationBudget {
    /// Maximum total bytes to read across all groups (0 = unlimited).
    pub max_bytes: u64,
    /// Maximum files to read (0 = unlimited).
    pub max_files: u32,
    /// Bytes read so far.
    pub bytes_used: u64,
    /// Files read so far.
    pub files_used: u32,
}

impl VerificationBudget {
    /// Create a new budget with the given limits.
    ///
    /// Pass `0` for unlimited.
    #[must_use]
    pub const fn new(max_bytes: u64, max_files: u32) -> Self {
        Self {
            max_bytes,
            max_files,
            bytes_used: 0,
            files_used: 0,
        }
    }

    /// Unlimited budget — no I/O limits.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self::new(0, 0)
    }

    /// Check whether the budget allows reading `bytes` more bytes.
    const fn can_read(&self, bytes: u64) -> bool {
        if self.max_bytes > 0 && self.bytes_used + bytes > self.max_bytes {
            return false;
        }
        if self.max_files > 0 && self.files_used >= self.max_files {
            return false;
        }
        true
    }

    /// Record that `bytes` were read from one file.
    const fn record_read(&mut self, bytes: u64) {
        self.bytes_used += bytes;
        self.files_used += 1;
    }

    /// Whether the budget has been exhausted.
    #[must_use]
    pub const fn exhausted(&self) -> bool {
        (self.max_bytes > 0 && self.bytes_used >= self.max_bytes)
            || (self.max_files > 0 && self.files_used >= self.max_files)
    }

    /// Total bytes read so far.
    #[must_use]
    pub const fn bytes_used(&self) -> u64 {
        self.bytes_used
    }

    /// Total files read so far.
    #[must_use]
    pub const fn files_used(&self) -> u32 {
        self.files_used
    }
}

impl Default for VerificationBudget {
    fn default() -> Self {
        Self::unlimited()
    }
}

// ── Outcome + Summary ────────────────────────────────────────────────────

/// Internal outcome of verifying a single group.
enum VerifyOutcome {
    /// All members have identical content.
    Match,
    /// Content differs between members.
    Mismatch,
    /// An I/O error prevented verification.
    IoError,
}

/// Summary of a verification pass.
#[derive(Debug, Clone, Default)]
pub struct VerificationSummary {
    /// Groups that were fully verified (content matches).
    pub groups_verified: usize,
    /// Groups rejected (content differs).
    pub groups_rejected: usize,
    /// Groups skipped due to budget exhaustion.
    pub groups_skipped: usize,
    /// Groups with I/O errors during verification.
    pub groups_errored: usize,
    /// Whether the budget was exhausted before all groups were verified.
    pub budget_exhausted: bool,
    /// Total bytes read during verification.
    pub bytes_read: u64,
    /// Total files read during verification.
    pub files_read: u32,
}

// ── Verifier ─────────────────────────────────────────────────────────────

/// Verifies duplicate candidate groups by reading file content.
///
/// Instantiate with the desired mode and budget, then call [`Self::verify`].
pub struct DuplicateVerifier {
    /// Verification mode to use.
    mode: DuplicateVerify,
    /// I/O budget for this verification run.
    budget: VerificationBudget,
}

impl DuplicateVerifier {
    /// Create a verifier with the given mode and budget.
    #[must_use]
    pub const fn new(mode: DuplicateVerify, budget: VerificationBudget) -> Self {
        Self { mode, budget }
    }

    /// Verify a [`DuplicateResult`] in place.
    ///
    /// For each group with ≥2 members, reads file content and compares.
    /// Groups that pass verification get `verified = true`.
    /// Groups that fail (content differs) are removed from the result.
    ///
    /// Returns the updated result plus a summary of the verification pass.
    pub fn verify(
        &mut self,
        mut result: DuplicateResult,
        reader: &dyn FileReader,
    ) -> (DuplicateResult, VerificationSummary) {
        let mut summary = VerificationSummary::default();

        if matches!(self.mode, DuplicateVerify::None) {
            return (result, summary);
        }

        let mut verified_groups = Vec::with_capacity(result.groups.len());

        for mut group in result.groups.drain(..) {
            if group.member_indices.len() < 2 {
                // Single-member groups can't be duplicates.
                continue;
            }

            // Pre-flight: estimate total reads for this group and skip if
            // the budget can't accommodate them.
            let per_file_estimate = match self.mode {
                DuplicateVerify::None => 0,
                DuplicateVerify::FirstBytes { count } => group.file_size.min(u64::from(count)),
                DuplicateVerify::Sha256 => group.file_size,
            };
            let group_estimate = per_file_estimate * group.member_indices.len() as u64;

            if self.budget.exhausted() || !self.budget.can_read(group_estimate) {
                summary.groups_skipped += 1;
                summary.budget_exhausted = true;
                // Keep the group as-is (unverified).
                verified_groups.push(group);
                continue;
            }

            match self.verify_group(&group, reader) {
                VerifyOutcome::Match => {
                    group.verified = true;
                    summary.groups_verified += 1;
                    verified_groups.push(group);
                }
                VerifyOutcome::Mismatch => {
                    // Content differs — not true duplicates.
                    summary.groups_rejected += 1;
                    // Don't add to verified_groups.
                }
                VerifyOutcome::IoError => {
                    summary.groups_errored += 1;
                    // Keep unverified so user sees partial results.
                    verified_groups.push(group);
                }
            }
        }

        // Update totals.
        result.groups = verified_groups;
        result.candidate_groups = result.groups.len();
        result.candidate_files = result
            .groups
            .iter()
            .map(|group| group.member_indices.len() as u64)
            .sum();
        result.total_duplicate_bytes = result.groups.iter().map(|group| group.total_bytes).sum();
        result.total_reclaimable_bytes = result
            .groups
            .iter()
            .map(|group| {
                let per_copy = group.total_bytes.checked_div(group.count).unwrap_or(0);
                group.total_bytes.saturating_sub(per_copy)
            })
            .sum();
        result.verification_mode = self.mode;

        summary.bytes_read = self.budget.bytes_used();
        summary.files_read = self.budget.files_used();

        (result, summary)
    }

    /// Verify a single group. Returns whether all members match.
    fn verify_group(&mut self, group: &DuplicateGroup, reader: &dyn FileReader) -> VerifyOutcome {
        match self.mode {
            DuplicateVerify::None => VerifyOutcome::Match,
            DuplicateVerify::FirstBytes { count } => self.verify_first_bytes(group, reader, count),
            DuplicateVerify::Sha256 => self.verify_sha256(group, reader),
        }
    }

    /// Compare first N bytes across all group members.
    fn verify_first_bytes(
        &mut self,
        group: &DuplicateGroup,
        reader: &dyn FileReader,
        count: u32,
    ) -> VerifyOutcome {
        let mut reference: Option<Vec<u8>> = None;

        for &(record_idx, drive_ordinal) in &group.member_indices {
            // Estimate using the file size (capped by requested count).
            let read_estimate = group.file_size.min(u64::from(count));
            if !self.budget.can_read(read_estimate) {
                return VerifyOutcome::IoError; // Budget would be exceeded
            }

            match reader.read_first_bytes(record_idx, drive_ordinal, count) {
                Ok(bytes) => {
                    self.budget.record_read(bytes.len() as u64);
                    match &reference {
                        None => reference = Some(bytes),
                        Some(ref_bytes) => {
                            if bytes != *ref_bytes {
                                return VerifyOutcome::Mismatch;
                            }
                        }
                    }
                }
                Err(_) => return VerifyOutcome::IoError,
            }
        }

        VerifyOutcome::Match
    }

    /// Full SHA-256 hash verification.
    ///
    /// Reads the entire file for each member, computes SHA-256, and
    /// compares all hashes. All members must hash identically.
    fn verify_sha256(&mut self, group: &DuplicateGroup, reader: &dyn FileReader) -> VerifyOutcome {
        use sha2::{Digest as _, Sha256};

        let mut reference_hash: Option<[u8; 32]> = None;

        for &(record_idx, drive_ordinal) in &group.member_indices {
            // Estimate read size from file_size for budget check.
            if !self.budget.can_read(group.file_size) {
                return VerifyOutcome::IoError;
            }

            match reader.read_all(record_idx, drive_ordinal) {
                Ok(bytes) => {
                    self.budget.record_read(bytes.len() as u64);
                    let hash: [u8; 32] = Sha256::digest(&bytes).into();
                    match &reference_hash {
                        None => reference_hash = Some(hash),
                        Some(ref_hash) => {
                            if hash != *ref_hash {
                                return VerifyOutcome::Mismatch;
                            }
                        }
                    }
                }
                Err(_) => return VerifyOutcome::IoError,
            }
        }

        VerifyOutcome::Match
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests assert against fixtures with known shape; indexing panic = test failure"
)]
mod tests {
    use super::*;
    use crate::aggregate::duplicates::{DuplicateGroup, DuplicateResult};
    use crate::aggregate::spec::DuplicateVerify;

    /// Mock file reader that returns controlled byte content.
    struct MockReader {
        /// Map from (`record_idx`, `drive_ordinal`) → file bytes.
        files: std::collections::HashMap<(usize, u8), Vec<u8>>,
    }

    impl MockReader {
        fn new() -> Self {
            Self {
                files: std::collections::HashMap::new(),
            }
        }

        fn add(&mut self, idx: usize, drive: u8, content: Vec<u8>) {
            self.files.insert((idx, drive), content);
        }
    }

    impl FileReader for MockReader {
        fn read_first_bytes(
            &self,
            record_idx: usize,
            drive_ordinal: u8,
            count: u32,
        ) -> io::Result<Vec<u8>> {
            let bytes = self
                .files
                .get(&(record_idx, drive_ordinal))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))?;
            let n = uffs_mft::u32_as_usize(count).min(bytes.len());
            Ok(bytes[..n].to_vec())
        }

        fn read_all(&self, record_idx: usize, drive_ordinal: u8) -> io::Result<Vec<u8>> {
            self.files
                .get(&(record_idx, drive_ordinal))
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
        }
    }

    fn make_group(members: Vec<(usize, u8)>, file_size: u64) -> DuplicateGroup {
        let count = members.len() as u64;
        DuplicateGroup {
            count,
            total_bytes: file_size * count,
            file_size,
            reclaimable_bytes: file_size * (count - 1),
            member_indices: members,
            sample_rows: Vec::new(),
            verified: false,
        }
    }

    fn make_result(groups: Vec<DuplicateGroup>) -> DuplicateResult {
        let candidate_files: u64 = groups.iter().map(|group| group.count).sum();
        let total_dup = groups.iter().map(|group| group.total_bytes).sum();
        let total_reclaim = groups.iter().map(|group| group.reclaimable_bytes).sum();
        DuplicateResult {
            candidate_groups: groups.len(),
            candidate_files,
            total_duplicate_bytes: total_dup,
            total_reclaimable_bytes: total_reclaim,
            groups,
            verification_mode: DuplicateVerify::None,
        }
    }

    // ── first_bytes tests ────────────────────────────────────────

    #[test]
    fn first_bytes_match_keeps_group() {
        let mut reader = MockReader::new();
        reader.add(0, 0, vec![1, 2, 3, 4]);
        reader.add(1, 0, vec![1, 2, 3, 4]);

        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 4)]);
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::unlimited(),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(vfy_result.groups.len(), 1);
        assert!(vfy_result.groups[0].verified);
        assert_eq!(summary.groups_verified, 1);
        assert_eq!(summary.groups_rejected, 0);
    }

    #[test]
    fn first_bytes_mismatch_rejects_group() {
        let mut reader = MockReader::new();
        reader.add(0, 0, vec![1, 2, 3, 4]);
        reader.add(1, 0, vec![5, 6, 7, 8]); // different content

        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 4)]);
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::unlimited(),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(vfy_result.groups.len(), 0); // rejected
        assert_eq!(summary.groups_rejected, 1);
    }

    // ── sha256 tests ─────────────────────────────────────────────

    #[test]
    fn sha256_match_keeps_group() {
        let content = b"hello world duplicate content".to_vec();
        let mut reader = MockReader::new();
        reader.add(0, 0, content.clone());
        reader.add(1, 0, content);

        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 28)]);
        let mut verifier =
            DuplicateVerifier::new(DuplicateVerify::Sha256, VerificationBudget::unlimited());
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(vfy_result.groups.len(), 1);
        assert!(vfy_result.groups[0].verified);
        assert_eq!(summary.groups_verified, 1);
    }

    #[test]
    fn sha256_mismatch_rejects_group() {
        let mut reader = MockReader::new();
        reader.add(0, 0, b"file A content".to_vec());
        reader.add(1, 0, b"file B content".to_vec());

        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 14)]);
        let mut verifier =
            DuplicateVerifier::new(DuplicateVerify::Sha256, VerificationBudget::unlimited());
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(vfy_result.groups.len(), 0);
        assert_eq!(summary.groups_rejected, 1);
    }

    // ── Budget tests ─────────────────────────────────────────────

    #[test]
    fn budget_exhaustion_skips_remaining() {
        let mut reader = MockReader::new();
        // Group 1: 2 files, 4 bytes each → 8 bytes read
        reader.add(0, 0, vec![1, 2, 3, 4]);
        reader.add(1, 0, vec![1, 2, 3, 4]);
        // Group 2: 2 files — should be skipped
        reader.add(2, 0, vec![5, 6, 7, 8]);
        reader.add(3, 0, vec![5, 6, 7, 8]);

        let result = make_result(vec![
            make_group(vec![(0, 0), (1, 0)], 4),
            make_group(vec![(2, 0), (3, 0)], 4),
        ]);

        // Budget: 10 bytes max → group 1 reads 8 bytes (ok), group 2 needs 8 more (over
        // budget)
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::new(10, 0),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(summary.groups_verified, 1);
        assert_eq!(summary.groups_skipped, 1);
        assert!(summary.budget_exhausted);
        // Both groups kept: 1 verified, 1 unverified
        assert_eq!(vfy_result.groups.len(), 2);
        assert!(vfy_result.groups[0].verified);
        assert!(!vfy_result.groups[1].verified);
    }

    #[test]
    fn file_count_budget() {
        let mut reader = MockReader::new();
        reader.add(0, 0, vec![1]);
        reader.add(1, 0, vec![1]);
        reader.add(2, 0, vec![2]);
        reader.add(3, 0, vec![2]);

        let result = make_result(vec![
            make_group(vec![(0, 0), (1, 0)], 1),
            make_group(vec![(2, 0), (3, 0)], 1),
        ]);

        // Max 2 file reads → first group verified (2 reads), second skipped
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::new(0, 2),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(summary.groups_verified, 1);
        assert_eq!(summary.groups_skipped, 1);
        assert!(summary.budget_exhausted);
        assert_eq!(vfy_result.groups.len(), 2);
    }

    // ── Edge cases ───────────────────────────────────────────────

    #[test]
    fn none_mode_passes_through() {
        let reader = MockReader::new();
        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 4)]);
        let mut verifier =
            DuplicateVerifier::new(DuplicateVerify::None, VerificationBudget::unlimited());
        let (vfy_result, summary) = verifier.verify(result, &reader);

        // No verification — group kept as-is, not marked verified
        assert_eq!(vfy_result.groups.len(), 1);
        assert!(!vfy_result.groups[0].verified);
        assert_eq!(summary.groups_verified, 0);
    }

    #[test]
    fn io_error_keeps_group_unverified() {
        let reader = MockReader::new(); // no files → read will fail

        let result = make_result(vec![make_group(vec![(0, 0), (1, 0)], 4)]);
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::unlimited(),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        // I/O error keeps group unverified but doesn't reject
        assert_eq!(vfy_result.groups.len(), 1);
        assert!(!vfy_result.groups[0].verified);
        assert_eq!(summary.groups_errored, 1);
    }

    #[test]
    fn single_member_group_removed() {
        let mut reader = MockReader::new();
        reader.add(0, 0, vec![1, 2, 3]);

        let result = make_result(vec![make_group(vec![(0, 0)], 3)]);
        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::unlimited(),
        );
        let (vfy_result, _) = verifier.verify(result, &reader);

        // Single-member groups are removed
        assert!(vfy_result.groups.is_empty());
    }

    #[test]
    fn multi_group_mixed_results() {
        let mut reader = MockReader::new();
        // Group 1: matches
        reader.add(0, 0, vec![1, 2, 3]);
        reader.add(1, 0, vec![1, 2, 3]);
        // Group 2: mismatches
        reader.add(2, 0, vec![4, 5, 6]);
        reader.add(3, 0, vec![7, 8, 9]);
        // Group 3: matches
        reader.add(4, 0, vec![10, 11]);
        reader.add(5, 0, vec![10, 11]);

        let result = make_result(vec![
            make_group(vec![(0, 0), (1, 0)], 3),
            make_group(vec![(2, 0), (3, 0)], 3),
            make_group(vec![(4, 0), (5, 0)], 2),
        ]);

        let mut verifier = DuplicateVerifier::new(
            DuplicateVerify::FirstBytes { count: 4096 },
            VerificationBudget::unlimited(),
        );
        let (vfy_result, summary) = verifier.verify(result, &reader);

        assert_eq!(vfy_result.groups.len(), 2); // group 2 rejected
        assert_eq!(summary.groups_verified, 2);
        assert_eq!(summary.groups_rejected, 1);
        assert!(vfy_result.groups.iter().all(|group| group.verified));
    }
}
