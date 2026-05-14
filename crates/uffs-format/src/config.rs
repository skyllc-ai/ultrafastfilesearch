// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `OutputConfig` — the knobs that drive [`crate::write_rows`].
//!
//! This is the canonical configuration type.  `uffs-core` re-exports
//! it and `uffs-client` re-exports it, so downstream consumers see
//! one `OutputConfig` regardless of which end of the pipeline they
//! touch.

use crate::column::{OutputColumn, PARITY_COLUMN_ORDER};

/// Configuration for columnar CSV output.
///
/// The defaults reproduce the legacy CLI behaviour (comma separator,
/// double-quote quote character, header row on, positive / negative
/// boolean rendered as `"1"` / `"0"`).  The TZ offset defaults to the
/// host's local timezone, matching the pre-unification CLI behaviour.
///
/// # Field discipline (Phase 3b §3.4)
///
/// All fields are `pub` because this is a **configuration DTO**:
/// callers fill in the knobs they care about and rely on
/// [`Default::default`] for the rest.  The builder methods
/// ([`Self::with_columns`], [`Self::with_separator`], etc.) layer
/// fluent ergonomics on top but never claim sole-constructor status.
///
/// # `#[non_exhaustive]` decision (Phase 3b §3.6)
///
/// **Kept exhaustive** for now.  Two workspace-internal call sites
/// (`uffs_daemon::handler_blob::core_config_to_format` and
/// `uffs_core::output::tests_format_parity`) struct-literal-construct
/// this type; both live in the same monorepo and can be updated
/// atomically.  When `uffs-format` graduates from its Polars-blocked
/// state and becomes externally publishable
/// (`docs/architecture/crate-graph.md` §5), revisit and add
/// `#[non_exhaustive]` plus typed builder methods for the fields that
/// today only have string-parsing builders.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Columns to output.
    ///
    /// `None` means "all baseline columns" — see
    /// [`crate::column::BASELINE_COLUMN_ORDER`].
    pub columns: Option<Vec<OutputColumn>>,
    /// Field separator (default: `","`).
    pub separator: String,
    /// Quote character wrapping string / quoted columns (default: `"\""`).
    pub quote: String,
    /// Whether to emit a header row before the data (default: `true`).
    pub header: bool,
    /// Text for a `true` boolean flag column (default: `"1"`).
    pub pos: String,
    /// Text for a `false` boolean flag column (default: `"0"`).
    pub neg: String,
    /// Fixed timezone offset (seconds from UTC) applied to all
    /// timestamp columns.  Matches Windows'
    /// `FileTimeToLocalFileTime()` behaviour: a single CURRENT offset
    /// is applied to every row, ignoring historical DST transitions.
    pub timezone_offset_secs: i32,
    /// Parity-compat mode: directories get trailing `\` in `Path`,
    /// empty `Name`, self-path in `PathOnly`, and treesize for `Size`.
    ///
    /// Set by `--parity-compat` at the CLI.  The writer's parity-dir
    /// logic activates when this flag is `true` and the row is a
    /// directory.
    pub parity_compat: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        // Auto-detect the host's local offset once — matches the
        // legacy CLI behaviour and `uffs-core::output::OutputConfig`
        // before the v0.5.62 unification.
        let timezone_offset_secs = chrono::Local::now().offset().local_minus_utc();

        Self {
            columns: None,
            separator: ",".to_owned(),
            quote: "\"".to_owned(),
            header: true,
            pos: "1".to_owned(),
            neg: "0".to_owned(),
            timezone_offset_secs,
            parity_compat: false,
        }
    }
}

