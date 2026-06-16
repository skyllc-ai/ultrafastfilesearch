// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 4 — bundle assembly + dated `REPORT-DRAFT.md` scaffold (plan §10).
//!
//! Assembly is read-only with respect to *host* state: it only reads the
//! artifacts the earlier stages wrote into the bundle and writes a single
//! draft back into that same bundle. The draft renders the environment table
//! and the negotiated matrix, then cites and embeds each measurement raw log
//! (cross-tool CSV, parity transcript, full-suite transcript).
//!
//! The draft is **never auto-committed**: promotion into `docs/benchmarks/` is
//! a manual, reviewed step. The draft's header records the suggested canonical
//! `YYYY-MM-vX.Y.Z-<scope>.md` name for that promotion.
//!
//! [`render`] is a pure function of its [`ReportInputs`] (no host access), so
//! it is covered by a golden test; [`assemble`] loads those inputs through the
//! [`Host`] seam and persists the result, keeping it testable under `MockHost`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike as _, Utc};

use crate::error::{BenchError, Result};
use crate::host::Host;
use crate::matrix::{self, Matrix};
use crate::preflight::{self, PreflightResult};
use crate::{baseline, charts, env, storage, summary};

/// Bundle-relative name of the assembled report draft (plan §11).
pub const REPORT_DRAFT: &str = "REPORT-DRAFT.md";

/// Bundle-relative name of the rendered environment table (Stage 0a).
const ENV_MD: &str = "env.md";
/// Bundle-relative name of the negotiated matrix (Stage 0d).
const MATRIX_JSON: &str = "matrix.json";
/// Bundle-relative name of the competitor preflight result (Stage 0b).
const PREFLIGHT_JSON: &str = "competitor-preflight.json";
/// Bundle-relative name of the Stage 1 cross-tool summary CSV.
const CROSS_TOOL_CSV: &str = "cross-tool-summary.csv";
/// Bundle-relative name of the Stage 2 parity transcript.
const PARITY_TXT: &str = "parity.txt";
/// Bundle-relative name of the Stage 3 full-suite transcript.
const FULL_SUITE_TXT: &str = "full-suite.txt";
/// Bundle-relative name of the Stage 3 full-suite machine CSV.
const FULL_SUITE_CSV: &str = "full-suite.csv";

/// The artifacts the draft renderer assembles into a report skeleton.
///
/// Each measurement field is `None` when its raw log was not produced this run
/// (for example, a single-stage run), in which case the section cites the file
/// as missing rather than embedding it.
pub struct ReportInputs {
    /// Suite version (`CARGO_PKG_VERSION`), stamped into the title + name.
    pub version: String,
    /// Coverage scope label (for example the participating drives).
    pub scope: String,
    /// When the draft was assembled.
    pub generated_at: DateTime<Utc>,
    /// Rendered `## At a glance` header (`summary.md`), if present.
    pub summary_md: Option<String>,
    /// Rendered Stage 0a environment markdown (`env.md`), if present.
    pub env_md: Option<String>,
    /// Rendered Stage 0d matrix markdown (from `matrix.json`), if present.
    pub matrix_md: Option<String>,
    /// Rendered `## Storage devices` markdown (from `drives.json`), if present.
    pub storage_md: Option<String>,
    /// Rendered `## Everything RAM budget` markdown (from the preflight JSON) —
    /// the per-drive rationale for which drives ran cross-tool, if present.
    pub es_budget_md: Option<String>,
    /// Stage 1 cross-tool summary CSV contents, if present.
    pub cross_tool_csv: Option<String>,
    /// Stage 2 parity transcript contents, if present.
    pub parity_txt: Option<String>,
    /// Stage 3 full-suite transcript contents, if present.
    pub full_suite_txt: Option<String>,
    /// Stage 3 full-suite machine CSV contents, if present (rendered as a
    /// table; the transcript is the fallback).
    pub full_suite_csv: Option<String>,
    /// Rendered `## vs baseline` comparison (current run vs the last canonical
    /// report's numbers), if a baseline was found.
    pub baseline_md: Option<String>,
    /// Rendered `## Charts` section embedding the generated brand-kit SVGs,
    /// if any chart could be produced from this run's data.
    pub charts_md: Option<String>,
}

