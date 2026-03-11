//! Integration tests for the `uffs` CLI binary.

#![allow(
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    unused_crate_dependencies
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
    fn test_no_args_prints_top_level_help() {
        assert_success(
            "no_args_help",
            &[],
            &["Command-line interface for UFFS", "Usage:", "[PATTERN]"],
        );
    }

    #[test]
    fn test_help_flag_prints_examples() {
        assert_success(
            "help_flag",
            &["--help"],
            &[
                "Search is the default action",
                "uffs '*.txt'",
                "uffs index -d C index.parquet",
            ],
        );
    }

    #[test]
    fn test_version_flag_prints_binary_version() {
        assert_success(
            "version_flag",
            &["--version"],
            &["uffs", env!("CARGO_PKG_VERSION")],
        );
    }

    #[test]
    fn test_unknown_flag_reports_error() {
        assert_failure(
            "unknown_flag",
            &["--bogus"],
            &["unexpected argument '--bogus'", "Usage:"],
        );
    }

    #[test]
    fn test_index_help_prints_examples() {
        assert_success(
            "index_help",
            &["index", "--help"],
            &[
                "Build an index from drive MFT(s)",
                "uffs index --drives C,D,E out.parquet",
            ],
        );
    }

    #[test]
    fn test_info_help_prints_required_path_argument() {
        assert_success(
            "info_help",
            &["info", "--help"],
            &["Show information about an index file", "<PATH>"],
        );
    }

    #[test]
    fn test_stats_help_prints_top_option() {
        assert_success(
            "stats_help",
            &["stats", "--help"],
            &["Show statistics about files in an index", "--top <TOP>"],
        );
    }

    #[test]
    fn test_index_requires_output_argument() {
        assert_failure(
            "index_missing_output",
            &["index"],
            &["required arguments were not provided", "<OUTPUT>"],
        );
    }

    #[test]
    fn test_info_requires_path_argument() {
        assert_failure(
            "info_missing_path",
            &["info"],
            &["required arguments were not provided", "<PATH>"],
        );
    }

    #[test]
    fn test_stats_requires_path_argument() {
        assert_failure(
            "stats_missing_path",
            &["stats"],
            &["required arguments were not provided", "<PATH>"],
        );
    }

    #[test]
    fn test_stats_rejects_non_numeric_top() {
        assert_failure(
            "stats_invalid_top",
            &["stats", "saved.parquet", "--top", "abc"],
            &["invalid value 'abc'", "--top <TOP>"],
        );
    }

    #[test]
    fn test_search_rejects_invalid_drive_letter() {
        assert_failure(
            "search_invalid_drive",
            &["*.rs", "--drive", "1"],
            &["invalid value '1'", "must be A-Z"],
        );
    }

    #[test]
    fn test_search_rejects_conflicting_drive_flags() {
        assert_failure(
            "search_drive_conflict",
            &["*.rs", "--drive", "C", "--drives", "D"],
            &["cannot be used with", "--drives <DRIVES>"],
        );
    }

    #[test]
    fn test_search_rejects_conflicting_index_and_mft_file() {
        assert_failure(
            "search_index_mft_conflict",
            &["*.rs", "--index", "saved.parquet", "--mft-file", "raw.bin"],
            &["cannot be used with", "--mft-file <MFT_FILE>"],
        );
    }

    #[test]
    fn test_search_rejects_conflicting_index_and_drive() {
        assert_failure(
            "search_index_drive_conflict",
            &["*.rs", "--index", "saved.parquet", "--drive", "C"],
            &["cannot be used with", "--drive <DRIVE>"],
        );
    }

    #[test]
    fn test_search_rejects_conflicting_mft_file_and_drives() {
        assert_failure(
            "search_mft_drives_conflict",
            &["*.rs", "--mft-file", "raw.bin", "--drives", "C,D"],
            &["cannot be used with", "--drives <DRIVES>"],
        );
    }

    #[test]
    fn test_search_rejects_non_numeric_min_size() {
        assert_failure(
            "search_invalid_min_size",
            &["*.rs", "--min-size", "abc"],
            &["invalid value 'abc'", "--min-size <MIN_SIZE>"],
        );
    }

    #[test]
    fn test_search_rejects_non_numeric_max_size() {
        assert_failure(
            "search_invalid_max_size",
            &["*.rs", "--max-size", "abc"],
            &["invalid value 'abc'", "--max-size <MAX_SIZE>"],
        );
    }

    #[test]
    fn test_search_rejects_non_numeric_limit() {
        assert_failure(
            "search_invalid_limit",
            &["*.rs", "--limit", "abc"],
            &["invalid value 'abc'", "--limit <LIMIT>"],
        );
    }

    #[test]
    fn test_search_rejects_non_numeric_tz_offset() {
        assert_failure(
            "search_invalid_tz_offset",
            &["*.rs", "--tz-offset", "abc"],
            &["invalid value 'abc'", "--tz-offset <TZ_OFFSET>"],
        );
    }

    #[test]
    fn test_index_rejects_invalid_single_drive() {
        assert_failure(
            "index_invalid_drive",
            &["index", "out.parquet", "--drive", "1"],
            &["invalid value '1'", "must be A-Z"],
        );
    }

    #[test]
    fn test_index_rejects_conflicting_drive_flags() {
        assert_failure(
            "index_drive_conflict",
            &["index", "out.parquet", "--drive", "C", "--drives", "D"],
            &["cannot be used with", "--drives <DRIVES>"],
        );
    }

    #[test]
    fn test_index_rejects_invalid_multi_drive_list() {
        assert_failure(
            "index_invalid_drives",
            &["index", "out.parquet", "--drives", "c:,1"],
            &["invalid value '1'", "must be A-Z"],
        );
    }

    #[test]
    fn test_info_rejects_unexpected_extra_argument() {
        assert_failure(
            "info_unexpected_extra_arg",
            &["info", "saved.parquet", "extra"],
            &["unexpected argument 'extra'", "Usage:"],
        );
    }

    #[test]
    fn test_stats_rejects_unexpected_extra_argument() {
        assert_failure(
            "stats_unexpected_extra_arg",
            &["stats", "saved.parquet", "extra"],
            &["unexpected argument 'extra'", "Usage:"],
        );
    }

    #[test]
    fn test_index_rejects_unexpected_extra_argument() {
        assert_failure(
            "index_unexpected_extra_arg",
            &["index", "out.parquet", "extra"],
            &["unexpected argument 'extra'", "Usage:"],
        );
    }
}