impl OutputConfig {
    /// Construct a config with the default field values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a comma-separated column spec.
    ///
    /// `"all"` returns `None` (meaning "all baseline columns").
    /// `"parity"` returns the 25-column parity baseline.  Any other
    /// input is split on `,`, each token is resolved via
    /// [`OutputColumn::parse`], and unrecognised tokens are silently
    /// dropped.  An empty / all-unrecognised input returns `None`
    /// (fall-through to defaults).
    #[must_use]
    #[expect(
        clippy::shadow_reuse,
        reason = "rebinding input to the trimmed + lowered form is idiomatic"
    )]
    pub fn parse_columns(input: &str) -> Option<Vec<OutputColumn>> {
        let input = input.trim().to_lowercase();
        if input == "all" {
            return None;
        }
        if input == "parity" {
            return Some(PARITY_COLUMN_ORDER.to_vec());
        }

        let cols: Vec<OutputColumn> = input
            .split(',')
            .filter_map(|col| OutputColumn::parse(col.trim()))
            .collect();

        if cols.is_empty() { None } else { Some(cols) }
    }

    /// Parse a separator string with the legacy special-name escapes.
    ///
    /// Supports (case-insensitive): `"TAB"` → `\t`, `"NEWLINE"` /
    /// `"NEW LINE"` → `\n`, `"SPACE"` → ` `, `"RETURN"` → `\r`,
    /// `"DOUBLE"` → `"`, `"SINGLE"` → `'`, `"NULL"` → `\0`.  Any other
    /// input is returned verbatim (so `--sep ";"` works as expected).
    #[must_use]
    pub fn parse_separator(input: &str) -> String {
        match input.to_uppercase().as_str() {
            "TAB" => "\t".to_owned(),
            "NEWLINE" | "NEW LINE" => "\n".to_owned(),
            "SPACE" => " ".to_owned(),
            "RETURN" => "\r".to_owned(),
            "DOUBLE" => "\"".to_owned(),
            "SINGLE" => "'".to_owned(),
            "NULL" => "\0".to_owned(),
            _ => input.to_owned(),
        }
    }

    /// Set columns from a string (uses [`Self::parse_columns`]).
    #[must_use]
    pub fn with_columns(mut self, columns: &str) -> Self {
        self.columns = Self::parse_columns(columns);
        self
    }

    /// Set field separator (uses [`Self::parse_separator`]).
    #[must_use]
    pub fn with_separator(mut self, sep: &str) -> Self {
        self.separator = Self::parse_separator(sep);
        self
    }

    /// Set quote character.
    #[must_use]
    pub fn with_quote(mut self, quote: &str) -> Self {
        quote.clone_into(&mut self.quote);
        self
    }

    /// Toggle header-row emission.
    #[must_use]
    pub const fn with_header(mut self, header: bool) -> Self {
        self.header = header;
        self
    }

    /// Set text for a `true` boolean flag column.
    #[must_use]
    pub fn with_pos(mut self, pos: &str) -> Self {
        pos.clone_into(&mut self.pos);
        self
    }

    /// Set text for a `false` boolean flag column.
    #[must_use]
    pub fn with_neg(mut self, neg: &str) -> Self {
        neg.clone_into(&mut self.neg);
        self
    }

    /// Override the timezone offset (hours from UTC).
    #[must_use]
    pub const fn with_tz_offset_hours(mut self, hours: i32) -> Self {
        self.timezone_offset_secs = hours * 3_600_i32;
        self
    }

    /// Toggle parity-compat directory formatting.
    #[must_use]
    pub const fn with_parity_compat(mut self, enabled: bool) -> Self {
        self.parity_compat = enabled;
        self
    }

    /// Check whether the `Descendants` column is requested.
    ///
    /// Returns `false` when `columns` is `None` (meaning "all") —
    /// matches the pre-unification behaviour where `--columns all`
    /// deliberately does not materialise the descendants column.
    #[must_use]
    pub fn needs_descendants(&self) -> bool {
        self.columns
            .as_ref()
            .is_some_and(|cols| cols.contains(&OutputColumn::Descendants))
    }

    /// Check whether the path column (or `PathOnly`) is requested.
    ///
    /// Returns `true` when `columns` is `None` because `--columns all`
    /// includes `Path`.
    #[must_use]
    pub fn needs_path_column(&self) -> bool {
        self.columns.as_ref().is_none_or(|cols| {
            cols.contains(&OutputColumn::Path) || cols.contains(&OutputColumn::PathOnly)
        })
    }
}
