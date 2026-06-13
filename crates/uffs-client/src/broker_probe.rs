// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Detect a running UFFS Access Broker.
//!
//! The broker is an elevated Windows service that vends read-only NTFS
//! volume handles to a non-elevated daemon.  [`broker_pipe_present`] lets
//! the client's daemon-spawn logic ([`crate::daemon_spawn`]) decide
//! whether a non-elevated `uffs` can start the daemon anyway (the daemon
//! then obtains handles from the broker) instead of returning
//! [`crate::error::ClientError::DaemonNeedsElevation`].
//!
//! This whole module is `#[cfg(windows)]` (declared so in `lib.rs`), so
//! it carries no per-item cfg gates.

/// Return `true` when the UFFS Access Broker named pipe exists — i.e. the
/// broker Windows service is installed and running.
///
/// Mirrors `uffs-daemon::broker_client::broker_available` (the daemon
/// can't be a dependency of the client), using a single
/// `GetFileAttributesW` probe on [`uffs_broker_protocol::PIPE_NAME`] —
/// cheap, opens no handle, no side effects.
pub(crate) fn broker_pipe_present() -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    use uffs_broker_protocol::PIPE_NAME;
    use windows::Win32::Storage::FileSystem::GetFileAttributesW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = std::ffi::OsStr::new(PIPE_NAME)
        .encode_wide()
        .chain(Some(0))
        .collect();

    #[expect(unsafe_code, reason = "GetFileAttributesW probe for pipe existence")]
    // SAFETY: `wide` is a null-terminated UTF-16 buffer that outlives the
    // call; `GetFileAttributesW` only reads through the pointer.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };

    // INVALID_FILE_ATTRIBUTES (u32::MAX) means the pipe does not exist.
    attrs != u32::MAX
}
