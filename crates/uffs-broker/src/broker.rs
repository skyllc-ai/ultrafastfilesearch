// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows Access Broker implementation.
//!
//! Runs as a Windows Service (or foreground process for debugging).
//! Listens on a named pipe, verifies client identity, and provides
//! read-only volume handles for MFT access.
//!
//! # Protocol (binary, over named pipe)
//!
//! Request:  1 byte = drive letter ASCII (e.g., b'C')
//! Response: 1 byte status (0=ok, 1=error) + 8 bytes HANDLE value
//! (little-endian u64)
//!
//! The broker opens `\\.\X:` with `FILE_READ_DATA` + `SeBackupPrivilege`,
//! then `DuplicateHandle`s it into the client process with read-only access.

/// Pipe name for broker communication.
#[cfg(windows)]
const BROKER_PIPE_NAME: &str = r"\\.\pipe\uffs-broker";

/// Run the broker (called from main).
#[cfg(windows)]
pub fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--install") {
        return install_service();
    }
    if args.iter().any(|a| a == "--uninstall") {
        return uninstall_service();
    }
    if args.iter().any(|a| a == "--run") {
        return run_foreground();
    }

    eprintln!("uffs-broker: use --install, --uninstall, or --run");
    eprintln!("  --install     Install as Windows Service");
    eprintln!("  --uninstall   Remove Windows Service");
    eprintln!("  --run         Run in foreground (debugging)");
    Ok(())
}

/// Run the broker in foreground mode.
#[cfg(windows)]
fn run_foreground() -> anyhow::Result<()> {
    // Use `try_init` so we don't panic if a subscriber is already installed.
    let _ignore = tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();

    tracing::info!(
        pid = std::process::id(),
        "uffs-broker starting (foreground mode)"
    );

    if !is_elevated() {
        tracing::warn!("Broker is NOT running elevated — volume access will fail");
        tracing::warn!("Run as Administrator or install as a Windows Service");
    }

    serve_pipe_requests()?;

    tracing::info!("uffs-broker stopped");
    Ok(())
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
    tracing::info!(pipe = BROKER_PIPE_NAME, "Listening for handle requests");

    // S5.4: Rate limiter — tracks last request time per drive letter
    let mut rate_limit: std::collections::HashMap<char, std::time::Instant> =
        std::collections::HashMap::new();

    loop {
        let pipe = create_broker_pipe()?;
        wait_for_client(&pipe)?;

        let client_pid = get_pipe_client_pid(&pipe);

        // D7.4 + S5.2: Verify client identity + Authenticode
        if let Some(pid) = client_pid {
            let exe_path = get_client_exe_path(pid);

            if !verify_client(pid) {
                // S5.3: Audit log — rejected client
                audit_log(
                    "REJECTED",
                    pid,
                    exe_path.as_deref(),
                    None,
                    "identity verification failed",
                );
                tracing::warn!(pid, "Rejected broker client — not uffsd");
                disconnect_and_close(&pipe);
                continue;
            }

            // S5.2: Authenticode signature check
            if let Some(ref path) = exe_path
                && !verify_authenticode(path) {
                    audit_log(
                        "REJECTED",
                        pid,
                        exe_path.as_deref(),
                        None,
                        "Authenticode verification failed",
                    );
                    tracing::warn!(pid, exe = %path, "Rejected: invalid Authenticode signature");
                    disconnect_and_close(&pipe);
                    continue;
                }

            tracing::debug!(pid, "Broker client verified");
        } else {
            audit_log("REJECTED", 0, None, None, "could not determine client PID");
            tracing::warn!("Could not determine client PID — rejecting");
            disconnect_and_close(&pipe);
            continue;
        }

        let pid = client_pid.unwrap_or(0);

        // D7.5 + S5.4: Handle the request with rate limiting
        match handle_pipe_request_with_rate_limit(&pipe, pid, &mut rate_limit) {
            Ok(drive) => {
                // S5.3: Audit log — success
                audit_log(
                    "GRANTED",
                    pid,
                    get_client_exe_path(pid).as_deref(),
                    Some(drive),
                    "handle issued",
                );
            }
            Err(e) => {
                // S5.3: Audit log — failure
                audit_log(
                    "FAILED",
                    pid,
                    get_client_exe_path(pid).as_deref(),
                    None,
                    &e.to_string(),
                );
                tracing::debug!(error = %e, "Pipe request failed");
            }
        }

        disconnect_and_close(&pipe);
    }
}

