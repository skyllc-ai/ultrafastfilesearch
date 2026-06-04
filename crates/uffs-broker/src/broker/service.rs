// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Windows Service registration helpers for the access broker.
//!
//! Extracted from `broker.rs` (which sits at the 800-LOC file-size ceiling) as
//! a self-contained, behavior-neutral unit: `install_service` /
//! `uninstall_service` shell out to `sc` and share no state with the
//! pipe-serving or client-verification logic. Both are admin-only CLI commands
//! invoked from `broker::run`.

/// Register the broker as a Windows Service via `sc create`.
#[cfg(windows)]
#[expect(
    clippy::print_stdout,
    reason = "CLI admin command — stdout is the user-visible result channel"
)]
pub(super) fn install_service() -> anyhow::Result<()> {
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
        // AUDIT-OK(bytes): `sc` command stderr surfaced verbatim in an
        // error message for the operator — display only, no decision.
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
pub(super) fn uninstall_service() -> anyhow::Result<()> {
    let output = std::process::Command::new("sc")
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
