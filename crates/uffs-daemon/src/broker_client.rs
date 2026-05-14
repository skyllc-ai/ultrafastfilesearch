// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon-side broker client (D7.7).
//!
//! When running on Windows, the daemon can optionally use the Access Broker
//! to obtain elevated volume handles instead of requiring its own elevation.
//!
//! Flow:
//! 1. Check if the broker pipe exists ([`uffs_broker_protocol::PIPE_NAME`])
//! 2. Connect to it
//! 3. Encode the drive letter via
//!    [`uffs_broker_protocol::HandleRequest::encode`]
//! 4. Decode the response via [`uffs_broker_protocol::HandleResponse::parse`]
//! 5. Use the handle for MFT reading
//!
//! The wire format used to be duplicated here as a `const BROKER_PIPE_NAME`
//! plus hand-rolled byte-slicing with a `// must match
//! uffs-broker/src/broker.rs` reviewer-comment as the only protection
//! against drift.  F5 (issue #205) promoted those shared symbols to
//! the dedicated [`uffs_broker_protocol`] crate, eliminating the
//! textual coupling.
//!
//! `uffs-broker-protocol` is scoped to
//! `[target.'cfg(windows)'.dependencies]` in `Cargo.toml`, so it isn't
//! an extern crate at all on non-Windows targets — no `use … as _;`
//! marker is needed.

/// Check if the Access Broker is available (pipe exists).
#[cfg(windows)]
pub(crate) fn broker_available() -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    use uffs_broker_protocol::PIPE_NAME;
    use windows::Win32::Storage::FileSystem::GetFileAttributesW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = std::ffi::OsStr::new(PIPE_NAME)
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
pub(crate) fn request_volume_handle(
    drive_letter: uffs_mft::platform::DriveLetter,
) -> anyhow::Result<u64> {
    use std::io::{Read as _, Write as _};

    use uffs_broker_protocol::{
        HandleRequest, HandleResponse, PIPE_NAME, RESPONSE_WIRE_LEN, Status,
    };

    // Connect to broker pipe
    let pipe_path = std::path::Path::new(PIPE_NAME);
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path)
        .map_err(|err| anyhow::anyhow!("Failed to connect to broker: {err}"))?;

    // Encode the 1-byte request via the shared protocol module.
    // `uffs-broker-protocol` is a leaf crate with no `uffs-mft` dep,
    // so we convert at the boundary.
    let request_bytes = HandleRequest {
        drive: drive_letter.as_char(),
    }
    .encode();
    pipe.write_all(&request_bytes)?;
    pipe.flush()?;

    // Read and parse the 9-byte response via the shared protocol module.
    let mut response = [0_u8; RESPONSE_WIRE_LEN];
    pipe.read_exact(&mut response)?;
    let parsed = HandleResponse::parse(response).map_err(|parse_err| {
        anyhow::anyhow!("malformed broker response for drive {drive_letter}: {parse_err}")
    })?;

    match parsed.status {
        Status::Ok => {
            tracing::info!(
                drive = %drive_letter,
                handle = parsed.handle,
                "Received volume handle from broker"
            );
            Ok(parsed.handle)
        }
        Status::Error => {
            anyhow::bail!("Broker returned Status::Error for drive {drive_letter}")
        }
    }
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
pub(crate) fn request_volume_handle(
    _drive_letter: uffs_mft::platform::DriveLetter,
) -> anyhow::Result<u64> {
    anyhow::bail!("Access Broker is a Windows-only feature")
}
