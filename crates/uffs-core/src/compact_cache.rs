// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Compact index cache: serialize/deserialize + encrypted disk I/O.
//!
//! Stores `DriveCompactIndex` as zstd-compressed, AES-256-GCM encrypted
//! `{DRIVE}_compact.uffs` alongside the full `.uffs` `MftIndex` cache.
//!
//! **v6** (current): char-trigram CSR stored on disk (keys `u64[]`, offsets
//! `u32[]`, values `u32[]`).  Zero rebuild on load — saves ~220 ms.
//!
//! **v5**: `names_lower` removed from disk — trigram rebuilt from on-the-fly
//! `CaseFold` lowered names on load.  Still accepted; trigram rebuilt.
//!
//! **v4**: trigram index not stored on disk — rebuilt from `names_lower` on
//! load.  Still accepted on load; `names_lower` is read then dropped.
//!
//! **v3**: adds `source_epoch` (u64) to the header.  Still accepted on load;
//! old byte-trigram CSR is skipped, char-trigram rebuilt.
//!
//! **v2**: old byte-trigram posting lists serialized in CSR format.
//! Accepted on load; `source_epoch` defaults to 0 (always stale).
//!
//! **v1** (legacy): rejected — returns error, caller rebuilds.
//! Exception: `file_size_policy` — serialize/deserialize pipeline, tight
//! coupling.

use std::path::PathBuf;
use std::time::Instant;

use crate::compact::{
    ChildrenIndex, CompactRecord, DriveCompactIndex, ExtensionIndex, IndexSource,
};
use crate::compact_storage::ColumnStorage;
use crate::trigram::TrigramIndex;

/// Magic bytes for compact cache files.
const COMPACT_MAGIC: &[u8; 8] = b"UFFSCOM\0";
/// Current compact cache format version.
/// - v7: `ext_names` table
/// - v8: `path_len: u16` added to `CompactRecord` (uses 2 bytes of former
///   `_pad`)
const COMPACT_VERSION: u16 = 8;
/// Bytes per `CompactRecord`.
const RECORD_BYTES: usize = size_of::<CompactRecord>();
/// zstd compression level for compact cache.
const ZSTD_LEVEL: i32 = 3;
/// zstd frame magic bytes (little-endian `0xFD2FB528`).
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Returns the cache file path for a compact index.
#[must_use]
pub fn compact_cache_path(drive_letter: char) -> PathBuf {
    uffs_mft::cache::cache_dir().join(format!("{drive_letter}_compact.uffs"))
}

/// Serializes the compact index (records, names, children, char-trigram CSR).
///
/// **v7**: `ext_names` table appended after trigram CSR.
/// Format: `ext_count: u32`, then `ext_count` length-prefixed strings
/// (`u16 len` + `len` UTF-8 bytes each).
///
/// **v6**: char-trigram CSR is stored on disk — zero-rebuild on load.
/// Format after children CSR:
///   - `trigram_key_count: u32`
///   - `trigram_keys: u64[key_count]`
///   - `trigram_offsets: u32[key_count + 1]`
///   - `trigram_values_count: u32`
///   - `trigram_values: u32[values_count]`
#[must_use]
pub fn serialize_compact(index: &DriveCompactIndex) -> Vec<u8> {
    let record_count = index.records.len();
    let names_len = index.names.len();

    // Children CSR — already in contiguous layout.
    let (csr_offsets, csr_values) = index.children.as_csr();

    // Trigram CSR.
    let (tri_keys, tri_offsets, tri_values) = index.trigram.as_csr();

    let total = 26 // header: 8 (magic) + 2 (ver) + 4 (rc) + 4 (nl) + 8 (epoch)
        + record_count * RECORD_BYTES
        + names_len
        + csr_offsets.len() * 4
        + csr_values.len() * 4
        + 4                         // trigram_key_count
        + tri_keys.len() * 8        // trigram_keys (u64)
        + tri_offsets.len() * 4     // trigram_offsets (u32)
        + 4                         // trigram_values_count
        + tri_values.len() * 4; // trigram_values (u32)
    let mut buf = Vec::with_capacity(total);

    // Header (26 bytes for v3+)
    buf.extend_from_slice(COMPACT_MAGIC);
    buf.extend_from_slice(&COMPACT_VERSION.to_le_bytes());
    push_u32(&mut buf, record_count);
    push_u32(&mut buf, names_len);
    // v3+: source_epoch
    buf.extend_from_slice(&index.source_epoch.to_le_bytes());

    // Records — single bulk copy via bytemuck (Pod layout = on-disk layout)
    buf.extend_from_slice(bytemuck::cast_slice(&index.records));

    // Names (original case only)
    buf.extend_from_slice(&index.names);

    // Children CSR — bulk cast (u32 slices → &[u8] via bytemuck, zero-copy on LE)
    buf.extend_from_slice(bytemuck::cast_slice(csr_offsets));
    buf.extend_from_slice(bytemuck::cast_slice(csr_values));

    // v6: char-trigram CSR
    push_u32(&mut buf, tri_keys.len());
    buf.extend_from_slice(bytemuck::cast_slice(tri_keys));
    buf.extend_from_slice(bytemuck::cast_slice(tri_offsets));
    push_u32(&mut buf, tri_values.len());
    buf.extend_from_slice(bytemuck::cast_slice(tri_values));

    // v7: `ext_names` table — length-prefixed strings.
    push_u32(&mut buf, index.ext_names.len());
    for name in &index.ext_names {
        let bytes = name.as_bytes();
        let len: u16 = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(bytes.get(..usize::from(len)).unwrap_or(bytes));
    }

    buf
}

