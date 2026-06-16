// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon identity verification (S4.3.4-7).
//!
//! After connecting to the daemon socket, the client reads the PID file
//! and verifies:
//! 1. The PID in the file is alive
//! 2. The exe path of that PID matches the expected `uffsd` binary
//!
//! This prevents a rogue process from impersonating the daemon by placing
//! a fake socket file.

use std::path::PathBuf;

/// Verify that the daemon process identified by `pid` is running the
/// expected `uffsd` binary.
///
/// Returns `true` if verification passes or cannot be performed (graceful
/// degradation — don't block the user if the OS API isn't available).
pub(crate) fn verify_daemon_identity(pid: u32) -> bool {
    let Some(daemon_path) = get_process_exe_path(pid) else {
        tracing::debug!(
            pid,
            "Could not determine daemon exe path, skipping verification"
        );
        return true; // graceful degradation
    };

    if !is_uffs_daemon_binary(&daemon_path) {
        log_identity_failed(pid, &daemon_path);
        return false;
    }

    // S4.3.8: Also verify code signature (graceful — warn but don't block)
    let sig_ok = verify_code_signature(&daemon_path);
    log_identity_result(pid, &daemon_path, sig_ok);

    // Return true even if signature fails — graceful degradation
    // (unsigned dev builds should still work)
    true
}

/// Log a failed identity check.
fn log_identity_failed(pid: u32, daemon_path: &std::path::Path) {
    tracing::warn!(
        pid,
        exe = %daemon_path.display(),
        "Daemon identity verification FAILED — process is not uffsd"
    );
}

/// Log the final identity verification result.
fn log_identity_result(pid: u32, daemon_path: &std::path::Path, sig_ok: bool) {
    if !sig_ok {
        tracing::warn!(
            pid,
            exe = %daemon_path.display(),
            "Daemon code signature verification failed"
        );
    }
    tracing::debug!(
        pid,
        exe = %daemon_path.display(),
        signed = sig_ok,
        "Daemon identity verified"
    );
}

/// Check whether `path` looks like a valid daemon binary name.
///
/// Accepts both `uffsd` (current) and legacy `uffs-daemon` / `uffs_daemon`
/// names for backward compatibility.
fn is_uffs_daemon_binary(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|osn| osn.to_str()).unwrap_or("");
    name == "uffsd"
        || name == "uffsd.exe"
        || name == "uffs-daemon"
        || name == "uffs-daemon.exe"
        || name.starts_with("uffs-daemon")
        || name.starts_with("uffs_daemon")
}

/// Verify daemon identity using the PID file at the given path.
///
/// Reads the PID file, extracts the PID and `exe_path_hash`, then:
/// 1. Checks the PID is alive
/// 2. Gets the exe path of that PID
/// 3. Computes FNV-1a hash of the exe path
/// 4. Compares against the hash in the PID file
///
/// Returns `true` if verification passes.
pub(crate) fn verify_daemon_pid_file(pid_path: &std::path::Path) -> bool {
    let Ok(content) = std::fs::read_to_string(pid_path) else {
        return true; // no PID file = can't verify, allow
    };

    let mut lines = content.lines();
    let Some(pid) = lines.next().and_then(|line| line.parse::<u32>().ok()) else {
        return true;
    };
    let _timestamp: u64 = lines.next().and_then(|line| line.parse().ok()).unwrap_or(0);
    let Some(expected_hash) = lines.next().and_then(|line| line.parse::<u64>().ok()) else {
        return true; // old format PID file without hash
    };

    // Skip hash verification if 0 (couldn't determine at write time)
    if expected_hash == 0 {
        return verify_daemon_identity(pid);
    }

    // Get the exe path and compute its hash
    let Some(exe_path) = get_process_exe_path(pid) else {
        return true; // can't get path, allow
    };

    // FNV-1a 64-bit hash (must match uffs-daemon/lifecycle.rs)
    let actual_hash = {
        let data = exe_path.to_string_lossy();
        let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
        for &byte in data.as_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01B3);
        }
        hash
    };

    if actual_hash != expected_hash {
        tracing::warn!(
            pid,
            exe = %exe_path.display(),
            expected_hash,
            actual_hash,
            "Daemon exe_path_hash mismatch — possible impersonation"
        );
        return false;
    }

    true
}

