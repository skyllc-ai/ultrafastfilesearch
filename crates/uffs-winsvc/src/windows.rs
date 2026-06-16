// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows implementation: native SCM control + the `WaitNamedPipe` probe.
//!
//! All states are read as the **numeric** `SERVICE_STATUS_CURRENT_STATE`
//! via `QueryServiceStatusEx`, never by parsing `sc query` text (which is
//! localized). Handles are closed by [`ScHandle`]'s `Drop`.

use core::time::Duration;

use anyhow::{Context as _, Result, bail};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTROL_STOP,
    SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP,
    StartServiceW,
};
use windows::core::PCWSTR;

use crate::{ServiceInfo, ServiceState};

/// Poll interval while waiting for a service state transition.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Overall budget for a service to reach the requested state.
const WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// NUL-terminated UTF-16 encoding of `text` for the `*W` Win32 calls.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// RAII wrapper that closes its SC handle exactly once on drop.
struct ScHandle(
    /// The owned `SC_HANDLE` from a successful `Open*` call.
    SC_HANDLE,
);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful Open* call and is closed
        // exactly once (here, at end of scope).
        #[expect(unsafe_code, reason = "Win32 FFI — CloseServiceHandle")]
        let _closed = unsafe { CloseServiceHandle(self.0) };
    }
}

/// Open the local Service Control Manager with connect access.
fn open_scm() -> Result<ScHandle> {
    // SAFETY: documented Win32 call; null machine + database select the
    // local SCM. Returns `Err` on failure.
    #[expect(unsafe_code, reason = "Win32 FFI — OpenSCManagerW")]
    let handle = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .context("OpenSCManagerW")?;
    Ok(ScHandle(handle))
}

/// Open `service` with `access`. `Err` (typically "does not exist") when
/// the service is not registered.
fn open_service(scm: &ScHandle, service: &str, access: u32) -> Result<ScHandle> {
    let name = wide(service);
    // SAFETY: `scm.0` is a valid SCM handle; `name` is a NUL-terminated
    // UTF-16 buffer living across the call.
    #[expect(unsafe_code, reason = "Win32 FFI — OpenServiceW")]
    let handle =
        unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), access) }.context("OpenServiceW")?;
    Ok(ScHandle(handle))
}

/// Read an open service's current state + (when running) its pid.
fn query_status(service: &ScHandle) -> Result<(ServiceState, Option<u32>)> {
    let mut info = SERVICE_STATUS_PROCESS::default();
    let size = size_of::<SERVICE_STATUS_PROCESS>();
    // SAFETY: `info` is a live, properly-aligned `SERVICE_STATUS_PROCESS`;
    // we expose exactly its byte length for the kernel to fill in place.
    #[expect(unsafe_code, reason = "view the status struct as its byte buffer")]
    let buf = unsafe {
        core::slice::from_raw_parts_mut(core::ptr::from_mut(&mut info).cast::<u8>(), size)
    };
    let mut needed = 0_u32;
    // SAFETY: `service.0` is a valid service handle; `buf` is exactly one
    // `SERVICE_STATUS_PROCESS`; `needed` is a live `u32`.
    #[expect(unsafe_code, reason = "Win32 FFI — QueryServiceStatusEx")]
    unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            Some(buf),
            core::ptr::from_mut(&mut needed),
        )
    }
    .context("QueryServiceStatusEx")?;

    let state = ServiceState::from_raw(info.dwCurrentState.0);
    let pid = (state == ServiceState::Running && info.dwProcessId != 0).then_some(info.dwProcessId);
    Ok((state, pid))
}

/// Best-effort state + pid; any failure (including "no such service") maps
/// to [`ServiceInfo::not_installed`].
pub(crate) fn query(service: &str) -> ServiceInfo {
    match query_inner(service) {
        Ok((state, pid)) => ServiceInfo { state, pid },
        Err(_unavailable) => ServiceInfo::not_installed(),
    }
}

/// Fallible inner of [`query`].
fn query_inner(service: &str) -> Result<(ServiceState, Option<u32>)> {
    let scm = open_scm()?;
    let svc = open_service(&scm, service, SERVICE_QUERY_STATUS)?;
    query_status(&svc)
}

/// Start the service and wait for Running; a no-op if already running.
pub(crate) fn start(service: &str) -> Result<()> {
    let scm = open_scm()?;
    let svc = open_service(&scm, service, SERVICE_START | SERVICE_QUERY_STATUS)?;
    if query_status(&svc)?.0 == ServiceState::Running {
        return Ok(());
    }
    // SAFETY: `svc.0` is a valid service handle; no start argv.
    #[expect(unsafe_code, reason = "Win32 FFI — StartServiceW")]
    unsafe { StartServiceW(svc.0, None) }.context("StartServiceW")?;
    wait_for(&svc, ServiceState::Running)
}

/// Stop the service and wait for Stopped; a no-op if already stopped or not
/// installed.
pub(crate) fn stop(service: &str) -> Result<()> {
    let scm = open_scm()?;
    let Ok(svc) = open_service(&scm, service, SERVICE_STOP | SERVICE_QUERY_STATUS) else {
        return Ok(()); // not installed → nothing to stop
    };
    if query_status(&svc)?.0 == ServiceState::Stopped {
        return Ok(());
    }
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `svc.0` is a valid service handle; `status` is a live
    // `SERVICE_STATUS` the call fills in.
    #[expect(unsafe_code, reason = "Win32 FFI — ControlService(STOP)")]
    unsafe {
        ControlService(
            svc.0,
            SERVICE_CONTROL_STOP,
            core::ptr::from_mut(&mut status),
        )
    }
    .context("ControlService(STOP)")?;
    wait_for(&svc, ServiceState::Stopped)
}

/// Poll the open service until it reaches `target` or the timeout elapses.
fn wait_for(service: &ScHandle, target: ServiceState) -> Result<()> {
    let deadline = std::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if query_status(service)?.0 == target {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            bail!(
                "service did not reach {} within {}s",
                target.label(),
                WAIT_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Non-connecting `WaitNamedPipe` readiness probe.
pub(crate) fn pipe_serving(pipe_name: &str, timeout_ms: u32) -> bool {
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let name = wide(pipe_name);
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer living across the
    // call; `WaitNamedPipe` only waits for availability — it opens nothing.
    #[expect(unsafe_code, reason = "Win32 FFI — WaitNamedPipeW")]
    let ready = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), timeout_ms) };
    ready.as_bool()
}
