//! JSON formatting helpers (test-only on non-Windows platforms).
//!
//! On Windows, these live in `streaming.rs` and are used for production code.
//! On non-Windows, they're only needed for test parity validation.

/// Escape a string for JSON output.
pub fn format_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control <= '\u{1F}' => push_json_unicode_escape(&mut escaped, control),
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn push_json_unicode_escape(buf: &mut String, ch: char) {
    const HEX: &[char; 16] = &[
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    let code = ch as u32;
    buf.push_str("\\u");
    for shift in [12_u32, 8_u32, 4_u32, 0_u32] {
        let nibble = usize::try_from((code >> shift) & 0xF).unwrap_or_default();
        buf.push(*HEX.get(nibble).unwrap_or(&'0'));
    }
}

/// Format a cell value for JSON output.
pub fn format_json_value(col: &uffs_polars::Column, row_idx: usize) -> String {
    use uffs_polars::{AnyValue, TimeUnit};

    match col.get(row_idx) {
        Ok(AnyValue::Null) | Err(_) => "null".to_owned(),
        Ok(AnyValue::String(value)) => format_json_string(value),
        Ok(AnyValue::Boolean(boolean)) => if boolean { "true" } else { "false" }.to_owned(),
        Ok(AnyValue::Datetime(ts, TimeUnit::Microseconds, _)) => {
            let secs = ts.div_euclid(1_000_000);
            let micros = u32::try_from(ts.rem_euclid(1_000_000)).unwrap_or_default();
            chrono::DateTime::from_timestamp(secs, micros * 1000).map_or_else(
                || "null".to_owned(),
                |datetime| format_json_string(&datetime.format("%Y-%m-%d %H:%M:%S").to_string()),
            )
        }
        Ok(AnyValue::UInt8(n)) => n.to_string(),
        Ok(AnyValue::UInt16(n)) => n.to_string(),
        Ok(AnyValue::UInt32(n)) => n.to_string(),
        Ok(AnyValue::UInt64(n)) => n.to_string(),
        Ok(AnyValue::Int8(n)) => n.to_string(),
        Ok(AnyValue::Int16(n)) => n.to_string(),
        Ok(AnyValue::Int32(n)) => n.to_string(),
        Ok(AnyValue::Int64(n)) => n.to_string(),
        Ok(AnyValue::Float32(n)) => n.to_string(),
        Ok(AnyValue::Float64(n)) => n.to_string(),
        Ok(value) => format_json_string(&value.to_string()),
    }
}

