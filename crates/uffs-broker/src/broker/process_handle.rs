// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Client process-handle acquisition and identity verification for the broker.
//!
//! **WI-8.1 (Category 8 — resolve before trust boundary):** the broker opens
//! the client process **exactly once** ([`OwnedProcessHandle::open_client`])
//! and uses that single handle for both the identity check
//! ([`verify_client_handle`]) and the later `DuplicateHandle` target — so a
//! PID-reuse race cannot redirect the granted volume handle to an unverified
//! process. Extracted from `broker.rs` to keep that file under the 800-LOC
//! ceiling while grouping the trust-boundary machinery in one place.

/// RAII wrapper around a client process `HANDLE` opened via `OpenProcess`.
///
/// Opening once, with the union of rights both phases need
/// (`PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_DUP_HANDLE`), means the handle
/// whose identity is verified is the **same** handle the grant duplicates into
/// — there is no second PID→handle resolution to race against. `Drop` closes
/// it on every path.
#[cfg(windows)]
pub(super) struct OwnedProcessHandle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl OwnedProcessHandle {
    /// Open the client process once with the combined rights needed for both
    /// identity verification (`QueryFullProcessImageNameW`) and the
    /// `DuplicateHandle` target. Returns `None` if the process cannot be
    /// opened (e.g. it already exited).
    #[expect(unsafe_code, reason = "Win32 OpenProcess FFI")]
    pub(super) fn open_client(pid: u32) -> Option<Self> {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: `OpenProcess` returns `Result<HANDLE>`; on failure we map to
        // `None` and never construct an `OwnedProcessHandle` around an invalid
        // handle, so `Drop` only ever closes a real open handle.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_DUP_HANDLE,
                false,
                pid,
            )
        }
        .ok()?;
        Some(Self(handle))
    }

    /// The raw handle, for passing to Win32 query / duplicate calls. The
    /// returned handle is owned by `self` and must not outlive it.
    pub(super) const fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for OwnedProcessHandle {
    #[expect(unsafe_code, reason = "CloseHandle for the owned process handle")]
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful `OpenProcess` in
        // `open_client` and is owned exclusively by this value.
        if let Err(close_err) = unsafe { windows::Win32::Foundation::CloseHandle(self.0) } {
            tracing::debug!(err = ?close_err, "CloseHandle failed dropping OwnedProcessHandle");
        }
    }
}

/// Query a process's full image path from an already-open handle.
///
/// Shared by the identity verification and audit-path-name lookups so both
/// read the name from the **same** verified handle (WI-8.1) — no second
/// PID→handle resolution. Returns `None` if the query fails or yields an
/// empty path.
///
/// Decodes losslessly via `OsString::from_wide` (not `from_utf16_lossy`):
/// the result feeds the daemon-identity decision in [`verify_client_handle`],
/// so a non-UTF-8/WTF-8 path component must not be mangled to U+FFFD before
/// the allow-list match (Category 4 / WI-4.2). The daemon names are ASCII, so
/// a path that is not valid UTF-8 simply fails the `to_str()` in
/// [`is_uffs_daemon_image`] and is rejected.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Win32 QueryFullProcessImageNameW requires unsafe FFI"
)]
pub(super) fn query_process_image_name(
    handle: windows::Win32::Foundation::HANDLE,
) -> Option<std::ffi::OsString> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;

    use windows::Win32::System::Threading::{PROCESS_NAME_FORMAT, QueryFullProcessImageNameW};

    let mut buf = vec![0_u16; 4096];
    let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is a valid open process handle (owned by the caller's
    // `OwnedProcessHandle`); `buf` is a fixed 4096-wide allocation; `size` is
    // a stack-owned u32 whose address is exclusive to this call.
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut size,
        )
    };
    if result.is_err() || size == 0 {
        return None;
    }
    // u32→usize lossless on 64-bit; use get() to satisfy indexing_slicing.
    let len = size as usize;
    buf.get(..len).map(OsString::from_wide)
}

/// Returns `true` if `exe_path`'s file name is one of the allowed uffs-daemon
/// binaries. Pure (no FFI) so it is unit-testable on every platform — this is
/// the actual trust predicate behind [`verify_client_handle`].
///
/// Takes `&OsStr` so the lossless image path from [`query_process_image_name`]
/// is matched without a lossy conversion. The daemon names are ASCII; a file
/// name that is not valid UTF-8 fails `to_str()` and is correctly rejected.
pub(super) fn is_uffs_daemon_image(exe_path: &std::ffi::OsStr) -> bool {
    let name = std::path::Path::new(exe_path)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("");

    name == "uffsd"
        || name == "uffsd.exe"
        || name == "uffs-daemon.exe"
        || name == "uffs-daemon"
        || name.starts_with("uffs-daemon")
        || name.starts_with("uffs_daemon")
}

/// Verify that a client process is a legitimate uffs-daemon, reading its image
/// name from an **already-open** handle (WI-8.1).
///
/// The `handle` is the same `OwnedProcessHandle` the grant will duplicate into,
/// so the name we allow-list and the process we hand the volume handle to are
/// guaranteed identical — no PID re-resolution between verify and grant.
#[cfg(windows)]
pub(super) fn verify_client_handle(handle: windows::Win32::Foundation::HANDLE) -> bool {
    query_process_image_name(handle).is_some_and(|exe_name| is_uffs_daemon_image(&exe_name))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_uffs_daemon_image;

    #[test]
    fn accepts_known_daemon_names() {
        assert!(is_uffs_daemon_image(OsStr::new(
            r"C:\Program Files\uffs\uffsd.exe"
        )));
        assert!(is_uffs_daemon_image(OsStr::new("/usr/local/bin/uffsd")));
        assert!(is_uffs_daemon_image(OsStr::new(r"C:\x\uffs-daemon.exe")));
        assert!(is_uffs_daemon_image(OsStr::new("/opt/uffs-daemon")));
        // Prefix forms (versioned binaries).
        assert!(is_uffs_daemon_image(OsStr::new(
            r"C:\x\uffs-daemon-0.5.exe"
        )));
        assert!(is_uffs_daemon_image(OsStr::new(
            r"C:\x\uffs_daemon_dev.exe"
        )));
    }

    #[test]
    fn rejects_other_images() {
        assert!(!is_uffs_daemon_image(OsStr::new(
            r"C:\Windows\System32\cmd.exe"
        )));
        assert!(!is_uffs_daemon_image(OsStr::new("/bin/sh")));
        assert!(!is_uffs_daemon_image(OsStr::new(r"C:\evil\notuffsd.exe")));
        // A path whose *directory* contains "uffsd" but whose file name does
        // not must be rejected — we match on the file name, not a substring.
        assert!(!is_uffs_daemon_image(OsStr::new(r"C:\uffsd\malware.exe")));
        assert!(!is_uffs_daemon_image(OsStr::new("")));
    }
}
