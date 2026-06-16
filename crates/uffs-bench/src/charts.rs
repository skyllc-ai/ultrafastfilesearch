// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Shareable SVG chart generation from the run's measured data.
//!
//! Replaces the hand-written-SVG workflow documented in
//! `docs/benchmarks/charts/README.md` ("copy the previous directory,
//! find-and-replace the data values… ~20 minutes per set") with generation
//! straight from `cross-tool-summary.csv` at Stage 4 assembly.
//!
//! The output follows the UFFS brand kit (`docs/dev/architecture/brand-kit/
//! STYLE_GUIDE.md`): Charcoal `#0F0D0B` card, Cream `#F2EDE8` titles, Sand
//! `#9A8D82` secondary text and competitor bars, **Rust Orange `#CE422B`**
//! UFFS bars (the brand anchor — deliberately not the cool blue of every
//! other file-search tool), Ember `#F7B26B` win callouts, Inter typography.
//! Drop-in for the canonical report, the hub README, and social screenshots.
//! All geometry is computed in integer tenths of a pixel (the crate avoids
//! float arithmetic).

use std::path::Path;

use crate::baseline::{parse_run_csv, run_p50};
use crate::error::{BenchError, Result};
use crate::host::Host;

/// Bundle-relative path of the generated head-to-head chart (vs Everything).
pub const HEAD_TO_HEAD_SVG: &str = "charts/head-to-head-vs-everything.svg";
/// Bundle-relative path of the daemon-HOT vs C++ per-invocation chart.
pub const DAEMON_HOT_SVG: &str = "charts/daemon-hot-vs-cpp.svg";
/// Bundle-relative path of the full-scan throughput chart.
pub const FULL_SCAN_SVG: &str = "charts/full-scan-throughput.svg";

/// One paired head-to-head measurement (HOT phase, file sink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadToHeadCell {
    /// Drive scope (`"C"`, `"D"`, `"C+D+F+G"`).
    pub drive: String,
    /// Pattern label.
    pub pattern: String,
    /// UFFS p50 in milliseconds.
    pub uffs_p50_ms: u64,
    /// Rival tool's p50 in milliseconds.
    pub rival_p50_ms: u64,
}

/// Pair UFFS and `rival_tool` HOT/file p50s per `(drive, pattern)`.
///
/// `rival_tool` is the harness CSV label — `"Everything"` or `"UFFS-C++"`.
/// Reads the cross-tool summary CSV, preserving the CSV's UFFS row order.
/// Cells the rival did not run (e.g. `full_scan` for es.exe, `prefix` for
/// uffs.com) are skipped, and `(drive, pattern)` pairs are deduplicated
/// (first occurrence wins) — merged inputs overlap on shared drives.
#[must_use]
pub fn rival_cells(cross_tool_csv: &str, rival_tool: &str) -> Vec<HeadToHeadCell> {
    let run = parse_run_csv(cross_tool_csv);
    let mut cells: Vec<HeadToHeadCell> = Vec::new();
    for uffs in run.iter().filter(|cell| cell.tool == "UFFS") {
        let Some(rival_p50) = run_p50(&run, rival_tool, &uffs.drive, &uffs.pattern) else {
            continue;
        };
        if cells
            .iter()
            .any(|existing| existing.drive == uffs.drive && existing.pattern == uffs.pattern)
        {
            continue;
        }
        cells.push(HeadToHeadCell {
            drive: uffs.drive.clone(),
            pattern: uffs.pattern.clone(),
            uffs_p50_ms: uffs.p50_ms,
            rival_p50_ms: rival_p50,
        });
    }
    cells
}

/// One UFFS full-scan (`*` → CSV export) measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullScanCell {
    /// Drive scope.
    pub drive: String,
    /// Rows exported.
    pub rows: u64,
    /// p50 wall time in milliseconds.
    pub p50_ms: u64,
}

