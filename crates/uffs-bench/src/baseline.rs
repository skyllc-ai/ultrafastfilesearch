// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! "vs baseline" comparison for the report draft.
//!
//! The **baseline** is the last canonical benchmark report published under
//! `docs/benchmarks/` — its head-to-head numbers are mirrored in the
//! machine-readable [`BASELINE_PATH`] (`docs/benchmarks/baseline.json`),
//! refreshed as part of promoting a new canonical report (see the runbook).
//!
//! At Stage 4 assembly this module joins the new run's
//! `cross-tool-summary.csv` (HOT phase, file sink) against the baseline cells
//! on `(drive, pattern)` and renders a delta table, so every run answers
//! "did we get faster or slower than the last published numbers?" without
//! anyone diffing reports by hand.

use serde::Deserialize;

/// Repo-relative path of the machine-readable baseline.
pub const BASELINE_PATH: &str = "docs/benchmarks/baseline.json";

/// One `(drive, pattern)` cell of the canonical report.
#[derive(Debug, Clone, Deserialize)]
pub struct BaselineCell {
    /// Drive scope (`"C"`, `"D"`, `"C+D"`).
    pub drive: String,
    /// Pattern label (`"exact"`, `"ext_dll"`, …).
    pub pattern: String,
    /// UFFS p50 in milliseconds.
    pub uffs_p50_ms: u64,
    /// Everything p50 in milliseconds; `None` for UFFS-only cells
    /// (e.g. `full_scan`, which es.exe cannot export).
    pub es_p50_ms: Option<u64>,
}

/// The parsed baseline file: provenance + cells.
#[derive(Debug, Clone, Deserialize)]
pub struct Baseline {
    /// Canonical report filename the numbers were extracted from.
    pub report: String,
    /// Date the canonical run was measured (`YYYY-MM-DD`).
    pub measured: String,
    /// UFFS version of the canonical run.
    pub uffs_version: String,
    /// Everything version of the canonical run.
    pub es_version: String,
    /// Per-`(drive, pattern)` reference numbers.
    pub cells: Vec<BaselineCell>,
}

/// Parse `baseline.json`; `None` on any shape mismatch.
#[must_use]
pub fn parse(json: &str) -> Option<Baseline> {
    serde_json::from_str(json).ok()
}

/// Normalize a drive scope for matching: uppercase, drop `:`, `,` → `+`
/// (the harness emits `"C,D"` for combined runs; the baseline uses `"C+D"`).
fn norm_drive(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| *ch != ':')
        .map(|ch| {
            if ch == ',' {
                '+'
            } else {
                ch.to_ascii_uppercase()
            }
        })
        .collect()
}

/// One measured p50 from the new run's cross-tool summary.
pub(crate) struct RunCell {
    /// Tool label (`"UFFS"` / `"Everything"`).
    pub(crate) tool: String,
    /// Normalized drive scope.
    pub(crate) drive: String,
    /// Pattern label.
    pub(crate) pattern: String,
    /// Measured p50 in milliseconds.
    pub(crate) p50_ms: u64,
    /// Result rows reported for the cell.
    pub(crate) rows: u64,
}

/// Extract HOT/file-sink p50s from `cross-tool-summary.csv`.
///
/// Columns: `tool,phase,sink,drive,pattern,p50_ms,…` — quote-free by
/// construction (the harness writes plain labels).
pub(crate) fn parse_run_csv(csv: &str) -> Vec<RunCell> {
    let mut cells = Vec::new();
    for line in csv.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        // Combined drive scopes ("C,D,F,G") split across fields: drive is
        // everything between sink (index 2) and the trailing 8 fixed columns.
        if fields.len() < 12 {
            continue;
        }
        let (Some(tool), Some(phase), Some(sink)) = (fields.first(), fields.get(1), fields.get(2))
        else {
            continue;
        };
        if !phase.eq_ignore_ascii_case("hot") || !sink.eq_ignore_ascii_case("file") {
            continue;
        }
        let tail_start = fields.len() - 8;
        let Some(drive_parts) = fields.get(3..tail_start) else {
            continue;
        };
        let (Some(pattern), Some(p50_raw)) = (fields.get(tail_start), fields.get(tail_start + 1))
        else {
            continue;
        };
        // Only PASS cells carry meaningful timings — a DNF row records the
        // timeout cutoff (e.g. 131 s), which must not enter charts or
        // baseline deltas as if it were a measurement.
        if !fields
            .get(tail_start + 5)
            .is_some_and(|verdict| verdict.trim().eq_ignore_ascii_case("pass"))
        {
            continue;
        }
        let Ok(p50_ms) = p50_raw.trim().parse::<u64>() else {
            continue;
        };
        let rows = fields
            .get(tail_start + 3)
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(0);
        cells.push(RunCell {
            tool: (*tool).to_owned(),
            drive: norm_drive(&drive_parts.join(",")),
            pattern: (*pattern).to_owned(),
            p50_ms,
            rows,
        });
    }
    cells
}

/// Find the run p50 for `(tool, drive, pattern)`.
pub(crate) fn run_p50(cells: &[RunCell], tool: &str, drive: &str, pattern: &str) -> Option<u64> {
    cells
        .iter()
        .find(|cell| cell.tool == tool && cell.drive == drive && cell.pattern == pattern)
        .map(|cell| cell.p50_ms)
}

