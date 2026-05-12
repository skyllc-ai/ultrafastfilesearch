// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Integration tests for the `uffs` CLI binary.

#![expect(
    unused_crate_dependencies,
    reason = "integration test — relaxed linting for test clarity"
)]

#[cfg(test)]
mod tests {
    use std::process::{self, Output};
    use std::time::{SystemTime, UNIX_EPOCH};

    use assert_cmd::Command;

    fn run_cli(test_name: &str, args: &[&str]) -> Output {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let log_dir = std::env::temp_dir().join(format!(
            "uffs-cli-tests-{test_name}-{}-{nanos}",
            process::id()
        ));

        Command::cargo_bin("uffs")
            .expect("uffs test binary should build")
            .env("NO_COLOR", "1")
            .env("UFFS_LOG_DIR", log_dir)
            .env_remove("RUST_LOG")
            .env_remove("RUST_LOG_FILE")
            .args(args)
            .output()
            .expect("CLI command should run")
    }

    fn assert_success(test_name: &str, args: &[&str], stdout_snippets: &[&str]) {
        let output = run_cli(test_name, args);
        assert!(
            output.status.success(),
            "expected success for {args:?}; stderr was: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        for snippet in stdout_snippets {
            assert!(
                stdout.contains(snippet),
                "stdout for {args:?} did not contain {snippet:?}: {stdout}"
            );
        }
    }

    fn assert_failure(test_name: &str, args: &[&str], stderr_snippets: &[&str]) {
        let output = run_cli(test_name, args);
        assert!(
            !output.status.success(),
            "expected failure for {args:?}; stdout was: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        for snippet in stderr_snippets {
            assert!(
                stderr.contains(snippet),
                "stderr for {args:?} did not contain {snippet:?}: {stderr}"
            );
        }
    }

    #[test]
    fn no_args_prints_top_level_help() {
        assert_success("no_args_help", &[], &[
            "uffs - Ultra Fast File Search",
            "USAGE:",
        ]);
    }

    #[test]
    fn help_flag_prints_examples() {
        assert_success("help_flag", &["--help"], &[
            "Search is the default action",
            "uffs '*.txt'",
        ]);
    }

    #[test]
    fn version_flag_prints_binary_version() {
        assert_success("version_flag", &["--version"], &[
            "uffs",
            env!("CARGO_PKG_VERSION"),
        ]);
    }

    // ── Validation tests ────────────────────────────────────────────
    //
    // With the thin-client approach, search-flag validation happens on
    // the daemon side.  Tests that validated clap error messages for
    // search flags (--min-size, --limit, --tz-offset, --drive conflicts)
    // are now daemon-level concerns tested in uffs-client/uffs-daemon.
    //
    // Stats subcommand validation is still client-side.

    #[test]
    fn stats_rejects_non_numeric_top() {
        assert_failure(
            "stats_invalid_top",
            &["stats", "saved.parquet", "--top", "abc"],
            &["Bad --top"],
        );
    }

    // ── --name-only tests ───────────────────────────────────────────
    //
    // These validations now happen daemon-side via search_cli.
    // We keep smoke tests that don't require a running daemon.

    #[test]
    fn name_only_accepts_plain_literal() {
        // Should not error with "--name-only cannot be used with path
        // patterns". The command will fail because no daemon is running,
        // but the validation error should not appear.
        let output = run_cli("name_only_plain", &[
            "hallo",
            "--name-only",
            "--mft-file",
            "nonexistent.bin",
        ]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("--name-only cannot be used with path patterns"),
            "plain literal should be accepted with --name-only"
        );
    }

    #[test]
    fn name_only_accepts_glob_pattern() {
        let output = run_cli("name_only_glob", &[
            "*.txt",
            "--name-only",
            "--mft-file",
            "nonexistent.bin",
        ]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("--name-only cannot be used with path patterns"),
            "glob should be accepted with --name-only"
        );
    }

    #[test]
    fn name_only_accepts_regex_with_backslash_escapes() {
        let output = run_cli("name_only_regex", &[
            r">.*\.(jpg|png)",
            "--name-only",
            "--mft-file",
            "nonexistent.bin",
        ]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("--name-only cannot be used with path patterns"),
            "regex patterns (starting with >) should be accepted with --name-only"
        );
    }
}