/// Extract UFFS HOT/file `full_scan` cells (the workload Everything cannot
/// export) from the cross-tool summary CSV.
///
/// Deduplicated by drive scope (first occurrence wins) — merged inputs (a
/// cross-tool capture plus a dedicated all-drives full-scan capture) overlap
/// on the shared drives.
#[must_use]
pub fn full_scan_cells(cross_tool_csv: &str) -> Vec<FullScanCell> {
    let mut cells: Vec<FullScanCell> = Vec::new();
    for cell in parse_run_csv(cross_tool_csv) {
        if cell.tool != "UFFS" || cell.pattern != "full_scan" || cell.p50_ms == 0 {
            continue;
        }
        if cells.iter().any(|existing| existing.drive == cell.drive) {
            continue;
        }
        cells.push(FullScanCell {
            drive: cell.drive,
            rows: cell.rows,
            p50_ms: cell.p50_ms,
        });
    }
    cells
}

/// Plot-area width in pixels (x = 160 → 840, per the design system).
const PLOT_WIDTH_PX: u64 = 680;
/// Plot-area left edge.
const PLOT_X: u64 = 160;
/// Vertical pitch per cell row.
const ROW_PITCH: u64 = 40;
/// First bar's top y.
const BARS_TOP: u64 = 120;

/// Bar length in tenths of a pixel for `ms` against `axis_max`.
fn bar_tenths(ms: u64, axis_max: u64) -> u64 {
    (ms * PLOT_WIDTH_PX * 10).checked_div(axis_max).unwrap_or(0)
}

/// Render `tenths` as a fixed-point `"px.t"` SVG length.
fn px(tenths: u64) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Bar value label: milliseconds on small charts, one-decimal seconds on
/// seconds-scale charts (`"32.1 s"` instead of `"32120 ms"`).
fn ms_value_label(ms: u64, seconds_scale: bool) -> String {
    if seconds_scale {
        format!("{}.{} s", ms / 1_000, (ms % 1_000) / 100)
    } else {
        format!("{ms} ms")
    }
}

