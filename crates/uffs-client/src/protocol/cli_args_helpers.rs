// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stateless parsing helpers shared by [`super::cli_args`].
//!
//! Lifted out of `cli_args.rs` purely to keep that file under the
//! 800-line policy ceiling.  None of these helpers depend on
//! [`super::SearchParams`] state — they are pure conversions between
//! CLI token strings and typed values, so living in a sibling module
//! costs nothing at call sites and buys generous head-room in the
//! main file.

// `parse_size` is re-exported so the importer can glob-import every
// parsing helper the main file needs in a single `use` statement.
pub(super) use crate::format::parse_size;

/// Returns `Some(val)` if `val` is non-empty, otherwise `None`.
/// Used for optional output-config fields that should fall back to
/// `OutputConfig` defaults when the user did not supply them.
pub(super) fn non_empty(val: String) -> Option<String> {
    if val.is_empty() { None } else { Some(val) }
}

/// Consume next token or report missing value.
pub(super) fn take_next(
    flag: &str,
    iter: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("Missing value for {flag}"))
}

/// Handle `--flag=val` or `--flag <val>`.
pub(super) fn flag_val(
    cur: &str,
    flag: &str,
    iter: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    cur.strip_prefix(&format!("{flag}="))
        .map_or_else(|| take_next(flag, iter), |rest| Ok(rest.to_owned()))
}

/// Parse comma-separated drive letters.
pub(super) fn drives_csv(input: &str) -> Result<Vec<uffs_mft::platform::DriveLetter>, String> {
    input
        .split(',')
        .map(|part| {
            let stripped = part.trim();
            let trimmed = stripped.strip_suffix(':').unwrap_or(stripped);
            let ch = trimmed
                .chars()
                .next()
                .ok_or_else(|| "empty drive".to_owned())?;
            if trimmed.len() != 1 || !ch.is_ascii_alphabetic() {
                return Err(format!("Bad drive: '{part}'"));
            }
            // `is_ascii_alphabetic` proves the parse succeeds; we
            // forward the `DriveLetterError` text for the (impossible)
            // failure case so the error path is still readable if the
            // invariant ever changes.
            uffs_mft::platform::DriveLetter::parse(ch)
                .map_err(|err| format!("Bad drive: '{part}' ({err})"))
        })
        .collect()
}

/// Parse string to `u16`.
pub(super) fn parse_u16(flag: &str, text: &str) -> Result<u16, String> {
    text.parse().map_err(|err| format!("Bad {flag}: {err}"))
}
/// Parse string to `u32`.
pub(super) fn parse_u32(flag: &str, text: &str) -> Result<u32, String> {
    text.parse().map_err(|err| format!("Bad {flag}: {err}"))
}
/// Parse string to `u64`.
pub(super) fn parse_u64(flag: &str, text: &str) -> Result<u64, String> {
    text.parse().map_err(|err| format!("Bad {flag}: {err}"))
}
/// Parse string to `i32`.
pub(super) fn parse_i32(flag: &str, text: &str) -> Result<i32, String> {
    text.parse().map_err(|err| format!("Bad {flag}: {err}"))
}

/// Parse boolean value.
pub(super) fn parse_bool(flag: &str, text: &str) -> Result<bool, String> {
    match text {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(format!("Bad bool for {flag}: '{text}'")),
    }
}

/// Return `true` when `s` is exactly `*.<alnum+underscore>+` — a pure
/// extension glob that can be safely promoted to an `ExtensionIndex` lookup.
///
/// Examples that return `true`:  `*.dll`, `*.rs`, `*.tar_gz`, `*.7z`, `*.1`.
///
/// Examples that return `false` (must stay on the trigram path):
/// - `*.*`      — rest `*` is not alnum (matches ANY extension).
/// - `*.d??`    — question-mark not alnum.
/// - `*.[ch]`   — character class not alnum.
/// - `*.tar.gz` — dot in rest (multi-segment).
/// - `*.dll*`   — trailing star not alnum.
/// - `**/*.dll` — leading doublestar not `*.` prefix.
/// - `*.`       — empty extension.
///
/// Mirrored by `uffs_core::search::backend::is_pure_ext_glob` (the
/// daemon's belt-and-suspenders safety net at dispatch time).  Keep the
/// two definitions in sync.
pub(super) fn is_pure_ext_glob(pattern: &str) -> bool {
    pattern.strip_prefix("*.").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    })
}

