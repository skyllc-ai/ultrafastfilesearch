// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! GitHub Releases fetch + asset download (blocking `reqwest` + rustls).
//!
//! One-shot HTTP for the acquire step — a release lookup plus streaming
//! asset downloads. TLS is rustls with the system trust store; we never
//! follow off-host redirects beyond what `reqwest` validates against the
//! pinned `api.github.com` / release host.

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// User-agent GitHub requires for API requests.
const USER_AGENT: &str = concat!("uffs-update/", env!("CARGO_PKG_VERSION"));

/// A GitHub release (only the fields we use).
#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    /// The release tag (e.g. `v0.6.2`).
    pub(crate) tag_name: String,
    /// Downloadable assets attached to the release.
    pub(crate) assets: Vec<Asset>,
}

/// One downloadable release asset.
#[derive(Debug, Deserialize)]
pub(crate) struct Asset {
    /// Asset file name (e.g. `uffs-windows-x64.zip`).
    pub(crate) name: String,
    /// Direct download URL.
    pub(crate) browser_download_url: String,
}

impl Release {
    /// Find an asset by exact file name.
    pub(crate) fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// Build a blocking client with the required user agent.
fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("building HTTP client")
}

/// Fetch a release from `owner/repo`: the `latest` release, or the
/// specific `tag` when given.
///
/// # Errors
///
/// Propagates HTTP, status, and JSON-decode failures.
pub(crate) fn fetch_release(repo: &str, tag: Option<&str>) -> Result<Release> {
    let url = tag.map_or_else(
        || format!("https://api.github.com/repos/{repo}/releases/latest"),
        |wanted| format!("https://api.github.com/repos/{repo}/releases/tags/{wanted}"),
    );
    let response = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("GitHub returned an error for {url}"))?;
    response.json::<Release>().context("parsing release JSON")
}

/// Stream an asset URL to `dest`.
///
/// # Errors
///
/// Propagates HTTP, status, and file-write failures.
pub(crate) fn download_to(url: &str, dest: &Path) -> Result<()> {
    let mut response = client()?
        .get(url)
        .send()
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    std::io::copy(&mut response, &mut file)
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}