/// Render the head-to-head grouped-bar SVG, or `None` when `cells` is empty.
///
/// `uffs_label` / `rival_label` feed the legend and title (e.g.
/// `"UFFS v0.5.120"`, `"Everything 1.4.1.1032"`); `subtitle` is the context
/// line under the title.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "single linear SVG template: header, axis, per-row groups, footer — splitting would scatter the fixed geometry constants"
)]
pub fn head_to_head_svg(
    cells: &[HeadToHeadCell],
    uffs_label: &str,
    rival_label: &str,
    subtitle: &str,
) -> Option<String> {
    if cells.is_empty() {
        return None;
    }
    let rows = u64::try_from(cells.len()).unwrap_or(0);
    let wins = cells
        .iter()
        .filter(|cell| cell.uffs_p50_ms < cell.rival_p50_ms)
        .count();

    // Axis ceiling: round the max p50 up to a clean tick step. Sub-5 s charts
    // tick in milliseconds (multiples of 50); seconds-scale charts (e.g. the
    // C++ full-scan comparison) tick in whole seconds so labels stay readable.
    let max_ms = cells
        .iter()
        .map(|cell| cell.uffs_p50_ms.max(cell.rival_p50_ms))
        .max()
        .unwrap_or(0);
    let seconds_scale = max_ms >= 5_000;
    let axis_max = if seconds_scale {
        ((max_ms / 5_000) + 1) * 5_000
    } else {
        ((max_ms / 50) + 1) * 50
    };

    let axis_bottom = BARS_TOP + rows * ROW_PITCH + 10;
    let height = axis_bottom + 70;
    let title = format!(
        "{uffs_label} vs {rival_label} — {wins} / {} cells faster at p50",
        cells.len()
    );

    let mut parts = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 960 {height}\" \
         font-family=\"Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, \
         sans-serif\" role=\"img\" aria-label=\"Horizontal bar chart: {title}\">\n\
         <title>{title}</title>\n\
         <desc>Horizontal grouped bar chart comparing UFFS and {rival_label} p50 latency per \
         pattern-by-drive cell. UFFS bars are brand Rust Orange, rival bars are muted Sand. Generated \
         by the uffs-bench suite from cross-tool-summary.csv.</desc>\n\
         <rect x=\"0\" y=\"0\" width=\"960\" height=\"{height}\" fill=\"#0F0D0B\"/>\n\
         <rect x=\"0.5\" y=\"0.5\" width=\"959\" height=\"{}\" fill=\"none\" stroke=\"#1E1B18\" \
         stroke-width=\"1\"/>\n\
         <text x=\"480\" y=\"36\" text-anchor=\"middle\" font-size=\"19\" font-weight=\"700\" \
         fill=\"#F2EDE8\">{title}</text>\n\
         <text x=\"480\" y=\"58\" text-anchor=\"middle\" font-size=\"13\" \
         fill=\"#9A8D82\">{subtitle}</text>\n\
         <g transform=\"translate(300, 78)\" font-size=\"12\" fill=\"#F2EDE8\">
\
         <rect x=\"0\" y=\"2\" width=\"14\" height=\"12\" fill=\"#CE422B\"/>\n\
         <text x=\"20\" y=\"12\">{uffs_label}</text>\n\
         <rect x=\"180\" y=\"2\" width=\"14\" height=\"12\" fill=\"#9A8D82\"/>\n\
         <text x=\"200\" y=\"12\">{rival_label}</text>\n\
         </g>",
        height - 1,
    )];

    // Axis frame + gridlines: five equal segments.
    let mut axis = vec![format!(
        "<g font-size=\"11\" fill=\"#9A8D82\">\n\
         <line x1=\"160\" y1=\"110\" x2=\"160\" y2=\"{axis_bottom}\" stroke=\"#9A8D82\" \
         stroke-width=\"1\"/>\n\
         <line x1=\"160\" y1=\"{axis_bottom}\" x2=\"840\" y2=\"{axis_bottom}\" \
         stroke=\"#9A8D82\" stroke-width=\"1\"/>\n\
         <text x=\"160\" y=\"{}\" text-anchor=\"middle\">0</text>",
        axis_bottom + 18,
    )];
    for tick in 1..=5_u64 {
        let x = PLOT_X + tick * PLOT_WIDTH_PX / 5;
        let tick_ms = tick * axis_max / 5;
        let label = if seconds_scale {
            format!("{}.{}", tick_ms / 1_000, (tick_ms % 1_000) / 100)
        } else {
            tick_ms.to_string()
        };
        axis.push(format!(
            "<line x1=\"{x}\" y1=\"110\" x2=\"{x}\" y2=\"{axis_bottom}\" stroke=\"#1E1B18\" \
             stroke-width=\"1\"/>\n\
             <text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{label}</text>",
            axis_bottom + 18,
        ));
    }
    let axis_unit = if seconds_scale {
        "seconds"
    } else {
        "milliseconds"
    };
    axis.push(format!(
        "<text x=\"500\" y=\"{}\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">p50 latency ({axis_unit}) — lower is better</text>\n</g>",
        axis_bottom + 40,
    ));
    parts.push(axis.join("\n"));

    // One group per cell: row label, two bars, value labels, ratio callout.
    for (idx, cell) in cells.iter().enumerate() {
        let row = u64::try_from(idx).unwrap_or(0);
        let y_uffs = BARS_TOP + row * ROW_PITCH;
        let y_es = y_uffs + 18;
        let uffs_w = bar_tenths(cell.uffs_p50_ms, axis_max);
        let es_w = bar_tenths(cell.rival_p50_ms, axis_max);
        let ratio_hundredths = (cell.uffs_p50_ms * 100)
            .checked_div(cell.rival_p50_ms)
            .unwrap_or(100);
        // A ratio that rounds to zero hundredths reads better as a bound.
        let ratio_cell = if ratio_hundredths == 0 {
            "&lt;0.01×".to_owned()
        } else {
            format!("{}.{:02}×", ratio_hundredths / 100, ratio_hundredths % 100)
        };
        let ratio_color = if cell.uffs_p50_ms < cell.rival_p50_ms {
            "#F7B26B"
        } else {
            "#9A8D82"
        };
        parts.push(format!(
            "<g font-size=\"12\" fill=\"#F2EDE8\">
\
             <text x=\"154\" y=\"{label_y}\" text-anchor=\"end\">{drive}: {pattern}</text>\n\
             <rect x=\"160\" y=\"{y_uffs}\" width=\"{uffs_px}\" height=\"16\" fill=\"#CE422B\"/>\n\
             <rect x=\"160\" y=\"{y_es}\" width=\"{es_px}\" height=\"16\" fill=\"#9A8D82\"/>\n\
             <text x=\"{uffs_label_x}\" y=\"{label_y}\" font-size=\"11\" \
             fill=\"#9A8D82\">{uffs_value}</text>\n\
             <text x=\"{es_label_x}\" y=\"{es_label_y}\" font-size=\"11\" \
             fill=\"#9A8D82\">{es_value}</text>\n\
             <text x=\"848\" y=\"{ratio_y}\" font-size=\"12\" font-weight=\"700\" \
             fill=\"{ratio_color}\">{ratio_cell}</text>\n\
             </g>",
            label_y = y_uffs + 12,
            drive = cell.drive,
            pattern = cell.pattern,
            uffs_px = px(uffs_w),
            es_px = px(es_w),
            uffs_label_x = PLOT_X + uffs_w / 10 + 6,
            uffs_value = ms_value_label(cell.uffs_p50_ms, seconds_scale),
            es_label_x = PLOT_X + es_w / 10 + 6,
            es_label_y = y_es + 12,
            es_value = ms_value_label(cell.rival_p50_ms, seconds_scale),
            ratio_y = y_uffs + 21,
        ));
    }

    parts.push(format!(
        "<text x=\"480\" y=\"{}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#9A8D82\">\
         Generated by the uffs-bench suite from cross-tool-summary.csv (HOT phase, file sink). \
         Lower is better; ratio = UFFS ÷ rival.</text>\n</svg>",
        height - 14,
    ));
    Some(parts.join("\n"))
}

