// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Legacy "drive footer" that `--format custom` appends after the CSV
//! body.
//!
//! The footer is a historical artefact of the baseline CLI — it
//! summarises which drives were searched and emits the canonical
//! `MMMmmm that was FAST` heuristic line on small full-scan queries.
//! Both the CLI's own `write_native_results("custom", …)` path and the
//! daemon's [`crate::write_rows`]-plus-footer pre-format fast path
//! must emit this block byte-identically, so it lives here (alongside
//! the CSV writer) rather than in either of the two consumers.
//!
//! Output shape (CRLF line endings, matching the legacy baseline):
//!
//! ```text
//! <CSV body>
//! \r\n\r\n
//! Drives? \t<N>\t<letters>\r\n
//! \r\n
//! MMMmmm that was FAST ...\t<pattern>\r\n   (full-scan + small)
//! Search path. E.g. 'C:/' or 'C:\Prog**' \r\n
//! ```
//!
//! The fast-scan block is suppressed for result sets of 20 000 rows or
//! more and for any pattern the `is_full_scan_pattern` heuristic
//! does not classify as a full scan.

use std::io::{self, Write};

/// Drive-letter + pattern + row-count context the footer needs.
///
/// Lives in `uffs-format` so both `uffs-cli` (slow path) and
/// `uffs-daemon::handler` (fast path) can share the exact same struct.
/// Borrowed slices / strs keep the type free of allocations — the
/// footer writer only reads, never takes ownership.
///
/// # Field discipline (Phase 3b §3.4)
///
/// All three fields are `pub` because they are **required positional
/// inputs** to [`write_legacy_drive_footer`].  A builder pattern would
/// add lifetime-parameter friction (`DriveFooterContextBuilder<'a>`)
/// without changing the required-vs-optional nature of any field.
///
/// # `#[non_exhaustive]` decision (Phase 3b §3.6)
///
/// **Kept exhaustive.**  This is a borrowed-data DTO whose three
/// fields are all required arguments to the writer; future growth
/// would mean a new required argument (which is a breaking change
/// regardless of `#[non_exhaustive]`).  Both call sites
/// (`uffs_cli::commands::output::parity::write_legacy_drive_footer`
/// and `uffs_daemon::handler_blob`) live in the same workspace.
#[derive(Debug, Clone, Copy)]
pub struct DriveFooterContext<'a> {
    /// Drive letters the search targeted (e.g. `['C', 'D']`).  When
    /// empty the footer is omitted entirely — matches the CLI's
    /// behaviour for searches that did not specify an explicit
    /// `--drive` / `--drives`.
    pub output_targets: &'a [char],
    /// Raw search pattern the user supplied (`"*"`, `"*.dll"`,
    /// `">ext.*"`, …).  Used both to label the fast-scan warning
    /// line and to drive the full-scan heuristic.
    pub pattern: &'a str,
    /// Total number of result rows written before the footer.  The
    /// fast-scan warning only fires below the 20 000-row threshold
    /// (legacy baseline — anything larger is considered a "real"
    /// search, not a tripped-over-the-keyboard full scan).
    pub row_count: usize,
}

/// Row-count threshold below which a full-scan pattern triggers the
/// `MMMmmm that was FAST` warning line.  Matches the baseline CLI.
pub(crate) const FAST_SCAN_ROW_LIMIT: usize = 20_000;

/// Append the legacy drive footer to `writer`.
///
/// Emits nothing when `ctx.output_targets` is empty — matches the
/// baseline CLI where `--format custom` without an explicit drive
/// target produces pure CSV.
///
/// # Errors
///
/// Propagates any `io::Error` the underlying writer returns.
pub fn write_legacy_drive_footer<W: Write + ?Sized>(
    writer: &mut W,
    ctx: &DriveFooterContext<'_>,
) -> io::Result<()> {
    if ctx.output_targets.is_empty() {
        return Ok(());
    }

    write!(writer, "\r\n\r\n")?;
    write!(
        writer,
        "Drives? \t{count}\t{letters}\r\n",
        count = ctx.output_targets.len(),
        letters = format_legacy_drive_letters(ctx.output_targets)
    )?;
    write!(writer, "\r\n")?;

    if ctx.row_count < FAST_SCAN_ROW_LIMIT && is_full_scan_pattern(ctx.pattern) {
        write!(
            writer,
            "MMMmmm that was FAST ... maybe your searchstring was wrong?\t{pattern}\r\n",
            pattern = ctx.pattern,
        )?;
        write!(writer, "Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n")?;
    }

    Ok(())
}