/// Streaming variant of [`serialize_compact`]: writes the same byte layout
/// directly to any `impl Write` without allocating a contiguous buffer.
///
/// For a 7M-record drive the serialized size is ~1.1 GB.  Piping this into a
/// `zstd::Encoder<Vec<u8>>` reduces peak memory from ~1.3 GB to ~200 MB
/// (the compressed output size).
///
/// # Errors
///
/// Returns an error if any write fails.
pub fn serialize_compact_to_writer<W: std::io::Write>(
    index: &DriveCompactIndex,
    writer: &mut W,
) -> std::io::Result<()> {
    let record_count = index.records.len();
    let names_len = index.names.len();

    let (csr_offsets, csr_values) = index.children.as_csr();
    let (tri_keys, tri_offsets, tri_values) = index.trigram.as_csr();

    // Header (26 bytes for v3+)
    writer.write_all(COMPACT_MAGIC)?;
    writer.write_all(&COMPACT_VERSION.to_le_bytes())?;
    write_u32(writer, record_count)?;
    write_u32(writer, names_len)?;
    writer.write_all(&index.source_epoch.to_le_bytes())?;

    // Records — bulk copy via bytemuck
    writer.write_all(bytemuck::cast_slice(&index.records))?;

    // Names (original case)
    writer.write_all(&index.names)?;

    // Children CSR
    writer.write_all(bytemuck::cast_slice(csr_offsets))?;
    writer.write_all(bytemuck::cast_slice(csr_values))?;

    // v6: char-trigram CSR
    write_u32(writer, tri_keys.len())?;
    writer.write_all(bytemuck::cast_slice(tri_keys))?;
    writer.write_all(bytemuck::cast_slice(tri_offsets))?;
    write_u32(writer, tri_values.len())?;
    writer.write_all(bytemuck::cast_slice(tri_values))?;

    // v7: ext_names table — length-prefixed strings
    write_u32(writer, index.ext_names.len())?;
    for name in &index.ext_names {
        let bytes = name.as_bytes();
        let len: u16 = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
        writer.write_all(&len.to_le_bytes())?;
        writer.write_all(bytes.get(..usize::from(len)).unwrap_or(bytes))?;
    }

    writer.flush()?;
    Ok(())
}

/// Write a `usize` as little-endian `u32` to a writer.
fn write_u32<W: std::io::Write>(writer: &mut W, val: usize) -> std::io::Result<()> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "record/name counts fit in u32 for any realistic MFT"
    )]
    let val_u32 = val as u32;
    writer.write_all(&val_u32.to_le_bytes())
}

