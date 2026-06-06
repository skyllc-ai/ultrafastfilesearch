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

/// Bundle-relative name of the assembled report draft (plan §11).
pub const REPORT_DRAFT: &str = "REPORT-DRAFT.md";

/// Bundle-relative name of the rendered environment table (Stage 0a).
const ENV_MD: &str = "env.md";
/// Bundle-relative name of the negotiated matrix (Stage 0d).
const MATRIX_JSON: &str = "matrix.json";
/// Bundle-relative name of the Stage 1 cross-tool summary CSV.
const CROSS_TOOL_CSV: &str = "cross-tool-summary.csv";
/// Bundle-relative name of the Stage 2 parity transcript.
const PARITY_TXT: &str = "parity.txt";
/// Bundle-relative name of the Stage 3 full-suite transcript.
const FULL_SUITE_TXT: &str = "full-suite.txt";

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
    /// Rendered Stage 0a environment markdown (`env.md`), if present.
    pub env_md: Option<String>,
    /// Rendered Stage 0d matrix markdown (from `matrix.json`), if present.
    pub matrix_md: Option<String>,
    /// Stage 1 cross-tool summary CSV contents, if present.
    pub cross_tool_csv: Option<String>,
    /// Stage 2 parity transcript contents, if present.
    pub parity_txt: Option<String>,
    /// Stage 3 full-suite transcript contents, if present.
    pub full_suite_txt: Option<String>,
}

/// Suggested canonical `YYYY-MM-vX.Y.Z-<scope>.md` promotion name.
fn promotion_name(generated_at: DateTime<Utc>, version: &str, scope: &str) -> String {
    format!(
        "{year:04}-{month:02}-v{version}-{scope}.md",
        year = generated_at.year(),
        month = generated_at.month(),
    )
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
        format!(
            "- **Suite version:** v{}\n- **Generated:** {}\n- **Scope:** {}",
            inputs.version,
            inputs.generated_at.format("%Y-%m-%d %H:%M:%S UTC"),
            inputs.scope,
        ),
        embedded("## Test environment", inputs.env_md.as_ref()),
        embedded("## Negotiated matrix", inputs.matrix_md.as_ref()),
        fenced(
            "## Cross-tool head-to-head (§1)",
            CROSS_TOOL_CSV,
            "csv",
            inputs.cross_tool_csv.as_ref(),
        ),
        inlined(
            "## Per-drive parity (§2)",
            PARITY_TXT,
            inputs.parity_txt.as_ref(),
        ),
        inlined(
            "## Full-suite (§3)",
            FULL_SUITE_TXT,
            inputs.full_suite_txt.as_ref(),
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

/// Assemble the bundle into `bundle_dir/REPORT-DRAFT.md` and return its path.
///
/// Reads the Stage 0/1/2/3 artifacts already in the bundle through the [`Host`]
/// seam, renders the draft with [`render`], and writes it back into the bundle.
/// Only the bundle is touched; host state is left untouched.
///
/// # Errors
/// Returns an error if the draft cannot be written into the bundle.
pub fn assemble(host: &dyn Host, bundle_dir: &Path, version: &str, scope: &str) -> Result<PathBuf> {
    let inputs = ReportInputs {
        version: version.to_owned(),
        scope: scope.to_owned(),
        generated_at: host.now(),
        env_md: load(host, bundle_dir, ENV_MD),
        matrix_md: load_matrix_md(host, bundle_dir),
        cross_tool_csv: load(host, bundle_dir, CROSS_TOOL_CSV),
        parity_txt: load(host, bundle_dir, PARITY_TXT),
        full_suite_txt: load(host, bundle_dir, FULL_SUITE_TXT),
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
            env_md: Some("## Test environment\n\n<ENV>".to_owned()),
            matrix_md: Some("## Negotiated matrix\n\n<MATRIX>".to_owned()),
            cross_tool_csv: Some("tool,rows\nuffs,5\n".to_owned()),
            parity_txt: Some("<PARITY>".to_owned()),
            full_suite_txt: Some("<FULL>".to_owned()),
        }
    }

    #[test]
    fn render_embeds_artifacts_and_promotion_name() {
        let md = render(&sample_inputs());

        assert!(md.starts_with("# UFFS Benchmark Report — DRAFT (v9.9.9)"));
        assert!(md.contains("Suggested canonical name: `2023-11-v9.9.9-cd.md`"));
        assert!(md.contains("## Test environment\n\n<ENV>"));
        assert!(md.contains("## Negotiated matrix\n\n<MATRIX>"));
        assert!(md.contains("```csv\ntool,rows\nuffs,5\n```"));
        assert!(md.contains("## Per-drive parity (§2)\n\n_Raw log: `parity.txt`._\n\n<PARITY>"));
        assert!(md.contains("## Full-suite (§3)\n\n_Raw log: `full-suite.txt`._\n\n<FULL>"));
        assert!(md.ends_with('\n'));
    }

    #[test]
    fn render_marks_absent_sections() {
        let inputs = ReportInputs {
            env_md: None,
            matrix_md: None,
            cross_tool_csv: None,
            parity_txt: None,
            full_suite_txt: None,
            ..sample_inputs()
        };
        let md = render(&inputs);

        assert!(md.contains("## Test environment\n\n_Not produced this run._"));
        assert!(md.contains("## Negotiated matrix\n\n_Not produced this run._"));
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
            .with_file(format!("{dir}/full-suite.txt"), "full body\n");

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
        assert!(md.contains("`C:` all_dlls"));
        assert!(md.contains("```csv\ntool,rows\nuffs,5\n```"));
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
