// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows `AF_UNIX` IPC server (Windows 10 1803+).
//!
//! Mirrors the Unix-domain-socket transport that the macOS / Linux
//! [`crate::ipc::run_ipc_server`] uses, but bridges the std blocking
//! [`std::os::windows::net::UnixStream`] handed out by `accept()` into
//! tokio duplex channels so [`crate::ipc::IpcServer::handle_connection`]
//! can drive the same `async` reader/writer loops on every platform.
//!
//! Split out of `ipc.rs` to keep that file under the workspace's 800
//! LOC file-size policy; every helper here is Windows-only and has no
//! macOS / Linux counterpart.

#![cfg(windows)]

use alloc::sync::Arc;

use crate::events::{EventReceiver, EventSender};
use crate::handler::RequestHandler;
use crate::index::IndexManager;
use crate::ipc::{IDLE_CONNECTION_SECS, IpcServer, MAX_CONNECTIONS};
use crate::lifecycle::LifecycleHandle;

/// Windows IPC server — uses Unix domain sockets (Windows 10 1803+).
///
/// Mirrors the Unix version: secure dir (icacls owner-only ACL), socket
/// file permissions, max connections, peer verification via ACL.
pub(crate) async fn run_ipc_server(
    index: Arc<IndexManager>,
    lifecycle: LifecycleHandle,
) -> anyhow::Result<()> {
    use std::os::windows::net::UnixListener as StdUnixListener;

    let sock_path = IpcServer::socket_path();

    if let Some(parent) = sock_path.parent() {
        uffs_security::fs::create_secure_dir(parent)?;
    }

    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    // Windows: use std blocking UnixListener in a spawn_blocking loop,
    // bridge each connection via tokio::io::duplex.
    let std_listener = StdUnixListener::bind(&sock_path)?;

    // Set socket permissions to owner-only AFTER bind creates the file
    uffs_security::fs::set_file_permissions_owner_only(&sock_path)?;

    tracing::info!(path = %sock_path.display(), "IPC server listening (Windows AF_UNIX)");

    let events = index.event_sender().clone();
    let handler = Arc::new(RequestHandler {
        index,
        lifecycle: lifecycle.clone(),
    });

    tracing::info!("[daemon-ipc] entering accept loop");
    loop {
        accept_one_unix_client(&std_listener, &handler, &lifecycle, &events).await?;
    }
}

/// Single iteration of the [`run_ipc_server`] accept loop: drive one
/// blocking `accept()`, run gatekeeping (peer verify + max-connection
/// cap), then hand the socket off to
/// [`spawn_unix_accepted_connection`].
async fn accept_one_unix_client(
    std_listener: &std::os::windows::net::UnixListener,
    handler: &Arc<RequestHandler>,
    lifecycle: &LifecycleHandle,
    events: &EventSender,
) -> anyhow::Result<()> {
    let accept_listener = std_listener.try_clone()?;
    tracing::info!("[daemon-ipc] waiting for accept...");
    let accept_result = tokio::task::spawn_blocking(move || accept_listener.accept()).await?;
    log_accept_outcome(&accept_result);

    let (std_stream, _addr) = accept_result?;
    std_stream.set_read_timeout(Some(core::time::Duration::from_secs(IDLE_CONNECTION_SECS)))?;

    if !IpcServer::verify_peer_credentials_win() {
        tracing::warn!("[daemon-ipc] Rejected connection (peer verification failed)");
        return Ok(());
    }

    if lifecycle.active_connections() >= MAX_CONNECTIONS {
        tracing::warn!(
            active = lifecycle.active_connections(),
            max = MAX_CONNECTIONS,
            "[daemon-ipc] Max connections reached"
        );
        return Ok(());
    }

    spawn_unix_accepted_connection(
        std_stream,
        Arc::clone(handler),
        lifecycle.clone(),
        events.subscribe(),
    )
}

/// Trace one round of `accept()` outcome — extracted so `run_ipc_server`
/// stays under clippy's cognitive-complexity budget while still surfacing
/// individual accept failures in the daemon log.
fn log_accept_outcome(
    result: &std::io::Result<(
        std::os::windows::net::UnixStream,
        std::os::windows::net::SocketAddr,
    )>,
) {
    match result {
        Ok(_) => tracing::info!("[daemon-ipc] accept() returned OK"),
        Err(accept_err) => {
            tracing::info!(error = %accept_err, "[daemon-ipc] accept() returned Err");
        }
    }
}

