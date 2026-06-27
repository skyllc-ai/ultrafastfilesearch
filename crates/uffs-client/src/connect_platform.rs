// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Platform-specific `platform_connect` implementations for
//! [`crate::connect::UffsClient`] (async variant).
//!
//! Extracted from `connect.rs` for file-size policy compliance.
//! All items live on [`crate::connect::UffsClient`] via split `impl` blocks —
//! no public surface moves.  Mirrors the sync-path split in
//! `connect_sync_platform.rs`.

use tokio::io::BufReader;

use crate::connect::UffsClient;

/// Unix: connect via Unix domain socket.
#[cfg(unix)]
impl UffsClient {
    /// Platform-specific connection over Unix domain socket.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ClientError`] if the Unix socket
    /// connection fails.
    pub(crate) async fn platform_connect() -> Result<Self, crate::error::ClientError> {
        let sock_path = crate::daemon_ctl::socket_path();
        let stream = tokio::net::UnixStream::connect(&sock_path)
            .await
            .map_err(|err| crate::error::ClientError::ConnectionFailed(err.to_string()))?;

        let (read_half, write_half) = stream.into_split();
        Ok(Self::from_parts(
            BufReader::new(Box::new(read_half)),
            Box::new(write_half),
        ))
    }
}

/// Windows: connect via named pipe using native tokio support.
///
/// This replaces the previous `AF_UNIX` path which required two
/// background threads per connection (each with its own tokio runtime)
/// to bridge a blocking `std::os::windows::net::UnixStream` to async via
/// `tokio::io::duplex`.  The bridge existed only because
/// `tokio::net::UnixStream` is `cfg(unix)`-only; tokio's native named-pipe
/// client is `AsyncRead + AsyncWrite` directly, so the entire bridge
/// machinery — and the `ws2_32.dll` import cost — disappears.
#[cfg(windows)]
impl UffsClient {
    /// Platform-specific connection via tokio named-pipe client.
    pub(crate) async fn platform_connect() -> Result<Self, crate::error::ClientError> {
        use tokio::net::windows::named_pipe::ClientOptions;

        /// `ERROR_PIPE_BUSY` — all server instances are currently connected;
        /// retry after a short sleep.  The daemon creates the next instance
        /// immediately after accept, but there is a narrow window.
        const ERROR_PIPE_BUSY: i32 = 231;

        let pipe_name = crate::daemon_ctl::pipe_name()
            .map_err(|err| crate::error::ClientError::ConnectionFailed(err.to_string()))?;

        let pipe = {
            let mut last_err: Option<std::io::Error> = None;
            let mut client = None;
            for attempt in 0_u32..5 {
                match ClientOptions::new()
                    .read(true)
                    .write(true)
                    .open(pipe_name.as_str())
                {
                    Ok(stream) => {
                        client = Some(stream);
                        break;
                    }
                    Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                        tokio::time::sleep(core::time::Duration::from_millis(u64::from(
                            10_u32 << attempt,
                        )))
                        .await;
                        last_err = Some(err);
                    }
                    Err(err) => {
                        return Err(crate::error::ClientError::ConnectionFailed(err.to_string()));
                    }
                }
            }
            client.ok_or_else(|| {
                crate::error::ClientError::ConnectionFailed(last_err.map_or_else(
                    || "pipe busy after retries".to_owned(),
                    |err| format!("pipe busy after retries: {err}"),
                ))
            })?
        };

        let (read_half, write_half) = tokio::io::split(pipe);
        Ok(Self::from_parts(
            BufReader::new(Box::new(read_half)),
            Box::new(write_half),
        ))
    }
}
