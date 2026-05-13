// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Child-process handle for a daemon spawned by this crate.
//!
//! This is the **canonical home** of `DaemonChildHandle`.  Import
//! it from `crate::daemon_child` (or `uffs_client::daemon_child`
//! from outside the crate) — there is intentionally no `pub use`
//! cascade through `daemon_ctl`.
//!
//! The handle abstracts over three very different backends:
//!
//! | Variant   | Constructor                          | Backing state                                          |
//! |-----------|--------------------------------------|--------------------------------------------------------|
//! | `Unix`    | `DaemonChildHandle::from_unix_child`     | [`std::process::Child`] (stdlib handles reaping)       |
//! | `Windows` | `DaemonChildHandle::from_windows_process` | `PROCESS_INFORMATION.hProcess` (owned, closed on drop) |
//! | `Opaque`  | `DaemonChildHandle::opaque`              | no pollable handle (e.g. `ShellExecuteW("runas")`)     |
//!
//! `DaemonChildHandle::try_wait` normalises the three into a single
//! `Result<Option<i32>>`, which the sync / async connect retry loops
//! use to detect unexpected early exit (panic, clap parse error,
//! validation bail) *before* timing out the full 20-attempt window.

/// Platform-specific inner of [`DaemonChildHandle`].
#[doc(hidden)]
enum DaemonChildInner {
    /// Windows: keeps the `PROCESS_INFORMATION.hProcess` handle alive so
    /// `WaitForSingleObject` + `GetExitCodeProcess` can observe the child.
    #[cfg(windows)]
    Windows {
        /// `Option` so [`Drop`] can `take()` the handle before closing it.
        process_handle: Option<windows::Win32::Foundation::HANDLE>,
    },
    /// Unix: wraps `std::process::Child` — `try_wait` and reaping are
    /// both handled by the standard library.
    #[cfg(unix)]
    Unix {
        /// `Option` so [`DaemonChildHandle::try_wait`] can move the
        /// `Child` out when polling, then put it back if still alive.
        child: Option<std::process::Child>,
    },
    /// Opaque: the spawn path (e.g. `ShellExecuteW("runas")` on Windows)
    /// did not produce a pollable handle.  Liveness checks are a no-op.
    #[cfg(windows)]
    Opaque,
}

/// Handle to a spawned daemon process for early-exit detection.
///
/// The IPC-readiness retry loop in
/// [`UffsClientSync::connect_with_args`](crate::connect_sync::UffsClientSync::connect_with_args)
/// polls this between attempts.  If the daemon has exited, the retry
/// loop breaks out immediately with the observed exit code instead of
/// waiting for the full 31 s back-off window.  That turns the
/// previously-silent "uffsd died during startup" failure mode into an
/// actionable error like
/// `"Daemon exited with code 2 during startup — check uffsd.log"`.
pub(crate) struct DaemonChildHandle {
    /// Platform-specific state (Win32 `HANDLE`, `std::process::Child`,
    /// or opaque) — see [`DaemonChildInner`] for the full story.
    inner: DaemonChildInner,
    /// Process ID of the spawned daemon.  Kept even for `Opaque`
    /// variants (where it is `0`) so callers can log a consistent
    /// `pid=…` field.
    pid: u32,
}

impl DaemonChildHandle {
    /// Construct an opaque handle — used by the UAC elevation path
    /// where `ShellExecuteW("runas")` hands off to the shell and never
    /// returns a usable PID / `HANDLE`.
    #[cfg(windows)]
    #[must_use]
    pub(crate) const fn opaque() -> Self {
        Self {
            inner: DaemonChildInner::Opaque,
            pid: 0,
        }
    }

    /// Construct from a Unix `std::process::Child`.
    ///
    /// Only visible crate-internally because all construction must
    /// funnel through [`crate::daemon_spawn`] — exposing this publicly
    /// would let external code bypass our spawn hardening (detached
    /// stdio, no handle inheritance, etc.).
    #[cfg(unix)]
    #[must_use]
    pub(crate) fn from_unix_child(child: std::process::Child) -> Self {
        let pid = child.id();
        Self {
            inner: DaemonChildInner::Unix { child: Some(child) },
            pid,
        }
    }

