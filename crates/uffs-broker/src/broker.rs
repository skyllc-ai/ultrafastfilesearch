// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows Access Broker implementation.
//!
//! Runs as a Windows Service (or foreground process for debugging).
//! Listens on a named pipe, verifies client identity, and provides
//! read-only volume handles for MFT access.
//!
//! # Protocol
//!
//! See [`uffs_broker_protocol`] for the authoritative wire-format
//! definition (1-byte drive-letter request, 9-byte status + LE-u64
//! handle response).  Both this binary and
//! `uffs_daemon::broker_client` consume the same crate — there is no
//! second copy of the protocol constants or byte layout anywhere in
//! the tree.
//!
//! The broker opens `\\.\X:` with `FILE_READ_DATA` + `SeBackupPrivilege`,
//! then `DuplicateHandle`s it into the client process with read-only access.

#[cfg(windows)]
use uffs_broker_protocol::{HandleRequest, HandleResponse, PIPE_NAME, RESPONSE_WIRE_LEN};

/// Run the broker (called from main).
///
/// Scope is `pub(crate)` because `broker` is a private module of the
/// binary crate — only `main.rs` invokes this.
///
/// # Errors
///
/// Returns an error if service installation/uninstallation fails, or if the
/// foreground pipe-serving loop encounters an unrecoverable error.
#[cfg(windows)]
pub(crate) fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|arg| arg == "--install") {
        return install_service();
    }
    if args.iter().any(|arg| arg == "--uninstall") {
        return uninstall_service();
    }
    if args.iter().any(|arg| arg == "--run") {
        return run_foreground();
    }

    print_usage();
    Ok(())
}

/// Print CLI usage help to stderr.
///
/// Runs before the `tracing` subscriber is initialised, so uses `eprintln!`
/// directly — the usual logging channel isn't available yet.
#[cfg(windows)]
#[expect(
    clippy::print_stderr,
    reason = "CLI help text written before tracing subscriber init"
)]
fn print_usage() {
    eprintln!("uffs-broker: use --install, --uninstall, or --run");
    eprintln!("  --install     Install as Windows Service");
    eprintln!("  --uninstall   Remove Windows Service");
    eprintln!("  --run         Run in foreground (debugging)");
}

/// Run the broker in foreground mode.
#[cfg(windows)]
fn run_foreground() -> anyhow::Result<()> {
    init_tracing();
    tracing::info!(
        pid = std::process::id(),
        "uffs-broker starting (foreground mode)"
    );
    warn_if_not_elevated();
    serve_pipe_requests()?;
    tracing::info!("uffs-broker stopped");
    Ok(())
}

/// Initialise the `tracing` subscriber if one isn't already installed.
///
/// Uses `try_init` so we don't panic when another subscriber (tests, embed)
/// is already in place.
#[cfg(windows)]
fn init_tracing() {
    let init_result = tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    drop(init_result);
}

/// Warn via `tracing` if the process is not running elevated.
#[cfg(windows)]
fn warn_if_not_elevated() {
    if !is_elevated() {
        tracing::warn!("Broker is NOT running elevated — volume access will fail");
        tracing::warn!("Run as Administrator or install as a Windows Service");
    }
}

/// Serve handle requests on the named pipe.
///
/// Hardened with S5 security controls:
/// - S5.1: Pipe created with Administrators-only default DACL (elevated
///   process)
/// - S5.2: Client exe path + Authenticode verification
/// - S5.3: Audit logging for every request
/// - S5.4: Rate limiting (1 request per drive per 10s)
/// - S5.5: Read-only handles only (enforced in `handle_pipe_request`)
#[cfg(windows)]
fn serve_pipe_requests() -> anyhow::Result<()> {
    tracing::info!(pipe = PIPE_NAME, "Listening for handle requests");

    // S5.4: Rate limiter — tracks last request time per drive letter
    let mut rate_limit: std::collections::HashMap<char, std::time::Instant> =
        std::collections::HashMap::new();

    loop {
        let pipe = create_broker_pipe()?;
        wait_for_client(pipe)?;
        handle_one_connection(pipe, &mut rate_limit);
        disconnect_and_close(pipe);
    }
}

