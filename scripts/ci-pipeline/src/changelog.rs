// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

#![expect(
    clippy::print_stdout,
    reason = "operational CLI tool — ship progress lines go to stdout (issue #212)"
)]

//! Roll `## [Unreleased]` into a dated release section at ship time.
//!
//! `just ship` bumps the lockstep workspace version (see [`crate::version`])
//! and then creates the signed release commit.  Historically that flow never
//! rolled the changelog's `## [Unreleased]` section into a `## [vX.Y.Z]`
//! section, so `## [Unreleased]` silently accumulated already-shipped work and
//! every release in between went unrecorded — the drift repaired wholesale in
//! PR #490.
//!
//! [`roll_changelog_file`] closes that gap as part of the same release commit:
//! it moves the current `## [Unreleased]` body under a fresh dated
//! `## [version]` header, leaves an empty `## [Unreleased]` for the next cycle,
//! and keeps the Keep-a-Changelog footer compare-links correct.  The transform
//! ([`roll_unreleased`]) is pure and unit-tested; the file wrapper is the thin
//! IO shell.

use anyhow::{Context as _, Result};

/// The canonical unreleased-section header (Keep a Changelog).
const UNRELEASED_HEADER: &str = "## [Unreleased]";

/// Path to the workspace changelog, relative to the repo root that `just ship`
/// runs from.
const CHANGELOG_PATH: &str = "CHANGELOG.md";

/// Roll `CHANGELOG.md` in place: move the `## [Unreleased]` body under a dated
/// `## [version]` section and repoint the footer compare-links.
///
/// Called from Phase 2 of the ship pipeline right after the version bump, so
/// the rolled changelog is staged into the `chore: development vX.Y.Z` release
/// commit.  A missing `CHANGELOG.md` and an empty `## [Unreleased]` are both
/// soft no-ops (the ship flow still proceeds) — only a malformed changelog
/// (no `## [Unreleased]` header at all) is an error.
///
/// # Errors
///
/// Returns an error if the changelog exists but cannot be parsed (no
/// `## [Unreleased]` header) or cannot be written back.
pub(crate) fn roll_changelog_file(version: &str) -> Result<()> {
    let Ok(content) = std::fs::read_to_string(CHANGELOG_PATH) else {
        println!("📝 {CHANGELOG_PATH} not found — skipping changelog roll.");
        return Ok(());
    };
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    match roll_unreleased(&content, version, &date)? {
        Some(rolled) => {
            std::fs::write(CHANGELOG_PATH, rolled)
                .with_context(|| format!("writing rolled {CHANGELOG_PATH}"))?;
            println!("📝 Rolled CHANGELOG [Unreleased] → [{version}] - {date}");
        }
        None => println!("📝 CHANGELOG [Unreleased] is empty — nothing to roll."),
    }
    Ok(())
}

/// Roll the `## [Unreleased]` section of `content` (a Keep-a-Changelog
/// document) into a dated `## [version] - date` release section.
///
/// Returns `Ok(Some(new_content))` when `## [Unreleased]` held entries to roll,
/// or `Ok(None)` when it was empty (nothing notable to release-note — the
/// caller leaves the file untouched).  `version` is the bumped workspace
/// version without a leading `v` (e.g. `"0.6.16"`); `date` is `YYYY-MM-DD`.
///
/// The transform is re-run safe: the body moves down under the new header and a
/// fresh empty `## [Unreleased]` stays on top, so rolling the result again is a
/// no-op (`Ok(None)`).
///
/// # Errors
///
/// Returns an error if `content` has no `## [Unreleased]` header.
pub(crate) fn roll_unreleased(content: &str, version: &str, date: &str) -> Result<Option<String>> {
    let lines: Vec<&str> = content.lines().collect();
    let unreleased_idx = lines
        .iter()
        .position(|line| line.trim_end() == UNRELEASED_HEADER)
        .context("CHANGELOG.md has no `## [Unreleased]` header to roll")?;
    let body_start = unreleased_idx + 1;

    // The body runs to the next top-level `## ` section (the previous release),
    // or to end-of-document when only the footer link-refs follow.
    let next_section_idx = lines
        .iter()
        .enumerate()
        .skip(body_start)
        .find(|(_, line)| line.starts_with("## "))
        .map_or(lines.len(), |(idx, _)| idx);

    let body: Vec<&str> = lines
        .iter()
        .skip(body_start)
        .take(next_section_idx.saturating_sub(body_start))
        .copied()
        .collect();
    // Nothing notable since the last release → leave the file alone.
    if body.iter().all(|line| line.trim().is_empty()) {
        return Ok(None);
    }
    let trimmed = trim_blank_edges(&body);

    // Previous release version, parsed from the next `## [x] - ...` header — used
    // for the new footer compare-link.  Absent on a first release.
    let prev_version = lines
        .get(next_section_idx)
        .and_then(|header| parse_section_version(header));

    // Rebuild: prefix (through the [Unreleased] header) / blank / dated header /
    // blank / body / blank / the remaining sections.
    let mut out: Vec<String> = Vec::new();
    out.extend(lines.iter().take(body_start).map(|line| (*line).to_owned()));
    out.push(String::new());
    out.push(format!("## [{version}] - {date}"));
    out.push(String::new());
    out.extend(trimmed.iter().map(|line| (*line).to_owned()));
    out.push(String::new());
    out.extend(
        lines
            .iter()
            .skip(next_section_idx)
            .map(|line| (*line).to_owned()),
    );

    let mut rolled = out.join("\n");
    rolled.push('\n');
    let with_footer = update_footer_links(&rolled, version, prev_version.as_deref());
    Ok(Some(with_footer))
}

