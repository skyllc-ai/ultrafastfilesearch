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
/// uffs.com) are skipped.
#[must_use]
pub fn rival_cells(cross_tool_csv: &str, rival_tool: &str) -> Vec<HeadToHeadCell> {
    let run = parse_run_csv(cross_tool_csv);
    let mut cells = Vec::new();
    for uffs in run.iter().filter(|cell| cell.tool == "UFFS") {
        let Some(rival_p50) = run_p50(&run, rival_tool, &uffs.drive, &uffs.pattern) else {
            continue;
        };
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
#[must_use]
pub fn full_scan_cells(cross_tool_csv: &str) -> Vec<FullScanCell> {
    parse_run_csv(cross_tool_csv)
        .into_iter()
        .filter(|cell| cell.tool == "UFFS" && cell.pattern == "full_scan" && cell.p50_ms > 0)
        .map(|cell| FullScanCell {
            drive: cell.drive,
            rows: cell.rows,
            p50_ms: cell.p50_ms,
        })
        .collect()
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

/// Format `rows ÷ p50` as a human throughput string (`"2.1 M rec/s"` /
/// `"850 K rec/s"`), integer math only.
fn throughput_label(rows: u64, p50_ms: u64) -> String {
    let rps = (rows * 1000).checked_div(p50_ms).unwrap_or(0);
    if rps >= 1_000_000 {
        let tenths = rps * 10 / 1_000_000;
        format!("{}.{} M rec/s", tenths / 10, tenths % 10)
    } else {
        format!("{} K rec/s", rps / 1000)
    }
}

/// Render the full-scan throughput SVG (UFFS-only `*` → CSV export — the
/// workload Everything cannot run), or `None` when `cells` is empty.
#[must_use]
pub fn full_scan_svg(cells: &[FullScanCell], uffs_label: &str, subtitle: &str) -> Option<String> {
    if cells.is_empty() {
        return None;
    }
    let row_count = u64::try_from(cells.len()).unwrap_or(0);
    // One bar per row (no pair), so a tighter pitch keeps the card compact.
    let pitch = 34_u64;
    let max_ms = cells.iter().map(|cell| cell.p50_ms).max().unwrap_or(0);
    // Axis ceiling: next multiple of 1000 ms (≥ 1 s).
    let axis_max = ((max_ms / 1000) + 1) * 1000;
    let axis_bottom = BARS_TOP + row_count * pitch + 10;
    let height = axis_bottom + 70;
    let title = format!("{uffs_label} full-scan export — the workload Everything cannot run");

    let mut parts = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 960 {height}\" \
         font-family=\"Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, \
         Arial, sans-serif\" role=\"img\" aria-label=\"Bar chart: {title}\">\n\
         <title>{title}</title>\n\
         <desc>Bar chart of UFFS full-scan CSV-export wall time per drive scope, with row counts \
         and sustained records-per-second callouts. Generated by the uffs-bench suite from \
         cross-tool-summary.csv.</desc>\n\
         <rect x=\"0\" y=\"0\" width=\"960\" height=\"{height}\" fill=\"#0F0D0B\"/>\n\
         <rect x=\"0.5\" y=\"0.5\" width=\"959\" height=\"{}\" fill=\"none\" stroke=\"#1E1B18\" \
         stroke-width=\"1\"/>\n\
         <text x=\"480\" y=\"36\" text-anchor=\"middle\" font-size=\"19\" font-weight=\"700\" \
         fill=\"#F2EDE8\">{title}</text>\n\
         <text x=\"480\" y=\"58\" text-anchor=\"middle\" font-size=\"13\" \
         fill=\"#9A8D82\">{subtitle}</text>",
        height - 1,
    )];

    // Axis: seconds, five segments.
    let mut axis = vec![format!(
        "<g font-size=\"11\" fill=\"#9A8D82\">\n\
         <line x1=\"160\" y1=\"100\" x2=\"160\" y2=\"{axis_bottom}\" stroke=\"#9A8D82\" \
         stroke-width=\"1\"/>\n\
         <line x1=\"160\" y1=\"{axis_bottom}\" x2=\"840\" y2=\"{axis_bottom}\" \
         stroke=\"#9A8D82\" stroke-width=\"1\"/>\n\
         <text x=\"160\" y=\"{}\" text-anchor=\"middle\">0</text>",
        axis_bottom + 18,
    )];
    for tick in 1..=5_u64 {
        let x = PLOT_X + tick * PLOT_WIDTH_PX / 5;
        let tick_ms = tick * axis_max / 5;
        axis.push(format!(
            "<line x1=\"{x}\" y1=\"100\" x2=\"{x}\" y2=\"{axis_bottom}\" stroke=\"#1E1B18\" \
             stroke-width=\"1\"/>\n\
             <text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{}.{}</text>",
            axis_bottom + 18,
            tick_ms / 1000,
            (tick_ms % 1000) / 100,
        ));
    }
    axis.push(format!(
        "<text x=\"500\" y=\"{}\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">`*` → CSV wall time (seconds, p50) — lower is better</text>\n</g>",
        axis_bottom + 40,
    ));
    parts.push(axis.join("\n"));

    for (idx, cell) in cells.iter().enumerate() {
        let row = u64::try_from(idx).unwrap_or(0);
        let y_bar = BARS_TOP + row * pitch;
        let bar_w = bar_tenths(cell.p50_ms, axis_max);
        let secs_tenths = cell.p50_ms / 100;
        parts.push(format!(
            "<g font-size=\"12\" fill=\"#F2EDE8\">\n\
             <text x=\"154\" y=\"{label_y}\" text-anchor=\"end\">{drive}:</text>\n\
             <rect x=\"160\" y=\"{y_bar}\" width=\"{bar_px}\" height=\"18\" fill=\"#CE422B\"/>\n\
             <text x=\"{value_x}\" y=\"{label_y}\" font-size=\"11\" \
             fill=\"#9A8D82\">{secs_w}.{secs_f} s · {rows} rows</text>\n\
             <text x=\"848\" y=\"{label_y}\" font-size=\"12\" font-weight=\"700\" \
             fill=\"#F7B26B\">{throughput}</text>\n\
             </g>",
            label_y = y_bar + 13,
            drive = cell.drive,
            bar_px = px(bar_w),
            value_x = PLOT_X + bar_w / 10 + 6,
            secs_w = secs_tenths / 10,
            secs_f = secs_tenths % 10,
            rows = crate::storage::commas(cell.rows),
            throughput = throughput_label(cell.rows, cell.p50_ms),
        ));
    }

    parts.push(format!(
        "<text x=\"480\" y=\"{}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#9A8D82\">\
         Generated by the uffs-bench suite from cross-tool-summary.csv (HOT phase, file sink). \
         es.exe -export-csv aborts near ~2 GB over IPC, so this workload is UFFS-only.</text>\n\
         </svg>",
        height - 14,
    ));
    Some(parts.join("\n"))
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

    if let Some(svg) = head_to_head_svg(&rival_cells(csv, "Everything"), uffs_label, es_label, subtitle)
        && let Some(name) = write_svg(host, out_dir, HEAD_TO_HEAD_SVG, &svg)
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
        "complete `*` result set streamed from the daemon to CSV",
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
    csv_path: &Path,
    out_dir: &Path,
    uffs_label: &str,
    es_label: &str,
    cpp_label: &str,
) -> Result<()> {
    let bytes = host
        .read_file(csv_path)
        .map_err(|err| BenchError::Command(format!("read {}: {err}", csv_path.display())))?;
    // AUDIT-OK(bytes): display/measurement CSV, lossy decode is fine.
    let csv = String::from_utf8_lossy(&bytes);
    let written = render_all(host, out_dir, &csv, uffs_label, es_label, cpp_label);
    if written.is_empty() {
        return Err(BenchError::Command(format!(
            "{} contains no chartable HOT/file PASS cells",
            csv_path.display()
        )));
    }
    for (name, alt) in &written {
        host.out(&format!("[charts] wrote {} — {alt}", out_dir.join(name).display()));
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
        let svg =
            head_to_head_svg(&cells, "UFFS v1", "UFFS C++ (MFT re-read)", "s").expect("svg");
        assert!(svg.contains("UFFS v1 vs UFFS C++ (MFT re-read) — 1 / 1 cells faster at p50"));
    }

    #[test]
    fn full_scan_chart_carries_rows_and_throughput() {
        let cells = full_scan_cells(RUN_CSV);
        assert_eq!(cells.len(), 1);
        let svg = full_scan_svg(&cells, "UFFS v0.5.120", "subtitle").expect("svg");
        // 3,216,011 rows in 1500 ms → 2,144,007 rec/s → "2.1 M rec/s"; 1.5 s.
        assert!(svg.contains("1.5 s · 3,216,011 rows"));
        assert!(svg.contains(">2.1 M rec/s</text>"));
        assert!(svg.contains("the workload Everything cannot run"));
        // Brand bars.
        assert!(svg.contains("fill=\"#CE422B\""));
    }
}
