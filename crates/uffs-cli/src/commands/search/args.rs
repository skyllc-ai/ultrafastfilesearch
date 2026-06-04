// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! CLI argument transforms for the `search` subcommand.
//!
//! These helpers run between raw `std::env::args()` and the daemon's
//! `search_cli` RPC.  They are pure (string-in, string-out) so the
//! `main` dispatcher can stay focused on flow control while each
//! transform owns its own contract and unit tests:
//!
//! * `extract_spawn_args` — pick the subset of flags that need to travel to a
//!   freshly spawned daemon (e.g. `--data-dir`, `--mft-file`, `--no-cache`,
//!   `--drive`, `--log-level`).
//! * `resolve_out_path` — rewrite `--out` / `--out=<path>` so relative paths
//!   resolve against the CLI's `current_dir`, not the daemon's.
//! * `inject_no_output_for_null_stdout` — append `--no-output` when stdout is
//!   the platform's null device, skipping the IPC row transfer entirely.
//! * `maybe_inject_no_output` — pure decision logic backing the above so it can
//!   be unit-tested without piping the test harness to NUL.
//! * `resolve_to_absolute` — shared path-resolution primitive used by
//!   `resolve_out_path`.

/// Extract daemon-spawn-relevant flags from raw CLI args.
///
/// The daemon auto-start needs `--data-dir`, `--mft-file`, `--no-cache`,
/// `--drive`, and log env vars. Everything else is irrelevant for spawn.
pub(crate) fn extract_spawn_args(args: &[String]) -> Vec<std::ffi::OsString> {
    let mut spawn: Vec<std::ffi::OsString> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let flag = arg.split('=').next().unwrap_or(arg.as_str());
        match flag {
            "--data-dir" | "--mft-file" | "--drive" | "--drives" | "--log-level" | "--log-file" => {
                spawn.push(std::ffi::OsString::from(arg));
                // If not `--flag=val` form, consume the next token as value.
                if !arg.contains('=')
                    && iter.peek().is_some_and(|peeked| {
                        !peeked.starts_with('-') || flag == "--drive" || flag == "--drives"
                    })
                {
                    // peek() confirmed the value exists, so next() is safe.
                    spawn.push(
                        iter.next()
                            .map_or_else(std::ffi::OsString::new, std::ffi::OsString::from),
                    );
                }
            }
            "--no-cache" => spawn.push(std::ffi::OsString::from(arg)),
            _ => {}
        }
    }

    // Forward log env vars.
    if let Ok(ll) = std::env::var("UFFS_LOG") {
        spawn.push(std::ffi::OsString::from("--log-level"));
        spawn.push(std::ffi::OsString::from(ll));
    }
    if let Ok(lf) = std::env::var("UFFS_LOG_FILE") {
        spawn.push(std::ffi::OsString::from("--log-file"));
        spawn.push(std::ffi::OsString::from(lf));
    }

    spawn
}

/// Resolve a relative `--out` path to absolute using the CLI's working
/// directory.
///
/// The daemon runs in a different working directory, so relative paths in
/// `--out` or `--out=<path>` would resolve against the wrong directory if
/// passed through as-is.
pub(crate) fn resolve_out_path(args: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(val) = arg.strip_prefix("--out=") {
            // `--out=path` form — resolve inline value.
            let resolved = resolve_to_absolute(val);
            result.push(format!("--out={resolved}"));
        } else if arg == "--out" {
            result.push(arg.clone());
            // Next token is the path value.
            if let Some(val) = iter.next() {
                result.push(resolve_to_absolute(val));
            }
        } else {
            result.push(arg.clone());
        }
    }
    result
}

/// Append `--no-output` to `args` when stdout is redirected to the
/// null device, unless a disqualifying flag is already set.
///
/// Thin wrapper around [`maybe_inject_no_output`] that probes the real
/// stdout via [`uffs_client::stdout_kind::StdoutKind::detect`].  The
/// decision logic itself is in `maybe_inject_no_output` so it can be
/// unit-tested without fighting the test harness's stdout wiring.
pub(crate) fn inject_no_output_for_null_stdout(args: Vec<String>) -> Vec<String> {
    let stdout_is_null = uffs_client::stdout_kind::StdoutKind::detect().is_null();
    maybe_inject_no_output(args, stdout_is_null)
}

/// Pure decision logic for the NUL fast-path injection.
///
/// Returns `args` unchanged when `stdout_is_null == false` or when any
/// disqualifying flag is already present:
///
/// - `--no-output` already set: nothing to add.
/// - `--rows`: the user asked to force rows on even for aggregate queries —
///   honour that intent regardless of where stdout goes.
/// - `--out`: stdout is not the result destination; NUL on stdout is a benign
///   quirk, not the output target.
/// - `--agg` / `--facet` / `--stats` / `--histogram` / `--count`: any
///   aggregation flag already controls `include_rows` via its own sugar; adding
///   `--no-output` would be redundant at best.
pub(crate) fn maybe_inject_no_output(mut args: Vec<String>, stdout_is_null: bool) -> Vec<String> {
    if !stdout_is_null {
        return args;
    }
    let is_aggregate_flag = |flag: &str| {
        matches!(
            flag,
            "--agg" | "--facet" | "--stats" | "--histogram" | "--count"
        )
    };
    let disqualified = args.iter().any(|raw| {
        let flag = raw.split('=').next().unwrap_or(raw.as_str());
        flag == "--no-output" || flag == "--rows" || flag == "--out" || is_aggregate_flag(flag)
    });
    if disqualified {
        return args;
    }
    args.push("--no-output".to_owned());
    args
}