/// Hero throughput in hundredths of a million records/second (`211` → "2.11"),
/// rounded to nearest.
fn throughput_m_hundredths(rows: u64, p50_ms: u64) -> u64 {
    ((rows * 1000).checked_div(p50_ms).unwrap_or(0) * 100 + 500_000) / 1_000_000
}

/// Wall-clock seconds with one decimal, rounded to nearest (`11980` →
/// `"12.0"`).
fn secs_label(ms: u64) -> String {
    let tenths = (ms + 50) / 100;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Row count as a compact magnitude (`10 207 863` → `"10.2 M"`).
fn rows_label(rows: u64) -> String {
    if rows >= 1_000_000 {
        let tenths = rows * 10 / 1_000_000;
        format!("{}.{} M", tenths / 10, tenths % 10)
    } else {
        format!("{} K", rows / 1_000)
    }
}

/// Render the full-scan export stat card (UFFS-only `*` → CSV — the workload
/// Everything cannot run), or `None` when `cells` is empty.
///
/// Layout follows the hand-made 2026-04 design the operator standardized on:
/// a Rust-Orange-bordered UFFS panel with three hero figures (wall seconds,
/// rows written, sustained throughput) and a per-drive breakdown, plus a
/// muted "FOR REFERENCE" panel explaining Everything's ~2 GB `WM_COPYDATA`
/// IPC ceiling. The hero figures come from the widest scope (most rows);
/// `subtitle` is the context line under the title.
#[must_use]
pub fn full_scan_svg(cells: &[FullScanCell], uffs_label: &str, subtitle: &str) -> Option<String> {
    let hero = cells.iter().max_by_key(|cell| cell.rows)?;
    let secs = secs_label(hero.p50_ms);
    let rows = rows_label(hero.rows);
    let tput = throughput_m_hundredths(hero.rows, hero.p50_ms);
    let title = format!("Full-scan export: {rows} records → CSV in {secs} s");

    // Per-drive breakdown (everything except the hero scope), in CSV order.
    let breakdown: Vec<String> = cells
        .iter()
        .filter(|cell| cell.drive != hero.drive)
        .map(|cell| format!("{}: {} s", cell.drive, secs_label(cell.p50_ms)))
        .collect();
    let detail_line = if breakdown.is_empty() {
        "No shell stdout buffering, no IPC round-trip per row.".to_owned()
    } else {
        format!("Per drive: {}", breakdown.join("  ·  "))
    };

    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 960 360\" \
         font-family=\"Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, \
         Arial, sans-serif\" role=\"img\" aria-label=\"Stat card: {title}; Everything's \
         single-command bulk export is architecturally bounded by the 2 GB WM_COPYDATA IPC cap\">\n\
         <title>{title}</title>\n\
         <desc>Statistic card showing three UFFS full-scan-export numbers — wall-clock p50, rows \
         written, and sustained records-per-second — with a reference panel explaining \
         Everything's ~2 GB IPC export ceiling. Generated by the uffs-bench suite from \
         cross-tool-summary.csv.</desc>\n\
         <rect x=\"0\" y=\"0\" width=\"960\" height=\"360\" fill=\"#0F0D0B\"/>\n\
         <rect x=\"0.5\" y=\"0.5\" width=\"959\" height=\"359\" fill=\"none\" stroke=\"#1E1B18\" \
         stroke-width=\"1\"/>\n\
         <text x=\"480\" y=\"36\" text-anchor=\"middle\" font-size=\"19\" font-weight=\"700\" \
         fill=\"#F2EDE8\">{title}</text>\n\
         <text x=\"480\" y=\"58\" text-anchor=\"middle\" font-size=\"13\" \
         fill=\"#9A8D82\">{subtitle}</text>\n\
         <g>\n\
         <rect x=\"48\" y=\"86\" width=\"560\" height=\"240\" rx=\"10\" fill=\"#1E1B18\" \
         stroke=\"#CE422B\" stroke-width=\"2\"/>\n\
         <text x=\"328\" y=\"114\" text-anchor=\"middle\" font-size=\"13\" font-weight=\"700\" \
         fill=\"#CE422B\" letter-spacing=\"0.5\">{uffs_label}  ·  `*` → CSV</text>\n\
         <text x=\"148\" y=\"180\" text-anchor=\"middle\" font-size=\"54\" font-weight=\"800\" \
         fill=\"#F2EDE8\">{secs}<tspan font-size=\"28\" font-weight=\"500\" \
         fill=\"#9A8D82\">s</tspan></text>\n\
         <text x=\"148\" y=\"204\" text-anchor=\"middle\" font-size=\"12\" fill=\"#9A8D82\">p50 \
         wall-clock</text>\n\
         <line x1=\"248\" y1=\"150\" x2=\"248\" y2=\"208\" stroke=\"#0F0D0B\" stroke-width=\"1\"/>\n\
         <text x=\"328\" y=\"180\" text-anchor=\"middle\" font-size=\"40\" font-weight=\"800\" \
         fill=\"#F2EDE8\">{rows_n}<tspan font-size=\"24\" font-weight=\"500\" \
         fill=\"#9A8D82\">{rows_unit}</tspan></text>\n\
         <text x=\"328\" y=\"204\" text-anchor=\"middle\" font-size=\"12\" fill=\"#9A8D82\">rows \
         written to disk</text>\n\
         <line x1=\"408\" y1=\"150\" x2=\"408\" y2=\"208\" stroke=\"#0F0D0B\" stroke-width=\"1\"/>\n\
         <text x=\"508\" y=\"180\" text-anchor=\"middle\" font-size=\"40\" font-weight=\"800\" \
         fill=\"#CE422B\">{tput_whole}.{tput_frac:02}<tspan font-size=\"18\" font-weight=\"500\" \
         fill=\"#9A8D82\">M/s</tspan></text>\n\
         <text x=\"508\" y=\"204\" text-anchor=\"middle\" font-size=\"12\" \
         fill=\"#9A8D82\">sustained throughput</text>\n\
         <text x=\"328\" y=\"246\" text-anchor=\"middle\" font-size=\"12\" fill=\"#F2EDE8\">Daemon \
         serialises the compact index → writes directly to the target file.</text>\n\
         <text x=\"328\" y=\"264\" text-anchor=\"middle\" font-size=\"12\" \
         fill=\"#F2EDE8\">{detail_line}</text>\n\
         <text x=\"328\" y=\"298\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">Source: cross-tool-summary.csv (HOT phase, file sink)</text>\n\
         </g>\n\
         <g>\n\
         <rect x=\"632\" y=\"86\" width=\"280\" height=\"240\" rx=\"10\" fill=\"#1E1B18\" \
         stroke=\"#9A8D82\" stroke-width=\"1\"/>\n\
         <text x=\"772\" y=\"114\" text-anchor=\"middle\" font-size=\"12\" font-weight=\"700\" \
         fill=\"#9A8D82\" letter-spacing=\"0.5\">FOR REFERENCE</text>\n\
         <text x=\"772\" y=\"136\" text-anchor=\"middle\" font-size=\"13\" font-weight=\"600\" \
         fill=\"#F2EDE8\">Everything `*` → CSV</text>\n\
         <rect x=\"666\" y=\"158\" width=\"212\" height=\"54\" rx=\"6\" fill=\"#1E1B18\" \
         stroke=\"#B03618\" stroke-width=\"1.5\" stroke-dasharray=\"3,2\"/>\n\
         <text x=\"772\" y=\"180\" text-anchor=\"middle\" font-size=\"14\" font-weight=\"700\" \
         fill=\"#F7B26B\">fails at ~2 GB</text>\n\
         <text x=\"772\" y=\"200\" text-anchor=\"middle\" font-size=\"12\" \
         fill=\"#F7B26B\">WM_COPYDATA IPC cap</text>\n\
         <text x=\"772\" y=\"238\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">Everything's IPC transport was</text>\n\
         <text x=\"772\" y=\"253\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">designed for desktop-interactive</text>\n\
         <text x=\"772\" y=\"268\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">result sets, not scripting-scale</text>\n\
         <text x=\"772\" y=\"283\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">dumps. For multi-million-row sets,</text>\n\
         <text x=\"772\" y=\"298\" text-anchor=\"middle\" font-size=\"11\" font-weight=\"600\" \
         fill=\"#F2EDE8\">this comparison is not apples-to-apples.</text>\n\
         </g>\n\
         <text x=\"480\" y=\"346\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">`--hide-system --hide-ads` strips reserved NTFS metafiles and Alternate \
         Data Streams to match Everything's default scope. Generated by the uffs-bench suite from \
         cross-tool-summary.csv.</text>\n\
         </svg>",
        rows_n = rows.split(' ').next().unwrap_or("0"),
        rows_unit = rows.split(' ').nth(1).unwrap_or("M"),
        tput_whole = tput / 100,
        tput_frac = tput % 100,
    ))
}

