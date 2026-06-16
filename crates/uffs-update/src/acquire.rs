// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Acquire (self-update Phase C) — **per-binary**, no archive.
//!
//! For each binary the install actually has, download that binary as an
//! individual release asset (`uffsd.exe`, `uffs.exe`, …) and verify its
//! SHA-256 against the release's `SHA256SUMS`, straight into the staging
//! dir — which is exactly the loose-binary layout the apply phase reads.
//!
//! Why per-binary, not a bundle zip: it needs **no in-process archive
//! crate** (zip/tar pull unaudited deps and, shelled, aren't on every
//! Windows), each binary is **individually** SHA- and (at apply time)
//! Authenticode-verifiable, and we only fetch the installed subset.
//!
//! Requires the release to publish per-binary assets + `SHA256SUMS` (a
//! release-pipeline follow-up; the code is ready for it).

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use crate::orchestrate::{asset_name, exe_name};
use crate::{github, verify};

/// Inputs for one acquire run.
pub(crate) struct AcquirePlan {
    /// GitHub `owner/repo`.
    pub(crate) repo: String,
    /// Release tag (e.g. `v0.6.2`), or `None` for latest.
    pub(crate) tag: Option<String>,
    /// Directory to stage verified binaries into.
    pub(crate) stage: PathBuf,
    /// Checksums asset name.
    pub(crate) sums: String,
    /// Binary stems to fetch (e.g. `["uffs", "uffsd"]`).
    pub(crate) binaries: Vec<String>,
}

/// Download + SHA-verify every requested binary into the staging dir.
/// Returns the staged paths. Aborts (leaving nothing trusted) on a
/// missing asset, a missing checksum, or a SHA mismatch.
///
/// # Errors
///
/// Network/HTTP errors, missing assets/checksums, or hash mismatch.
pub(crate) fn run(plan: &AcquirePlan) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(&plan.stage)
        .with_context(|| format!("creating stage dir {}", plan.stage.display()))?;

    let release = github::fetch_release(&plan.repo, plan.tag.as_deref())?;

    // Checksums first.
    let sums_url = release
        .asset(&plan.sums)
        .with_context(|| format!("release {} has no asset {}", release.tag_name, plan.sums))?
        .browser_download_url
        .clone();
    let sums_path = plan.stage.join(&plan.sums);
    github::download_to(&sums_url, &sums_path)?;
    let sums_text = std::fs::read_to_string(&sums_path)
        .with_context(|| format!("reading {}", sums_path.display()))?;
    let sums = verify::parse_sha256sums(&sums_text);

    // Each binary as an individual asset. We download the platform-
    // suffixed release asset (`uffsd-windows-x64.exe`) but stage it under
    // the plain on-disk name (`uffsd.exe`) so the apply phase finds it.
    let mut staged = Vec::with_capacity(plan.binaries.len());
    for stem in &plan.binaries {
        let asset = asset_name(stem);
        let url = release
            .asset(&asset)
            .with_context(|| format!("release {} has no asset {asset}", release.tag_name))?
            .browser_download_url
            .clone();
        let dest = plan.stage.join(exe_name(stem));
        github::download_to(&url, &dest)?;

        let expected = verify::expected_hash(&sums, &asset)
            .with_context(|| format!("{asset} is not listed in {}", plan.sums))?;
        if !verify::verify_sha256(&dest, expected)? {
            let _ignore = std::fs::remove_file(&dest);
            bail!("SHA-256 mismatch for {asset} — download rejected, nothing staged");
        }
        staged.push(dest);
    }
    Ok(staged)
}
