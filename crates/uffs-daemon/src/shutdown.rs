// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Graceful-shutdown + force-exit watchdog for [`crate::run_daemon`].
//!
//! Extracted from `lib.rs` so the orchestrator and the `spawn_*`
//! cluster stay focused on lifecycle wiring without the
//! `process::exit` / `process::abort` catastrophe-path noise.
//! Every fn here is `pub(crate)` — no external caller.

use crate::lifecycle;

/// Wait for the idle timer / shutdown signal, then run the graceful
/// shutdown sequence: abort the IPC task, timeout-join the load task,
/// drop the lifecycle manager (which cleans up PID + socket files),
/// and finally force-exit via the watchdog.
///
/// Returns `!` because both legitimate exits (clean shutdown, watchdog
/// abort) terminate the process.
pub(crate) async fn await_shutdown_then_force_exit(
    mut lifecycle_mgr: lifecycle::LifecycleManager,
    ipc_task: tokio::task::JoinHandle<()>,
    load_task: tokio::task::JoinHandle<()>,
) -> ! {
    lifecycle_mgr.run_idle_timer().await;

    tracing::info!("Daemon shutting down");
    ipc_task.abort();
    // Give the load task a brief window to finish, then abandon it.
    // Stuck kernel-mode I/O threads cannot be cancelled, so we don't
    // wait indefinitely — process::exit at the bottom will clean up.
    let shutdown_deadline = tokio::time::timeout(core::time::Duration::from_secs(3), load_task);
    let _ignore = shutdown_deadline.await;
    tracing::info!("Daemon stopped");

    // Clean up PID + socket files before exiting.
    drop(lifecycle_mgr);

    force_exit_with_watchdog()
}

/// Final shutdown: spawn a 5 s watchdog thread that calls
/// [`std::process::abort`] if `process::exit` itself hangs (kernel
/// I/O can wedge atexit handlers), then force-exit.
///
/// Returns `!` because both arms terminate the process.
fn force_exit_with_watchdog() -> ! {
    tracing::info!("Spawning shutdown watchdog (5s grace period)");
    _ = std::thread::Builder::new()
        .name("shutdown-watchdog".into())
        .spawn(|| {
            std::thread::sleep(core::time::Duration::from_secs(5));
            // process::exit did not complete in 5 s — threads are stuck
            // in kernel I/O.  Force-terminate via abort().
            //
            // Use eprintln! as a last-resort — tracing may not flush
            // before abort().  print_stderr is intentional here: this is
            // a catastrophe path where the structured logging subsystem
            // may be unavailable.
            let msg = "Shutdown watchdog: process::exit stuck for 5s — calling abort()";
            tracing::error!("{msg}");
            #[expect(
                clippy::print_stderr,
                reason = "catastrophe path — tracing may be dead"
            )]
            let _: () = eprintln!("[CATASTROPHE] {msg}");
            std::process::abort();
        }); // best-effort; if thread spawn fails, exit may still work

    // Force-exit the process.  The Windows IPC server uses
    // `std::os::windows::net::UnixListener` with `spawn_blocking(accept)`
    // and per-connection `std::thread::spawn` bridge threads.  These
    // blocking std threads cannot be cancelled by `ipc_task.abort()` and
    // will keep the process alive indefinitely after the daemon logic has
    // finished, turning it into a multi-GB zombie.  `process::exit(0)` is
    // the standard pattern for daemons with uncancellable blocking threads.
    #[expect(
        clippy::exit,
        reason = "daemon has orphaned blocking threads that prevent normal exit"
    )]
    {
        std::process::exit(0);
    }
}
