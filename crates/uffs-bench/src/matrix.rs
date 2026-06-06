// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage 0d — drive/pattern matrix negotiation ("match the weakest tool").
//!
//! A cross-tool cell `(drive, pattern)` is only fair to time for *all* required
//! tools when *every* required tool can serve it. Everything else still runs as
//! a clearly-labelled **UFFS-only** cell (with a per-cell reason), so coverage
//! is never silently dropped. [`compute_matrix`] implements execution-plan
//! §8.4; [`render_md`] formats the negotiated matrix for the 0e plan gate and
//! the report; [`write()`] persists it to `bundle/matrix.json`.
//!
//! The only competitor that constrains drives today is Everything: it can serve
//! a drive only when its index is `loaded` *and* `hot`, and a `(drive,
//! pattern)` cell only when the estimated row count is under its IPC ceiling
//! ([`crate::preflight::ES_IPC_ROW_CEILING`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{BenchError, Result};
use crate::host::Host;
use crate::preflight::{CellFeasibility, DrivePreflight, PreflightResult};

/// Tool id of the competitor that constrains the cross-tool matrix.
pub const EVERYTHING_TOOL: &str = "everything";

/// Inputs that scope a matrix negotiation.
#[derive(Debug, Clone, Default)]
pub struct MatrixSpec {
    /// Tool ids that must all be able to serve a cell for it to be cross-tool.
    pub required_tools: Vec<String>,
    /// Drives the operator asked for, in display order.
    pub candidate_drives: Vec<char>,
    /// Pattern names to negotiate, in display order.
    pub patterns: Vec<String>,
}

/// An apples-to-apples cell timed for every required tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossCell {
    /// Drive letter the cell is for.
    pub drive: char,
    /// Pattern name the cell is for.
    pub pattern: String,
}

/// A cell measured for UFFS only, with the reason competitors were excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoloCell {
    /// Drive letter the cell is for.
    pub drive: char,
    /// Pattern name the cell is for.
    pub pattern: String,
    /// Why no competitor could serve this cell (per-cell, operator-facing).
    pub reason: String,
}

/// The negotiated matrix, serialized to `bundle/matrix.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matrix {
    /// Drives every required tool can serve, in candidate order.
    pub capable_drives: Vec<char>,
    /// Apples-to-apples cells counted in the head-to-head.
    pub cross_cells: Vec<CrossCell>,
    /// UFFS-only cells reported separately, each with a reason.
    pub uffs_only: Vec<SoloCell>,
}

/// Find a drive's preflight record by letter.
fn find_drive(drives: &[DrivePreflight], drive: char) -> Option<&DrivePreflight> {
    drives.iter().find(|state| state.drive == drive)
}

/// Find a `(drive, pattern)` feasibility cell.
fn find_cell<'a>(
    cells: &'a [CellFeasibility],
    drive: char,
    pattern: &str,
) -> Option<&'a CellFeasibility> {
    cells
        .iter()
        .find(|cell| cell.drive == drive && cell.pattern == pattern)
}

/// Whether Everything can serve `drive` (index loaded *and* hot).
fn everything_serves(preflight: &PreflightResult, drive: char) -> bool {
    find_drive(&preflight.drives, drive).is_some_and(|state| state.loaded && state.hot)
}

/// Explain why competitors are excluded from a `(drive, pattern)` cell.
///
/// Only reached when Everything is required and the cell is not cross-tool, so
/// the cause is one of: drive not configured, configured-but-not-ready, or the
/// row estimate exceeding the IPC ceiling.
fn solo_reason(preflight: &PreflightResult, drive: char, pattern: &str) -> String {
    match find_drive(&preflight.drives, drive) {
        None
        | Some(DrivePreflight {
            configured: false, ..
        }) => format!("{drive}: es not loaded (not configured in Everything.ini)"),
        Some(state) if !(state.loaded && state.hot) => {
            format!("{drive}: es not loaded (configured but still indexing)")
        }
        Some(_) => {
            let est = find_cell(&preflight.cells, drive, pattern).map_or(0, |cell| cell.est_rows);
            let ceiling = crate::preflight::ES_IPC_ROW_CEILING;
            format!("{pattern}: es infeasible (est {est} rows > {ceiling} IPC ceiling)")
        }
    }
}

