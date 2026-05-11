// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon-side broker client (D7.7).
//!
//! When running on Windows, the daemon can optionally use the Access Broker
//! to obtain elevated volume handles instead of requiring its own elevation.
//!
//! Flow:
//! 1. Check if the broker pipe exists (`\\.\pipe\uffs-broker`)
//! 2. Connect to it
//! 3. Send drive letter (1 byte)
//! 4. Receive status (1 byte) + handle value (8 bytes)
//! 5. Use the handle for MFT reading

/// The broker pipe name (must match `uffs-broker/src/broker.rs`).
///
/// File-local: only consumed by `broker_available` and
/// `request_volume_handle` below.  Kept private (not `pub(crate)`) to
/// match the Windows-only scope of its consumers and avoid polluting
/// the crate's internal namespace with a constant no other module uses.
#[cfg(windows)]
const BROKER_PIPE_NAME: &str = r"\\.\pipe\uffs-broker";

/// Check if the Access Broker is available (pipe exists).
#[cfg(windows)]
pub(crate) fn broker_available() -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::Win32::Storage::FileSystem::GetFileAttributesW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = std::ffi::OsStr::new(BROKER_PIPE_NAME)
        .encode_wide()
        .chain(Some(0))
        .collect();

    #[expect(unsafe_code, reason = "GetFileAttributesW to check pipe existence")]
    // SAFETY: `wide` is a null-terminated UTF-16 buffer that lives for
    // the duration of the call; `GetFileAttributesW` only reads from
    // the pointer.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };

    // If not INVALID_FILE_ATTRIBUTES, the pipe exists
    attrs != u32::MAX
}

/// Request a volume handle from the broker for a drive letter.
///
/// Returns the raw handle value (as a `u64`) that can be used for MFT reading.
/// The handle is already duplicated into our process by the broker.
#[cfg(windows)]
pub(crate) fn request_volume_handle(drive_letter: char) -> anyhow::Result<u64> {
    use std::io::{Read as _, Write as _};

    // Connect to broker pipe
    let pipe_path = std::path::Path::new(BROKER_PIPE_NAME);
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path)
        .map_err(|err| anyhow::anyhow!("Failed to connect to broker: {err}"))?;

    // Send drive letter (1 byte)
    let drive_byte = drive_letter.to_ascii_uppercase() as u8;
    pipe.write_all(&[drive_byte])?;
    pipe.flush()?;

    // Read response: 1 byte status + 8 bytes handle
    let mut response = [0_u8; 9];
    pipe.read_exact(&mut response)?;

    let status = response[0];
    if status != 0 {
        anyhow::bail!("Broker returned error status {status} for drive {drive_letter}:");
    }

    let handle_bytes: [u8; 8] = response[1..9]
        .try_into()
        .map_err(|err| anyhow::anyhow!("broker response handle slice size mismatch: {err}"))?;
    let handle_value = u64::from_le_bytes(handle_bytes);

    tracing::info!(
        drive = %drive_letter,
        handle = handle_value,
        "Received volume handle from broker"
    );

    Ok(handle_value)
}

/// Non-Windows: broker is never available.
#[cfg(not(windows))]
pub(crate) const fn broker_available() -> bool {
    false
}

/// Non-Windows: broker request always fails.
#[cfg(not(windows))]
#[expect(
    clippy::single_call_fn,
    reason = "platform stub — mirrors Windows variant"
)]
pub(crate) fn request_volume_handle(_drive_letter: char) -> anyhow::Result<u64> {
    anyhow::bail!("Access Broker is a Windows-only feature")
}