    /// Construct from a Windows `PROCESS_INFORMATION.hProcess` handle.
    ///
    /// Ownership transfers to the new handle — the caller must **not**
    /// `CloseHandle` it; [`Drop`] will do so when the
    /// `DaemonChildHandle` is dropped.
    #[cfg(windows)]
    #[must_use]
    pub(crate) const fn from_windows_process(
        handle: windows::Win32::Foundation::HANDLE,
        pid: u32,
    ) -> Self {
        Self {
            inner: DaemonChildInner::Windows {
                process_handle: Some(handle),
            },
            pid,
        }
    }

    /// Returns the spawned daemon's PID, or `0` for [`Self::opaque`].
    #[must_use]
    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }

    /// Poll the child non-blocking.
    ///
    /// * `Ok(None)` — child is still running.
    /// * `Ok(Some(code))` — child has exited with this code (Rust panics
    ///   surface as `101`, clap parse errors as `2`, graceful exit as `0`).
    /// * `Err(err)` — the poll itself failed (treat as unknown, keep retrying).
    ///
    /// For [`Self::opaque`] handles this is a no-op and always returns
    /// `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the underlying OS poll fails —
    /// e.g. Win32 `WaitForSingleObject` returns an unexpected status
    /// code, `GetExitCodeProcess` fails, or Unix
    /// `std::process::Child::try_wait` reports an `errno`.  Callers that
    /// are merely probing for early exit should treat poll errors as "still
    /// running" and keep retrying — the error path means we *don't know*,
    /// not that the child is dead.
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        match &mut self.inner {
            #[cfg(windows)]
            DaemonChildInner::Windows { process_handle } => try_wait_windows(*process_handle),
            #[cfg(unix)]
            DaemonChildInner::Unix { child } => {
                let Some(mut owned) = child.take() else {
                    return Ok(None);
                };
                match owned.try_wait() {
                    Ok(Some(status)) => Ok(Some(status.code().unwrap_or(-1))),
                    Ok(None) => {
                        // Still running — put the Child back so the next
                        // poll can reach it.
                        *child = Some(owned);
                        Ok(None)
                    }
                    Err(err) => {
                        *child = Some(owned);
                        Err(err)
                    }
                }
            }
            #[cfg(windows)]
            DaemonChildInner::Opaque => Ok(None),
        }
    }
}

/// Windows-side implementation of [`DaemonChildHandle::try_wait`].
///
/// Extracted so the platform-specific `use` statements (and their
/// interaction with clippy's `items_after_statements` lint) stay
/// out of the generic `match` body.
///
/// # Errors
///
/// Returns [`std::io::Error`] when `WaitForSingleObject` reports an
/// unexpected status or `GetExitCodeProcess` fails.
#[cfg(windows)]
fn try_wait_windows(
    process_handle: Option<windows::Win32::Foundation::HANDLE>,
) -> std::io::Result<Option<i32>> {
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

    let Some(handle) = process_handle else {
        return Ok(None);
    };
    // SAFETY: `handle` is a valid Win32 process handle owned by the
    // caller; the `Option::take` in Drop prevents double-close after
    // this call returns.
    #[expect(unsafe_code, reason = "Win32 WaitForSingleObject FFI")]
    let wait_result = unsafe { WaitForSingleObject(handle, 0) };
    if wait_result == WAIT_TIMEOUT {
        return Ok(None);
    }
    if wait_result != WAIT_OBJECT_0 {
        return Err(std::io::Error::other(format!(
            "WaitForSingleObject returned {:#x} — cannot determine child state",
            wait_result.0,
        )));
    }
    let mut exit_code: u32 = 0;
    // SAFETY: `handle` is valid; `exit_code` is a local u32 whose
    // address is valid for the call duration.  `&raw mut` avoids the
    // clippy::borrow_as_ptr warning.
    #[expect(unsafe_code, reason = "Win32 GetExitCodeProcess FFI")]
    let got_code = unsafe { GetExitCodeProcess(handle, &raw mut exit_code) };
    got_code.map_err(|err| std::io::Error::other(err.to_string()))?;
    // Windows exit codes are documented as `u32` by `GetExitCodeProcess`
    // but the historical Unix-style return type is `i32` (high bit
    // signals an exception code).  `u32::cast_signed` is the
    // documented exact-bit-pattern reinterpret that preserves both
    // representations without triggering `clippy::cast_possible_wrap`.
    let signed = exit_code.cast_signed();
    Ok(Some(signed))
}