/// Negotiate the cross-tool vs UFFS-only matrix (execution-plan §8.4).
///
/// `capable_drives` is the candidate set intersected with every required tool's
/// servable set (only Everything constrains it today). A cell is cross-tool
/// when its drive is capable and Everything finds it feasible; otherwise it is
/// UFFS-only with a per-cell reason.
#[must_use]
pub fn compute_matrix(spec: &MatrixSpec, preflight: &PreflightResult) -> Matrix {
    let everything_required = spec
        .required_tools
        .iter()
        .any(|tool| tool == EVERYTHING_TOOL);

    let capable_drives: Vec<char> = spec
        .candidate_drives
        .iter()
        .copied()
        .filter(|&drive| !everything_required || everything_serves(preflight, drive))
        .collect();

    let mut cross_cells = Vec::new();
    let mut uffs_only = Vec::new();
    for &drive in &spec.candidate_drives {
        let capable = capable_drives.contains(&drive);
        for pattern in &spec.patterns {
            let feasible = !everything_required
                || find_cell(&preflight.cells, drive, pattern).is_some_and(|cell| cell.es_feasible);
            if capable && feasible {
                cross_cells.push(CrossCell {
                    drive,
                    pattern: pattern.clone(),
                });
            } else {
                uffs_only.push(SoloCell {
                    drive,
                    pattern: pattern.clone(),
                    reason: solo_reason(preflight, drive, pattern),
                });
            }
        }
    }
    Matrix {
        capable_drives,
        cross_cells,
        uffs_only,
    }
}

