// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows Service registration helpers for the access broker.
//!
//! Extracted from `broker.rs` (which sits at the 800-LOC file-size ceiling) as
//! a self-contained, behavior-neutral unit: `install_service` /
//! `uninstall_service` shell out to `sc` and share no state with the
//! pipe-serving or client-verification logic. Both are admin-only CLI commands
//! invoked from `broker::run`.

/// Register the broker as an auto-start Windows Service and start it.
///
/// # Why the argv is split the way it is
///
/// `sc create` parses `option= value` where `option=` and the value are
/// **separate command-line tokens** (the space after `=` is the
/// delimiter).  The prior code passed `binPath= "<path>"` as a *single*
/// argument, so the registered `ImagePath` ended up as ` "<path>"` —
/// with a leading space and literal quotes — and the service failed to
/// start with `StartService` error 87 (`ERROR_INVALID_PARAMETER`).  Here
/// `binPath=` and the raw path are distinct argv elements; `std`'s
/// Windows argument quoting wraps the path in quotes only if it contains
/// spaces, producing a valid `ImagePath` in both cases.
#[cfg(windows)]
#[expect(
    clippy::print_stdout,
    reason = "CLI admin command — stdout is the user-visible result channel"
)]
pub(super) fn install_service() -> anyhow::Result<()> {
    if !super::is_elevated() {
        anyhow::bail!(
            "installing the broker service requires Administrator.\n\
             Open an elevated terminal (right-click PowerShell or cmd → \
             \"Run as administrator\") and re-run:\n    uffs-broker --install"
        );
    }

    let exe = std::env::current_exe()?;
    let create = std::process::Command::new("sc.exe")
        .args([
            "create",
            "UffsAccessBroker",
            "binPath=",
            &exe.display().to_string(),
            "start=",
            "auto",
            "DisplayName=",
            "UFFS Access Broker",
        ])
        .output()?;

    if !create.status.success() {
        // AUDIT-OK(bytes): `sc` stderr surfaced verbatim to the operator —
        // display only, no decision.
        let stderr = String::from_utf8_lossy(&create.stderr);
        anyhow::bail!("Install failed (sc create): {stderr}");
    }

    // Start it now so the broker is usable immediately — the whole point
    // is "no future UAC", which only holds once the service is running.
    // `start= auto` also brings it back on every boot.
    let start = std::process::Command::new("sc.exe")
        .args(["start", "UffsAccessBroker"])
        .output()?;

    if start.status.success() {
        println!(
            "UFFS Access Broker installed and started (auto-start on boot).\n\
             Non-elevated `uffs` searches will now use the broker for volume \
             access — no more UAC prompts."
        );
    } else {
        // AUDIT-OK(bytes): `sc` stderr surfaced verbatim to the operator.
        let stderr = String::from_utf8_lossy(&start.stderr);
        println!(
            "Service installed (auto-start on boot), but starting it failed: \
             {stderr}\nStart it manually from an elevated shell with:\n    \
             sc.exe start UffsAccessBroker"
        );
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
pub(super) fn uninstall_service() -> anyhow::Result<()> {
    if !super::is_elevated() {
        anyhow::bail!(
            "removing the broker service requires Administrator.\n\
             Open an elevated terminal and re-run:\n    uffs-broker --uninstall"
        );
    }
    let output = std::process::Command::new("sc.exe")
        .args(["delete", "UffsAccessBroker"])
        .output()?;

    if output.status.success() {
        println!("Service uninstalled.");
    } else {
        // AUDIT-OK(bytes): `sc` command stderr surfaced verbatim in an
        // error message for the operator — display only, no decision.
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Uninstall failed: {stderr}");
    }
    Ok(())
}