/// Deserializes a compact index from raw bytes.
///
/// **v6**: char-trigram CSR on disk — zero-rebuild.
/// **v5**: no trigram on disk — rebuilt with `CaseFold`.
/// **v4**: `names_lower` on disk, trigram rebuilt.
/// **v3/v2**: legacy byte-trigram / old format — trigram rebuilt.
/// **v1**: rejected — returns an error so the caller rebuilds.
///
/// Returns `(DriveCompactIndex, trigram_load_ms)`.
///
/// # Errors
/// Returns an error string if the data is truncated, wrong magic, or v1.
pub fn deserialize_compact(
    data: &[u8],
    drive_letter: char,
) -> Result<(DriveCompactIndex, u128), &'static str> {
    let (source_epoch, body_offset, version) = parse_compact_header(data)?;

    let rc = read_u32(data, 10) as usize;
    let nl = read_u32(data, 14) as usize;
    let re = body_offset + rc * RECORD_BYTES;
    let ne = re + nl;
    let cs = if version >= 5 { ne } else { ne + nl };
    let ce = cs + (rc + 1) * 4;
    if data.len() < ce {
        return Err("compact cache truncated");
    }

    let records: Vec<CompactRecord> =
        aligned_vec_from_bytes(data.get(body_offset..re).ok_or("truncated records")?);
    let names = data.get(re..ne).ok_or("truncated names")?.to_vec();
    let csr_off = data.get(cs..ce).ok_or("truncated CSR")?;
    let cp = read_u32(csr_off, rc * 4);
    let pe = ce + cp as usize * 4;
    if data.len() < pe {
        return Err("truncated CSR postings");
    }
    let cv = data.get(ce..pe).ok_or("CSR OOB")?;
    let children =
        ChildrenIndex::from_csr(aligned_vec_from_bytes(csr_off), aligned_vec_from_bytes(cv));
    let fold = crate::compact::resolve_case_fold(drive_letter);
    let tri_start = Instant::now();
    if data.len() < pe + 4 {
        return Err("truncated trigram header");
    }
    let tkc = read_u32(data, pe) as usize;
    let (trigram, after_tri) = if version >= 6 && tkc > 0 {
        let (ks, ke, oe) = (pe + 4, pe + 4 + tkc * 8, pe + 4 + tkc * 8 + (tkc + 1) * 4);
        if data.len() < oe + 4 {
            return Err("truncated trigram CSR");
        }
        let tk: Vec<u64> = aligned_vec_from_bytes(data.get(ks..ke).ok_or("trigram keys")?);
        let to: Vec<u32> = aligned_vec_from_bytes(data.get(ke..oe).ok_or("trigram offsets")?);
        let vc = read_u32(data, oe) as usize;
        let ve = oe + 4 + vc * 4;
        if data.len() < ve {
            return Err("truncated trigram values");
        }
        let tv: Vec<u32> = aligned_vec_from_bytes(data.get(oe + 4..ve).ok_or("trigram values")?);
        (TrigramIndex::from_csr(tk, to, tv), ve)
    } else {
        (TrigramIndex::build(&records, &names, fold), pe + 4)
    };
    let tri_ms = tri_start.elapsed().as_millis();

    let ext_names = if version >= 7 && data.len() >= after_tri + 4 {
        read_ext_names_table(data, after_tri)
    } else {
        rebuild_ext_names(&records, &names, fold)
    };

    let ext_t0 = Instant::now();
    let ext_index = ExtensionIndex::build(&records);
    let ext_build_ms = ext_t0.elapsed().as_millis();
    tracing::info!(
        drive = %drive_letter,
        entries = ext_index.total_entries(),
        build_ms = ext_build_ms,
        "ExtensionIndex built (cache load)"
    );

    Ok((
        DriveCompactIndex {
            letter: drive_letter,
            records: ColumnStorage::from_vec(records),
            names: ColumnStorage::from_vec(names),
            trigram,
            children,
            ext_index,
            fold,
            ext_names,
            source: IndexSource::MftFile(PathBuf::from(format!("{drive_letter}:"))),
            source_epoch,
        },
        tri_ms,
    ))
}

/// Read a length-prefixed `ext_names` table from v7+ compact cache bytes.
#[expect(
    clippy::single_call_fn,
    reason = "extracted from deserialize_compact for clarity"
)]
fn read_ext_names_table(data: &[u8], offset: usize) -> Vec<Box<str>> {
    let ext_count = read_u32(data, offset) as usize;
    let mut out = Vec::with_capacity(ext_count);
    let mut pos = offset + 4;
    for _ in 0..ext_count {
        let Some(&lo) = data.get(pos) else { break };
        let Some(&hi) = data.get(pos + 1) else { break };
        let slen = usize::from(u16::from_le_bytes([lo, hi]));
        pos += 2;
        let Some(slice) = data.get(pos..pos + slen) else {
            break;
        };
        out.push(Box::from(core::str::from_utf8(slice).unwrap_or("")));
        pos += slen;
    }
    out
}