/// Suggested canonical `YYYY-MM-vX.Y.Z-<scope>.md` promotion name.
fn promotion_name(generated_at: DateTime<Utc>, version: &str, scope: &str) -> String {
    // The header `scope` is human-readable ("C:, D:, …"); slugify it back to a
    // filename-safe compact form ("cd…").
    let collected: String = scope
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    let slug = if collected.is_empty() {
        "full"
    } else {
        &collected
    };
    format!(
        "{year:04}-{month:02}-v{version}-{slug}.md",
        year = generated_at.year(),
        month = generated_at.month(),
    )
}

/// Render the static "Patterns under test" matrix: which query shape runs on
/// which tool, and why the exceptions exist.
///
/// Keep in sync with `PATTERNS` in `scripts/windows/cross-tool-benchmark.rs`
/// (the cross-tool harness's single source of truth) and the Stage 3 native
/// suite cells in `stages.rs`.
fn patterns_md() -> String {
    "## Patterns under test\n\n\
     | Pattern | Query shape | UFFS | UFFS-C++ | Everything (es) |\n\
     |---------|-------------|:----:|:--------:|:---------------:|\n\
     | exact | `notepad.exe` | ✓ | ✓ | ✓ |\n\
     | prefix | `win*` | ✓ | ✗ ¹ | ✓ |\n\
     | ext_rare | `*.dbt` | ✓ | ✓ (`--ext=dbt`) | ✓ (`ext:dbt`) |\n\
     | ext_dll | `*.dll` | ✓ | ✓ (`--ext=dll`) | ✓ (`ext:dll`) |\n\
     | ext_regex_alt | `.*\\.(wav\\|idrc\\|cmake)$` | ✓ regex | ✓ multi-ext | ✓ OR-glob |\n\
     | substring | `config` | ✓ | ✓ | ✓ |\n\
     | full_scan | `*` (every record) | ✓ | ✓ | ✗ ² |\n\n\
     ¹ `uffs.com` does not support trailing-wildcard prefix globs (`win*`), so the \
     prefix cell is UFFS vs Everything only.\n\
     ² `es.exe -export-csv` streams results over Everything's IPC channel, which tops \
     out near ~2 GB (≈150 K rows in practice). An unbounded `*` export of millions of \
     rows aborts the CLI, so the full-scan cell runs without Everything; UFFS streams \
     the complete multi-million-row set to CSV directly from its daemon.\n\n\
     _The Stage 3 native suite additionally times `all_dlls` (`*.dll --count`) and \
     `full_scan` (`* --count`) per drive — UFFS-only, count sink (no row output)._\n\
     \n\
     <!-- keep in sync with PATTERNS in scripts/windows/cross-tool-benchmark.rs -->"
        .to_owned()
}

/// Render `full-suite.csv` as a markdown table, or `None` when the CSV is
/// absent or carries no data rows.
///
/// Columns (per `stages::render_csv`):
/// `tool,version,phase,sink,drive,pattern,rows,p50_ms,p95_ms,stddev_ms,rounds,
/// verdict,notes`.
fn render_full_suite_table(csv: &str) -> Option<String> {
    let mut rendered = Vec::new();
    for line in csv.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        let (
            Some(drive),
            Some(pattern),
            Some(rows),
            Some(p50),
            Some(p95),
            Some(stddev),
            Some(rounds),
            Some(verdict),
        ) = (
            fields.get(4),
            fields.get(5),
            fields.get(6),
            fields.get(7),
            fields.get(8),
            fields.get(9),
            fields.get(10),
            fields.get(11),
        )
        else {
            continue;
        };
        let rows_cell = rows
            .parse::<u64>()
            .map_or_else(|_| (*rows).to_owned(), storage::commas);
        rendered.push(format!(
            "| {drive}: | {pattern} | {rows_cell} | {p50} ms | {p95} ms | {stddev} ms | {rounds} | {verdict} |"
        ));
    }
    if rendered.is_empty() {
        return None;
    }
    Some(format!(
        "| Drive | Pattern | Rows | p50 | p95 | stddev | Rounds | Verdict |\n\
         |-------|---------|-----:|----:|----:|-------:|-------:|---------|\n\
         {}",
        rendered.join("\n")
    ))
}

