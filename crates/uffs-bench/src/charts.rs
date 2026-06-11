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

use crate::baseline::{parse_run_csv, run_p50};

/// Bundle-relative path of the generated head-to-head chart.
pub const HEAD_TO_HEAD_SVG: &str = "charts/head-to-head-vs-everything.svg";

/// One paired head-to-head measurement (HOT phase, file sink).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadToHeadCell {
    /// Drive scope (`"C"`, `"D"`, `"C+D+F+G"`).
    pub drive: String,
    /// Pattern label.
    pub pattern: String,
    /// UFFS p50 in milliseconds.
    pub uffs_p50_ms: u64,
    /// Everything p50 in milliseconds.
    pub es_p50_ms: u64,
}

/// Pair UFFS and Everything HOT/file p50s per `(drive, pattern)`.
///
/// Reads the cross-tool summary CSV, preserving the CSV's UFFS row order.
/// Cells without both tools (e.g. `full_scan`, which es.exe cannot export)
/// are skipped.
#[must_use]
pub fn head_to_head_cells(cross_tool_csv: &str) -> Vec<HeadToHeadCell> {
    let run = parse_run_csv(cross_tool_csv);
    let mut cells = Vec::new();
    for uffs in run.iter().filter(|cell| cell.tool == "UFFS") {
        let Some(es_p50) = run_p50(&run, "Everything", &uffs.drive, &uffs.pattern) else {
            continue;
        };
        cells.push(HeadToHeadCell {
            drive: uffs.drive.clone(),
            pattern: uffs.pattern.clone(),
            uffs_p50_ms: uffs.p50_ms,
            es_p50_ms: es_p50,
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

/// Render the head-to-head grouped-bar SVG, or `None` when `cells` is empty.
///
/// `uffs_label` / `es_label` feed the legend (e.g. `"UFFS v0.5.120"`,
/// `"Everything 1.4.1.1032"`); `subtitle` is the context line under the title.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "single linear SVG template: header, axis, per-row groups, footer — splitting would scatter the fixed geometry constants"
)]
pub fn head_to_head_svg(
    cells: &[HeadToHeadCell],
    uffs_label: &str,
    es_label: &str,
    subtitle: &str,
) -> Option<String> {
    if cells.is_empty() {
        return None;
    }
    let rows = u64::try_from(cells.len()).unwrap_or(0);
    let wins = cells
        .iter()
        .filter(|cell| cell.uffs_p50_ms < cell.es_p50_ms)
        .count();

    // Axis ceiling: max p50 rounded up to the next multiple of 50 (≥ 50).
    let max_ms = cells
        .iter()
        .map(|cell| cell.uffs_p50_ms.max(cell.es_p50_ms))
        .max()
        .unwrap_or(0);
    let axis_max = ((max_ms / 50) + 1) * 50;

    let axis_bottom = BARS_TOP + rows * ROW_PITCH + 10;
    let height = axis_bottom + 70;
    let title = format!(
        "{uffs_label} vs Everything — {wins} / {} cells faster at p50",
        cells.len()
    );

    let mut parts = vec![format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 960 {height}\" \
         font-family=\"Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, \
         sans-serif\" role=\"img\" aria-label=\"Horizontal bar chart: {title}\">\n\
         <title>{title}</title>\n\
         <desc>Horizontal grouped bar chart comparing UFFS and Everything p50 latency per \
         pattern-by-drive cell. UFFS bars are brand Rust Orange, Everything bars are muted Sand. Generated \
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
         <text x=\"200\" y=\"12\">{es_label}</text>\n\
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
        let label = tick * axis_max / 5;
        axis.push(format!(
            "<line x1=\"{x}\" y1=\"110\" x2=\"{x}\" y2=\"{axis_bottom}\" stroke=\"#1E1B18\" \
             stroke-width=\"1\"/>\n\
             <text x=\"{x}\" y=\"{}\" text-anchor=\"middle\">{label}</text>",
            axis_bottom + 18,
        ));
    }
    axis.push(format!(
        "<text x=\"500\" y=\"{}\" text-anchor=\"middle\" font-size=\"11\" \
         fill=\"#9A8D82\">p50 latency (milliseconds) — lower is better</text>\n</g>",
        axis_bottom + 40,
    ));
    parts.push(axis.join("\n"));

    // One group per cell: row label, two bars, value labels, ratio callout.
    for (idx, cell) in cells.iter().enumerate() {
        let row = u64::try_from(idx).unwrap_or(0);
        let y_uffs = BARS_TOP + row * ROW_PITCH;
        let y_es = y_uffs + 18;
        let uffs_w = bar_tenths(cell.uffs_p50_ms, axis_max);
        let es_w = bar_tenths(cell.es_p50_ms, axis_max);
        let ratio_hundredths = (cell.uffs_p50_ms * 100)
            .checked_div(cell.es_p50_ms)
            .unwrap_or(100);
        let ratio_color = if cell.uffs_p50_ms < cell.es_p50_ms {
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
             fill=\"#9A8D82\">{uffs_ms} ms</text>\n\
             <text x=\"{es_label_x}\" y=\"{es_label_y}\" font-size=\"11\" \
             fill=\"#9A8D82\">{es_ms} ms</text>\n\
             <text x=\"848\" y=\"{ratio_y}\" font-size=\"12\" font-weight=\"700\" \
             fill=\"{ratio_color}\">{ratio_w}.{ratio_f:02}×</text>\n\
             </g>",
            label_y = y_uffs + 12,
            drive = cell.drive,
            pattern = cell.pattern,
            uffs_px = px(uffs_w),
            es_px = px(es_w),
            uffs_label_x = PLOT_X + uffs_w / 10 + 6,
            uffs_ms = cell.uffs_p50_ms,
            es_label_x = PLOT_X + es_w / 10 + 6,
            es_label_y = y_es + 12,
            es_ms = cell.es_p50_ms,
            ratio_y = y_uffs + 21,
            ratio_w = ratio_hundredths / 100,
            ratio_f = ratio_hundredths % 100,
        ));
    }

    parts.push(format!(
        "<text x=\"480\" y=\"{}\" text-anchor=\"middle\" font-size=\"10\" fill=\"#9A8D82\">\
         Generated by the uffs-bench suite from cross-tool-summary.csv (HOT phase, file sink). \
         Lower is better; ratio = UFFS ÷ Everything.</text>\n</svg>",
        height - 14,
    ));
    Some(parts.join("\n"))
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
        let cells = head_to_head_cells(RUN_CSV);
        // full_scan (UFFS-only) and the COLD row must not pair.
        assert_eq!(cells.len(), 2);
        let first = cells.first().expect("first cell");
        assert_eq!(first.pattern, "exact");
        assert_eq!((first.uffs_p50_ms, first.es_p50_ms), (20, 46));
    }

    #[test]
    fn svg_carries_design_system_and_ratios() {
        let cells = head_to_head_cells(RUN_CSV);
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
    }
}