/// Rebuild `ext_names` from compact records for pre-v7 caches.
#[expect(
    clippy::single_call_fn,
    reason = "legacy v6 fallback — separate concern from v7 deserialization"
)]
fn rebuild_ext_names(
    records: &[CompactRecord],
    names: &[u8],
    fold: uffs_text::case_fold::CaseFold,
) -> Vec<Box<str>> {
    let max_id = records
        .iter()
        .map(|rec| rec.extension_id)
        .max()
        .map_or(0, usize::from);
    let mut table: Vec<Option<Box<str>>> = vec![None; max_id + 1];
    if let Some(slot) = table.get_mut(0) {
        *slot = Some(Box::from(""));
    }
    let mut fold_buf = Vec::with_capacity(64);
    for rec in records {
        let idx = usize::from(rec.extension_id);
        if table.get(idx).is_some_and(Option::is_some) {
            continue;
        }
        let nm = rec.name(names);
        if let Some(dot) = nm.rfind('.')
            && let Some(ext_raw) = nm.get(dot + 1..)
        {
            let folded = fold.fold_into(ext_raw, &mut fold_buf);
            if let Some(slot) = table.get_mut(idx) {
                *slot = Some(Box::from(folded));
            }
        }
    }
    table
        .into_iter()
        .map(|opt| opt.unwrap_or_else(|| Box::from("")))
        .collect()
}

/// Validates magic/version and returns `(source_epoch, body_offset, version)`.
fn parse_compact_header(data: &[u8]) -> Result<(u64, usize, u16), &'static str> {
    if data.len() < 18 {
        return Err("compact cache too short");
    }
    if data.get(..8) != Some(COMPACT_MAGIC.as_slice()) {
        return Err("bad compact magic");
    }
    let version = data
        .get(8..10)
        .and_then(|slice| <[u8; 2]>::try_from(slice).ok())
        .map_or(0, u16::from_le_bytes);
    if version < 2 {
        return Err("stale compact version (v1 → rebuild)");
    }
    if version > COMPACT_VERSION {
        return Err("unsupported compact version (future)");
    }
    if version >= 3 {
        if data.len() < 26 {
            return Err("compact cache truncated (v3 header)");
        }
        let epoch = data
            .get(18..26)
            .and_then(|slice| <[u8; 8]>::try_from(slice).ok())
            .map_or(0, u64::from_le_bytes);
        Ok((epoch, 26, version))
    } else {
        Ok((0, 18, version))
    }
}

// ─── Save / Load ────────────────────────────────────────────────────────────

/// Saves a compact index to its cache file (zstd + AES-256-GCM), blocking.
///
/// Uses streaming serialization — no ~1.1 GB intermediate buffer.
/// Prefer [`save_compact_cache_background`] for non-blocking saves.
///
/// # Errors
/// Returns an error if compression, encryption, or file writing fails.
pub fn save_compact_cache(index: &DriveCompactIndex) -> std::io::Result<()> {
    let profile = std::env::var_os("UFFS_CACHE_PROFILE").is_some();
    let path = compact_cache_path(index.letter);
    if let Some(dir) = path.parent() {
        uffs_mft::cache::create_secure_dir(dir)?;
    }
    uffs_mft::cache::compress_encrypt_write_streaming(
        |encoder| serialize_compact_to_writer(index, encoder),
        &path,
        ZSTD_LEVEL,
        profile,
        "compact",
    )
}

/// Streams the compact index into a zstd encoder on the calling thread
/// (no ~1.1 GB intermediate buffer), then spawns a background thread
/// for encryption and disk write.
///
/// Calling-thread work: serialize → zstd compress (~200 MB output).
/// Background-thread work: AES-256-GCM encrypt → atomic disk write.
///
/// Peak memory: ~200 MB (compressed) + ~200 MB (encrypted) = ~400 MB,
/// down from ~1.3 GB with the old `serialize_compact` + `compress` path.
///
/// # Errors
/// Returns an error only if compression or directory creation fails.
pub fn save_compact_cache_background(index: &DriveCompactIndex) -> std::io::Result<()> {
    let profile = std::env::var_os("UFFS_CACHE_PROFILE").is_some();
    let t_compress = Instant::now();

    // Serialize directly into the zstd encoder — no 1.1 GB buffer.
    let mut encoder = uffs_mft::cache::new_zstd_mt_encoder(ZSTD_LEVEL)?;
    serialize_compact_to_writer(index, &mut encoder)?;
    let compressed = encoder.finish()?;

    let compress_ms = t_compress.elapsed().as_millis();
    if profile {
        let comp_mb = compressed.len() / (1024 * 1024);
        tracing::debug!(
            target: "cache_profile",
            compress_ms = %compress_ms,
            comp_mb,
            "compact_streaming_ser+compress"
        );
    }

    let path = compact_cache_path(index.letter);
    if let Some(dir) = path.parent() {
        uffs_mft::cache::create_secure_dir(dir)?;
    }
    let drive = index.letter;

    // Background: encrypt + atomic write (only ~200 MB moves to thread).
    std::thread::Builder::new()
        .name(format!("compact-save-{drive}"))
        .spawn(move || {
            if let Err(err) = encrypt_and_write(drive, compressed, &path, profile) {
                tracing::warn!(drive = %drive, error = %err, "Background compact cache save failed");
            }
        })
        .map_err(|err| std::io::Error::other(format!("spawn failed: {err}")))?;
    Ok(())
}