/// Write one rendered SVG as `out_dir/<basename-of-rel>`; returns the basename.
fn write_svg(host: &dyn Host, out_dir: &Path, rel: &str, svg: &str) -> Option<String> {
    let name = Path::new(rel).file_name()?.to_string_lossy().into_owned();
    host.create_dir_all(out_dir).ok()?;
    let path = out_dir.join(&name);
    host.write_file(&path, svg.as_bytes()).ok()?;
    Some(name)
}

/// Render every competition chart the CSV supports into `out_dir` (flat
/// basenames), returning `(basename, alt-text)` per chart written.
///
/// Shared by Stage 4 assembly (writing into `bundle/charts/`) and the
/// `render-charts` subcommand (writing into a promoted charts directory).
pub fn render_all(
    host: &dyn Host,
    out_dir: &Path,
    csv: &str,
    uffs_label: &str,
    es_label: &str,
    cpp_label: &str,
) -> Vec<(String, &'static str)> {
    let subtitle = "HOT phase · file sink · p50 per (drive, pattern) cell — lower is better";
    let mut written = Vec::new();

    if let Some(svg) = head_to_head_svg(
        &rival_cells(csv, "Everything"),
        uffs_label,
        es_label,
        subtitle,
    ) && let Some(name) = write_svg(host, out_dir, HEAD_TO_HEAD_SVG, &svg)
    {
        written.push((name, "UFFS vs Everything head-to-head p50"));
    }

    // C++ chart is full-scan only: targeted cells differ by 2–3 orders of
    // magnitude (20 ms vs 90 s), which a shared linear axis cannot show — the
    // targeted carnage reads better as the report's ratio table. Full-scan
    // magnitudes are comparable (seconds vs tens of seconds).
    let cpp_full_scan: Vec<HeadToHeadCell> = rival_cells(csv, "UFFS-C++")
        .into_iter()
        .filter(|cell| cell.pattern == "full_scan")
        .collect();
    if let Some(svg) = head_to_head_svg(
        &cpp_full_scan,
        uffs_label,
        cpp_label,
        "HOT `*` full-scan → CSV per drive scope · UFFS daemon vs full MFT re-read — lower is better",
    ) && let Some(name) = write_svg(host, out_dir, DAEMON_HOT_SVG, &svg)
    {
        written.push((name, "UFFS daemon HOT vs C++ per-invocation MFT re-read"));
    }

    if let Some(svg) = full_scan_svg(
        &full_scan_cells(csv),
        uffs_label,
        "`uffs * --out dump.csv` · HOT · file sink · p50, end-to-end through the daemon pipe",
    ) && let Some(name) = write_svg(host, out_dir, FULL_SCAN_SVG, &svg)
    {
        written.push((name, "UFFS full-scan export throughput"));
    }

    written
}