/// Resolve a potentially relative path to absolute using `current_dir`.
fn resolve_to_absolute(path_str: &str) -> String {
    let path = std::path::Path::new(path_str);
    if path.is_absolute() {
        return path_str.to_owned();
    }
    std::env::current_dir()
        .unwrap_or_default()
        .join(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{maybe_inject_no_output, resolve_to_absolute};

    // ── maybe_inject_no_output (Phase 3.1 NUL fast path) ─────────

    /// Helper: run [`maybe_inject_no_output`] with `stdout_is_null = true`
    /// and return the resulting args.
    fn inject_null(args: &[&str]) -> Vec<String> {
        let owned: Vec<String> = args.iter().copied().map(String::from).collect();
        maybe_inject_no_output(owned, true)
    }

    /// Baseline: a plain search with NUL stdout gets `--no-output`
    /// appended.  This is the hot path we want for benchmarks and
    /// `uffs *.dll > NUL`-style invocations.
    #[test]
    fn maybe_inject_no_output_appends_on_null_stdout() {
        let out = inject_null(&["*.dll", "--drive", "D"]);
        assert_eq!(out, ["*.dll", "--drive", "D", "--no-output"]);
    }

    /// Non-null stdout (terminal, pipe, file) must leave args alone,
    /// otherwise the user would see no output on their terminal.
    #[test]
    fn maybe_inject_no_output_unchanged_on_non_null_stdout() {
        let owned: Vec<String> = ["*.dll", "--drive", "D"]
            .into_iter()
            .map(String::from)
            .collect();
        let out = maybe_inject_no_output(owned.clone(), false);
        assert_eq!(out, owned);
    }

    /// Explicit `--no-output` already present: do not double up.
    #[test]
    fn maybe_inject_no_output_skips_when_already_present() {
        let out = inject_null(&["*.dll", "--no-output"]);
        // Exactly one occurrence — no double-injection.
        let count = out
            .iter()
            .filter(|arg| arg.as_str() == "--no-output")
            .count();
        assert_eq!(count, 1, "auto-injection must not duplicate user's flag");
    }

    /// `--rows` is the explicit "force rows on" override that wins
    /// over the NUL fast-path: leave `args` untouched even though
    /// stdout is NUL.
    ///
    /// Covers the user who wants `uffs *.rs --rows > NUL` for timing
    /// the full round-trip including IPC transport.
    #[test]
    fn maybe_inject_no_output_respects_rows_flag() {
        let out = inject_null(&["*.rs", "--rows"]);
        assert!(
            !out.iter().any(|arg| arg == "--no-output"),
            "--rows must prevent --no-output auto-injection, got: {out:?}"
        );
    }

    /// `--out file` routes results daemon-direct to disk — NUL on
    /// stdout is a benign quirk, not the output destination.
    #[test]
    fn maybe_inject_no_output_respects_out_flag() {
        let out = inject_null(&["*.rs", "--out", "results.csv"]);
        assert!(
            !out.iter().any(|arg| arg == "--no-output"),
            "--out must prevent --no-output auto-injection, got: {out:?}"
        );
    }

    /// Aggregation flags already control `include_rows` via their
    /// own sugar; auto-injection would be redundant.  Covers `--count`,
    /// `--agg`, `--facet`, `--stats`, `--histogram`.
    #[test]
    fn maybe_inject_no_output_respects_aggregation_flags() {
        for flag in ["--count", "--agg", "--facet", "--stats", "--histogram"] {
            // `--agg` and friends take a value; the parse-time check
            // keys on the flag name only, so a bare `--agg count` suffix
            // still trips the disqualifier exactly the same way as the
            // `--agg=count` form.
            let out_with_value = inject_null(&["*", flag, "stub"]);
            assert!(
                !out_with_value.iter().any(|arg| arg == "--no-output"),
                "{flag} must prevent --no-output auto-injection, got: {out_with_value:?}"
            );
            let out_equals = inject_null(&["*", &format!("{flag}=stub")]);
            assert!(
                !out_equals.iter().any(|arg| arg == "--no-output"),
                "{flag}=… must prevent --no-output auto-injection, got: {out_equals:?}"
            );
        }
    }

    /// `--out=file.csv` (equals form) must also disqualify the
    /// auto-injection.  The parser keys on the flag name before `=`.
    #[test]
    fn maybe_inject_no_output_respects_out_equals_form() {
        let out = inject_null(&["*", "--out=results.csv"]);
        assert!(
            !out.iter().any(|arg| arg == "--no-output"),
            "--out=<path> must prevent --no-output auto-injection, got: {out:?}"
        );
    }

    /// Absolute paths must round-trip unchanged so the daemon receives
    /// exactly what the user typed (no double-resolution against
    /// `current_dir`).
    #[test]
    fn resolve_to_absolute_keeps_absolute_paths() {
        let abs = if cfg!(windows) {
            r"C:\tmp\out.csv"
        } else {
            "/tmp/out.csv"
        };
        assert_eq!(resolve_to_absolute(abs), abs);
    }

    /// Relative paths must be anchored to `current_dir`.  We don't
    /// assert the exact prefix (it depends on where the test
    /// harness was invoked), only that the result is now absolute.
    #[test]
    fn resolve_to_absolute_anchors_relative_paths() {
        let resolved = resolve_to_absolute("relative.csv");
        let path = std::path::Path::new(&resolved);
        assert!(
            path.is_absolute(),
            "relative path must be resolved to absolute, got: {resolved}"
        );
        assert!(
            resolved.ends_with("relative.csv"),
            "resolved path must keep the original filename, got: {resolved}"
        );
    }
}
