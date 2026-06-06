// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Stage P8 competitor pinning: parse `competitors.toml`, fetch the pinned
//! `es.exe` artifact into the bundle, and SHA-256-verify it **fail-closed**.
//!
//! `competitors.toml` (in `scripts/windows/`) is the single source of truth for
//! the one pinned competitor version cited across the repo. [`fetch`] downloads
//! the artifact through the [`Host`] seam, verifies its SHA-256 against the
//! manifest, and—on any mismatch—**deletes the suspect download and refuses to
//! proceed**, so a competitor is never run from unverified bytes. voidtools
//! binaries are link-and-hash-pinned only (never redistributed in the repo).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::error::{BenchError, Result};
use crate::host::Host;
use crate::tooling::{Acquisition, Disposition};

/// Repo-relative path to the pinned-competitor manifest.
pub const MANIFEST_PATH: &str = "scripts/windows/competitors.toml";

/// Sentinel `es_sha256` value meaning "operator has not pinned a hash yet".
const PLACEHOLDER_SHA256: &str = "<fill-in>";

/// Fallback artifact name when a URL has no usable trailing path segment.
const FALLBACK_ARTIFACT: &str = "es-download";

/// Parsed `competitors.toml` (only the fields the orchestrator consumes;
/// unknown tables such as `[uffs_cpp]` are ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// The pinned Everything / `es.exe` competitor entry.
    pub everything: EverythingPin,
}

/// The pinned Everything competitor: version, download URL, and artifact hash.
#[derive(Debug, Clone, Deserialize)]
pub struct EverythingPin {
    /// Canonical Everything CLI version (the single pinned number).
    pub version: String,
    /// Upstream download URL for the `es.exe` distribution (link only).
    pub es_url: String,
    /// Hex-encoded SHA-256 of the downloaded artifact.
    pub es_sha256: String,
}

impl EverythingPin {
    /// Whether the operator has pinned a real upstream hash yet.
    ///
    /// An empty value or the [`PLACEHOLDER_SHA256`] sentinel means "unpinned":
    /// [`fetch`] refuses to download unverifiable bytes in that case.
    fn is_pinned(&self) -> bool {
        let hash = self.es_sha256.trim();
        !hash.is_empty() && hash != PLACEHOLDER_SHA256
    }
}

/// Parse a `competitors.toml` document from raw bytes.
///
/// # Errors
/// Returns [`BenchError::Provision`] if the bytes are not UTF-8 or not a valid
/// manifest.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let text = core::str::from_utf8(bytes)
        .map_err(|err| BenchError::Provision(format!("competitors.toml is not UTF-8: {err}")))?;
    toml::from_str(text)
        .map_err(|err| BenchError::Provision(format!("malformed competitors.toml: {err}")))
}

/// Read and parse the manifest at `path` through the host.
///
/// # Errors
/// Returns an error if the file cannot be read or parsed.
pub fn load_manifest(host: &dyn Host, path: &Path) -> Result<Manifest> {
    let bytes = host
        .read_file(path)
        .map_err(|err| BenchError::io(path, err))?;
    parse_manifest(&bytes)
}

/// Hex-encoded SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Derive the on-disk artifact file name from a download URL.
fn artifact_name(url: &str) -> &str {
    url.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(FALLBACK_ARTIFACT)
}

/// Download `url` to `dest` via `curl`, failing closed on a non-zero exit.
fn download(host: &dyn Host, url: &str, dest: &Path) -> Result<()> {
    let dest_str = dest.to_str().ok_or_else(|| {
        BenchError::Provision(format!("download path is not UTF-8: {}", dest.display()))
    })?;
    host.out(&format!("downloading {url} -> {dest_str}"));
    let output = host
        .run("curl", &["-fsSL", "-o", dest_str, url])
        .map_err(|err| BenchError::io(dest, err))?;
    if output.success() {
        Ok(())
    } else {
        Err(BenchError::Provision(format!(
            "download of {url} failed (exit {:?}): {}",
            output.code,
            output.stderr.trim()
        )))
    }
}