// ── Platform-specific exe path lookup ───────────────────────────────────

/// Get the executable path for a running process by PID.
///
/// - **macOS**: `proc_pidpath()`
/// - **Linux**: `/proc/{pid}/exe` readlink
/// - **Windows**: `QueryFullProcessImageNameW()`
///
/// Returns `None` if the process is not found or the API is unavailable.
#[cfg(target_os = "macos")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    // MAXPATHLEN on macOS is 1024; proc_pidpath needs at least this
    let mut buf = vec![0_u8; 4096];
    let c_pid = i32::try_from(pid).ok()?;
    let buf_size = u32::try_from(buf.len()).unwrap_or(u32::MAX);

    // SAFETY: proc_pidpath is a documented macOS API (libproc.h).
    #[expect(unsafe_code, reason = "proc_pidpath requires unsafe FFI")]
    let len =
        unsafe { libc::proc_pidpath(c_pid, buf.as_mut_ptr().cast::<libc::c_void>(), buf_size) };

    let len_usize = usize::try_from(len).ok().filter(|&val| val > 0)?;
    let path_str = core::str::from_utf8(buf.get(..len_usize)?).ok()?;
    Some(PathBuf::from(path_str))
}

// ── Linux: /proc/{pid}/exe ──────────────────────────────────────────────

/// Linux: reads `/proc/{pid}/exe` symlink.
#[cfg(target_os = "linux")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    let proc_path = format!("/proc/{pid}/exe");
    std::fs::read_link(&proc_path).ok()
}

// ── Windows: QueryFullProcessImageNameW ─────────────────────────────────

/// Windows: uses `QueryFullProcessImageNameW()`.
#[cfg(target_os = "windows")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    let mut buf = vec![0_u16; 4096];
    let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);

    // SAFETY: `OpenProcess` returns `Result<HANDLE>`; we only proceed
    // with the handle on success.  `pid` is trusted input (our own PID).
    #[expect(unsafe_code, reason = "Win32 OpenProcess FFI")]
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    // SAFETY: `handle` is a valid open process handle.  `buf` lives for
    // the duration of the call.
    #[expect(unsafe_code, reason = "Win32 QueryFullProcessImageNameW FFI")]
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0), // Win32 path format
            windows::core::PWSTR(buf.as_mut_ptr()),
            core::ptr::from_mut(&mut size),
        )
    };

    // SAFETY: `handle` is owned by this function; we close it once.
    #[expect(unsafe_code, reason = "CloseHandle for owned Win32 handle")]
    let close_result = unsafe { CloseHandle(handle) };
    drop(close_result);

    if result.is_err() || size == 0 {
        return None;
    }

    // `size` is the count of UTF-16 code units written by
    // `QueryFullProcessImageNameW` and is always ≤ `buf.len()` on success.
    // Use `.get()` to stay panic-free against any future reallocation.
    let slice = buf.get(..size as usize)?;
    // Decode losslessly via `OsString::from_wide` (not `from_utf16_lossy`):
    // this path is compared for process-identity verification, so a
    // non-UTF-8/WTF-8 exe path must not be silently mangled to U+FFFD before
    // the comparison (Category 4, WI-4.2).
    Some(PathBuf::from(std::ffi::OsString::from_wide(slice)))
}

// ── Fallback for other platforms ────────────────────────────────────────

/// Fallback for unknown platforms.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn get_process_exe_path(_pid: u32) -> Option<PathBuf> {
    None // graceful degradation
}