/// Render an already-markdown sub-document, or a "not produced" placeholder
/// under `heading` when it is absent.
fn embedded(heading: &str, body: Option<&String>) -> String {
    body.map_or_else(
        || format!("{heading}\n\n_Not produced this run._"),
        Clone::clone,
    )
}

/// Render a raw log as a fenced `lang` block, citing `name`; a "not produced"
/// placeholder when absent.
fn fenced(heading: &str, name: &str, lang: &str, body: Option<&String>) -> String {
    body.map_or_else(
        || format!("{heading}\n\n_Raw log: `{name}` — not produced this run._"),
        |text| {
            format!(
                "{heading}\n\n_Raw log: `{name}`._\n\n```{lang}\n{}\n```",
                text.trim_end()
            )
        },
    )
}

/// Render a raw transcript inline (it already carries its own markdown), citing
/// `name`; a "not produced" placeholder when absent.
fn inlined(heading: &str, name: &str, body: Option<&String>) -> String {
    body.map_or_else(
        || format!("{heading}\n\n_Raw log: `{name}` — not produced this run._"),
        |text| format!("{heading}\n\n_Raw log: `{name}`._\n\n{}", text.trim_end()),
    )
}

/// Render the assembled report draft as markdown.
///
/// Pure in its [`ReportInputs`] (no host access), so it is covered by a golden
/// test. The output always ends with a single trailing newline.
#[must_use]
pub fn render(inputs: &ReportInputs) -> String {
    let promotion = promotion_name(inputs.generated_at, &inputs.version, &inputs.scope);
    let sections = [
        format!("# UFFS Benchmark Report — DRAFT (v{})", inputs.version),
        format!(
            "> **Draft scaffold — do not commit as-is.** Promotion into \
             `docs/benchmarks/` is a manual, reviewed step. Suggested canonical \
             name: `{promotion}`."
        ),
        // Lean provenance line; the title already carries the version, and the
        // "## At a glance" table below states what was actually benchmarked
        // (so the old smushed "Scope: cdefghims" bullet is dropped).
        format!(
            "_Generated {} · suite v{}._",
            inputs.generated_at.format("%Y-%m-%d %H:%M:%S UTC"),
            inputs.version,
        ),
        embedded("## At a glance", inputs.summary_md.as_ref()),
        embedded("## Test environment", inputs.env_md.as_ref()),
        // Flow: what we have (storage) → why ES is constrained (budget) →
        // what we negotiated (matrix) → the measurements.
        embedded("## Storage devices", inputs.storage_md.as_ref()),
        embedded("## Everything RAM budget", inputs.es_budget_md.as_ref()),
        embedded("## Negotiated matrix", inputs.matrix_md.as_ref()),
        patterns_md(),
        fenced(
            "## Cross-tool head-to-head (§1)",
            CROSS_TOOL_CSV,
            "csv",
            inputs.cross_tool_csv.as_ref(),
        ),
        embedded(
            "## vs baseline (last canonical report)",
            inputs.baseline_md.as_ref(),
        ),
        embedded("## Charts", inputs.charts_md.as_ref()),
        inlined(
            "## Per-drive parity (§2)",
            PARITY_TXT,
            inputs.parity_txt.as_ref(),
        ),
        // §3 prefers the machine CSV rendered as a table; the plain-text
        // transcript is the fallback when the CSV is absent.
        inputs
            .full_suite_csv
            .as_deref()
            .and_then(render_full_suite_table)
            .map_or_else(
                || {
                    inlined(
                        "## Full-suite (§3)",
                        FULL_SUITE_TXT,
                        inputs.full_suite_txt.as_ref(),
                    )
                },
                |table| {
                    format!(
                        "## Full-suite (§3)\n\n\
                         _UFFS native, count sink, hot tier. Raw: `{FULL_SUITE_CSV}` / \
                         `{FULL_SUITE_TXT}`._\n\n{table}"
                    )
                },
            ),
    ];
    format!("{}\n", sections.join("\n\n"))
}