/// Drop leading and trailing all-blank lines from a section body, preserving
/// interior blank lines.  The caller has already established that at least one
/// line is non-blank.
fn trim_blank_edges<'body>(body: &[&'body str]) -> Vec<&'body str> {
    let start = body
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(0);
    let end = body
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |idx| idx + 1);
    body.iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .copied()
        .collect()
}

/// Parse the version label out of a `## [x.y.z] - date` (or `## [x.y.z]`)
/// section header, returning `x.y.z` without the surrounding brackets.
fn parse_section_version(header: &str) -> Option<String> {
    let rest = header.strip_prefix("## [")?;
    let end = rest.find(']')?;
    rest.get(..end).map(str::to_owned)
}

/// Repoint the Keep-a-Changelog footer compare-links for the new release:
/// move `[Unreleased]` to `vNEW...HEAD` and add `[vNEW]: …/vPREV...vNEW`.
///
/// Defensive by design: a document with no `[Unreleased]:` link-ref (or a link
/// that does not use the GitHub `/compare/` form) is returned unchanged — the
/// section roll is the load-bearing part, the footer is best-effort polish.
fn update_footer_links(content: &str, version: &str, prev: Option<&str>) -> String {
    let Some(unreleased_link) = content
        .lines()
        .find(|line| line.starts_with("[Unreleased]:"))
    else {
        return content.to_owned();
    };
    // Strip the `[Unreleased]: ` label, then keep the URL up to `/compare/` so
    // the rebuilt links carry only the URL (not a doubled label).
    let Some((_, url)) = unreleased_link.split_once(": ") else {
        return content.to_owned();
    };
    let Some((url_prefix, _)) = url.split_once("/compare/") else {
        return content.to_owned();
    };
    let base = format!("{url_prefix}/compare/");
    let new_unreleased = format!("[Unreleased]: {base}v{version}...HEAD");
    let version_ref_prefix = format!("[{version}]:");
    let already_present = content
        .lines()
        .any(|line| line.starts_with(&version_ref_prefix));
    let new_version_ref =
        prev.map(|prev_ver| format!("[{version}]: {base}v{prev_ver}...v{version}"));

    let mut out: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.starts_with("[Unreleased]:") {
            out.push(new_unreleased.clone());
            if let Some(reference) = new_version_ref.as_ref()
                && !already_present
            {
                out.push(reference.clone());
            }
        } else {
            out.push(line.to_owned());
        }
    }
    let mut joined = out.join("\n");
    if content.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::{parse_section_version, roll_unreleased};

    /// A representative changelog: rich Unreleased body, one prior release, and
    /// Keep-a-Changelog footer compare-links.
    const SAMPLE: &str = "\
# Changelog

## [Unreleased]

### Added — a new thing

- did stuff (#100)

## [0.6.15] - 2026-06-28

### Fixed

- old fix (#1)

[Unreleased]: https://github.com/o/r/compare/v0.6.15...HEAD
[0.6.15]: https://github.com/o/r/compare/v0.6.14...v0.6.15
";

    #[test]
    fn rolls_unreleased_into_dated_section() {
        let out = roll_unreleased(SAMPLE, "0.6.16", "2026-06-30")
            .unwrap()
            .unwrap();
        assert!(out.contains("## [0.6.16] - 2026-06-30"));
        assert!(out.contains("### Added — a new thing"));
        // The Unreleased header survives but no longer holds the moved body.
        let before_new = out.split("## [0.6.16]").next().unwrap();
        assert!(before_new.contains("## [Unreleased]"));
        assert!(!before_new.contains("new thing"));
        // Footer links repointed.
        assert!(out.contains("[Unreleased]: https://github.com/o/r/compare/v0.6.16...HEAD"));
        assert!(out.contains("[0.6.16]: https://github.com/o/r/compare/v0.6.15...v0.6.16"));
    }

    #[test]
    fn empty_unreleased_is_a_noop() {
        let doc = "# Changelog\n\n## [Unreleased]\n\n## [0.6.15] - 2026-06-28\n\n- x\n";
        assert!(
            roll_unreleased(doc, "0.6.16", "2026-06-30")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rolling_twice_is_idempotent() {
        let once = roll_unreleased(SAMPLE, "0.6.16", "2026-06-30")
            .unwrap()
            .unwrap();
        assert!(
            roll_unreleased(&once, "0.6.17", "2026-07-01")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn missing_unreleased_header_errors() {
        roll_unreleased("# Changelog\n\n## [0.6.15]\n", "0.6.16", "2026-06-30").unwrap_err();
    }

    #[test]
    fn rolls_section_even_without_footer_links() {
        let doc = "## [Unreleased]\n\n- a change\n\n## [0.6.15] - 2026-06-28\n\n- old\n";
        let out = roll_unreleased(doc, "0.6.16", "2026-06-30")
            .unwrap()
            .unwrap();
        assert!(out.contains("## [0.6.16] - 2026-06-30"));
        assert!(out.contains("- a change"));
    }

    #[test]
    fn parses_section_version_label() {
        assert_eq!(
            parse_section_version("## [0.6.15] - 2026-06-28").as_deref(),
            Some("0.6.15")
        );
        // Returns whatever sits in the brackets verbatim; the roll only ever
        // feeds it a real release header (never the Unreleased one).
        assert_eq!(
            parse_section_version("## [Unreleased]").as_deref(),
            Some("Unreleased")
        );
        assert_eq!(parse_section_version("not a header"), None);
    }
}