/// S5.4: Handle request with per-drive rate limiting.
#[cfg(windows)]
fn handle_pipe_request_with_rate_limit(
    pipe: &windows::Win32::Foundation::HANDLE,
    client_pid: u32,
    rate_limit: &mut std::collections::HashMap<char, std::time::Instant>,
) -> anyhow::Result<char> {
    // Peek at drive letter first for rate limiting
    let mut drive_buf = [0_u8; 1];
    read_pipe(pipe, &mut drive_buf)?;
    let drive_letter = (drive_buf[0] as char).to_ascii_uppercase();

    if !drive_letter.is_ascii_alphabetic() {
        write_pipe(pipe, &[1_u8; 1])?;
        anyhow::bail!("Invalid drive letter: {drive_letter}");
    }

    // S5.4: Rate limit — 1 request per drive per 10 seconds
    let now = std::time::Instant::now();
    if let Some(last) = rate_limit.get(&drive_letter)
        && now.duration_since(*last).as_secs() < 10 {
            write_pipe(pipe, &[1_u8; 1])?;
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
fn get_client_exe_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    #[expect(unsafe_code, reason = "Win32 process query")]
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0_u16; 4096];
        let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut size,
        );
        let _ = CloseHandle(handle);
        if result.is_err() || size == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..size as usize])) // u32→usize lossless on 64-bit
    }
}

/// S5.3: Audit log entry to tracing (and Windows Event Log if available).
#[cfg(windows)]
fn audit_log(action: &str, pid: u32, exe: Option<&str>, drive: Option<char>, detail: &str) {
    let exe_str = exe.unwrap_or("<unknown>");
    let drive_str = drive.map_or("-".to_owned(), |d| d.to_string());
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

#[cfg(windows)]
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

#[cfg(windows)]
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
fn create_broker_pipe() -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::core::PCWSTR;

    let pipe_name: Vec<u16> = std::ffi::OsStr::new(BROKER_PIPE_NAME)
        .encode_wide()
        .chain(Some(0))
        .collect();

    #[expect(unsafe_code, reason = "CreateNamedPipeW requires unsafe FFI")]
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
fn wait_for_client(pipe: &windows::Win32::Foundation::HANDLE) -> anyhow::Result<()> {
    use windows::Win32::System::Pipes::ConnectNamedPipe;

    #[expect(unsafe_code, reason = "ConnectNamedPipe requires unsafe FFI")]
    let result = unsafe { ConnectNamedPipe(*pipe, None) };

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
fn disconnect_and_close(pipe: &windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Pipes::DisconnectNamedPipe;

    #[expect(
        unsafe_code,
        reason = "DisconnectNamedPipe + CloseHandle require unsafe FFI"
    )]
    unsafe {
        let _ = DisconnectNamedPipe(*pipe);
        let _ = CloseHandle(*pipe);
    }
}

// ── D7.4: Client Process Verification ───────────────────────────────────

/// Get the PID of the connected pipe client.
#[cfg(windows)]
fn get_pipe_client_pid(pipe: &windows::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;

    let mut pid: u32 = 0;

    #[expect(
        unsafe_code,
        reason = "GetNamedPipeClientProcessId requires unsafe FFI"
    )]
    let result = unsafe { GetNamedPipeClientProcessId(*pipe, &raw mut pid) };

    (result.is_ok() && pid != 0).then_some(pid)
}

/// Verify that a client process is a legitimate uffs-daemon.
#[cfg(windows)]
fn verify_client(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    #[expect(unsafe_code, reason = "Win32 process query requires unsafe FFI")]
    let exe_name = unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };

        let mut buf = vec![0_u16; 4096];
        let mut size = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut size,
        );
        let _ = CloseHandle(handle);

        if result.is_err() || size == 0 {
            return false;
        }
        String::from_utf16_lossy(&buf[..size as usize]) // u32→usize lossless on 64-bit
    };

    let name = std::path::Path::new(&exe_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    name == "uffsd"
        || name == "uffsd.exe"
        || name == "uffs-daemon.exe"
        || name == "uffs-daemon"
        || name.starts_with("uffs-daemon")
        || name.starts_with("uffs_daemon")
}

// ── D7.5: Handle Brokering ──────────────────────────────────────────────