/// Format drive letters in the legacy `C:|D:|E:` shape — uppercase
/// letters separated by `|`, each followed by `:`.
fn format_legacy_drive_letters(output_targets: &[char]) -> String {
    output_targets
        .iter()
        .map(|drive| format!("{}:", drive.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join("|")
}

/// Classify a pattern as a full-scan candidate for the fast-scan
/// warning.
///
/// Three shapes match:
/// - Empty string / `"*"` / `"**"` / `"**/*"` — the literal full-scan glob
///   forms the CLI accepts.
/// - `">"`-prefixed regex whose every alternation branch ends in `".*"` and has
///   length ≤ 4 (e.g. `">.*"`, `">C:.*"`, `">.*|.*"`) — these are regex
///   spellings of "match everything".
///
/// Extracted into a named function so the daemon-side fast path and
/// the CLI-side slow path share the exact same classification — and
/// so the regression test below can pin every accepted / rejected
/// shape without running the full footer writer.
#[must_use]
pub(crate) fn is_full_scan_pattern(pattern: &str) -> bool {
    matches!(pattern, "" | "*" | "**" | "**/*")
        || pattern.strip_prefix('>').is_some_and(|rest| {
            rest.split('|')
                .all(|seg| seg.ends_with(".*") && seg.len() <= 4)
        })
}

#[cfg(test)]
mod tests {
    use super::{DriveFooterContext, is_full_scan_pattern, write_legacy_drive_footer};

    /// Empty `output_targets` must skip the footer entirely — matches
    /// the baseline CLI's "no drives, no footer" rule.
    #[test]
    fn empty_targets_omits_footer() {
        let mut buf = Vec::new();
        let ctx = DriveFooterContext {
            output_targets: &[],
            pattern: "*",
            row_count: 0,
        };
        write_legacy_drive_footer(&mut buf, &ctx).expect("write");
        assert!(buf.is_empty(), "expected no bytes, got {buf:?}");
    }

    /// Single-drive full scan below the row-count threshold triggers
    /// the fast-scan warning.  Pins the exact byte sequence so a
    /// regression on either the format or the threshold is caught.
    #[test]
    fn single_drive_full_scan_includes_fast_warning() {
        let mut buf = Vec::new();
        let ctx = DriveFooterContext {
            output_targets: &['G'],
            pattern: "*",
            row_count: 100,
        };
        write_legacy_drive_footer(&mut buf, &ctx).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        assert_eq!(
            text,
            "\r\n\r\nDrives? \t1\tG:\r\n\r\n\
             MMMmmm that was FAST ... maybe your searchstring was wrong?\t*\r\n\
             Search path. E.g. 'C:/' or 'C:\\Prog**' \r\n",
        );
    }

    /// Multi-drive footer joins with `|` in the legacy style.
    #[test]
    fn multi_drive_letters_are_pipe_joined() {
        let mut buf = Vec::new();
        let ctx = DriveFooterContext {
            output_targets: &['c', 'D'],
            pattern: ">.*\\.(jpg|png)",
            row_count: 5_000,
        };
        write_legacy_drive_footer(&mut buf, &ctx).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(
            text.starts_with("\r\n\r\nDrives? \t2\tC:|D:\r\n\r\n"),
            "unexpected header: {text:?}"
        );
        // Real regex (has alternation with a real suffix) — fast-scan
        // warning must NOT fire.
        assert!(!text.contains("MMMmmm"));
    }

    /// At or above the 20 000-row threshold the fast-scan warning is
    /// suppressed even for full-scan patterns.  Mirrors
    /// `test_legacy_footer_omits_fast_scan_message_when_many_results`
    /// on the CLI side so the threshold stays in sync.
    #[test]
    fn fast_scan_suppressed_at_row_threshold() {
        let mut buf = Vec::new();
        let ctx = DriveFooterContext {
            output_targets: &['G'],
            pattern: "*",
            row_count: super::FAST_SCAN_ROW_LIMIT,
        };
        write_legacy_drive_footer(&mut buf, &ctx).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("Drives? \t1\tG:"));
        assert!(!text.contains("MMMmmm"));
    }

    /// `is_full_scan_pattern` classifier coverage — pin every branch.
    #[test]
    fn is_full_scan_pattern_branches() {
        // Literal full-scan shapes.
        assert!(is_full_scan_pattern(""));
        assert!(is_full_scan_pattern("*"));
        assert!(is_full_scan_pattern("**"));
        assert!(is_full_scan_pattern("**/*"));

        // Regex full-scan shapes (≤ 4-char segments ending in `.*`).
        assert!(is_full_scan_pattern(">.*"));
        assert!(is_full_scan_pattern(">C:.*"));
        assert!(is_full_scan_pattern(">.*|.*"));

        // Real patterns — must not match.
        assert!(!is_full_scan_pattern("*.txt"));
        assert!(!is_full_scan_pattern(">.*\\.dll"));
        assert!(
            !is_full_scan_pattern(">name|.*"),
            "long segment disqualifies"
        );
    }
}