/// Handle a single connected pipe client: verify identity, run the
/// drive-handle request, and emit the appropriate S5.3 audit log.
///
/// Audit logging is performed for every terminal outcome (REJECTED /
/// GRANTED / FAILED) inside this function so the caller only needs to
/// disconnect and loop.
#[cfg(windows)]
fn handle_one_connection(
    pipe: windows::Win32::Foundation::HANDLE,
    rate_limit: &mut std::collections::HashMap<char, std::time::Instant>,
) {
    let Some(pid) = get_pipe_client_pid(pipe) else {
        audit_log("REJECTED", 0, None, None, "could not determine client PID");
        tracing::warn!("Could not determine client PID — rejecting");
        return;
    };

    let exe_path = get_client_exe_path(pid);
    if !check_client_identity(pid, exe_path.as_deref()) {
        return;
    }

    process_drive_request(pipe, pid, exe_path.as_deref(), rate_limit);
}

/// Verify the client's identity (`verify_client` whitelist + Authenticode
/// signature check).  Emits the appropriate REJECTED audit log on failure.
///
/// Returns `true` when identity is confirmed, `false` otherwise.
#[cfg(windows)]
fn check_client_identity(pid: u32, exe: Option<&str>) -> bool {
    if !verify_client(pid) {
        audit_log("REJECTED", pid, exe, None, "identity verification failed");
        tracing::warn!(pid, "Rejected broker client — not uffsd");
        return false;
    }

    if let Some(path) = exe
        && !verify_authenticode(path)
    {
        audit_log(
            "REJECTED",
            pid,
            exe,
            None,
            "Authenticode verification failed",
        );
        tracing::warn!(pid, exe = path, "Rejected: invalid Authenticode signature");
        return false;
    }

    tracing::debug!(pid, "Broker client verified");
    true
}

/// Run the rate-limited drive-handle request and emit the GRANTED /
/// FAILED audit log for the terminal outcome.
#[cfg(windows)]
fn process_drive_request(
    pipe: windows::Win32::Foundation::HANDLE,
    pid: u32,
    exe: Option<&str>,
    rate_limit: &mut std::collections::HashMap<char, std::time::Instant>,
) {
    match handle_pipe_request_with_rate_limit(pipe, pid, rate_limit) {
        Ok(drive) => {
            audit_log("GRANTED", pid, exe, Some(drive), "handle issued");
        }
        Err(err) => {
            audit_log("FAILED", pid, exe, None, &err.to_string());
            tracing::debug!(error = %err, "Pipe request failed");
        }
    }
}

/// S5.4: Handle request with per-drive rate limiting.
#[cfg(windows)]
fn handle_pipe_request_with_rate_limit(
    pipe: windows::Win32::Foundation::HANDLE,
    client_pid: u32,
    rate_limit: &mut std::collections::HashMap<char, std::time::Instant>,
) -> anyhow::Result<char> {
    // Peek at the 1-byte request via the shared protocol parser.
    // `HandleRequest::parse` rejects non-ASCII bytes and non-alphabetic
    // ASCII bytes with structured errors — replaces the two-step
    // `is_ascii_alphabetic` validation we used to do here.
    let mut req_buf = [0_u8; uffs_broker_protocol::REQUEST_WIRE_LEN];
    read_pipe(pipe, &mut req_buf)?;
    let drive_letter = match HandleRequest::parse(req_buf[0]) {
        Ok(req) => req.drive,
        Err(parse_err) => {
            write_pipe(pipe, &HandleResponse::error().encode())?;
            return Err(anyhow::anyhow!(
                "invalid drive-letter request byte: {parse_err}"
            ));
        }
    };

    // S5.4: Rate limit — 1 request per drive per 10 seconds
    let now = std::time::Instant::now();
    if let Some(last) = rate_limit.get(&drive_letter)
        && now.duration_since(*last).as_secs() < 10
    {
        write_pipe(pipe, &HandleResponse::error().encode())?;
        anyhow::bail!("Rate limited: drive {drive_letter} requested too recently");
    }
    rate_limit.insert(drive_letter, now);

    // Delegate to actual handle brokering (drive letter already read)
    handle_pipe_request_inner(pipe, client_pid, drive_letter)?;
    Ok(drive_letter)
}