impl Drop for DaemonChildHandle {
    fn drop(&mut self) {
        match &mut self.inner {
            #[cfg(windows)]
            DaemonChildInner::Windows { process_handle } => {
                if let Some(handle) = process_handle.take() {
                    use windows::Win32::Foundation::CloseHandle;
                    // SAFETY: handle was obtained from CreateProcessW,
                    // is not aliased, and `take()` guarantees single-close.
                    #[expect(unsafe_code, reason = "closing Win32 process handle on drop")]
                    let close_result = unsafe { CloseHandle(handle) };
                    drop(close_result);
                }
            }
            #[cfg(unix)]
            DaemonChildInner::Unix { .. } => {
                // `std::process::Child::drop` does NOT reap the zombie
                // on Unix — that's by design, to let callers explicitly
                // choose between `wait`, `kill`, or detachment.  We just
                // let it go; the init process will adopt and reap when
                // the daemon exits.  Not holding a reference keeps
                // startup-time resource usage bounded.
            }
            #[cfg(windows)]
            DaemonChildInner::Opaque => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DaemonChildHandle;

    /// Opaque handles report PID 0 and never observe an exit.
    ///
    /// Locks in the contract used by the UAC / `ShellExecuteW("runas")`
    /// spawn path: no handle, no exit code, `try_wait` must be safe
    /// and return `Ok(None)` every time so the retry loop falls back
    /// to plain connect-timeout semantics.
    #[cfg(windows)]
    #[test]
    fn opaque_handle_never_observes_exit() {
        let mut handle = DaemonChildHandle::opaque();
        assert_eq!(handle.pid(), 0);
        assert!(matches!(handle.try_wait(), Ok(None)));
        // A second poll must also be Ok(None) — no stateful side effect.
        assert!(matches!(handle.try_wait(), Ok(None)));
    }

    /// A Unix-child handle built from a short-lived `true` command
    /// observes the exit within a few milliseconds.  Regression guard
    /// for the `Unix { child: Some(child) }` + `Option::take` +
    /// put-back dance in `try_wait` — a bug there would either leak
    /// the child or lose the exit code.
    #[cfg(unix)]
    #[test]
    fn unix_child_handle_observes_exit() {
        let child = std::process::Command::new("true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .spawn()
            .expect("`true` must be on PATH on every Unix ship box");
        let mut handle = DaemonChildHandle::from_unix_child(child);
        assert!(handle.pid() > 0, "Unix children have a real PID");

        // Poll up to ~1 s for the child to report exit.  `true`
        // returns immediately but we still need to let the kernel
        // reap it; a generous bound keeps this deterministic on
        // overloaded CI.
        let mut observed: Option<i32> = None;
        for _ in 0_u32..100_u32 {
            match handle.try_wait() {
                Ok(Some(code)) => {
                    observed = Some(code);
                    break;
                }
                Ok(None) => std::thread::sleep(core::time::Duration::from_millis(10)),
                Err(err) => panic!("try_wait failed unexpectedly: {err}"),
            }
        }
        assert_eq!(observed, Some(0_i32), "`true` must exit with status 0");
    }
}
