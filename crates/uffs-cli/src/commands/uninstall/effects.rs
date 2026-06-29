// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Live [`Effects`] for `uffs --uninstall` (tasks U-41/U-42): the real
//! filesystem / process / service side effects, kept apart from the executor
//! ([`super::remove`]) so the orchestration stays testable against a fake.
//!
//! Deletions are **idempotent** (an absent target is a success). Process stop,
//! service removal, and `winget` delegation shell out (`kill`/`taskkill`,
//! `sc`, `winget`) rather than via `libc`, so this crate stays `unsafe`-free.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

use super::remove::Effects;
use crate::commands::update::model::Scope;

/// The production effects implementation. Zero-sized; holds no state.
pub(crate) struct SystemEffects;

impl SystemEffects {
    /// Construct the live effects sink.
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl Effects for SystemEffects {
    fn stop_process(&mut self, _component: &str, pid: u32) -> Result<()> {
        terminate_pid(pid)
    }

    fn remove_service(&mut self, service: &str) -> Result<()> {
        remove_windows_service(service)
    }

    fn delete_binaries(&mut self, dir: &Path, stems: &[String]) -> Result<()> {
        for stem in stems {
            let path = dir.join(exe_file_name(stem));
            remove_file_if_present(&path)
                .with_context(|| format!("removing {}", path.display()))?;
        }
        Ok(())
    }

    fn delegate_winget(&mut self, package_id: &str, scope: Scope) -> Result<()> {
        winget_uninstall(package_id, scope)
    }

    fn remove_dir(&mut self, path: &Path) -> Result<()> {
        remove_dir_if_present(path).with_context(|| format!("removing {}", path.display()))
    }

    fn remove_path_entry(&mut self, dir: &Path) -> Result<()> {
        remove_path_entry_impl(dir)
    }
}

/// Windows: remove `dir` from the persisted user + machine PATH (the registry),
/// each guarded so a write (and thus elevation) only happens when that scope
/// actually contains the entry. `[Environment]::SetEnvironmentVariable`
/// broadcasts `WM_SETTINGCHANGE` so open shells pick up the change.
#[cfg(windows)]
fn remove_path_entry_impl(dir: &Path) -> Result<()> {
    let dir_str = dir.display().to_string();
    let escaped = dir_str.replace('\'', "''");
    let script = format!(
        "$d='{escaped}'; foreach($t in 'User','Machine'){{ \
         $p=[Environment]::GetEnvironmentVariable('Path',$t); \
         if($p){{ $new=($p -split ';' | Where-Object {{ $_ -and ($_ -ne $d) }}) -join ';'; \
         if($new -ne $p){{ [Environment]::SetEnvironmentVariable('Path',$new,$t) }} }} }}"
    );
    run_quiet(
        Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]),
        &format!("removing {dir_str} from PATH"),
    )
}

/// Unix: the shell owns PATH (rc files), so editing it automatically is unsafe.
/// Write a manual-cleanup hint to stderr instead (genuinely fallible, so no
/// `unnecessary_wraps`).
#[cfg(not(windows))]
fn remove_path_entry_impl(dir: &Path) -> Result<()> {
    use std::io::Write as _;

    writeln!(
        std::io::stderr(),
        "  note: remove {} from your shell PATH manually (e.g. ~/.profile or ~/.zshrc)",
        dir.display()
    )
    .context("writing PATH cleanup hint")
}

/// The on-disk file name for a binary stem (`uffsd` -> `uffsd.exe` on Windows).
fn exe_file_name(stem: &str) -> String {
    #[cfg(windows)]
    {
        format!("{stem}.exe")
    }
    #[cfg(not(windows))]
    {
        stem.to_owned()
    }
}

/// Remove a file; an already-absent target is success (idempotent). A real
/// failure (permission, sharing violation) is propagated.
fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(_) if confirmed_absent(path) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Recursively remove a directory; an already-absent target is success
/// (idempotent). A real failure is propagated.
fn remove_dir_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(_) if confirmed_absent(path) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Whether `path` is *confirmed* not to exist. `try_exists` returns `Ok(false)`
/// only when the absence is certain; an `Err` (e.g. permission denied on the
/// parent) is treated as "still present", so a genuine failure is not masked.
fn confirmed_absent(path: &Path) -> bool {
    path.try_exists().is_ok_and(|exists| !exists)
}

/// Run `command` with stdio suppressed; map a non-zero exit to an error.
fn run_quiet(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("spawning {what}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{what} exited with {status}");
    }
}

/// Stop a process by pid (`taskkill` on Windows, `kill` on Unix).
fn terminate_pid(pid: u32) -> Result<()> {
    let pid_str = pid.to_string();
    run_quiet(&mut stop_command(&pid_str), &format!("stop of pid {pid}"))
}

/// Windows: build the `taskkill` command for `pid_str`.
#[cfg(windows)]
fn stop_command(pid_str: &str) -> Command {
    let mut command = Command::new("taskkill");
    command.args(["/PID", pid_str, "/T", "/F"]);
    command
}

/// Unix: build the `kill` command for `pid_str`.
#[cfg(not(windows))]
fn stop_command(pid_str: &str) -> Command {
    let mut command = Command::new("kill");
    command.arg(pid_str);
    command
}

/// Stop + delete the broker Windows service. No-op off Windows (where no such
/// service exists, so the plan never produces this item).
#[cfg(windows)]
fn remove_windows_service(service: &str) -> Result<()> {
    // Best-effort stop first (ignore "already stopped"), then delete.
    let _ = uffs_winsvc::stop(service);
    run_quiet(
        Command::new("sc").args(["delete", service]),
        &format!("sc delete {service}"),
    )
}

/// Non-Windows: there is no broker service, so removal is not applicable. The
/// plan never produces this item off Windows, so this is never reached; if it
/// somehow were, erroring is the honest outcome.
#[cfg(not(windows))]
fn remove_windows_service(service: &str) -> Result<()> {
    bail!("cannot remove service {service}: the broker is Windows-only")
}

/// Delegate removal of a `WinGet`-managed root to `winget uninstall`.
fn winget_uninstall(package_id: &str, scope: Scope) -> Result<()> {
    let mut command = Command::new("winget");
    command.args([
        "uninstall",
        "--id",
        package_id,
        "--silent",
        "--accept-source-agreements",
    ]);
    match scope {
        Scope::Machine => {
            command.args(["--scope", "machine"]);
        }
        Scope::User => {
            command.args(["--scope", "user"]);
        }
        Scope::Unknown => {}
    }
    run_quiet(&mut command, &format!("winget uninstall {package_id}"))
}