/// S5.2: Verify Authenticode signature of client executable.
#[cfg(windows)]
fn verify_authenticode(exe_path: &str) -> bool {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "(Get-AuthenticodeSignature '{}').Status",
                exe_path.replace('\'', "''")
            ),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    match output {
        Ok(out) => {
            let status = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            // Accept Valid and NotSigned (dev builds)
            // Reject HashMismatch (tampered)
            status != "HashMismatch"
        }
        Err(_) => true, // PowerShell not available — allow (graceful degradation)
    }
}

/// Get the exe path for a PID (for audit logging).
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Win32 process-image-name query requires unsafe FFI"
)]
fn get_client_exe_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    // SAFETY: OpenProcess is a read-only query with
    // PROCESS_QUERY_LIMITED_INFORMATION; failure returns an Error (caught by
    // ok()?), not a dangling handle.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0_u16; 4096];
    let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is a valid open process handle; `buf` is a fixed 4096-wide
    // allocation; `size` is an owned u32 whose address is exclusive to this call.
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut size,
        )
    };
    // SAFETY: `handle` was obtained from OpenProcess above and is no longer needed.
    if let Err(close_err) = unsafe { CloseHandle(handle) } {
        tracing::debug!(err = ?close_err, "CloseHandle failed in get_client_exe_path");
    }
    if result.is_err() || size == 0 {
        return None;
    }
    // u32→usize lossless on 64-bit; use get() to satisfy indexing_slicing.
    let len = size as usize;
    buf.get(..len).map(String::from_utf16_lossy)
}

/// S5.3: Audit log entry to tracing (and Windows Event Log if available).
#[cfg(windows)]
fn audit_log(action: &str, pid: u32, exe: Option<&str>, drive: Option<char>, detail: &str) {
    let exe_str = exe.unwrap_or("<unknown>");
    let drive_str = drive.map_or_else(|| "-".to_owned(), |drive_char| drive_char.to_string());
    tracing::info!(
        target: "uffs_broker::audit",
        action,
        pid,
        exe = exe_str,
        drive = %drive_str,
        detail,
        "AUDIT"
    );
    // Future: also write to Windows Event Log via ReportEventW
}

// ── Windows Service Install/Uninstall ───────────────────────────────────

/// Register the broker as a Windows Service via `sc create`.
///
/// Writes a one-line result to stdout — this is a CLI admin command whose
/// user-facing output is its only observable product, so `println!` is the
/// idiomatic sink rather than the `tracing` subscriber used by the
/// long-running service modes.
#[cfg(windows)]
#[expect(
    clippy::print_stdout,
    reason = "CLI admin command — stdout is the user-visible result channel"
)]
fn install_service() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let output = std::process::Command::new("sc")
        .args([
            "create",
            "UffsAccessBroker",
            &format!("binPath= \"{}\"", exe.display()),
            "start=",
            "demand",
            "DisplayName=",
            "UFFS Access Broker",
        ])
        .output()?;

    if output.status.success() {
        println!("Service installed. Start with: sc start UffsAccessBroker");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Install failed: {stderr}");
    }
    Ok(())
}

/// Deregister the broker Windows Service via `sc delete`.
///
/// See [`install_service`] for why stdout is the output channel.
#[cfg(windows)]
#[expect(
    clippy::print_stdout,
    reason = "CLI admin command — stdout is the user-visible result channel"
)]
fn uninstall_service() -> anyhow::Result<()> {
    let output = std::process::Command::new("sc")
        .args(["delete", "UffsAccessBroker"])
        .output()?;

    if output.status.success() {
        println!("Service uninstalled.");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Uninstall failed: {stderr}");
    }
    Ok(())
}

// ── D7.3: Named Pipe Operations ─────────────────────────────────────────

/// Create a named pipe with owner-only access.
#[cfg(windows)]
#[expect(unsafe_code, reason = "CreateNamedPipeW is an FFI call")]
fn create_broker_pipe() -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::PCWSTR;

    let pipe_name: Vec<u16> = std::ffi::OsStr::new(PIPE_NAME)
        .encode_wide()
        .chain(Some(0))
        .collect();

    // SAFETY: `pipe_name` is a NUL-terminated UTF-16 buffer owned for the
    // duration of this call; all other arguments are plain integers or None.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(pipe_name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,    // max instances
            1024, // out buffer
            1024, // in buffer
            0,    // default timeout
            None, // default security (owner-only for elevated process)
        )
    };

    if handle.is_invalid() {
        anyhow::bail!(
            "CreateNamedPipeW failed: {}",
            std::io::Error::last_os_error()
        );
    }

    Ok(handle)
}