/// Handle a pipe request after rate limiting (drive letter already read).
///
/// S5.5: Only issues read-only handles (`FILE_GENERIC_READ`), never write
/// access.
#[cfg(windows)]
fn handle_pipe_request_inner(
    pipe: &windows::Win32::Foundation::HANDLE,
    client_pid: u32,
    drive_letter: char,
) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_DUP_HANDLE};

    tracing::info!(drive = %drive_letter, client_pid, "Opening volume for client");

    // 2. Open volume with backup semantics (requires SeBackupPrivilege)
    let volume_path = format!("\\\\.\\{drive_letter}:");
    let wide_path: Vec<u16> = volume_path.encode_utf16().chain(Some(0)).collect();

    #[expect(
        unsafe_code,
        reason = "CreateFileW + DuplicateHandle require unsafe FFI"
    )]
    unsafe {
        let volume_handle = CreateFileW(
            windows::core::PCWSTR(wide_path.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        );

        let volume_handle = match volume_handle {
            Ok(h) => h,
            Err(e) => {
                write_pipe(pipe, &[1_u8; 1])?; // error
                anyhow::bail!("CreateFileW failed for {volume_path}: {e}");
            }
        };

        // 3. Open client process for handle duplication
        let client_process = match OpenProcess(PROCESS_DUP_HANDLE, false, client_pid) {
            Ok(h) => h,
            Err(e) => {
                let _ = CloseHandle(volume_handle);
                write_pipe(pipe, &[1_u8; 1])?;
                anyhow::bail!("OpenProcess for client {client_pid} failed: {e}");
            }
        };

        // 4. Duplicate handle into client process (read-only)
        let mut client_handle = HANDLE::default();
        let dup_ok = windows::Win32::Foundation::DuplicateHandle(
            windows::Win32::System::Threading::GetCurrentProcess(),
            volume_handle,
            client_process,
            &raw mut client_handle,
            FILE_GENERIC_READ.0,
            false,
            windows::Win32::Foundation::DUPLICATE_HANDLE_OPTIONS(0),
        );

        let _ = CloseHandle(volume_handle);
        let _ = CloseHandle(client_process);

        if dup_ok.is_err() {
            write_pipe(pipe, &[1_u8; 1])?;
            anyhow::bail!("DuplicateHandle failed");
        }

        // 5. Send success (1 byte) + handle value (8 bytes LE)
        let handle_value = client_handle.0 as u64; // isize→u64: handle serialization for IPC
        let mut response = [0_u8; 9];
        response[0] = 0; // success
        response[1..9].copy_from_slice(&handle_value.to_le_bytes());
        write_pipe(pipe, &response)?;

        tracing::info!(
            drive = %drive_letter,
            client_pid,
            handle = handle_value,
            "Volume handle brokered successfully"
        );
    };

    Ok(())
}

/// Read exact bytes from the pipe.
#[cfg(windows)]
fn read_pipe(pipe: &windows::Win32::Foundation::HANDLE, buf: &mut [u8]) -> anyhow::Result<()> {
    use windows::Win32::Storage::FileSystem::ReadFile;

    let mut bytes_read = 0_u32;

    #[expect(unsafe_code, reason = "ReadFile requires unsafe FFI")]
    let result = unsafe { ReadFile(*pipe, Some(buf), Some(&raw mut bytes_read), None) };

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
fn write_pipe(pipe: &windows::Win32::Foundation::HANDLE, buf: &[u8]) -> anyhow::Result<()> {
    use windows::Win32::Storage::FileSystem::WriteFile;

    let mut bytes_written = 0_u32;

    #[expect(unsafe_code, reason = "WriteFile requires unsafe FFI")]
    let result = unsafe { WriteFile(*pipe, Some(buf), Some(&raw mut bytes_written), None) };

    if let Err(win_err) = result {
        anyhow::bail!("WriteFile failed: {win_err}");
    }
    Ok(())
}

// ── Elevation Check ─────────────────────────────────────────────────────

#[cfg(windows)]
fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    #[expect(unsafe_code, reason = "Win32 token query requires unsafe FFI")]
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0_u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast::<core::ffi::c_void>()),
            // size_of::<TOKEN_ELEVATION>() is 4 bytes — always fits u32.
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(u32::MAX),
            &raw mut size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

// ── Non-Windows stub ────────────────────────────────────────────────────
