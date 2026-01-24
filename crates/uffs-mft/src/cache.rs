//! Index caching with TTL (Time-To-Live) support.
//!
//! This module provides automatic caching of MFT indices in the system temp
//! directory with configurable TTL. Indices are automatically refreshed when
//! stale.
//!
//! # Cache Location
//!
//! Indices are stored in `{TEMP}/uffs_index_cache/`:
//! - `C_index.uffs` - Index for C: drive
//! - `D_index.uffs` - Index for D: drive
//! - etc.
//!
//! # TTL Behavior
//!
//! - **Single drive**: If TTL expired, refresh that drive's index only
//! - **Multi-drive**: If ANY file is expired, remove ALL and rebuild all
//! - **Cleanup**: Remove cache directory if all files are expired

use core::time::Duration;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::index::{IndexHeader, MftIndex};

/// Default TTL for cached indices (10 minutes).
pub const INDEX_TTL_SECONDS: u64 = 600;

/// Name of the cache directory in the system temp folder.
const CACHE_DIR_NAME: &str = "uffs_index_cache";

/// Gets the cache directory path.
///
/// Returns `{TEMP}/uffs_index_cache/`.
#[must_use]
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join(CACHE_DIR_NAME)
}

/// Gets the cache file path for a specific drive.
///
/// Returns `{TEMP}/uffs_index_cache/{DRIVE}_index.uffs`.
#[must_use]
pub fn cache_file_path(drive: char) -> PathBuf {
    cache_dir().join(format!("{}_index.uffs", drive.to_ascii_uppercase()))
}

/// Checks if a cached index file exists and is fresh (within TTL).
///
/// # Arguments
///
/// * `drive` - Drive letter to check
/// * `ttl_seconds` - TTL in seconds (use `INDEX_TTL_SECONDS` for default)
///
/// # Returns
///
/// `true` if the cache file exists and was modified within the TTL window.
#[must_use]
pub fn is_cache_fresh(drive: char, ttl_seconds: u64) -> bool {
    let path = cache_file_path(drive);
    std::fs::metadata(&path).is_ok_and(|meta| {
        meta.modified().is_ok_and(|modified| {
            let age = SystemTime::now()
                .duration_since(modified)
                .unwrap_or(Duration::MAX);
            age.as_secs() < ttl_seconds
        })
    })
}

/// Gets the age of a cache file in seconds.
///
/// Returns `None` if the file doesn't exist or age cannot be determined.
#[must_use]
pub fn cache_age_seconds(drive: char) -> Option<u64> {
    let path = cache_file_path(drive);
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    Some(age.as_secs())
}

/// Loads a cached index if it exists and is fresh.
///
/// # Arguments
///
/// * `drive` - Drive letter to load
/// * `ttl_seconds` - TTL in seconds
///
/// # Returns
///
/// `Some((index, header))` if cache is fresh, `None` otherwise.
#[must_use]
pub fn load_cached_index(drive: char, ttl_seconds: u64) -> Option<(MftIndex, IndexHeader)> {
    if !is_cache_fresh(drive, ttl_seconds) {
        return None;
    }

    let path = cache_file_path(drive);
    match MftIndex::load_from_file(&path) {
        Ok((index, header)) => {
            // Verify the volume matches
            (header.volume == drive.to_ascii_uppercase()).then_some((index, header))
        }
        Err(_) => None,
    }
}

/// Saves an index to the cache.
///
/// Creates the cache directory if it doesn't exist.
///
/// # Errors
///
/// Returns an error if directory creation or file writing fails.
pub fn save_to_cache(
    index: &MftIndex,
    drive: char,
    volume_serial: u64,
    usn_journal_id: u64,
    next_usn: i64,
) -> std::io::Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;

    let path = cache_file_path(drive);
    index.save_to_file(&path, volume_serial, usn_journal_id, next_usn)?;

    Ok(path)
}

/// Removes a cached index file for a specific drive.
///
/// Does nothing if the file doesn't exist.
pub fn remove_cached_index(drive: char) {
    let path = cache_file_path(drive);
    drop(std::fs::remove_file(path));
}

/// Removes all cached index files and the cache directory.
///
/// Does nothing if the directory doesn't exist.
pub fn remove_all_cached_indices() {
    let dir = cache_dir();
    drop(std::fs::remove_dir_all(dir));
}

/// Lists all cached drive letters.
///
/// Returns a vector of drive letters that have cached indices.
#[must_use]
pub fn list_cached_drives() -> Vec<char> {
    let dir = cache_dir();
    let mut drives = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                // Parse "C_index.uffs" -> 'C'
                if name.ends_with("_index.uffs") && name.len() >= 12 {
                    if let Some(drive_char) = name.chars().next() {
                        if drive_char.is_ascii_alphabetic() {
                            drives.push(drive_char.to_ascii_uppercase());
                        }
                    }
                }
            }
        }
    }

    drives.sort_unstable();
    drives
}