/// Decode bundle-artifact bytes for embedding into the draft.
fn decode(bytes: &[u8]) -> String {
    // AUDIT-OK(bytes): display-only embedding of a bundle artifact into a report
    // draft; never a parse or security decision.
    String::from_utf8_lossy(bytes).into_owned()
}

/// Load a bundle artifact as text, returning `None` when it was not produced.
fn load(host: &dyn Host, bundle_dir: &Path, name: &str) -> Option<String> {
    let bytes = host.read_file(&bundle_dir.join(name)).ok()?;
    Some(decode(&bytes))
}

/// Load `matrix.json` and re-render it as markdown, `None` if absent/invalid.
fn load_matrix_md(host: &dyn Host, bundle_dir: &Path) -> Option<String> {
    let bytes = host.read_file(&bundle_dir.join(MATRIX_JSON)).ok()?;
    let matrix: Matrix = serde_json::from_slice(&bytes).ok()?;
    Some(matrix::render_md(&matrix))
}

/// Load `drives.json` and render the `## Storage devices` table, flagging the
/// drives the matrix selected. `None` if the inventory was not captured.
fn load_storage_md(host: &dyn Host, bundle_dir: &Path) -> Option<String> {
    let bytes = host
        .read_file(&bundle_dir.join(storage::DRIVES_JSON))
        .ok()?;
    let drives = storage::parse(&decode(&bytes));
    // Best-effort: an absent/invalid matrix just means nothing is flagged.
    let benched = host
        .read_file(&bundle_dir.join(MATRIX_JSON))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Matrix>(&raw).ok())
        .map(|matrix| matrix.capable_drives)
        .unwrap_or_default();
    storage::render_md(&drives, &benched)
}

/// Load the preflight JSON and re-render the per-drive Everything RAM-budget
/// table — the rationale for which drives ran cross-tool. `None` if
/// absent/invalid/empty.
fn load_es_budget_md(host: &dyn Host, bundle_dir: &Path) -> Option<String> {
    let bytes = host.read_file(&bundle_dir.join(PREFLIGHT_JSON)).ok()?;
    let result: PreflightResult = serde_json::from_slice(&bytes).ok()?;
    let table = preflight::render_drive_table(&result, preflight::ES_RAM_BUDGET_BYTES);
    if table.trim().is_empty() {
        return None;
    }
    Some(format!(
        "## Everything RAM budget\n\n_Everything's in-process index is RAM-bound; drives are \
         admitted in candidate order until the budget is hit. Drives marked **✗ over budget** \
         were measured UFFS-only (no cross-tool cell)._\n\n{table}"
    ))
}

/// Load `docs/benchmarks/baseline.json` (repo-relative — the bench runs from
/// the repository root) and render the `## vs baseline` comparison against
/// this run's cross-tool summary. `None` when the baseline file, the run CSV,
/// or any matching cell is absent.
fn load_baseline_md(host: &dyn Host, cross_tool_csv: Option<&String>) -> Option<String> {
    let csv = cross_tool_csv?;
    let bytes = host.read_file(Path::new(baseline::BASELINE_PATH)).ok()?;
    let parsed = baseline::parse(&decode(&bytes))?;
    baseline::render_md(&parsed, csv)
}