/// Extract a pure trailing-extension alternation from a regex pattern.
///
/// Returns `Some(exts)` when `pattern` matches a narrow shape that is
/// semantically equivalent to `.*\.(e1|e2|...)$` — i.e. the extensions
/// can be routed through the `ExtensionIndex` fast path without
/// changing the result set.  Returns `None` for any more complex shape
/// so the regex stays on the full-scan path.
///
/// All accepted forms **require a trailing `$` anchor**: without it,
/// `\.jpg` matches `.jpg` anywhere in the name (e.g. `foo.jpg.txt`),
/// which the ext-index cannot replicate — it matches by the
/// `extension_id` on the trailing dot-segment only.  Requiring `$`
/// keeps the rewrite semantically lossless.
///
/// Examples that parse:
/// - `>.*\.(jpg|png|heic)$` → `Some(["jpg", "png", "heic"])`
/// - `>\.rs$`                → `Some(["rs"])`
/// - `>^\.(a|b)$`            → `Some(["a", "b"])`
/// - `>(?i).*\.(DLL|EXE)$`   → `Some(["dll", "exe"])` (lower-cased)
///
/// Examples that return `None`:
/// - `>.*\.jpg`              — missing `$`
/// - `>.*\.(tar\.gz|zip)$`   — dot inside alternation
/// - `>.*\.(jp.?)$`          — wildcard inside alternation
/// - `>C:\\Users\\.*\.dll$`  — literal prefix (must keep path-anchor semantics)
///
/// Mirrored by `uffs_core::search::dispatch::extract_extensions_from_regex`
/// (the daemon's belt-and-suspenders safety net at dispatch time).
/// Keep the two definitions in sync.
pub(super) fn extract_extensions_from_regex(pattern: &str) -> Option<Vec<String>> {
    let mut body = pattern.strip_prefix('>')?;
    if body.is_empty() {
        return None;
    }

    // Strip optional inline case-insensitive flag group.
    body = body.strip_prefix("(?i)").unwrap_or(body);
    // Strip optional start-of-string anchor.
    body = body.strip_prefix('^').unwrap_or(body);
    // Strip optional `.*` prefix (match-any-prefix).
    body = body.strip_prefix(".*").unwrap_or(body);
    // Must start with a literal dot `\.`.
    body = body.strip_prefix("\\.")?;
    // Required trailing `$` anchor.
    body = body.strip_suffix('$')?;

    let exts: Vec<String> = body
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .map_or_else(
            || vec![body.to_ascii_lowercase()],
            |group| group.split('|').map(str::to_ascii_lowercase).collect(),
        );

    exts.iter()
        .all(|ext| {
            !ext.is_empty()
                && ext
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .then_some(exts)
}

/// Parse a bare drive-letter prefix from a pattern.
///
/// Returns `Some((letter_upper, rest))` when `pattern` matches exactly:
/// - a single ASCII alphabetic character (the drive letter), followed by
/// - a literal `:`, followed by
/// - a non-empty `rest` that does **not** start with `\` or `/` (if it does,
///   the pattern is path-anchored and must route through the tree walker in
///   `uffs_core::search::tree`, which already scopes its walk to the drive
///   root).
///
/// Examples that parse:
/// - `C:*.dll`       → `('C', "*.dll")`
/// - `D:notepad.exe` → `('D', "notepad.exe")`
/// - `c:*.log`       → `('C', "*.log")` — letter is uppercased
///
/// Examples that return `None`:
/// - `C:\*.dll`      — rest starts with `\` (path pattern, tree walker).
/// - `C:/home/*.dll` — rest starts with `/` (path pattern).
/// - `C:`            — empty rest.
/// - `C`             — no colon.
/// - `*.dll`         — no drive prefix.
/// - `12:34`         — letter is not alphabetic.
///
/// Mirrored by `uffs_core::search::backend::parse_bare_drive_prefix`
/// (the daemon's belt-and-suspenders safety net at dispatch time).
/// Keep the two definitions in sync.
pub(super) fn parse_bare_drive_prefix(
    pattern: &str,
) -> Option<(uffs_mft::platform::DriveLetter, &str)> {
    let bytes = pattern.as_bytes();
    let letter = *bytes.first()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    if *bytes.get(1)? != b':' {
        return None;
    }
    // Drive-letter + ':' are both ASCII → the byte offset to `rest` is 2.
    let rest = pattern.get(2..)?;
    if rest.is_empty() || rest.starts_with(['\\', '/']) {
        return None;
    }
    // The `is_ascii_alphabetic` guard above proves the `try_from`
    // cannot fail; using `?` keeps the API a `Option` and avoids an
    // unwrap.
    let drive_letter = uffs_mft::platform::DriveLetter::try_from(letter).ok()?;
    Some((drive_letter, rest))
}