/// Checks if ANY cached index is expired for multi-drive operations.
///
/// For multi-drive operations, if ANY index is expired (or close to expiry),
/// we should rebuild ALL indices to ensure consistency.
///
/// # Arguments
///
/// * `drives` - List of drives to check
/// * `ttl_seconds` - TTL in seconds
///
/// # Returns
///
/// `true` if any drive's cache is missing or expired.
#[must_use]
pub fn any_cache_expired(drives: &[char], ttl_seconds: u64) -> bool {
    for &drive in drives {
        if !is_cache_fresh(drive, ttl_seconds) {
            return true;
        }
    }
    false
}

/// Checks if ALL cached indices are expired.
///
/// Used to determine if we should clean up the entire cache directory.
///
/// # Arguments
///
/// * `ttl_seconds` - TTL in seconds
///
/// # Returns
///
/// `true` if all cached indices are expired (or no indices exist).
#[must_use]
pub fn all_caches_expired(ttl_seconds: u64) -> bool {
    let drives = list_cached_drives();
    if drives.is_empty() {
        return true;
    }

    for drive in drives {
        if is_cache_fresh(drive, ttl_seconds) {
            return false;
        }
    }
    true
}

/// Cleans up the cache directory if all indices are expired.
///
/// Call this periodically to avoid accumulating stale cache files.
pub fn cleanup_expired_cache(ttl_seconds: u64) {
    if all_caches_expired(ttl_seconds) {
        remove_all_cached_indices();
    }
}

/// Result of a cache check operation.
// The Fresh variant is intentionally large as it contains the loaded index.
// Boxing would add indirection overhead for the common case.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum CacheStatus {
    /// Cache is fresh and ready to use.
    Fresh {
        /// The loaded index.
        index: MftIndex,
        /// The index header with metadata.
        header: IndexHeader,
        /// Age of the cache in seconds.
        age_seconds: u64,
    },
    /// Cache is stale and needs refresh.
    Stale {
        /// Age of the stale cache in seconds (if it exists).
        age_seconds: Option<u64>,
    },
    /// No cache exists for this drive.
    Missing,
}

/// Checks the cache status for a drive.
///
/// This is a high-level function that returns the cache status and optionally
/// loads the index if fresh.
#[must_use]
pub fn check_cache_status(drive: char, ttl_seconds: u64) -> CacheStatus {
    let path = cache_file_path(drive);

    // Check if file exists
    let Ok(meta) = std::fs::metadata(&path) else {
        return CacheStatus::Missing;
    };

    // Get age
    let age_seconds = meta
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|dur| dur.as_secs());

    // Check if fresh
    let is_fresh = age_seconds.is_some_and(|age| age < ttl_seconds);

    if is_fresh {
        // Try to load the index
        match MftIndex::load_from_file(&path) {
            Ok((index, header)) => {
                if header.volume == drive.to_ascii_uppercase() {
                    CacheStatus::Fresh {
                        index,
                        header,
                        age_seconds: age_seconds.unwrap_or(0),
                    }
                } else {
                    CacheStatus::Stale { age_seconds }
                }
            }
            Err(_) => CacheStatus::Stale { age_seconds },
        }
    } else {
        CacheStatus::Stale { age_seconds }
    }
}

/// Multi-drive cache status for coordinated operations.
#[derive(Debug)]
pub enum MultiDriveCacheStatus {
    /// All drives have fresh caches.
    AllFresh(Vec<(char, MftIndex, IndexHeader)>),
    /// Some or all drives need refresh - rebuild all.
    NeedsRebuild {
        /// Drives that need rebuilding.
        stale_drives: Vec<char>,
        /// Drives that are fresh (but will be rebuilt anyway for consistency).
        fresh_drives: Vec<char>,
    },
}

/// Checks cache status for multiple drives.
///
/// For multi-drive operations, if ANY drive is stale, we recommend rebuilding
/// ALL drives to ensure consistency.
#[must_use]
pub fn check_multi_drive_cache(drives: &[char], ttl_seconds: u64) -> MultiDriveCacheStatus {
    let mut fresh = Vec::new();
    let mut stale = Vec::new();
    let mut indices = Vec::new();

    for &drive in drives {
        match check_cache_status(drive, ttl_seconds) {
            CacheStatus::Fresh { index, header, .. } => {
                fresh.push(drive);
                indices.push((drive, index, header));
            }
            CacheStatus::Stale { .. } | CacheStatus::Missing => {
                stale.push(drive);
            }
        }
    }

    if stale.is_empty() {
        MultiDriveCacheStatus::AllFresh(indices)
    } else {
        MultiDriveCacheStatus::NeedsRebuild {
            stale_drives: stale,
            fresh_drives: fresh,
        }
    }
}