/// Legend label for `tool_name`, read from the bundle's `env.json`;
/// `fallback` (e.g. `"Everything"`) when the version is unavailable.
fn tool_chart_label(host: &dyn Host, bundle_dir: &Path, tool_name: &str, fallback: &str) -> String {
    host.read_file(&bundle_dir.join("env.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<env::EnvFingerprint>(&bytes).ok())
        .and_then(|fp| {
            fp.tools
                .iter()
                .find(|tool| tool.name == tool_name)
                .map(|tool| tool.version.clone())
        })
        .filter(|version| version.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map_or_else(|| fallback.to_owned(), |ver| format!("{fallback} {ver}"))
}

/// Generate the brand-kit SVG charts into `bundle/charts/` and return the
/// `## Charts` markdown embedding them — the head-to-head vs Everything, the
/// daemon-HOT vs C++ full-scan comparison, and the UFFS-only full-scan
/// throughput. Best-effort: charts whose cells are absent are skipped;
/// `None` (no section) when nothing could be produced.
fn generate_charts_md(
    host: &dyn Host,
    bundle_dir: &Path,
    version: &str,
    cross_tool_csv: Option<&String>,
) -> Option<String> {
    let csv = cross_tool_csv?;
    let written = charts::render_all(
        host,
        &bundle_dir.join("charts"),
        csv,
        &format!("UFFS v{version}"),
        &tool_chart_label(host, bundle_dir, "everything_gui", "Everything"),
        &tool_chart_label(host, bundle_dir, "uffs_cpp", "UFFS C++ (MFT re-read)"),
    );
    if written.is_empty() {
        return None;
    }
    let images: Vec<String> = written
        .iter()
        .map(|(name, alt)| format!("![{alt}](charts/{name})"))
        .collect();
    Some(format!(
        "{}\n\n_Brand-kit SVGs generated from this run's `{CROSS_TOOL_CSV}` — drop-in for the \
         canonical report, hub README, and social posts._",
        images.join("\n\n")
    ))
}

/// Assemble the bundle into `bundle_dir/REPORT-DRAFT.md` and return its path.
///
/// Reads the Stage 0/1/2/3 artifacts already in the bundle through the [`Host`]
/// seam, renders the draft with [`render`], and writes it back into the bundle.
/// Only the bundle is touched; host state is left untouched.
///
/// # Errors
/// Returns an error if the draft cannot be written into the bundle.
pub fn assemble(host: &dyn Host, bundle_dir: &Path, version: &str, scope: &str) -> Result<PathBuf> {
    let cross_tool_csv = load(host, bundle_dir, CROSS_TOOL_CSV);
    let baseline_md = load_baseline_md(host, cross_tool_csv.as_ref());
    let charts_md = generate_charts_md(host, bundle_dir, version, cross_tool_csv.as_ref());
    let inputs = ReportInputs {
        version: version.to_owned(),
        scope: scope.to_owned(),
        generated_at: host.now(),
        summary_md: load(host, bundle_dir, summary::SUMMARY_MD),
        env_md: load(host, bundle_dir, ENV_MD),
        matrix_md: load_matrix_md(host, bundle_dir),
        storage_md: load_storage_md(host, bundle_dir),
        es_budget_md: load_es_budget_md(host, bundle_dir),
        cross_tool_csv,
        parity_txt: load(host, bundle_dir, PARITY_TXT),
        full_suite_txt: load(host, bundle_dir, FULL_SUITE_TXT),
        full_suite_csv: load(host, bundle_dir, FULL_SUITE_CSV),
        baseline_md,
        charts_md,
    };
    let path = bundle_dir.join(REPORT_DRAFT);
    host.write_file(&path, render(&inputs).as_bytes())
        .map_err(|err| BenchError::io(&path, err))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::{DateTime, Utc};

    use super::{REPORT_DRAFT, ReportInputs, assemble, render};
    use crate::host::{Call, MockHost};
    use crate::matrix::{CrossCell, Matrix, SoloCell};

    /// A fixed, deterministic instant (2023-11-14 UTC) for golden assertions.
    fn fixed_now() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    /// Fully-populated inputs for the renderer golden test.
    fn sample_inputs() -> ReportInputs {
        ReportInputs {
            version: "9.9.9".to_owned(),
            scope: "cd".to_owned(),
            generated_at: fixed_now(),
            summary_md: Some("## At a glance\n\n<SUMMARY>".to_owned()),
            env_md: Some("## Test environment\n\n<ENV>".to_owned()),
            matrix_md: Some("## Negotiated matrix\n\n<MATRIX>".to_owned()),
            storage_md: Some("## Storage devices\n\n<STORAGE>".to_owned()),
            es_budget_md: Some("## Everything RAM budget\n\n<ESBUDGET>".to_owned()),
            cross_tool_csv: Some("tool,rows\nuffs,5\n".to_owned()),
            parity_txt: Some("<PARITY>".to_owned()),
            full_suite_txt: Some("<FULL>".to_owned()),
            full_suite_csv: None,
            baseline_md: Some("## vs baseline (last canonical report)\n\n<BASE>".to_owned()),
            charts_md: Some("![chart](charts/head-to-head-vs-everything.svg)".to_owned()),
        }
    }

    #[test]
    fn render_embeds_artifacts_and_promotion_name() {
        let md = render(&sample_inputs());

        assert!(md.starts_with("# UFFS Benchmark Report — DRAFT (v9.9.9)"));
        assert!(md.contains("Suggested canonical name: `2023-11-v9.9.9-cd.md`"));
        assert!(md.contains("## At a glance\n\n<SUMMARY>"));
        assert!(md.contains("## Test environment\n\n<ENV>"));
        assert!(md.contains("## Storage devices\n\n<STORAGE>"));
        assert!(md.contains("## Negotiated matrix\n\n<MATRIX>"));
        assert!(md.contains("## Everything RAM budget\n\n<ESBUDGET>"));
        assert!(md.contains("```csv\ntool,rows\nuffs,5\n```"));
        assert!(md.contains("## Per-drive parity (§2)\n\n_Raw log: `parity.txt`._\n\n<PARITY>"));
        assert!(md.contains("## Full-suite (§3)\n\n_Raw log: `full-suite.txt`._\n\n<FULL>"));
        // Static pattern matrix is always present, with the es full-scan note.
        assert!(md.contains("## Patterns under test"));
        assert!(md.contains("~2 GB"));
        // Baseline comparison embeds when provided.
        assert!(md.contains("## vs baseline (last canonical report)\n\n<BASE>"));
        assert!(md.ends_with('\n'));
    }

    #[test]
    fn full_suite_csv_renders_as_table_over_raw_txt() {
        let csv = "tool,version,phase,sink,drive,pattern,rows,p50_ms,p95_ms,stddev_ms,rounds,verdict,notes\n\
                   uffs,uffs 0.5.120,hot,count,C,all_dlls,166684,24.0,48.0,7.3,10,ok,\n\
                   uffs,uffs 0.5.120,hot,count,D,full_scan,7066038,65.0,197.0,39.8,10,ok,\n";
        let inputs = ReportInputs {
            full_suite_csv: Some(csv.to_owned()),
            ..sample_inputs()
        };
        let md = render(&inputs);

        // Proper table rows with thousands separators, not the raw [ok] lines.
        assert!(md.contains("| C: | all_dlls | 166,684 | 24.0 ms | 48.0 ms | 7.3 ms | 10 | ok |"));
        assert!(
            md.contains("| D: | full_scan | 7,066,038 | 65.0 ms | 197.0 ms | 39.8 ms | 10 | ok |")
        );
        // The raw-txt fallback body must NOT be used when the CSV renders.
        assert!(!md.contains("<FULL>"));
    }

    #[test]
    fn render_marks_absent_sections() {
        let inputs = ReportInputs {
            summary_md: None,
            env_md: None,
            matrix_md: None,
            storage_md: None,
            es_budget_md: None,
            cross_tool_csv: None,
            parity_txt: None,
            full_suite_txt: None,
            full_suite_csv: None,
            baseline_md: None,
            charts_md: None,
            ..sample_inputs()
        };
        let md = render(&inputs);

        assert!(md.contains("## At a glance\n\n_Not produced this run._"));
        assert!(md.contains("## Test environment\n\n_Not produced this run._"));
        assert!(md.contains("## Storage devices\n\n_Not produced this run._"));
        assert!(md.contains("## Negotiated matrix\n\n_Not produced this run._"));
        assert!(md.contains("## Everything RAM budget\n\n_Not produced this run._"));
        assert!(md.contains("`cross-tool-summary.csv` — not produced this run."));
        assert!(md.contains("`parity.txt` — not produced this run."));
        assert!(md.contains("`full-suite.txt` — not produced this run."));
    }

    #[test]
    fn assemble_reads_bundle_and_writes_single_draft() {
        let dir = "/bundle";
        let matrix = Matrix {
            capable_drives: vec!['C'],
            cross_cells: vec![CrossCell {
                drive: 'C',
                pattern: "all_dlls".to_owned(),
            }],
            uffs_only: Vec::<SoloCell>::new(),
        };
        let matrix_json = serde_json::to_vec(&matrix).expect("serialize matrix");
        let host = MockHost::new()
            .with_now(fixed_now())
            .with_file(format!("{dir}/env.md"), "## Test environment\n\nbody\n")
            .with_file(format!("{dir}/matrix.json"), matrix_json)
            .with_file(
                format!("{dir}/cross-tool-summary.csv"),
                "tool,rows\nuffs,5\n",
            )
            .with_file(format!("{dir}/parity.txt"), "parity body\n")
            .with_file(format!("{dir}/full-suite.txt"), "full body\n")
            .with_file(
                format!("{dir}/drives.json"),
                "[{\"drive\":\"C\",\"boot\":true,\"label\":\"OS\",\"drive_type\":\"NVMe\",\
                 \"total_bytes\":1099511627776,\"used_pct\":50.0,\"mft_records\":1000},\
                 {\"drive\":\"E\",\"boot\":false,\"label\":\"DATA\",\"drive_type\":\"HDD\",\
                 \"total_bytes\":2199023255552,\"used_pct\":90.0,\"mft_records\":2000}]",
            );

        let path = assemble(&host, Path::new(dir), "9.9.9", "cd").expect("assemble draft");

        assert_eq!(path, PathBuf::from(format!("{dir}/{REPORT_DRAFT}")));
        // Exactly one write — the draft — and it lands inside the bundle.
        let writes: Vec<Call> = host
            .calls()
            .into_iter()
            .filter(|call| matches!(call, Call::WriteFile(_)))
            .collect();
        assert_eq!(writes, vec![Call::WriteFile(path.clone())]);

        let bytes = host.file(&path).expect("draft written");
        let md = String::from_utf8(bytes).expect("utf8 draft");
        assert!(md.contains("Suggested canonical name: `2023-11-v9.9.9-cd.md`"));
        assert!(md.contains("## Test environment\n\nbody"));
        // Negotiated matrix summarizes cross-tool cells by count now.
        assert!(md.contains("**Cross-tool cells:** 1"));
        assert!(md.contains("```csv\ntool,rows\nuffs,5\n```"));
        // Storage devices: C is the matrix's only capable drive → flagged ✓;
        // E is listed but not benched.
        assert!(md.contains("## Storage devices"));
        assert!(md.contains("| C: (boot) | NVMe | OS | 1.0 TB | 50.0% | 1,000 | ✓ |"));
        assert!(md.contains("| E: | HDD | DATA | 2.0 TB | 90.0% | 2,000 |  |"));
    }

    #[test]
    fn assemble_with_empty_bundle_writes_missing_markers() {
        let host = MockHost::new();

        let path = assemble(&host, Path::new("/empty"), "1.0.0", "full").expect("assemble draft");

        let md = String::from_utf8(host.file(&path).expect("draft written")).expect("utf8 draft");
        assert!(md.contains("## Test environment\n\n_Not produced this run._"));
        assert!(md.contains("`full-suite.txt` — not produced this run."));
    }
}