/// Encrypt compressed cache data and write atomically to disk.
///
/// Extracted from the `save_compact_cache_background` closure to keep
/// cognitive complexity low.
fn encrypt_and_write(
    drive: char,
    compressed: Vec<u8>,
    path: &std::path::Path,
    profile: bool,
) -> std::io::Result<()> {
    let t_enc = Instant::now();
    let key = uffs_security::keystore::get_cache_key()
        .map_err(|err| std::io::Error::other(format!("cache key unavailable: {err}")))?;
    let encrypted = uffs_security::crypto::encrypt_cache(&compressed, &key)?;
    // Drop compressed — only encrypted needed for write.
    drop(compressed);
    uffs_mft::cache::atomic_write(path, &encrypted)?;
    if profile {
        let enc_write_ms = t_enc.elapsed().as_millis();
        tracing::debug!(
            target: "cache_profile",
            drive = %drive,
            enc_write_ms = %enc_write_ms,
            "compact_bg_encrypt+write"
        );
    }
    Ok(())
}

/// Check whether the compact cache at `path` is still fresh enough to
/// load, based on TTL and (optionally) mtime comparison against the
/// `MftIndex` `.uffs` file.
///
/// Returns `true` when the cache passes both checks and should be read.
/// Returns `false` when the file is missing, older than `ttl_seconds`,
/// or older than the `MftIndex` source (cross-process staleness — the
/// daemon rebuilt the MFT after this compact cache was written).
/// When `trust_ttl_only` is true the mtime comparison is skipped — the
/// caller vouches that the TTL alone is sufficient freshness evidence.
fn is_compact_cache_fresh(
    path: &std::path::Path,
    drive_letter: char,
    ttl_seconds: u64,
    trust_ttl_only: bool,
) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(compact_mtime) = meta.modified() else {
        return false;
    };
    let Ok(age) = compact_mtime.elapsed() else {
        return false;
    };
    if age.as_secs() > ttl_seconds {
        return false;
    }
    if trust_ttl_only {
        return true;
    }
    let mft_path = uffs_mft::cache::cache_file_path(drive_letter);
    if let Ok(mft_meta) = std::fs::metadata(&mft_path)
        && let Ok(mft_mtime) = mft_meta.modified()
        && mft_mtime > compact_mtime
    {
        tracing::debug!(
            drive = %drive_letter,
            "Compact cache older than MftIndex cache — rebuilding"
        );
        return false;
    }
    true
}