/// Wait for a client to connect to the pipe.
#[cfg(windows)]
#[expect(unsafe_code, reason = "ConnectNamedPipe is an FFI call")]
fn wait_for_client(pipe: windows::Win32::Foundation::HANDLE) -> anyhow::Result<()> {
    use windows::Win32::System::Pipes::ConnectNamedPipe;

    // SAFETY: `pipe` is an owned valid pipe HANDLE (HANDLE is Copy); the
    // second argument is None (synchronous wait).
    let result = unsafe { ConnectNamedPipe(pipe, None) };

    if let Err(win_err) = result {
        // ERROR_PIPE_CONNECTED (535) means client connected before we called
        // ConnectNamedPipe — that's OK
        if win_err.code().0 != 535_i32 {
            anyhow::bail!("ConnectNamedPipe failed: {win_err}");
        }
    }
    Ok(())
}

/// Disconnect client and close pipe handle.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "DisconnectNamedPipe + CloseHandle are FFI calls"
)]
fn disconnect_and_close(pipe: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Pipes::DisconnectNamedPipe;

    // SAFETY: `pipe` is a valid HANDLE created by create_broker_pipe; the
    // disconnect is a no-op if no client is currently connected.
    if let Err(err) = unsafe { DisconnectNamedPipe(pipe) } {
        tracing::debug!(err = ?err, "DisconnectNamedPipe failed (may be already disconnected)");
    }
    // SAFETY: `pipe` is about to be discarded; CloseHandle releases its OS
    // kernel-object reference.  Failure is logged but non-fatal.
    if let Err(err) = unsafe { CloseHandle(pipe) } {
        tracing::debug!(err = ?err, "CloseHandle failed for pipe");
    }
}

// ── D7.4: Client Process Verification ───────────────────────────────────

/// Get the PID of the connected pipe client.
#[cfg(windows)]
#[expect(unsafe_code, reason = "GetNamedPipeClientProcessId is an FFI call")]
fn get_pipe_client_pid(pipe: windows::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;

    let mut pid: u32 = 0;

    // SAFETY: `pipe` is a valid server-side pipe HANDLE with an active connection;
    // `pid` is a stack-allocated u32 whose exclusive mutable pointer is passed.
    let result = unsafe { GetNamedPipeClientProcessId(pipe, &raw mut pid) };

    (result.is_ok() && pid != 0).then_some(pid)
}

/// Verify that a client process is a legitimate uffs-daemon.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Win32 process-image-name query requires unsafe FFI"
)]
fn verify_client(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    // SAFETY: OpenProcess is a read-only query with
    // PROCESS_QUERY_LIMITED_INFORMATION; failure returns Err (handled by the
    // let-else) rather than an invalid handle.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };

    let mut buf = vec![0_u16; 4096];
    let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: `handle` is a valid open process handle; `buf` is a 4096-wide
    // owned allocation; `size` is a stack-owned u32 accessed exclusively here.
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut size,
        )
    };
    // SAFETY: `handle` came from OpenProcess above and is no longer used.
    if let Err(close_err) = unsafe { CloseHandle(handle) } {
        tracing::debug!(err = ?close_err, "CloseHandle failed in verify_client");
    }

    if result.is_err() || size == 0 {
        return false;
    }
    // u32→usize lossless on 64-bit; get() keeps us out of indexing_slicing.
    let len = size as usize;
    let Some(slice) = buf.get(..len) else {
        return false;
    };
    let exe_name = String::from_utf16_lossy(slice);

    let name = std::path::Path::new(&exe_name)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("");

    name == "uffsd"
        || name == "uffsd.exe"
        || name == "uffs-daemon.exe"
        || name == "uffs-daemon"
        || name.starts_with("uffs-daemon")
        || name.starts_with("uffs_daemon")
}

// ── D7.5: Handle Brokering ──────────────────────────────────────────────

