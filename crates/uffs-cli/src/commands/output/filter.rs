//! Record filter matching, attribute/age parsing, sort specification parsing,
//! and multi-tier record comparison.

use super::types::{AttrKind, AttrRequirement, SortColumn, SortKind, StreamingRecordFilter};

impl StreamingRecordFilter {
    /// Check if a record passes ALL filters (AND logic).
    #[inline]
    #[must_use]
    pub fn matches(&self, record: &uffs_mft::index::FileRecord) -> bool {
        // Type filter.
        let is_dir = record.is_directory();
        if self.files_only && is_dir {
            return false;
        }
        if self.dirs_only && !is_dir {
            return false;
        }

        // Legacy hide-system (combines hidden + system).
        if self.hide_system && (record.stdinfo.is_system() || record.stdinfo.is_hidden()) {
            return false;
        }

        // Size filter.
        let size = record.first_stream.size.length;
        if let Some(min) = self.min_size {
            if size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if size > max {
                return false;
            }
        }

        // Attribute requirements (AND — all must pass).
        for req in &self.attr_filters {
            match req {
                AttrRequirement::Include(kind) => {
                    if !kind.is_set(record) {
                        return false;
                    }
                }
                AttrRequirement::Exclude(kind) => {
                    if kind.is_set(record) {
                        return false;
                    }
                }
            }
        }

        // Date range filters (all three NTFS timestamps).
        if let Some(ts) = self.newer_modified {
            if record.stdinfo.modified < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_modified {
            if record.stdinfo.modified > ts {
                return false;
            }
        }
        if let Some(ts) = self.newer_created {
            if record.stdinfo.created < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_created {
            if record.stdinfo.created > ts {
                return false;
            }
        }
        if let Some(ts) = self.newer_accessed {
            if record.stdinfo.accessed < ts {
                return false;
            }
        }
        if let Some(ts) = self.older_accessed {
            if record.stdinfo.accessed > ts {
                return false;
            }
        }

        true
    }
}

/// Parse a comma-separated `--attr` string into attribute requirements.
///
/// # Examples
/// - `"hidden"` → `[Include(Hidden)]`
/// - `"!hidden"` → `[Exclude(Hidden)]`
/// - `"hidden,compressed"` → `[Include(Hidden), Include(Compressed)]`
/// - `"!system,!hidden"` → `[Exclude(System), Exclude(Hidden)]`
pub(in crate::commands) fn parse_attr_filter(input: &str) -> Vec<AttrRequirement> {
    input
        .split(',')
        .filter_map(|raw_token| {
            let trimmed = raw_token.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.strip_prefix('!').map_or_else(
                || AttrKind::parse(trimmed).map(AttrRequirement::Include),
                |name| AttrKind::parse(name).map(AttrRequirement::Exclude),
            )
        })
        .collect()
}

/// Parse a `--newer` / `--older` duration or date string into a timestamp.
///
/// Supports:
/// - `7d` → 7 days ago
/// - `24h` → 24 hours ago
/// - `30m` → 30 minutes ago
/// - `2026-01-15` → specific date (midnight UTC)
/// - `2026-01-15T10:30:00` → specific datetime
pub(in crate::commands) fn parse_age_filter(raw_input: &str) -> Option<i64> {
    /// Helper: compute microseconds-since-epoch for "now minus N seconds".
    fn now_minus_secs(secs: i64) -> Option<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        let now_us = i64::try_from(now.as_micros()).ok()?;
        Some(now_us - secs * 1_000_000)
    }

    let input = raw_input.trim();

    // Duration format: Nd, Nh, Nm
    if let Some(days) = input
        .strip_suffix('d')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(days * 86_400);
    }
    if let Some(hours) = input
        .strip_suffix('h')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(hours * 3_600);
    }
    if let Some(mins) = input
        .strip_suffix('m')
        .and_then(|val| val.parse::<i64>().ok())
    {
        return now_minus_secs(mins * 60);
    }

    // ISO date/datetime format via chrono
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp_micros());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        return Some(dt.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros());
    }

    None
}