/// Loads a compact index from its cache file if fresh. Returns `None` if
/// cache is missing, stale, corrupt, or built from an older `MftIndex`.
///
/// `mft_build_epoch` is the `build_epoch` of the current `MftIndex`.
/// If the compact cache was built from an older epoch it is considered stale
/// and `None` is returned so the caller rebuilds.
///
/// When `trust_ttl_only` is `true`, the mtime comparison against the
/// `MftIndex` `.uffs` file is skipped — only the TTL age check is used.
/// This is useful for hot-path searches where the caller knows the compact
/// cache was just built or the `MftIndex` hasn't changed.
#[must_use]
pub fn load_compact_cache(
    drive_letter: char,
    ttl_seconds: u64,
    mft_build_epoch: u64,
    trust_ttl_only: bool,
) -> Option<DriveCompactIndex> {
    let path = compact_cache_path(drive_letter);
    if !is_compact_cache_fresh(&path, drive_letter, ttl_seconds, trust_ttl_only) {
        return None;
    }

    let profile = std::env::var_os("UFFS_CACHE_PROFILE").is_some();
    let t_total = Instant::now();

    let t_read = Instant::now();
    let raw = std::fs::read(&path).ok()?;
    let read_ms = t_read.elapsed().as_millis();
    let raw_len = raw.len();

    let key = uffs_security::keystore::get_cache_key().ok()?;
    let t_decrypt = Instant::now();
    let decrypted = uffs_security::crypto::decrypt_cache(&raw, &key).ok()?;
    let decrypt_ms = t_decrypt.elapsed().as_millis();

    let t_decompress = Instant::now();
    let is_compressed = decrypted.get(..4).is_some_and(|magic| magic == ZSTD_MAGIC);
    let plaintext = if is_compressed {
        zstd::decode_all(decrypted.as_slice()).ok()?
    } else {
        decrypted
    };
    let decompress_ms = t_decompress.elapsed().as_millis();
    let plaintext_len = plaintext.len();

    // Early staleness check — inspect header before full deserialization.
    if mft_build_epoch > 0
        && let Ok((source_epoch, _, _)) = parse_compact_header(&plaintext)
        && source_epoch < mft_build_epoch
    {
        tracing::debug!(
            target: "cache_profile",
            source_epoch,
            mft_build_epoch,
            "compact: STALE"
        );
        tracing::debug!(
            drive = %drive_letter,
            compact_epoch = source_epoch,
            mft_epoch = mft_build_epoch,
            "Compact cache stale (source_epoch < mft build_epoch) — rebuilding"
        );
        return None;
    }

    let t_deser = Instant::now();
    let (index, tri_ms) = deserialize_compact(&plaintext, drive_letter).ok()?;
    let deser_ms = t_deser.elapsed().as_millis();

    if profile {
        log_compact_load_profile(&index, &CompactLoadProfile {
            raw_len,
            plaintext_len,
            read_ms,
            decrypt_ms,
            is_compressed,
            decompress_ms,
            deser_ms,
            tri_ms,
            total_ms: t_total.elapsed().as_millis(),
        });
    }
    Some(index)
}

/// Timing profile for compact-cache loading stages.
struct CompactLoadProfile {
    /// Size of raw (encrypted/compressed) data in bytes.
    raw_len: usize,
    /// Size after decryption in bytes.
    plaintext_len: usize,
    /// Time to read from disk.
    read_ms: u128,
    /// Time to decrypt.
    decrypt_ms: u128,
    /// Whether the data was compressed.
    is_compressed: bool,
    /// Time to decompress (0 if not compressed).
    decompress_ms: u128,
    /// Time to deserialize.
    deser_ms: u128,
    /// Time to build/load trigram index.
    tri_ms: u128,
    /// Total wall-clock time.
    total_ms: u128,
}

/// Emit a `cache_profile` tracing event with compact-load timings.
fn log_compact_load_profile(index: &DriveCompactIndex, profile: &CompactLoadProfile) {
    let raw_mb = profile.raw_len / (1024 * 1024);
    let plain_mb = profile.plaintext_len / (1024 * 1024);
    let tri_label = if profile.tri_ms > 100 {
        "tri_rebuild"
    } else {
        "tri_load"
    };
    tracing::debug!(
        target: "cache_profile",
        read_ms = %profile.read_ms,
        raw_mb,
        decrypt_ms = %profile.decrypt_ms,
        is_compressed = profile.is_compressed,
        decompress_ms = %profile.decompress_ms,
        plain_mb,
        deser_ms = %profile.deser_ms,
        records = index.records.len(),
        tri_label,
        tri_ms = %profile.tri_ms,
        total_ms = %profile.total_ms,
        source_epoch = index.source_epoch,
        "compact_load"
    );
}

// ─── Build-or-load + save ────────────────────────────────────────────────────

/// Ensures the compact cache is up-to-date for a given drive.
///
/// - If a fresh compact cache exists on disk → loads and returns it.
/// - Otherwise → builds from the given `MftIndex` → saves → returns.
///
/// Emits `cache_profile` tracing events at `debug` level.
/// The caller may discard the returned index if only the `MftIndex` is needed.
pub fn ensure_compact_cached(
    drive_letter: char,
    mft_index: &uffs_mft::MftIndex,
) -> DriveCompactIndex {
    // Try loading existing compact cache (epoch check catches stale caches).
    // Not TTL-only: we have the MftIndex, so mtime validation is cheap & correct.
    if let Some(cached) = load_compact_cache(
        drive_letter,
        super::compact::INDEX_TTL_SECONDS,
        mft_index.build_epoch,
        false,
    ) {
        tracing::debug!(
            target: "cache_profile",
            records = cached.records.len(),
            "compact: loaded from cache"
        );
        return cached;
    }

    // Build from MftIndex
    let t_build = Instant::now();
    let (compact, build_ms, tri_ms) = crate::compact::build_compact_index(drive_letter, mft_index);
    let total_build_ms = t_build.elapsed().as_millis();

    tracing::debug!(
        target: "cache_profile",
        build_ms = %build_ms,
        records = compact.records.len(),
        tri_ms = %tri_ms,
        total_ms = %total_build_ms,
        "compact_build"
    );

    // Save to disk (best-effort)
    if let Err(err) = save_compact_cache(&compact) {
        tracing::warn!(drive = %drive_letter, error = %err, "Failed to save compact cache");
    } else {
        log_disk_summary(drive_letter);
    }

    compact
}

