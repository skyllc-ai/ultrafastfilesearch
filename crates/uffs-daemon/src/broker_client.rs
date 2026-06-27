// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon-side broker client (D7.7).
//!
//! When running on Windows, the daemon can optionally use the Access Broker
//! to obtain elevated volume handles instead of requiring its own elevation.
//!
//! Flow:
//! 1. Check if the broker pipe exists (`uffs_broker_protocol::PIPE_NAME`)
//! 2. Connect to it
//! 3. Encode the drive letter via `uffs_broker_protocol::HandleRequest::encode`
//! 4. Decode the response via `uffs_broker_protocol::HandleResponse::parse`
//! 5. Use the handle for MFT reading
//!
//! The wire format used to be duplicated here as a `const BROKER_PIPE_NAME`
//! plus hand-rolled byte-slicing with a `// must match
//! uffs-broker/src/broker.rs` reviewer-comment as the only protection
//! against drift.  F5 (issue #205) promoted those shared symbols to
//! the dedicated `uffs_broker_protocol` crate, eliminating the
//! textual coupling.
//!
//! `uffs-broker-protocol` is scoped to
//! `[target.'cfg(windows)'.dependencies]` in `Cargo.toml`, so it isn't
//! an extern crate at all on non-Windows targets — no `use … as _;`
//! marker is needed.

// NOTE: there is intentionally no `broker_available()` probe.  A
// `GetFileAttributesW` existence check on the pipe *connects to* the broker's
// single instance and leaves it busy, starving the real handle request with
// ERROR_PIPE_BUSY (2026-06-13 VM finding).  Broker presence is established
// solely by `request_volume_handle` attempting the connection.

/// Request a volume handle from the broker for a drive letter.
///
/// Returns the raw handle value (as a `u64`) that can be used for MFT reading.
/// The handle is already duplicated into our process by the broker.
#[cfg(windows)]
pub(crate) fn request_volume_handle(
    drive_letter: uffs_mft::platform::DriveLetter,
) -> anyhow::Result<u64> {
    let response = broker_pipe_round_trip(drive_letter)?;
    interpret_handle_response(drive_letter, response)
}

/// Open the broker pipe, send the 1-byte drive request, and read the raw
/// 9-byte response.  Split from [`request_volume_handle`] to keep both under
/// the cognitive-complexity ceiling and to isolate the I/O failure points
/// (the diagnostic `tracing` calls pinpoint where a non-elevated daemon's
/// access to the broker pipe breaks).
#[cfg(windows)]
fn broker_pipe_round_trip(
    drive_letter: uffs_mft::platform::DriveLetter,
) -> anyhow::Result<[u8; uffs_broker_protocol::RESPONSE_WIRE_LEN]> {
    use std::io::{Read as _, Write as _};

    use uffs_broker_protocol::{HandleRequest, PIPE_NAME, RESPONSE_WIRE_LEN};

    tracing::debug!(drive = %drive_letter, pipe = PIPE_NAME, "Opening broker pipe");
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(std::path::Path::new(PIPE_NAME))
        .map_err(|err| anyhow::anyhow!("opening broker pipe: {err}"))?;
    tracing::debug!(drive = %drive_letter, "Broker pipe opened; sending request");

    let request_bytes = HandleRequest {
        drive: drive_letter.as_char(),
    }
    .encode();
    pipe.write_all(&request_bytes)?;
    pipe.flush()?;

    let mut response = [0_u8; RESPONSE_WIRE_LEN];
    pipe.read_exact(&mut response)?;
    tracing::debug!(drive = %drive_letter, "Broker response received");
    Ok(response)
}

/// Parse and interpret the broker's 9-byte response into a handle value.
///
/// Split out of [`request_volume_handle`] to keep that function under the
/// cognitive-complexity ceiling.
#[cfg(windows)]
fn interpret_handle_response(
    drive_letter: uffs_mft::platform::DriveLetter,
    response: [u8; uffs_broker_protocol::RESPONSE_WIRE_LEN],
) -> anyhow::Result<u64> {
    use uffs_broker_protocol::{HandleResponse, Status};

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