/// Parse a comma-separated `--sort` string into sort tiers.
///
/// Each tier is `column` or `column:asc` or `column:desc`.
/// Default direction: ascending.  `--sort-desc` reverses ALL tiers.
///
/// # Examples
/// - `"size"` → `[Size(asc)]`
/// - `"size:desc,name"` → `[Size(desc), Name(asc)]`
/// - `"modified:desc,size:asc,name"` → `[Modified(desc), Size(asc), Name(asc)]`
pub(in crate::commands) fn parse_sort_spec(input: &str) -> Vec<SortColumn> {
    input
        .split(',')
        .filter_map(|raw_token| {
            let trimmed = raw_token.trim();
            let (name, dir) = if let Some((col_name, dir_str)) = trimmed.split_once(':') {
                (col_name.trim(), Some(dir_str.trim()))
            } else {
                (trimmed, None) // no explicit direction → use smart default
            };
            let kind = SortKind::parse(name)?;
            let descending = dir.map_or_else(
                || kind.default_descending(),
                |dir_str| {
                    matches!(
                        dir_str.to_ascii_lowercase().as_str(),
                        "desc" | "d" | "descending"
                    )
                },
            );
            Some(SortColumn { kind, descending })
        })
        .collect()
}

/// Compare two records by multi-tier sort specification.
///
/// Extracts sort keys on-demand from the index — no pre-materialized keys.
/// For name/path sorts, uses the names buffer directly (zero allocation).
pub(in crate::commands) fn compare_records(
    a_idx: usize,
    b_idx: usize,
    index: &uffs_mft::MftIndex,
    sort_spec: &[SortColumn],
    global_desc: bool,
) -> core::cmp::Ordering {
    use core::cmp::Ordering;

    let Some(rec_a) = index.records.get(a_idx) else {
        return Ordering::Equal;
    };
    let Some(rec_b) = index.records.get(b_idx) else {
        return Ordering::Equal;
    };

    for col in sort_spec {
        let ord = match col.kind {
            SortKind::Size => rec_a
                .first_stream
                .size
                .length
                .cmp(&rec_b.first_stream.size.length),
            SortKind::SizeOnDisk => rec_a
                .first_stream
                .size
                .allocated
                .cmp(&rec_b.first_stream.size.allocated),
            SortKind::Modified => rec_a.stdinfo.modified.cmp(&rec_b.stdinfo.modified),
            SortKind::Created => rec_a.stdinfo.created.cmp(&rec_b.stdinfo.created),
            SortKind::Accessed => rec_a.stdinfo.accessed.cmp(&rec_b.stdinfo.accessed),
            SortKind::Name => {
                let na = index.record_name(rec_a);
                let nb = index.record_name(rec_b);
                na.to_ascii_lowercase().cmp(&nb.to_ascii_lowercase())
            }
            SortKind::Path => Ordering::Equal,
            SortKind::Extension => rec_a
                .first_name
                .name
                .extension_id()
                .cmp(&rec_b.first_name.name.extension_id()),
            SortKind::Descendants => rec_a.descendants.cmp(&rec_b.descendants),
            SortKind::Hidden => rec_a.stdinfo.is_hidden().cmp(&rec_b.stdinfo.is_hidden()),
            SortKind::System => rec_a.stdinfo.is_system().cmp(&rec_b.stdinfo.is_system()),
            SortKind::Archive => rec_a.stdinfo.is_archive().cmp(&rec_b.stdinfo.is_archive()),
            SortKind::ReadOnly => rec_a
                .stdinfo
                .is_readonly()
                .cmp(&rec_b.stdinfo.is_readonly()),
            SortKind::Compressed => rec_a
                .stdinfo
                .is_compressed()
                .cmp(&rec_b.stdinfo.is_compressed()),
            SortKind::Encrypted => rec_a
                .stdinfo
                .is_encrypted()
                .cmp(&rec_b.stdinfo.is_encrypted()),
            SortKind::Directory => rec_a.is_directory().cmp(&rec_b.is_directory()),
        };

        if ord != Ordering::Equal {
            // Per-tier direction: col.descending XOR global_desc.
            let effective_desc = col.descending ^ global_desc;
            return if effective_desc { ord.reverse() } else { ord };
        }
    }

    Ordering::Equal
}