/// Per-accept handoff for the Windows `AF_UNIX` [`run_ipc_server`].
///
/// Splits the std blocking socket into independent read/write halves,
/// builds a pair of tokio duplex bridges, spawns the read/write bridge
/// threads, and dispatches [`IpcServer::handle_connection`] on the
/// async side.
fn spawn_unix_accepted_connection(
    std_stream: std::os::windows::net::UnixStream,
    handler: Arc<RequestHandler>,
    lifecycle: LifecycleHandle,
    event_rx: EventReceiver,
) -> anyhow::Result<()> {
    // Bridge std blocking socket to async duplex channels.  Each bridge
    // thread gets its own dedicated tokio current-thread runtime —
    // `Handle::block_on` on the main runtime caused `DuplexStream`
    // waker issues (premature EOF after ~10 RPCs).
    let std_read = std_stream.try_clone()?;
    let std_write = std_stream;

    let (async_read, bridge_write) = tokio::io::duplex(65536);
    let (bridge_read, async_write) = tokio::io::duplex(65536);

    spawn_socket_to_bridge_thread(std_read, bridge_write);
    spawn_bridge_to_socket_thread(bridge_read, std_write);

    lifecycle.connection_opened();
    let lc_clone = lifecycle;

    tokio::spawn(async move {
        tracing::debug!(
            connections = lc_clone.active_connections(),
            "Client connected"
        );

        if let Err(conn_err) =
            IpcServer::handle_connection(async_read, async_write, handler, event_rx).await
        {
            tracing::debug!(error = %conn_err, "Connection ended");
        }

        lc_clone.connection_closed();
        tracing::debug!(
            connections = lc_clone.active_connections(),
            "Client disconnected"
        );
    });

    Ok(())
}

/// Background bridge: pump bytes from the std blocking socket into the
/// tokio `bridge_write` half until EOF or the first I/O error.
fn spawn_socket_to_bridge_thread(
    std_read: std::os::windows::net::UnixStream,
    mut bridge_write: tokio::io::DuplexStream,
) {
    std::thread::spawn(move || {
        use std::io::Read as _;
        tracing::info!("[daemon-bridge-read] thread started");
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .inspect_err(|rt_err| {
                tracing::info!(error = %rt_err, "[daemon-bridge-read] failed to create runtime");
            })
        else {
            return;
        };
        rt.block_on(async move {
            use tokio::io::AsyncWriteExt as _;
            let mut reader = std::io::BufReader::new(std_read);
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        tracing::info!("[daemon-bridge-read] EOF from client socket");
                        break;
                    }
                    Err(read_err) => {
                        tracing::info!(error = %read_err, "[daemon-bridge-read] read error");
                        break;
                    }
                    Ok(n) => {
                        // `n <= buf.len()` is guaranteed by the `Read`
                        // contract; `.get(..n)` keeps clippy happy and
                        // turns a programmer-error overflow into a
                        // benign empty write.
                        let payload = buf.get(..n).unwrap_or(&[]);
                        if let Err(write_err) = bridge_write.write_all(payload).await {
                            tracing::info!(
                                error = %write_err,
                                "[daemon-bridge-read] bridge write failed"
                            );
                            break;
                        }
                    }
                }
            }
        });
        tracing::info!("[daemon-bridge-read] thread exiting");
    });
}

/// Background bridge: drain the tokio `bridge_read` half and forward
/// every chunk back onto the std blocking socket; flush after each
/// write so client RPCs are observable promptly.
fn spawn_bridge_to_socket_thread(
    mut bridge_read: tokio::io::DuplexStream,
    std_write: std::os::windows::net::UnixStream,
) {
    std::thread::spawn(move || {
        use std::io::Write as _;
        tracing::info!("[daemon-bridge-write] thread started");
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .inspect_err(|rt_err| {
                tracing::info!(error = %rt_err, "[daemon-bridge-write] failed to create runtime");
            })
        else {
            return;
        };
        let mut writer = std_write;
        rt.block_on(async {
            use tokio::io::AsyncReadExt as _;
            let mut buf = [0_u8; 8192];
            loop {
                match bridge_read.read(&mut buf).await {
                    Ok(0) => {
                        tracing::info!("[daemon-bridge-write] EOF from bridge");
                        break;
                    }
                    Err(read_err) => {
                        tracing::info!(
                            error = %read_err,
                            "[daemon-bridge-write] bridge read error"
                        );
                        break;
                    }
                    Ok(n) => {
                        // `n <= buf.len()` guaranteed by `AsyncRead`;
                        // `.get(..n)` keeps clippy's
                        // indexing-can-panic check satisfied.
                        let payload = buf.get(..n).unwrap_or(&[]);
                        if writer.write_all(payload).is_err() {
                            tracing::info!("[daemon-bridge-write] socket write_all failed");
                            break;
                        }
                        if writer.flush().is_err() {
                            tracing::info!("[daemon-bridge-write] socket flush failed");
                            break;
                        }
                    }
                }
            }
        });
        tracing::info!("[daemon-bridge-write] thread exiting");
    });
}