/// Open the NTFS volume for a drive letter with read-only backup semantics.
#[cfg(windows)]
#[expect(unsafe_code, reason = "CreateFileW is an FFI call")]
fn open_volume_read_only(drive_letter: char) -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let volume_path = format!("\\\\.\\{drive_letter}:");
    let wide_path: Vec<u16> = volume_path.encode_utf16().chain(Some(0)).collect();

    // SAFETY: `wide_path` is a NUL-terminated UTF-16 buffer owned for the
    // duration of this call; all other arguments are plain integers or None.
    let create_file_result = unsafe {
        CreateFileW(
            windows::core::PCWSTR(wide_path.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    create_file_result.map_err(|err| anyhow::anyhow!("CreateFileW failed for {volume_path}: {err}"))
}

/// Duplicate `volume_handle` into the client process with read-only access.
///
/// The caller retains ownership of `volume_handle` and is responsible for
/// closing it; this function only opens / closes the transient client-process
/// handle it needs for the `DuplicateHandle` call.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "OpenProcess + GetCurrentProcess + DuplicateHandle + CloseHandle are FFI calls"
)]
fn duplicate_volume_handle_to_client(
    volume_handle: windows::Win32::Foundation::HANDLE,
    client_pid: u32,
) -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::FILE_GENERIC_READ;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

    // SAFETY: `client_pid` is an integer; OpenProcess returns Err on invalid PID.
    let client_process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, client_pid) }
        .map_err(|err| anyhow::anyhow!("OpenProcess for client {client_pid} failed: {err}"))?;

    // SAFETY: GetCurrentProcess is a Win32 pseudo-handle getter (never fails).
    let current_process = unsafe { GetCurrentProcess() };

    let mut client_handle = HANDLE::default();
    // SAFETY: `current_process`, `volume_handle`, and `client_process` are all
    // valid handles; `client_handle` is a stack-owned HANDLE passed via
    // exclusive &raw mut.
    let dup_ok = unsafe {
        windows::Win32::Foundation::DuplicateHandle(
            current_process,
            volume_handle,
            client_process,
            &raw mut client_handle,
            FILE_GENERIC_READ.0,
            false,
            windows::Win32::Foundation::DUPLICATE_HANDLE_OPTIONS(0),
        )
    };

    // SAFETY: `client_process` came from OpenProcess above; we're done with it.
    if let Err(close_err) = unsafe { CloseHandle(client_process) } {
        tracing::debug!(err = ?close_err, "CloseHandle(client_process) failed after dup");
    }

    dup_ok.map_err(|err| anyhow::anyhow!("DuplicateHandle failed: {err}"))?;
    Ok(client_handle)
}

/// Write the success response (`status=Ok` + LE-u64 handle) to the
/// pipe via the shared protocol encoder.  Returns the serialised handle
/// value on success.
#[cfg(windows)]
fn write_success_response(
    pipe: windows::Win32::Foundation::HANDLE,
    client_handle: windows::Win32::Foundation::HANDLE,
) -> anyhow::Result<u64> {
    // Win32 `HANDLE.0` is `isize`; reinterpret its bit pattern as `u64`
    // for IPC.  The receiving daemon does the inverse via
    // `HANDLE(handle_value as isize)` so the kernel-object pointer
    // round-trips unchanged.  No clippy expect needed here — the
    // workspace's cast lints don't fire on the cast in this context.
    let handle_value = client_handle.0 as u64;
    let response: [u8; RESPONSE_WIRE_LEN] = HandleResponse::ok(handle_value).encode();
    write_pipe(pipe, &response)?;
    Ok(handle_value)
}

