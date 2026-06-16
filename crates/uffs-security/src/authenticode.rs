// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! In-process Authenticode (`WinVerifyTrust`) signature verification.
//!
//! The single shared implementation used by both the Access Broker
//! (verifying its client image) and the self-updater (verifying a
//! downloaded binary before it is allowed to replace anything).
//!
//! Consolidated here so the one piece of `WinVerifyTrust` FFI lives in a
//! single audited place rather than drifting across crates. Includes a
//! per-`(exe_path, mtime)` result cache so repeated checks of the same
//! image verify once; a replaced binary (different mtime) is a cache
//! **miss** and is re-verified — a substituted image can never inherit
//! an old "trusted" verdict.

#![cfg(windows)]

/// Map of `exe_path` → (image mtime, verdict) backing [`AUTHENTICODE_CACHE`].
type AuthenticodeCache = std::collections::HashMap<String, (Option<std::time::SystemTime>, bool)>;

/// Per-`exe_path` cache of verification results, invalidated by image mtime.
static AUTHENTICODE_CACHE: std::sync::OnceLock<std::sync::Mutex<AuthenticodeCache>> =
    std::sync::OnceLock::new();

/// Verify the Authenticode signature of the executable at `exe_path`.
///
/// In-process `WinVerifyTrust`. The result is cached per `(exe_path,
/// mtime)` so repeated checks of the same image verify only once.
///
/// **Policy:** reject only a **tampered** image (`TRUST_E_BAD_DIGEST` /
/// hash mismatch); accept `Valid`, `NotSigned` (unsigned dev builds),
/// and any other non-tamper state. Callers that require a *valid*
/// signature (not merely "untampered") must layer that on top — this
/// primitive matches the broker's long-standing "reject only tamper"
/// contract so the unsigned dev daemon still passes.
#[must_use]
pub fn verify_authenticode(exe_path: &str) -> bool {
    let mtime = std::fs::metadata(exe_path)
        .and_then(|meta| meta.modified())
        .ok();
    if let Some(cached) = cached_authenticode(exe_path, mtime) {
        return cached;
    }
    let trusted = win_verify_trust(exe_path);
    store_authenticode(exe_path, mtime, trusted);
    trusted
}

/// Read a cached verification for `exe_path`, valid only while the stored
/// mtime still matches the image's current mtime.
fn cached_authenticode(exe_path: &str, mtime: Option<std::time::SystemTime>) -> Option<bool> {
    let cache = AUTHENTICODE_CACHE.get()?;
    // Copy the entry out and release the lock before comparing.
    let (stored_mtime, result) = cache.lock().ok()?.get(exe_path).copied()?;
    (stored_mtime == mtime).then_some(result)
}

/// Store a verification result for `exe_path` keyed by its current mtime.
fn store_authenticode(exe_path: &str, mtime: Option<std::time::SystemTime>, result: bool) {
    let cache =
        AUTHENTICODE_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(exe_path.to_owned(), (mtime, result));
    }
}

/// In-process Authenticode verification of `exe_path` via `WinVerifyTrust`.
#[expect(unsafe_code, reason = "FFI: WinVerifyTrust signature verification")]
fn win_verify_trust(exe_path: &str) -> bool {
    use core::mem::size_of;

    use windows::Win32::Foundation::{HWND, TRUST_E_BAD_DIGEST};
    use windows::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WinVerifyTrust,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = exe_path.encode_utf16().chain(core::iter::once(0)).collect();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(size_of::<WINTRUST_FILE_INFO>()).unwrap_or(0),
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(size_of::<WINTRUST_DATA>()).unwrap_or(0),
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: core::ptr::from_mut(&mut file_info),
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;

    // SAFETY: `data` / `file_info` / `wide` / `action` outlive both calls; the
    // payload is a valid `*mut WINTRUST_DATA` reinterpreted as the documented
    // `*mut c_void`.  `WinVerifyTrust` takes the action GUID by `*mut` but does
    // not mutate it.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            core::ptr::from_mut(&mut action),
            core::ptr::from_mut(&mut data).cast(),
        )
    };

    // Always release the trust state, whatever the verdict.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: same `data` / `action` from the VERIFY call; CLOSE frees its state.
    let _close: i32 = unsafe {
        WinVerifyTrust(
            HWND::default(),
            core::ptr::from_mut(&mut action),
            core::ptr::from_mut(&mut data).cast(),
        )
    };

    status != TRUST_E_BAD_DIGEST.0
}