/// `render-charts` subcommand: re-render the brand-kit charts from a
/// cross-tool summary CSV into `out` — promotion-time tooling, any OS.
///
/// # Errors
/// Returns [`BenchError::Command`] when the CSV cannot be read or yields no
/// chartable cells.
pub fn render_charts_cli(
    host: &dyn Host,
    csv_paths: &[std::path::PathBuf],
    out_dir: &Path,
    uffs_label: &str,
    es_label: &str,
    cpp_label: &str,
) -> Result<()> {
    // Merge rows across all given CSVs (each carries its own header line —
    // the parser skips the first line per chunk, so chunks are concatenated
    // with their headers intact). Lets a UFFS-only all-drives full-scan
    // capture extend a cross-tool run's scope.
    let mut merged = String::new();
    for csv_path in csv_paths {
        let bytes = host
            .read_file(csv_path)
            .map_err(|err| BenchError::Command(format!("read {}: {err}", csv_path.display())))?;
        // AUDIT-OK(bytes): display/measurement CSV, lossy decode is fine.
        let csv = String::from_utf8_lossy(&bytes);
        if merged.is_empty() {
            merged.push_str(&csv);
        } else {
            // Drop the subsequent file's header line before appending.
            let body = csv.split_once('\n').map_or("", |(_, rest)| rest);
            if !merged.ends_with('\n') {
                merged.push('\n');
            }
            merged.push_str(body);
        }
    }
    let written = render_all(host, out_dir, &merged, uffs_label, es_label, cpp_label);
    if written.is_empty() {
        return Err(BenchError::Command(
            "the given CSV(s) contain no chartable HOT/file PASS cells".to_owned(),
        ));
    }
    for (name, alt) in &written {
        host.out(&format!(
            "[charts] wrote {} — {alt}",
            out_dir.join(name).display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_CSV: &str = "\
tool,phase,sink,drive,pattern,p50_ms,p95_ms,rows,bad,verdict,rounds_ok,rounds_total
UFFS,HOT,file,C,exact,20,40,30,0,PASS,10,10
Everything,HOT,file,C,exact,46,50,30,0,PASS,10,10
UFFS,HOT,file,C,ext_dll,91,94,162330,0,PASS,10,10
Everything,HOT,file,C,ext_dll,199,215,162330,0,PASS,10,10
UFFS,HOT,file,C,full_scan,1500,1600,3216011,0,PASS,10,10
UFFS,COLD,file,C,exact,900,1000,30,0,PASS,3,3
";

    #[test]
    fn pairs_only_cells_with_both_tools_hot_file() {
        let cells = rival_cells(RUN_CSV, "Everything");
        // full_scan (UFFS-only) and the COLD row must not pair.
        assert_eq!(cells.len(), 2);
        let first = cells.first().expect("first cell");
        assert_eq!(first.pattern, "exact");
        assert_eq!((first.uffs_p50_ms, first.rival_p50_ms), (20, 46));
    }

    #[test]
    fn svg_carries_design_system_and_ratios() {
        let cells = rival_cells(RUN_CSV, "Everything");
        let svg = head_to_head_svg(&cells, "UFFS v0.5.120", "Everything 1.4.1.1032", "subtitle")
            .expect("svg renders");
        // Design system anchors.
        assert!(svg.contains("fill=\"#CE422B\"")); // UFFS blue
        assert!(svg.contains("fill=\"#9A8D82\"")); // Everything slate
        assert!(svg.contains("2 / 2 cells faster at p50"));
        // Ratio callouts: 20/46 = 0.43×, 91/199 = 0.45× — both emerald wins.
        assert!(svg.contains(">0.43×</text>"));
        assert!(svg.contains(">0.45×</text>"));
        assert!(svg.contains("fill=\"#F7B26B\""));
        // Axis ceiling: max 199 ms → 200; ticks at 40/80/120/160/200.
        assert!(svg.contains(">200</text>"));
        // Bar geometry: 20 ms of 200 max over 680 px = 68.0 px.
        assert!(svg.contains("width=\"68.0\""));
    }

    #[test]
    fn empty_cells_render_none() {
        assert!(head_to_head_svg(&[], "u", "e", "s").is_none());
        assert!(full_scan_svg(&[], "u", "s").is_none());
    }

    const CPP_CSV: &str = "\
tool,phase,sink,drive,pattern,p50_ms,p95_ms,rows,bad,verdict,rounds_ok,rounds_total
UFFS,HOT,file,C,full_scan,1531,1600,3216011,0,PASS,10,10
UFFS-C++,HOT,file,C,full_scan,8621,9000,3216011,0,PASS,10,10
";

    #[test]
    fn rival_cells_pairs_cpp_too() {
        let cells = rival_cells(CPP_CSV, "UFFS-C++");
        assert_eq!(cells.len(), 1);
        let first = cells.first().expect("cpp pair");
        assert_eq!((first.uffs_p50_ms, first.rival_p50_ms), (1531, 8621));
        // Title reflects the rival label.
        let svg = head_to_head_svg(&cells, "UFFS v1", "UFFS C++ (MFT re-read)", "s").expect("svg");
        assert!(svg.contains("UFFS v1 vs UFFS C++ (MFT re-read) — 1 / 1 cells faster at p50"));
    }

    #[test]
    fn full_scan_chart_is_a_stat_card_with_hero_numbers() {
        let cells = full_scan_cells(RUN_CSV);
        assert_eq!(cells.len(), 1);
        let svg = full_scan_svg(&cells, "UFFS v0.5.120", "subtitle").expect("svg");
        // 3,216,011 rows in 1500 ms: title + hero figures (3.2 M, 1.5 s,
        // 2,144,007 rec/s → "2.14 M/s").
        assert!(svg.contains("<title>Full-scan export: 3.2 M records → CSV in 1.5 s</title>"));
        assert!(svg.contains(">3.2<tspan"));
        assert!(svg.contains(">1.5<tspan"));
        assert!(svg.contains(">2.14<tspan"));
        // Everything reference panel with the IPC-ceiling callout.
        assert!(svg.contains("FOR REFERENCE"));
        assert!(svg.contains("fails at ~2 GB"));
        assert!(svg.contains("WM_COPYDATA IPC cap"));
        // Brand: Rust-Orange UFFS panel border + Ember warning text.
        assert!(svg.contains("stroke=\"#CE422B\""));
        assert!(svg.contains("fill=\"#F7B26B\""));
    }

    #[test]
    fn full_scan_card_hero_is_widest_scope_with_per_drive_breakdown() {
        let cells = vec![
            FullScanCell {
                drive: "C".to_owned(),
                rows: 3_295_681,
                p50_ms: 1_561,
            },
            FullScanCell {
                drive: "C+D+F+G".to_owned(),
                rows: 10_207_863,
                p50_ms: 4_833,
            },
        ];
        let svg = full_scan_svg(&cells, "UFFS v0.5.120", "subtitle").expect("svg");
        // Hero = combined scope (most rows): 10.2 M, 4.8 s, 2.11 M/s.
        assert!(svg.contains("<title>Full-scan export: 10.2 M records → CSV in 4.8 s</title>"));
        assert!(svg.contains(">2.11<tspan"));
        // Per-drive breakdown lists the non-hero scopes.
        assert!(svg.contains("Per drive: C: 1.6 s"));
    }
}