/// Format a `base → now` transition with a signed integer-percent delta.
/// Negative = faster than baseline.
fn delta_cell(base: u64, now: u64) -> String {
    if base == 0 {
        return format!("{base} ms → {now} ms");
    }
    let base_i = i64::try_from(base).unwrap_or(i64::MAX);
    let now_i = i64::try_from(now).unwrap_or(i64::MAX);
    let pct = (now_i.saturating_sub(base_i)).saturating_mul(100) / base_i;
    let sign = if pct >= 0 { "+" } else { "" };
    format!("{base} ms → {now} ms ({sign}{pct}%)")
}

/// Render the `## vs baseline` comparison table, joining the baseline cells
/// against the run's measured p50s. `None` when not a single cell matches
/// (e.g. a drive set disjoint from the canonical run).
#[must_use]
pub fn render_md(baseline: &Baseline, cross_tool_csv: &str) -> Option<String> {
    let run = parse_run_csv(cross_tool_csv);
    let mut rows = Vec::new();
    for cell in &baseline.cells {
        let drive = norm_drive(&cell.drive);
        let uffs_now = run_p50(&run, "UFFS", &drive, &cell.pattern);
        let es_now = run_p50(&run, "Everything", &drive, &cell.pattern);
        if uffs_now.is_none() && es_now.is_none() {
            continue;
        }
        let uffs_cell =
            uffs_now.map_or_else(|| "—".to_owned(), |now| delta_cell(cell.uffs_p50_ms, now));
        let es_cell = match (cell.es_p50_ms, es_now) {
            (Some(base), Some(now)) => delta_cell(base, now),
            _ => "—".to_owned(),
        };
        rows.push(format!(
            "| {}: | {} | {} | {} |",
            cell.drive, cell.pattern, uffs_cell, es_cell
        ));
    }
    if rows.is_empty() {
        return None;
    }
    Some(format!(
        "## vs baseline (last canonical report)\n\n\
         _Baseline: `{report}` (measured {measured}, UFFS v{uffs} vs Everything \
         {es}). HOT phase, file sink, matched on (drive, pattern); negative Δ = \
         faster than the published numbers. Cells absent from either run are \
         omitted._\n\n\
         | Drive | Pattern | UFFS p50 (base → now) | ES p50 (base → now) |\n\
         |-------|---------|----------------------:|--------------------:|\n\
         {body}",
        report = baseline.report,
        measured = baseline.measured,
        uffs = baseline.uffs_version,
        es = baseline.es_version,
        body = rows.join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE_JSON: &str = r#"{
      "report": "2026-06-v0.5.120-vs-everything.md",
      "measured": "2026-06-09",
      "uffs_version": "0.5.120",
      "es_version": "1.4.1.1032",
      "phase": "hot",
      "sink": "file",
      "cells": [
        { "drive": "C", "pattern": "exact", "uffs_p50_ms": 20, "es_p50_ms": 46, "rows": 30 },
        { "drive": "C+D", "pattern": "ext_dll", "uffs_p50_ms": 101, "es_p50_ms": 258, "rows": 206859 },
        { "drive": "C", "pattern": "full_scan", "uffs_p50_ms": 1500, "es_p50_ms": null, "rows": 3216011 }
      ]
    }"#;

    const RUN_CSV: &str = "\
tool,phase,sink,drive,pattern,p50_ms,p95_ms,rows,bad,verdict,rounds_ok,rounds_total
UFFS,HOT,file,C,exact,22,40,30,0,PASS,10,10
Everything,HOT,file,C,exact,44,50,30,0,PASS,10,10
UFFS,HOT,file,C,D,ext_dll,90,101,206859,0,PASS,10,10
Everything,HOT,file,C,D,ext_dll,260,271,206859,0,PASS,10,10
UFFS,HOT,file,C,full_scan,1400,1500,3216011,0,PASS,10,10
UFFS,COLD,file,C,exact,900,1000,30,0,PASS,3,3
UFFS,HOT,stdout,C,exact,21,39,30,0,PASS,10,10
";

    #[test]
    fn joins_baseline_against_run_with_deltas() {
        let baseline = parse(BASELINE_JSON).expect("baseline parses");
        let md = render_md(&baseline, RUN_CSV).expect("matched cells");
        assert!(md.starts_with("## vs baseline (last canonical report)"));
        assert!(md.contains("`2026-06-v0.5.120-vs-everything.md`"));
        // C exact: UFFS 20 → 22 (+10%), ES 46 → 44 (-4%).
        assert!(md.contains("| C: | exact | 20 ms → 22 ms (+10%) | 46 ms → 44 ms (-4%) |"));
        // Combined drive "C,D" in the CSV matches baseline "C+D".
        assert!(md.contains("| C+D: | ext_dll | 101 ms → 90 ms (-10%) | 258 ms → 260 ms (+0%) |"));
        // full_scan is UFFS-only: ES column renders an em-dash.
        assert!(md.contains("| C: | full_scan | 1500 ms → 1400 ms (-6%) | — |"));
        // COLD / stdout rows must NOT leak into the HOT/file comparison.
        assert!(!md.contains("900"));
    }

    #[test]
    fn no_matches_renders_none() {
        let baseline = parse(BASELINE_JSON).expect("baseline parses");
        let csv = "tool,phase,sink,drive,pattern,p50_ms,p95_ms,rows,bad,verdict,rounds_ok,rounds_total\n\
                   UFFS,HOT,file,Z,exact,10,11,1,0,PASS,10,10\n";
        assert!(render_md(&baseline, csv).is_none());
    }

    #[test]
    fn bad_json_parses_to_none() {
        assert!(parse("not json").is_none());
    }
}
