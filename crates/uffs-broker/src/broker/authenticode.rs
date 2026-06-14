// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! S5.2 Authenticode verification of the broker's client image.
//!
//! In-process `WinVerifyTrust` (replacing the old per-request PowerShell
//! `Get-AuthenticodeSignature` spawn — hundreds of ms each, plus a hard
//! dependency on PowerShell), with a per-`(exe_path, mtime)` result cache so a
//! client's repeated drive requests verify once.  Split out of `broker.rs` to
//! keep that file under the 800-LOC ceiling (alongside `service.rs` and
//! `process_handle.rs`).

/// Map of `exe_path` → (image mtime, verdict) backing [`AUTHENTICODE_CACHE`].
#[cfg(windows)]
type AuthenticodeCache = std::collections::HashMap<String, (Option<std::time::SystemTime>, bool)>;

/// Per-`exe_path` cache of Authenticode results, invalidated by image mtime.
///
/// Keyed by path; the value carries the image's last-write time so a replaced
/// binary (different mtime) is a cache **miss** and gets re-verified — a
/// substituted image can never inherit an old "trusted" verdict.
#[cfg(windows)]
static AUTHENTICODE_CACHE: std::sync::OnceLock<std::sync::Mutex<AuthenticodeCache>> =
    std::sync::OnceLock::new();

/// Read a cached verification for `exe_path`, valid only while the stored mtime
/// still matches the image's current mtime.
#[cfg(windows)]
fn cached_authenticode(exe_path: &str, mtime: Option<std::time::SystemTime>) -> Option<bool> {
    let cache = AUTHENTICODE_CACHE.get()?;
    // Copy the entry out and release the lock before comparing.
    let (stored_mtime, result) = cache.lock().ok()?.get(exe_path).copied()?;
    (stored_mtime == mtime).then_some(result)
}

/// Store a verification result for `exe_path` keyed by its current mtime.
#[cfg(windows)]
fn store_authenticode(exe_path: &str, mtime: Option<std::time::SystemTime>, result: bool) {
    let cache =
        AUTHENTICODE_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(exe_path.to_owned(), (mtime, result));
    }
}

/// S5.2: Verify the Authenticode signature of the client executable.
///
/// In-process `WinVerifyTrust` — replaces the old per-request PowerShell
/// `Get-AuthenticodeSignature` spawn (hundreds of ms each, plus a hard
/// dependency on PowerShell being present and on PATH).  Result is cached per
/// `(exe_path, mtime)` so a client's repeated drive requests verify once.
#[cfg(windows)]
pub(super) fn verify_authenticode(exe_path: &str) -> bool {
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

/// In-process Authenticode verification of `exe_path` via `WinVerifyTrust`.
///
/// Policy is **preserved** from the old PowerShell check: reject only a
/// **tampered** image (`TRUST_E_BAD_DIGEST` / `HashMismatch`); accept `Valid`
/// and `NotSigned` (dev builds) and any other non-tamper state — so the
/// unsigned dev daemon still passes.
#[cfg(windows)]
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
