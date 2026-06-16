// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Acquire orchestration (self-update Phase C).
//!
//! Fetch the target GitHub release, download the platform bundle plus its
//! `SHA256SUMS`, verify the bundle's SHA-256, and leave the verified
//! bundle staged. Extraction, Authenticode, stopping services, and the
//! actual replace are the **apply** phase's job — acquire never touches a
//! live install.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use crate::{github, verify};

/// Inputs for one acquire run.
pub(crate) struct AcquirePlan {
    /// GitHub `owner/repo` to fetch from.
    pub(crate) repo: String,
    /// Release tag (e.g. `v0.6.2`), or `None` for the latest release.
    pub(crate) tag: Option<String>,
    /// Directory to stage downloads into.
    pub(crate) stage: PathBuf,
    /// Platform bundle asset name.
    pub(crate) bundle: String,
    /// Checksums asset name.
    pub(crate) sums: String,
}

impl AcquirePlan {
    /// Default bundle asset name for the current OS/arch.
    pub(crate) const fn default_bundle() -> &'static str {
        if cfg!(windows) {
            "uffs-windows-x64.zip"
        } else if cfg!(target_os = "macos") {
            "uffs-macos-arm64.tar.gz"
        } else {
            "uffs-linux-x64.tar.gz"
        }
    }
}

/// Run the acquire and return the path to the verified, staged bundle.
///
/// # Errors
///
/// Fails on network/HTTP errors, a missing asset, a missing checksum
/// entry, or a SHA-256 mismatch (in which case nothing is left trusted).
pub(crate) fn run(plan: &AcquirePlan) -> Result<PathBuf> {
    std::fs::create_dir_all(&plan.stage)
        .with_context(|| format!("creating stage dir {}", plan.stage.display()))?;

    let release = github::fetch_release(&plan.repo, plan.tag.as_deref())?;
    let bundle_url = release
        .asset(&plan.bundle)
        .with_context(|| {
            format!(
                "release {} has no asset named {}",
                release.tag_name, plan.bundle
            )
        })?
        .browser_download_url
        .clone();
    let sums_url = release
        .asset(&plan.sums)
        .with_context(|| {
            format!(
                "release {} has no asset named {}",
                release.tag_name, plan.sums
            )
        })?
        .browser_download_url
        .clone();

    let bundle_path = plan.stage.join(&plan.bundle);
    let sums_path = plan.stage.join(&plan.sums);
    github::download_to(&sums_url, &sums_path)?;
    github::download_to(&bundle_url, &bundle_path)?;

    let sums_text = std::fs::read_to_string(&sums_path)
        .with_context(|| format!("reading {}", sums_path.display()))?;
    let sums = verify::parse_sha256sums(&sums_text);
    let expected = verify::expected_hash(&sums, &plan.bundle)
        .with_context(|| format!("{} is not listed in {}", plan.bundle, plan.sums))?;

    if !verify::verify_sha256(&bundle_path, expected)? {
        // Remove the unverified download so it can never be mistaken for trusted.
        let _ignore = std::fs::remove_file(&bundle_path);
        bail!(
            "SHA-256 mismatch for {} — download rejected, nothing staged",
            plan.bundle
        );
    }
    Ok(bundle_path)
}
