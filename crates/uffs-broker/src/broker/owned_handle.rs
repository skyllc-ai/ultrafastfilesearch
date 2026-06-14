// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `Send`-safe RAII wrapper for an owned Win32 `HANDLE` (SBB-2).
//!
//! The broker's multi-instance serve loop (FU-5) hands a **connected pipe
//! instance** off to a worker thread.  A raw `HANDLE` is neither `Send` (so it
//! can't move into the thread) nor self-closing (so every path must remember
//! `CloseHandle`).  `OwnedHandle` wraps it with a documented `Send` impl and a
//! `Drop` that closes exactly once — so a worker that panics still releases the
//! pipe instance.

#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

/// Owns a Win32 kernel handle and closes it on drop.
#[cfg(windows)]
pub(super) struct OwnedHandle(HANDLE);

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "kernel HANDLE has no thread affinity; safe to move between threads"
)]
// SAFETY: a Win32 kernel handle is a process-wide value with no thread
// affinity, so moving it between threads is sound.  Concurrent *use* of the
// same handle still requires external synchronisation, exactly as with the raw
// Win32 API — `OwnedHandle` only makes the *move* type-safe.
unsafe impl Send for OwnedHandle {}

#[cfg(windows)]
impl OwnedHandle {
    /// Take ownership of `handle`; its lifetime is now tied to this value.
    pub(super) const fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    /// Borrow the raw handle for an FFI call without giving up ownership.
    ///
    /// The returned `HANDLE` must not outlive `self` (which closes it on drop).
    pub(super) const fn raw(&self) -> HANDLE {
        self.0
    }
}

#[cfg(windows)]
impl Drop for OwnedHandle {
    #[expect(unsafe_code, reason = "CloseHandle for the owned handle")]
    fn drop(&mut self) {
        // SAFETY: `self.0` was handed to `new` as an owned, still-open handle
        // and is closed exactly once here.
        if let Err(err) = unsafe { windows::Win32::Foundation::CloseHandle(self.0) } {
            tracing::debug!(err = ?err, "CloseHandle failed dropping OwnedHandle");
        }
    }
}
