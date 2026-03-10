//! Formatting and display helpers for the `uffs_mft` binary.

use std::path::{Path, PathBuf};

/// Formats a duration intelligently based on magnitude.
///
/// Output format varies by duration:
/// - Days+: `2d 3h 5m 10s`
/// - Hours+: `3h 5m 10s`
/// - Minutes+: `5 m 10 s`
/// - Seconds+: `10 s 500 ms`
/// - Milliseconds+: `500 ms 250 μs`
/// - Microseconds+: `250 μs 100 ns`
/// - Nanoseconds only: `100 ns`
pub fn format_duration(duration: core::time::Duration) -> String {
    let total_seconds = duration.as_secs();
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3600) % 24;
    let days = total_seconds / 86400;

    let milliseconds = duration.subsec_millis();
    let microseconds = duration.subsec_micros() % 1_000;
    let nanoseconds = duration.subsec_nanos() % 1_000;

    if days > 0 {
        format!("{days:>2}d {hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if hours > 0 {
        format!("{hours:>2}h {minutes:>2}m {seconds:>2}s")
    } else if minutes > 0 {
        format!("{minutes:>3} m  {seconds:>3} s ")
    } else if seconds > 0 {
        format!("{seconds:>3} s  {milliseconds:>3} ms")
    } else if milliseconds > 0 {
        format!("{milliseconds:>3} ms {microseconds:>3} μs")
    } else if microseconds > 0 {
        format!("{microseconds:>3} μs {nanoseconds:>3} ns")
    } else {
        format!("{nanoseconds:>3} ns")
    }
}

/// Formats a byte count intelligently based on magnitude.
///
/// Output format varies by size:
/// - < 1 KB: `1234 B`
/// - < 1 MB: `123.45 KB`
/// - < 1 GB: `123.45 MB`
/// - < 1 TB: `123.45 GB`
/// - >= 1 TB: `123.45 TB`
#[expect(
    clippy::cast_precision_loss,
    reason = "precision loss acceptable for display"
)]
#[expect(
    clippy::float_arithmetic,
    reason = "floating-point arithmetic required for human-readable byte formatting"
)]
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes:>4} B")
    } else if bytes < 1024 * 1024 {
        format!("{:>7.2} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:>7.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes < 1024 * 1024 * 1024 * 1024 {
        format!("{:>7.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!(
            "{:>7.2} TB",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
        )
    }
}

/// Formats a number with comma separators for readability.
///
/// Examples: 1234567 → "1,234,567", 1000 → "1,000"
pub fn format_number_commas(num: u64) -> String {
    let num_str = num.to_string();
    let mut result = String::with_capacity(num_str.len() + num_str.len() / 3);
    for (idx, char) in num_str.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            result.push(',');
        }
        result.push(char);
    }
    result.chars().rev().collect()
}

/// Cleans up a path for user-friendly display.
///
/// On Windows, `std::fs::canonicalize` returns extended-length paths with
/// the `\\?\` prefix. This function strips that prefix for cleaner output.
pub fn clean_path_for_display(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    path_str
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// Truncates a string to a maximum length, adding "..." if truncated.
#[cfg(windows)]
pub fn truncate_string(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_owned()
    } else if max_len <= 3 {
        text.chars().take(max_len).collect()
    } else {
        // Use char boundary-safe truncation
        let truncate_at = max_len - 3;
        let safe_end = text
            .char_indices()
            .take_while(|(idx, _)| *idx < truncate_at)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        format!("{}...", &text[..safe_end])
    }
}

// ============================================================================
// Benchmark Command
// ============================================================================

#[cfg(windows)]

/// Converts a byte to a printable ASCII character or '.' for non-printable.
#[cfg(windows)]
pub fn char_or_dot(byte: u8) -> char {
    if byte.is_ascii_graphic() || byte == b' ' {
        byte as char
    } else {
        '.'
    }
}

// ============================================================================
// Full Index Build Benchmark Command (matches the legacy baseline --benchmark-index exactly)
// ============================================================================

/// Full index build benchmark matching C++ `--benchmark-index` output exactly.
///
/// This measures the complete UFFS indexing pipeline: async I/O + parsing +
/// `DataFrame` building. This is what users experience when indexing.
#[cfg(windows)]

/// Format USN reason flags as a short string.
#[cfg(windows)]
pub fn format_usn_reason(reason: u32) -> String {
    use uffs_mft::usn::reason;

    let mut parts = Vec::new();
    if reason & reason::FILE_CREATE != 0 {
        parts.push("CREATE");
    }
    if reason & reason::FILE_DELETE != 0 {
        parts.push("DELETE");
    }
    if reason & reason::RENAME_NEW_NAME != 0 {
        parts.push("RENAME");
    }
    if reason & reason::DATA_EXTEND != 0 || reason & reason::DATA_TRUNCATION != 0 {
        parts.push("SIZE");
    }
    if reason & reason::BASIC_INFO_CHANGE != 0 {
        parts.push("META");
    }
    if reason & reason::CLOSE != 0 {
        parts.push("CLOSE");
    }

    if parts.is_empty() {
        format!("0x{:08X}", reason)
    } else {
        parts.join("+")
    }
}

/// Format a number with thousands separators.
#[cfg(windows)]
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