/// Render the negotiated matrix as markdown for the 0e gate and the report.
///
/// A pure function of its input (no host access), so it is covered by a golden
/// test.
#[must_use]
pub fn render_md(matrix: &Matrix) -> String {
    let capable = if matrix.capable_drives.is_empty() {
        "_none_".to_owned()
    } else {
        matrix
            .capable_drives
            .iter()
            .map(char::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let cross = if matrix.cross_cells.is_empty() {
        "_none_".to_owned()
    } else {
        matrix
            .cross_cells
            .iter()
            .map(|cell| format!("- `{}:` {}", cell.drive, cell.pattern))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let solo = if matrix.uffs_only.is_empty() {
        "_none_".to_owned()
    } else {
        matrix
            .uffs_only
            .iter()
            .map(|cell| format!("- `{}:` {} — {}", cell.drive, cell.pattern, cell.reason))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "## Negotiated matrix\n\n\
         - **Capable drives (all tools):** {capable}\n\
         \n### Cross-tool cells (head-to-head)\n\n\
         {cross}\n\
         \n### UFFS-only cells\n\n\
         {solo}\n"
    )
}

/// Serialize `matrix` to `bundle_dir/matrix.json`.
///
/// # Errors
/// Returns an error if serialization fails or the file cannot be written.
pub fn write(host: &dyn Host, matrix: &Matrix, bundle_dir: &Path) -> Result<()> {
    let json = serde_json::to_vec_pretty(matrix)?;
    let path = bundle_dir.join("matrix.json");
    host.write_file(&path, &json)
        .map_err(|err| BenchError::io(&path, err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Matrix, MatrixSpec, compute_matrix, render_md, write};
    use crate::host::{Call, MockHost};
    use crate::preflight::{CellFeasibility, DrivePreflight, PreflightResult};

    /// A hot, loaded, configured drive record.
    fn hot_drive(drive: char, record_count: u64) -> DrivePreflight {
        DrivePreflight {
            drive,
            configured: true,
            loaded: true,
            hot: true,
            record_count,
        }
    }

    /// A feasibility cell.
    fn cell(drive: char, pattern: &str, est_rows: u64, es_feasible: bool) -> CellFeasibility {
        CellFeasibility {
            drive,
            pattern: pattern.to_owned(),
            est_rows,
            es_feasible,
        }
    }

    /// The default three-tool requirement (`uffs`, `uffs_cpp`, `everything`).
    fn three_tools() -> Vec<String> {
        vec![
            "uffs".to_owned(),
            "uffs_cpp".to_owned(),
            "everything".to_owned(),
        ]
    }

    #[test]
    fn everything_only_serves_loaded_hot_drives() {
        // Everything holds only C and D; the operator asked for C,D,E,F,M,S.
        let preflight = PreflightResult {
            drives: vec![hot_drive('C', 1000), hot_drive('D', 2000), DrivePreflight {
                drive: 'E',
                configured: false,
                loaded: false,
                hot: false,
                record_count: 0,
            }],
            cells: vec![
                cell('C', "all_dlls", 500, true),
                cell('D', "all_dlls", 600, true),
            ],
        };
        let spec = MatrixSpec {
            required_tools: three_tools(),
            candidate_drives: vec!['C', 'D', 'E', 'F', 'M', 'S'],
            patterns: vec!["all_dlls".to_owned()],
        };

        let matrix = compute_matrix(&spec, &preflight);

        assert_eq!(matrix.capable_drives, vec!['C', 'D']);
        assert_eq!(
            matrix
                .cross_cells
                .iter()
                .map(|cell| cell.drive)
                .collect::<Vec<_>>(),
            vec!['C', 'D']
        );
        // E/F/M/S all land in UFFS-only with an "es not loaded" reason.
        let solo_drives: Vec<char> = matrix.uffs_only.iter().map(|cell| cell.drive).collect();
        assert_eq!(solo_drives, vec!['E', 'F', 'M', 'S']);
        assert!(
            matrix
                .uffs_only
                .iter()
                .all(|cell| cell.reason.contains("es not loaded"))
        );
    }

    #[test]
    fn loaded_drive_over_ceiling_is_uffs_only() {
        let preflight = PreflightResult {
            drives: vec![hot_drive('C', 1000)],
            cells: vec![cell('C', "full_scan", 9_000_000, false)],
        };
        let spec = MatrixSpec {
            required_tools: three_tools(),
            candidate_drives: vec!['C'],
            patterns: vec!["full_scan".to_owned()],
        };

        let matrix = compute_matrix(&spec, &preflight);

        assert!(matrix.cross_cells.is_empty());
        assert_eq!(matrix.uffs_only.len(), 1);
        let reason = &matrix.uffs_only.first().expect("one solo cell").reason;
        assert!(reason.contains("es infeasible"));
        assert!(reason.contains("9000000"));
    }

    #[test]
    fn configured_but_indexing_drive_is_uffs_only() {
        let preflight = PreflightResult {
            drives: vec![DrivePreflight {
                drive: 'C',
                configured: true,
                loaded: false,
                hot: false,
                record_count: 0,
            }],
            cells: Vec::new(),
        };
        let spec = MatrixSpec {
            required_tools: three_tools(),
            candidate_drives: vec!['C'],
            patterns: vec!["all_dlls".to_owned()],
        };

        let matrix = compute_matrix(&spec, &preflight);

        assert!(matrix.capable_drives.is_empty());
        assert!(
            matrix
                .uffs_only
                .first()
                .expect("one solo cell")
                .reason
                .contains("still indexing")
        );
    }

    #[test]
    fn without_everything_every_cell_is_cross_tool() {
        let preflight = PreflightResult {
            drives: Vec::new(),
            cells: Vec::new(),
        };
        let spec = MatrixSpec {
            required_tools: vec!["uffs".to_owned(), "uffs_cpp".to_owned()],
            candidate_drives: vec!['C', 'E'],
            patterns: vec!["all_dlls".to_owned()],
        };

        let matrix = compute_matrix(&spec, &preflight);

        assert_eq!(matrix.capable_drives, vec!['C', 'E']);
        assert_eq!(matrix.cross_cells.len(), 2);
        assert!(matrix.uffs_only.is_empty());
    }

    #[test]
    fn render_md_lists_capable_cross_and_solo() {
        let matrix = Matrix {
            capable_drives: vec!['C', 'D'],
            cross_cells: vec![super::CrossCell {
                drive: 'C',
                pattern: "all_dlls".to_owned(),
            }],
            uffs_only: vec![super::SoloCell {
                drive: 'E',
                pattern: "all_dlls".to_owned(),
                reason: "E: es not loaded (not configured in Everything.ini)".to_owned(),
            }],
        };

        let md = render_md(&matrix);

        assert!(md.contains("**Capable drives (all tools):** C, D"));
        assert!(md.contains("- `C:` all_dlls"));
        assert!(md.contains("E: es not loaded"));
    }

    #[test]
    fn write_emits_matrix_json_and_round_trips() {
        let host = MockHost::new();
        let matrix = Matrix {
            capable_drives: vec!['C'],
            cross_cells: Vec::new(),
            uffs_only: Vec::new(),
        };
        let dir = PathBuf::from("/bundle");

        write(&host, &matrix, &dir).expect("write matrix json");

        let path = dir.join("matrix.json");
        assert_eq!(host.calls(), vec![Call::WriteFile(path.clone())]);
        let json = host.file(&path).expect("matrix json written");
        let parsed: Matrix = serde_json::from_slice(&json).expect("valid json");
        assert_eq!(parsed, matrix);
    }
}
