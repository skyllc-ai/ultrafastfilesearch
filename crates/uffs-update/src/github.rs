// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! GitHub Releases fetch + asset download (blocking `reqwest` + rustls).
//!
//! One-shot HTTP for the acquire step — a release lookup plus streaming
//! asset downloads. TLS is rustls with the system trust store; we never
//! follow off-host redirects beyond what `reqwest` validates against the
//! pinned `api.github.com` / release host.

use core::time::Duration;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

/// User-agent GitHub requires for API requests.
const USER_AGENT: &str = concat!("uffs-update/", env!("CARGO_PKG_VERSION"));

/// Cap on how long we wait to establish a TCP/TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-operation (connect/read/write) inactivity cap. On the blocking
/// client this is a socket-level read/write timeout, so a stalled socket
/// is killed without bounding the total time of a large download.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Total attempts (initial + retries) for a transient HTTP failure.
const MAX_ATTEMPTS: u32 = 4;

/// Base back-off; the delay before attempt *n* is `BASE_BACKOFF * 2^(n-1)`.
const BASE_BACKOFF: Duration = Duration::from_millis(500);

/// Hard ceiling on a single downloaded asset, defending the disk against
/// a truncated, malicious, or runaway response. Our largest binary is a
/// few tens of MiB; 512 MiB is generous head-room.
const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// Streaming copy buffer size.
const CHUNK_BYTES: usize = 64 * 1024;

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

/// Build a blocking client with the required user agent and the connect
/// + read timeouts (a hung socket can never wedge an update forever).
fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(READ_TIMEOUT)
        .build()
        .context("building HTTP client")
}

/// Whether a `reqwest` failure is worth retrying: a connect/read timeout
/// or a server-side (5xx / 429) status. Client (4xx) errors and decode
/// errors are deterministic, so they fail fast.
fn is_retryable(err: &reqwest::Error) -> bool {
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    err.status()
        .is_some_and(|status| status.as_u16() == 429 || status.is_server_error())
}

/// Run `op` with bounded exponential back-off, retrying only transient
/// failures (see [`is_retryable`]). `label` describes the operation for
/// the final error context.
fn with_retry<T, F>(label: &str, mut op: F) -> Result<T>
where
    F: FnMut() -> reqwest::Result<T>,
{
    let mut attempt: u32 = 1;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) if attempt < MAX_ATTEMPTS && is_retryable(&err) => {
                let backoff = BASE_BACKOFF * 2_u32.pow(attempt.saturating_sub(1));
                std::thread::sleep(backoff);
                attempt = attempt.saturating_add(1);
            }
            Err(err) => {
                return Err(err).with_context(|| format!("{label} (after {attempt} attempt(s))"));
            }
        }
    }
}

/// Stream `reader` into `writer`, aborting if the total exceeds `cap`.
/// Returns the number of bytes written.
fn copy_capped<R: Read, W: Write>(reader: &mut R, writer: &mut W, cap: u64) -> Result<u64> {
    let mut buf = vec![0_u8; CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = reader.read(&mut buf).context("reading response body")?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > cap {
            bail!("asset exceeds the {cap}-byte cap — aborting download");
        }
        let chunk = buf.get(..read).context("response chunk out of range")?;
        writer.write_all(chunk).context("writing to disk")?;
    }
    Ok(total)
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
    let client = client()?;
    let response = with_retry(&format!("requesting {url}"), || {
        client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()?
            .error_for_status()
    })?;
    response.json::<Release>().context("parsing release JSON")
}

/// Stream an asset URL to `dest`.
///
/// # Errors
///
/// Propagates HTTP, status, and file-write failures.
pub(crate) fn download_to(url: &str, dest: &Path) -> Result<()> {
    let client = client()?;
    let mut response = with_retry(&format!("downloading {url}"), || {
        client.get(url).send()?.error_for_status()
    })?;
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    copy_capped(&mut response, &mut file, MAX_ASSET_BYTES)
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::copy_capped;

    #[test]
    fn copy_capped_writes_all_under_cap() {
        let src = vec![7_u8; 200];
        let mut reader = src.as_slice();
        let mut sink: Vec<u8> = Vec::new();
        let written = copy_capped(&mut reader, &mut sink, 1024).expect("under cap copies");
        assert_eq!(written, 200);
        assert_eq!(sink, src);
    }

    #[test]
    fn copy_capped_aborts_over_cap() {
        let src = vec![0_u8; 4096];
        let mut reader = src.as_slice();
        let mut sink: Vec<u8> = Vec::new();
        let err = copy_capped(&mut reader, &mut sink, 100).expect_err("over cap must abort");
        assert!(err.to_string().contains("cap"), "unexpected: {err}");
    }

    #[test]
    fn copy_capped_handles_empty_body() {
        let mut reader: &[u8] = &[];
        let mut sink: Vec<u8> = Vec::new();
        let written = copy_capped(&mut reader, &mut sink, 100).expect("empty copies");
        assert_eq!(written, 0);
        assert!(sink.is_empty());
    }
}