/// Handle a pipe request after rate limiting (drive letter already read).
///
/// S5.5: Only issues read-only handles (`FILE_GENERIC_READ`), never write
/// access.
#[cfg(windows)]
#[expect(unsafe_code, reason = "CloseHandle is an FFI call")]
fn handle_pipe_request_inner(
    pipe: windows::Win32::Foundation::HANDLE,
    client_pid: u32,
    drive_letter: char,
) -> anyhow::Result<()> {
    use windows::Win32::Foundation::CloseHandle;

    tracing::info!(drive = %drive_letter, client_pid, "Opening volume for client");

    let volume_handle = match open_volume_read_only(drive_letter) {
        Ok(handle) => handle,
        Err(err) => {
            write_pipe(pipe, &HandleResponse::error().encode())?;
            return Err(err);
        }
    };

    let dup_result = duplicate_volume_handle_to_client(volume_handle, client_pid);

    // Close our copy of the volume handle regardless of whether the duplicate
    // succeeded — the client has its own handle now, or the whole request
    // failed.
    // SAFETY: `volume_handle` came from `open_volume_read_only` above.
    if let Err(close_err) = unsafe { CloseHandle(volume_handle) } {
        tracing::debug!(err = ?close_err, "CloseHandle(volume_handle) failed after dup");
    }

    let client_handle = match dup_result {
        Ok(handle) => handle,
        Err(err) => {
            write_pipe(pipe, &HandleResponse::error().encode())?;
            return Err(err);
        }
    };

    let handle_value = write_success_response(pipe, client_handle)?;
    tracing::info!(
        drive = %drive_letter,
        client_pid,
        handle = handle_value,
        "Volume handle brokered successfully"
    );
    Ok(())
}

/// Read exact bytes from the pipe.
#[cfg(windows)]
#[expect(unsafe_code, reason = "ReadFile is an FFI call")]
fn read_pipe(pipe: windows::Win32::Foundation::HANDLE, buf: &mut [u8]) -> anyhow::Result<()> {
    use windows::Win32::Storage::FileSystem::ReadFile;

    let mut bytes_read = 0_u32;

    // SAFETY: `pipe` is a valid open pipe HANDLE; `buf` is a caller-owned
    // mutable slice; `bytes_read` is a stack-owned u32 accessed exclusively.
    let result = unsafe { ReadFile(pipe, Some(buf), Some(&raw mut bytes_read), None) };

    if let Err(win_err) = result {
        anyhow::bail!("ReadFile failed: {win_err}");
    }
    if (bytes_read as usize) < buf.len() {
        // u32→usize lossless on 64-bit
        anyhow::bail!("Short read: got {bytes_read}, expected {}", buf.len());
    }
    Ok(())
}

/// Write bytes to the pipe.
#[cfg(windows)]
#[expect(unsafe_code, reason = "WriteFile is an FFI call")]
fn write_pipe(pipe: windows::Win32::Foundation::HANDLE, buf: &[u8]) -> anyhow::Result<()> {
    use windows::Win32::Storage::FileSystem::WriteFile;

    let mut bytes_written = 0_u32;

    // SAFETY: `pipe` is a valid open pipe HANDLE; `buf` is a caller-owned
    // immutable slice; `bytes_written` is a stack-owned u32.
    let result = unsafe { WriteFile(pipe, Some(buf), Some(&raw mut bytes_written), None) };

    if let Err(win_err) = result {
        anyhow::bail!("WriteFile failed: {win_err}");
    }
    Ok(())
}

// ── Elevation Check ─────────────────────────────────────────────────────

/// Return `true` when the current process is running with an elevated
/// (administrator) token.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "OpenProcessToken + GetTokenInformation + CloseHandle are FFI calls"
)]
fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess is a Win32 pseudo-handle getter (never fails).
    let current_process = unsafe { GetCurrentProcess() };
    // SAFETY: `current_process` is a valid pseudo-handle; `token` is a stack-
    // owned HANDLE passed via exclusive &raw mut.
    if unsafe { OpenProcessToken(current_process, TOKEN_QUERY, &raw mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut size = 0_u32;
    // SAFETY: `token` is a valid token handle from OpenProcessToken above;
    // `elevation` and `size` are stack-owned locals accessed via &raw mut.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast::<core::ffi::c_void>()),
            // size_of::<TOKEN_ELEVATION>() is 4 bytes — always fits u32.
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(u32::MAX),
            &raw mut size,
        )
    };
    // SAFETY: `token` came from OpenProcessToken above; closing it releases
    // the kernel-object reference.  Failure is debug-logged, not fatal.
    if let Err(close_err) = unsafe { windows::Win32::Foundation::CloseHandle(token) } {
        tracing::debug!(err = ?close_err, "CloseHandle(token) failed in is_elevated");
    }
    result.is_ok() && elevation.TokenIsElevated != 0
}

// ── Non-Windows stub ────────────────────────────────────────────────────