// ────────────────────────────────────────────────────────────────────────────
// S4.3.8: Code Signature Verification
// ────────────────────────────────────────────────────────────────────────────

/// Verify the code signature of the daemon binary (S4.3.8).
///
/// - **macOS**: uses `codesign --verify` (checks Apple code signature)
/// - **Windows**: uses `Get-AuthenticodeSignature` via PowerShell (checks
///   Authenticode / Microsoft code signature)
/// - **Linux**: no standard code signing — always returns `true`
///
/// Returns `true` if the signature is valid or verification is unavailable.
/// Logs a warning if the signature check fails but does NOT block connection
/// (graceful degradation).
/// macOS: verify via `codesign --verify --strict`.
#[cfg(target_os = "macos")]
pub(crate) fn verify_code_signature(exe_path: &std::path::Path) -> bool {
    let output = std::process::Command::new("codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(exe_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();

    let out = match output {
        Ok(out) => out,
        Err(_codesign_err) => {
            tracing::debug!("Code signature verification not available on this platform");
            return true;
        }
    };

    classify_codesign_output(exe_path, &out)
}

/// Classify the output of `codesign --verify` into pass/fail.
#[cfg(target_os = "macos")]
fn classify_codesign_output(exe_path: &std::path::Path, out: &std::process::Output) -> bool {
    if out.status.success() {
        tracing::debug!(exe = %exe_path.display(), "Code signature valid");
        return true;
    }

    // AUDIT-OK(bytes): substring probe of codesign stderr. A lossy decode
    // can only FAIL to match "not signed", which routes to the `false`
    // (tampered/reject) branch below — the fail-closed direction. So lossy
    // here cannot turn a tampered binary into an accepted one. (WI-4.3)
    let stderr = String::from_utf8_lossy(&out.stderr);
    let unsigned = stderr.contains("not signed") || stderr.contains("code object is not signed");
    if unsigned {
        tracing::debug!(exe = %exe_path.display(), "Binary is not code-signed (acceptable for dev builds)");
        return true;
    }

    tracing::warn!(exe = %exe_path.display(), "Code signature INVALID — binary may have been tampered with");
    false
}

/// Windows: verify Authenticode signature via PowerShell.
#[cfg(target_os = "windows")]
pub(crate) fn verify_code_signature(exe_path: &std::path::Path) -> bool {
    let path_str = exe_path.to_string_lossy();
    let script = format!(
        "(Get-AuthenticodeSignature '{}').Status",
        path_str.replace('\'', "''")
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    let out = match output {
        Ok(out) => out,
        Err(_ps_err) => {
            tracing::debug!("Code signature verification not available on this platform");
            return true;
        }
    };

    // Strict decode: this status drives the code-signature trust decision,
    // so invalid UTF-8 fails closed (treat as not-verified) rather than
    // feeding a U+FFFD-mangled status into the classifier. (WI-4.3)
    let Ok(stdout) = core::str::from_utf8(&out.stdout) else {
        tracing::warn!("signature-verify output was not valid UTF-8; treating as unverified");
        return false;
    };
    classify_authenticode_status(exe_path, stdout.trim())
}

/// Classify an Authenticode status string from PowerShell.
#[cfg(target_os = "windows")]
fn classify_authenticode_status(exe_path: &std::path::Path, status: &str) -> bool {
    match status {
        "Valid" => {
            tracing::debug!(exe = %exe_path.display(), "Code signature valid");
            true
        }
        "HashMismatch" | "UnknownError" => {
            tracing::warn!(exe = %exe_path.display(), "Code signature INVALID — binary may have been tampered with");
            false
        }
        _ => {
            tracing::debug!(exe = %exe_path.display(), "Binary is not code-signed (acceptable for dev builds)");
            true
        }
    }
}

/// Linux + other platforms: no standard code signing mechanism.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn verify_code_signature(_exe_path: &std::path::Path) -> bool {
    tracing::debug!("Code signature verification not available on this platform");
    true
}
