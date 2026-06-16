// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Native process liveness — "is this PID still alive?".
//!
//! Used by Phase H (§19.4) to decide whether a journal's owning updater
//! is gone (→ recover) or still running (→ defer). Native Win32 / POSIX
//! so it is **version-independent** on every Windows (7/10/11) and Unix —
//! no `tasklist` / `kill` shell-out, which we deliberately avoid.

/// Windows sentinel exit code reported for a still-running process.
#[cfg(windows)]
const STILL_ACTIVE_CODE: u32 = 259;

/// `true` if a process with `pid` currently exists.
#[cfg(windows)]
#[must_use]
pub(crate) fn is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: `OpenProcess` is a documented Win32 call; a query-only mask
    // and a plain pid. It returns `Err` for a non-existent process.
    #[expect(unsafe_code, reason = "Win32 FFI — OpenProcess")]
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    if handle.is_invalid() {
        return false;
    }

    let mut code: u32 = 0;
    // SAFETY: `handle` is a valid process handle from `OpenProcess`;
    // `code` is a valid writable `u32`.
    #[expect(unsafe_code, reason = "Win32 FFI — GetExitCodeProcess")]
    let queried = unsafe { GetExitCodeProcess(handle, core::ptr::from_mut(&mut code)) }.is_ok();

    // SAFETY: `handle` came from `OpenProcess` and is closed exactly once.
    #[expect(unsafe_code, reason = "Win32 FFI — CloseHandle")]
    let _closed = unsafe { CloseHandle(handle) };

    queried && code == STILL_ACTIVE_CODE
}

/// `true` if a process with `pid` currently exists.
///
/// Uses `kill(pid, 0)` — signal 0 delivers nothing, it only probes
/// existence. The journal owner is always a prior same-user `uffs-update`,
/// so a permission error can't arise; any non-zero result means gone.
#[cfg(unix)]
#[must_use]
pub(crate) fn is_alive(pid: u32) -> bool {
    let Ok(signed_pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: `kill` with signal 0 performs no signal delivery; the pid is
    // a plain integer.
    #[expect(unsafe_code, reason = "POSIX FFI — kill(pid, 0) existence probe")]
    let result = unsafe { libc::kill(signed_pid, 0) };
    result == 0
}

#[cfg(test)]
mod tests {
    use super::is_alive;

    #[test]
    fn current_process_is_alive() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn improbable_pid_is_dead() {
        assert!(!is_alive(u32::MAX));
    }
}