/// Fetch + SHA-256-verify the pinned competitor into `<bundle>/tools/`.
///
/// On a hash mismatch the suspect download is deleted and a
/// [`BenchError::Provision`] is returned (fail-closed). On success the verified
/// [`Acquisition`] is returned for the caller to persist in `state.json`.
///
/// # Errors
/// Returns [`BenchError::Provision`] if the manifest is unpinned, the download
/// fails, or the SHA-256 does not match; or an I/O error on directory/file ops.
pub fn fetch(
    host: &dyn Host,
    manifest: &Manifest,
    bundle_dir: &Path,
    disposition: Disposition,
) -> Result<Acquisition> {
    let pin = &manifest.everything;
    if !pin.is_pinned() {
        return Err(BenchError::Provision(
            "es_sha256 is unpinned in competitors.toml; refusing to fetch unverifiable bytes"
                .to_owned(),
        ));
    }
    let tools_dir: PathBuf = bundle_dir.join("tools");
    host.create_dir_all(&tools_dir)
        .map_err(|err| BenchError::io(&tools_dir, err))?;
    let name = artifact_name(&pin.es_url);
    let dest = tools_dir.join(name);

    download(host, &pin.es_url, &dest)?;

    let bytes = host
        .read_file(&dest)
        .map_err(|err| BenchError::io(&dest, err))?;
    let computed = sha256_hex(&bytes);
    if !computed.eq_ignore_ascii_case(pin.es_sha256.trim()) {
        // Fail closed: never leave unverified bytes where a later stage runs them.
        if let Err(err) = host.remove_file(&dest) {
            host.out(&format!(
                "warning: could not delete unverified download {}: {err}",
                dest.display()
            ));
        }
        return Err(BenchError::Provision(format!(
            "SHA-256 mismatch for {name}: expected {}, got {computed}",
            pin.es_sha256.trim()
        )));
    }
    Ok(Acquisition::new(
        host,
        name,
        dest,
        pin.es_url.clone(),
        computed,
        disposition,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Manifest, fetch, parse_manifest, sha256_hex};
    use crate::error::BenchError;
    use crate::host::{Call, MockHost};
    use crate::tooling::Disposition;

    /// The verified artifact bytes used across the fetch tests.
    const ARTIFACT: &[u8] = b"pinned-es-artifact";

    /// Build a manifest TOML string pinning `es.zip` to `sha256`.
    fn manifest_toml(sha256: &str) -> String {
        format!(
            "[everything]\nversion = \"1.1.0.30\"\n\
             es_url = \"https://example.test/dir/es.zip\"\nes_sha256 = \"{sha256}\"\n\
             [uffs_cpp]\nversion = \"v0.4.x\"\nlocation = \"~/bin/uffs.com\"\n"
        )
    }

    /// Parse the canonical manifest shape (extra `[uffs_cpp]` table ignored).
    fn pinned_manifest() -> Manifest {
        parse_manifest(manifest_toml(&sha256_hex(ARTIFACT)).as_bytes())
            .expect("canonical manifest parses")
    }

    #[test]
    fn parses_manifest_and_ignores_extra_tables() {
        let manifest = pinned_manifest();
        assert_eq!(manifest.everything.version, "1.1.0.30");
        assert_eq!(
            manifest.everything.es_url,
            "https://example.test/dir/es.zip"
        );
    }

    #[test]
    fn fetch_records_verified_acquisition() {
        // The downloader is mocked: seed the artifact the (recorded) curl call
        // would have produced so the post-download verify reads real bytes.
        let dest = "/out/bench/tools/es.zip";
        let host = MockHost::new().with_file(dest, ARTIFACT.to_vec());

        let acq = fetch(
            &host,
            &pinned_manifest(),
            Path::new("/out/bench"),
            Disposition::Keep,
        )
        .expect("verified fetch succeeds");

        assert_eq!(acq.name, "es.zip");
        assert_eq!(acq.path, Path::new(dest));
        assert_eq!(acq.sha256, sha256_hex(ARTIFACT));
        assert_eq!(acq.disposition, Disposition::Keep);
        // The bundle tools dir is created and the download is shelled out.
        assert!(host.calls().iter().any(|call| matches!(
            call,
            Call::Run(exe, _) if exe == "curl"
        )));
    }

    #[test]
    fn fetch_tampered_hash_fails_closed_and_deletes_download() {
        let dest = "/out/bench/tools/es.zip";
        // Manifest pins a hash the seeded bytes do NOT produce.
        let tampered = parse_manifest(manifest_toml(&"a".repeat(64)).as_bytes())
            .expect("tampered manifest parses");
        let host = MockHost::new().with_file(dest, ARTIFACT.to_vec());

        let err = fetch(
            &host,
            &tampered,
            Path::new("/out/bench"),
            Disposition::Remove,
        )
        .expect_err("a hash mismatch must abort the fetch");

        assert!(matches!(err, BenchError::Provision(_)));
        // The unverified download is deleted (fail-closed).
        assert!(host.calls().iter().any(|call| matches!(
            call,
            Call::RemoveFile(path) if path == Path::new(dest)
        )));
        assert!(host.file(Path::new(dest)).is_none());
    }

    #[test]
    fn fetch_refuses_unpinned_placeholder() {
        let unpinned = parse_manifest(manifest_toml("<fill-in>").as_bytes())
            .expect("placeholder manifest parses");
        let host = MockHost::new();

        let err = fetch(
            &host,
            &unpinned,
            Path::new("/out/bench"),
            Disposition::Remove,
        )
        .expect_err("an unpinned manifest must refuse to fetch");

        assert!(matches!(err, BenchError::Provision(_)));
        // No download is attempted when the manifest is unpinned.
        assert!(
            !host
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Run(_, _)))
        );
    }
}