/// Log on-disk sizes of both MFT and compact caches for a drive.
fn log_disk_summary(drive_letter: char) {
    let compact_path = compact_cache_path(drive_letter);
    let mft_path = uffs_mft::cache::cache_file_path(drive_letter);
    let compact_disk = std::fs::metadata(&compact_path).map_or(0, |meta| meta.len());
    let mft_disk = std::fs::metadata(mft_path).map_or(0, |meta| meta.len());
    let compact_disk_mb = compact_disk / (1024 * 1024);
    let mft_disk_mb = mft_disk / (1024 * 1024);
    let total_disk_mb = compact_disk_mb + mft_disk_mb;
    tracing::debug!(
        target: "cache_profile",
        mft_disk_mb,
        compact_disk_mb,
        total_disk_mb,
        "disk_summary"
    );
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Writes a usize as u32 LE (saturates at `u32::MAX`).
fn push_u32(buf: &mut Vec<u8>, value: usize) {
    buf.extend_from_slice(&uffs_mft::len_to_u32(value).to_le_bytes());
}

/// Read a little-endian u32 from `data` at `offset`.
fn read_u32(data: &[u8], offset: usize) -> u32 {
    data.get(offset..offset + 4)
        .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        .map_or(0, u32::from_le_bytes)
}

/// Alignment-safe bulk copy from a `&[u8]` slice into a properly aligned
/// `Vec<T>`.
///
/// Unlike `bytemuck::cast_slice`, this works regardless of the source
/// pointer's alignment. It allocates a `Vec<T>` (which the allocator
/// guarantees to be `align_of::<T>()`-aligned), then copies the raw bytes
/// in via `copy_from_slice`.
///
/// # Panics
///
/// Panics if `bytes.len()` is not an exact multiple of `size_of::<T>()`.
fn aligned_vec_from_bytes<T: bytemuck::Pod>(bytes: &[u8]) -> Vec<T> {
    let elem_size = size_of::<T>();
    assert!(
        elem_size > 0 && bytes.len().is_multiple_of(elem_size),
        "byte slice length {} is not a multiple of element size {}",
        bytes.len(),
        elem_size,
    );
    let count = bytes.len() / elem_size;
    let mut vec = vec![T::zeroed(); count];
    // The Vec<T> is guaranteed aligned by the allocator. Copy raw bytes in.
    let dst = bytemuck::cast_slice_mut::<T, u8>(&mut vec);
    dst.copy_from_slice(bytes);
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `DriveCompactIndex` with 3 records for testing.
    fn make_test_index() -> DriveCompactIndex {
        let names = b"foobarbaz".to_vec(); // "foo" [0..3], "bar" [3..6], "baz" [6..9]
        let records = vec![
            CompactRecord {
                name_offset: 0,
                flags: 0x0010, // directory
                parent_idx: u32::MAX,
                name_len: 3,
                name_first_byte: b'f',
                ..CompactRecord::default()
            },
            CompactRecord {
                name_offset: 3,
                parent_idx: 0,
                name_len: 3,
                name_first_byte: b'b',
                ..CompactRecord::default()
            },
            CompactRecord {
                name_offset: 6,
                parent_idx: 0,
                name_len: 3,
                name_first_byte: b'b',
                ..CompactRecord::default()
            },
        ];
        let fold = uffs_text::case_fold::CaseFold::default_table();
        let trigram = TrigramIndex::build(&records, &names, fold);
        let children = ChildrenIndex::build(&records);
        let ext_index = ExtensionIndex::build(&records);
        DriveCompactIndex {
            letter: 'T',
            records: ColumnStorage::from_vec(records),
            names: ColumnStorage::from_vec(names),
            trigram,
            children,
            ext_index,
            fold,
            ext_names: vec![Box::from("")],
            source: IndexSource::MftFile(PathBuf::from("T:")),
            source_epoch: 42,
        }
    }

    #[test]
    fn v6_round_trip_preserves_trigram() {
        let index = make_test_index();
        let (tri_keys, tri_offsets, tri_values) = index.trigram.as_csr();
        let original_key_count = tri_keys.len();
        assert!(original_key_count > 0, "test index should have trigrams");

        let serialized = serialize_compact(&index);
        let (loaded, tri_ms) = deserialize_compact(&serialized, 'T').unwrap();

        // Trigram loaded from disk — should be fast (< 10ms on any hardware).
        assert!(
            tri_ms < 500,
            "trigram took {tri_ms}ms — should be near-zero for cached CSR"
        );

        // Verify trigram CSR is identical.
        let (loaded_keys, loaded_offsets, loaded_values) = loaded.trigram.as_csr();
        assert_eq!(loaded_keys, tri_keys, "trigram keys mismatch");
        assert_eq!(loaded_offsets, tri_offsets, "trigram offsets mismatch");
        assert_eq!(loaded_values, tri_values, "trigram values mismatch");

        // Verify other fields survived.
        assert_eq!(loaded.letter, 'T');
        assert_eq!(loaded.records.len(), 3);
        assert_eq!(loaded.names.as_slice(), b"foobarbaz");
        assert_eq!(loaded.source_epoch, 42);
    }

    #[test]
    fn v5_backward_compat_rebuilds_trigram() {
        // Serialize a v6 index, then patch the version to v5 and replace
        // the trigram section with the v5 sentinel (trigram_count = 0).
        let index = make_test_index();
        let mut serialized = serialize_compact(&index);

        // Patch version to 5.
        serialized
            .get_mut(8..10)
            .expect("buffer too short for version")
            .copy_from_slice(&5_u16.to_le_bytes());

        // Find the trigram section: after children CSR.
        // Children CSR starts after names, offsets are (records+1)*4, then values.
        let record_count = index.records.len();
        let names_len = index.names.len();
        let records_end = 26 + record_count * RECORD_BYTES;
        let names_end = records_end + names_len;
        let csr_offsets_end = names_end + (record_count + 1) * 4;
        let total_children = index.children.total_children();
        let postings_end = csr_offsets_end + total_children * 4;

        // Truncate at postings_end + 4 (v5 sentinel: trigram_count = 0).
        serialized.truncate(postings_end + 4);
        serialized
            .get_mut(postings_end..postings_end + 4)
            .expect("buffer too short for trigram sentinel")
            .copy_from_slice(&0_u32.to_le_bytes());

        let (loaded, _tri_ms) = deserialize_compact(&serialized, 'T').unwrap();

        // Trigram was rebuilt — should match the original.
        let (orig_keys, orig_offsets, orig_values) = index.trigram.as_csr();
        let (loaded_keys, loaded_offsets, loaded_values) = loaded.trigram.as_csr();
        assert_eq!(loaded_keys, orig_keys, "rebuilt trigram keys mismatch");
        assert_eq!(
            loaded_offsets, orig_offsets,
            "rebuilt trigram offsets mismatch"
        );
        assert_eq!(
            loaded_values, orig_values,
            "rebuilt trigram values mismatch"
        );
    }

    #[test]
    fn v8_header_version() {
        let index = make_test_index();
        let serialized = serialize_compact(&index);
        let b8 = *serialized.get(8).expect("missing byte 8");
        let b9 = *serialized.get(9).expect("missing byte 9");
        let version = u16::from_le_bytes([b8, b9]);
        assert_eq!(version, COMPACT_VERSION);
    }

    #[test]
    fn v1_rejected() {
        let mut data = vec![0_u8; 64];
        data.get_mut(..8)
            .expect("buffer too short for magic")
            .copy_from_slice(COMPACT_MAGIC);
        data.get_mut(8..10)
            .expect("buffer too short for version")
            .copy_from_slice(&1_u16.to_le_bytes());
        assert!(deserialize_compact(&data, 'X').is_err());
    }

    #[test]
    fn truncated_data_rejected() {
        assert!(deserialize_compact(b"short", 'X').is_err());
    }

    #[test]
    fn ext_names_round_trips() {
        let index = make_test_index();
        let serialized = serialize_compact(&index);
        let (deser, _) = deserialize_compact(&serialized, 'T').expect("deser");
        assert_eq!(deser.ext_names, index.ext_names);
    }
}
