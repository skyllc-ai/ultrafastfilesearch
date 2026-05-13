// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Daemon spawn logic: Unix `Command::spawn`, Windows `CreateProcessW`
//! + optional `ShellExecuteW("runas")`, and shared elevation policy.
//!
//! This is the **canonical home** of `spawn_daemon`,
//! `ElevationPolicy`, `resolve_elevation_policy`, and
//! `elevation_policy_from`.  Import them from
//! `crate::daemon_spawn` (or `uffs_client::daemon_spawn` from
//! outside the crate) — there is intentionally no `pub use`
//! cascade through `daemon_ctl`.

use crate::daemon_child::DaemonChildHandle;

// ── Elevation policy ──────────────────────────────────────────────────────

/// Policy for whether `spawn_daemon` may trigger a Windows UAC prompt.
///
/// Before v0.5.36, `spawn_daemon` on Windows unconditionally used
/// `ShellExecuteW("runas")` whenever the current process was not
/// elevated — so any non-admin shell running `uffs <pattern>` with the
/// daemon stopped would get a UAC dialog as a side-effect.  That was
/// surprising and made piping or scripting the CLI fragile.
///
/// The new default is [`ElevationPolicy::RequireExistingElevation`]:
/// the spawn succeeds only if the current process is already elevated;
/// otherwise it returns [`crate::error::ClientError::DaemonNeedsElevation`] and
/// the CLI renders an actionable message.  Callers that actually want the
/// UAC dialog (e.g. `uffs daemon start --elevate`) must opt in with
/// [`ElevationPolicy::AllowUacPrompt`].
///
/// Has no effect on Unix — Unix spawn never triggers UAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ElevationPolicy {
    /// Spawn only if this process is already elevated.  If not, return
    /// [`crate::error::ClientError::DaemonNeedsElevation`] without
    /// touching the UI.
    ///
    /// This is the default for every implicit auto-spawn path (e.g.
    /// `UffsClient::connect_with_args`).
    #[default]
    RequireExistingElevation,

    /// When not elevated, request a UAC prompt via `ShellExecuteW`
    /// with the `"runas"` verb.  Preserves the pre-v0.5.36 behavior.
    ///
    /// Used by `uffs daemon start --elevate` and by auto-spawn paths
    /// when the environment variable `UFFS_ELEVATE=1` is set.
    AllowUacPrompt,
}

/// Pure policy decision used by [`resolve_elevation_policy`].
///
/// Rules, in priority order:
///
/// 1. If `force_allow` is `true` (e.g. `uffs daemon start --elevate`), return
///    [`ElevationPolicy::AllowUacPrompt`].
/// 2. Otherwise, if `env_value` contains a truthy token (`1`, `true`, `yes`,
///    `on`, case-insensitive — leading/trailing whitespace is trimmed), return
///    [`ElevationPolicy::AllowUacPrompt`].  This is how `UFFS_ELEVATE` is
///    interpreted.
/// 3. Otherwise, return [`ElevationPolicy::RequireExistingElevation`].
///
/// Kept env-free so both the async and sync clients (and tests) can
/// share one decision matrix without racing on real environment state.
#[must_use]
pub(crate) fn elevation_policy_from(force_allow: bool, env_value: Option<&str>) -> ElevationPolicy {
    if force_allow {
        return ElevationPolicy::AllowUacPrompt;
    }
    if let Some(raw) = env_value {
        let normalized = raw.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "1" | "true" | "yes" | "on") {
            return ElevationPolicy::AllowUacPrompt;
        }
    }
    ElevationPolicy::RequireExistingElevation
}

/// Resolve the effective [`ElevationPolicy`] for an implicit
/// auto-spawn.
///
/// Reads the `UFFS_ELEVATE` environment variable once and feeds the
/// result into [`elevation_policy_from`].  `force_allow = true` from
/// an explicit `--elevate` flag short-circuits the env lookup.
#[must_use]
pub(crate) fn resolve_elevation_policy(force_allow: bool) -> ElevationPolicy {
    elevation_policy_from(force_allow, std::env::var("UFFS_ELEVATE").ok().as_deref())
}

// ── Spawn dispatchers ─────────────────────────────────────────────────────

/// Spawn the daemon as a detached background process.
///
/// On **Unix**, uses a normal `Command::new` spawn (no elevation needed);
/// the `policy` parameter is ignored.
///
/// On **Windows**, behavior depends on `policy` and the current
/// elevation state:
///
/// | already elevated | policy                        | action                        |
/// |------------------|-------------------------------|-------------------------------|
/// | yes              | any                           | `CreateProcessW` (no UAC)     |
/// | no               | `RequireExistingElevation`    | return `DaemonNeedsElevation` |
/// | no               | `AllowUacPrompt`              | `ShellExecuteW("runas")` + UAC|
///
/// # Errors
///
/// Returns [`crate::error::ClientError::DaemonStartFailed`] if the
/// process creation itself fails, or
/// [`crate::error::ClientError::DaemonNeedsElevation`] if the policy
/// does not allow a UAC prompt in the current elevation state.
#[cfg(unix)]
pub(crate) fn spawn_daemon(
    exe: &std::path::Path,
    args: &[&str],
    _policy: ElevationPolicy,
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    // `policy` is Windows-only; the Unix spawn never prompts for
    // elevation.  The parameter stays in the public signature so
    // callers can pass the same value on every platform.
    spawn_daemon_unix(exe, args)
}

/// Windows implementation of [`spawn_daemon`].
///
/// Behavior is decided by `policy` combined with the current
/// elevation state (see [`spawn_daemon_windows`] for the full
/// decision tree).
///
/// # Errors
///
/// Returns [`ClientError`](crate::error::ClientError) on spawn
/// failure, including:
/// * [`crate::error::ClientError::DaemonNeedsElevation`] when the policy
///   forbids UAC and the caller is not elevated.
/// * [`crate::error::ClientError::DaemonStartFailed`] when `CreateProcessW` /
///   `ShellExecuteW` itself rejects the launch.
#[cfg(windows)]
pub(crate) fn spawn_daemon(
    exe: &std::path::Path,
    args: &[&str],
    policy: ElevationPolicy,
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    spawn_daemon_windows(exe, args, policy)
}

// ── Platform-specific spawn impls ─────────────────────────────────────────

/// Unix daemon spawn: simple detached process.
/// # Errors
///
/// Returns [`ClientError`](crate::error::ClientError) if the daemon process
/// cannot be spawned.
#[cfg(unix)]
#[expect(
    clippy::single_call_fn,
    reason = "platform-specific helper — clarity over inlining"
)]
fn spawn_daemon_unix(
    exe: &std::path::Path,
    args: &[&str],
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    let child = std::process::Command::new(exe)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|spawn_err| {
            crate::error::ClientError::DaemonStartFailed(format!(
                "Failed to spawn {}: {spawn_err}",
                exe.display()
            ))
        })?;
    Ok(DaemonChildHandle::from_unix_child(child))
}

/// Windows daemon spawn: elevation-aware.
#[cfg(windows)]
#[expect(
    clippy::single_call_fn,
    reason = "platform-specific helper — clarity over inlining"
)]
fn spawn_daemon_windows(
    exe: &std::path::Path,
    args: &[&str],
    policy: ElevationPolicy,
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    let elevated = is_elevated();
    tracing::debug!(
        exe = %exe.display(),
        ?args,
        elevated,
        ?policy,
        "spawn_daemon_windows"
    );

    if elevated {
        tracing::debug!("spawning via CreateProcessW (no handle inheritance)");
        return spawn_detached_no_inherit(exe, args);
    }

    match policy {
        ElevationPolicy::AllowUacPrompt => spawn_via_uac_prompt(exe, args),
        ElevationPolicy::RequireExistingElevation => {
            tracing::info!("Not elevated and policy forbids UAC — returning DaemonNeedsElevation");
            Err(crate::error::ClientError::DaemonNeedsElevation {
                daemon_path: exe.display().to_string(),
            })
        }
    }
}

/// UAC-prompt arm of [`spawn_daemon_windows`].
///
/// `ShellExecuteW("runas")` does not hand back a process handle — the OS
/// shell owns the elevated child — so we cannot poll for early exit on
/// this path.  Return an `opaque` handle; the retry loop falls back to
/// the plain "could not connect after N attempts" error for UAC spawns.
#[cfg(windows)]
fn spawn_via_uac_prompt(
    exe: &std::path::Path,
    args: &[&str],
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    tracing::debug!("NOT elevated, using ShellExecuteW runas (policy allows UAC)");
    tracing::info!("Not elevated — requesting elevation via UAC prompt");
    shell_execute_elevated(exe, args)?;
    tracing::debug!("ShellExecuteW returned OK");
    Ok(DaemonChildHandle::opaque())
}

// ── CreateProcessW arg quoting (MSVCRT-compatible) ────────────────────────

/// Escape a single command-line argument for `CreateProcessW` per the
/// Microsoft argv-parsing rules used by `CommandLineToArgvW` and the
/// standard C runtime (`__wgetmainargs`).
///
/// Rules (condensed from Raymond Chen's "Everyone quotes command line
/// arguments the wrong way" and the MSVCRT parser source):
///
/// * An **empty** argument must become `""` — otherwise it collapses into the
///   separating space and disappears from the child's argv.  This is exactly
///   what caused the silent `uffs daemon start` failure (`LOG/Output`): the CLI
///   pushed `["--log-level", ""]` and the child saw only `--log-level`, then
///   consumed the *next* flag as its value.
/// * If the arg contains no whitespace, double-quote, or control chars, emit it
///   verbatim — cheap and readable.
/// * Otherwise wrap in `"..."` and, inside the quotes, double every run of
///   backslashes that precedes a `"`, and escape each `"` as `\"`. Trailing
///   backslashes just before the closing `"` must also be doubled so the
///   closing quote is not interpreted as escaped.
///
/// This function is pure string manipulation and is compiled (and unit
/// tested) on every platform, even though it is only *called* from
/// [`spawn_detached_no_inherit`] on Windows.  We gate the item on
/// `any(windows, test)` so macOS/Linux release builds don't emit a
/// `dead_code` warning, while `cargo test` still compiles it everywhere
/// and the unit tests run on the ship box.
#[cfg(any(windows, test))]
#[must_use]
fn quote_arg_for_createprocess(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    // Fast path: nothing that needs escaping.
    let needs_quoting = arg
        .chars()
        .any(|chr| chr == ' ' || chr == '\t' || chr == '\n' || chr == '\x0b' || chr == '"');
    if !needs_quoting {
        return arg.to_owned();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut pending_backslashes: usize = 0;
    for chr in arg.chars() {
        if chr == '\\' {
            pending_backslashes += 1;
        } else if chr == '"' {
            // Double the pending backslashes, then escape the quote.
            for _ in 0..=(pending_backslashes * 2) {
                out.push('\\');
            }
            out.push('"');
            pending_backslashes = 0;
        } else {
            for _ in 0..pending_backslashes {
                out.push('\\');
            }
            out.push(chr);
            pending_backslashes = 0;
        }
    }
    // Trailing backslashes must be doubled so the closing quote is not
    // swallowed as an escape target.
    for _ in 0..(pending_backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

// ── CreateProcessW spawn ──────────────────────────────────────────────────

/// Spawn the daemon as a fully detached process with NO handle inheritance.
///
/// Uses `CreateProcessW` directly with `bInheritHandles = FALSE` and
/// `DETACHED_PROCESS` creation flag.
///
/// Returns a [`DaemonChildHandle`] that keeps the process handle alive so
/// the caller's IPC-readiness retry loop can detect early exit via
/// [`DaemonChildHandle::try_wait`] — without this, a daemon that panics or
/// clap-rejects its argv looks identical to a daemon that just hasn't
/// bound its pipe yet, and the client spins through all 20 retries with
/// no diagnostic signal (the `LOG/Output` silent-failure scenario).
#[cfg(windows)]
fn spawn_detached_no_inherit(
    exe: &std::path::Path,
    args: &[&str],
) -> Result<DaemonChildHandle, crate::error::ClientError> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    // Build the command-line string using full MSVCRT-compatible escaping.
    // The previous naive implementation dropped empty args entirely and
    // mangled any arg containing spaces or quotes — see
    // `quote_arg_for_createprocess` for the gory details.
    let mut cmd_line = String::new();
    cmd_line.push_str(&quote_arg_for_createprocess(&exe.to_string_lossy()));
    for arg in args {
        cmd_line.push(' ');
        cmd_line.push_str(&quote_arg_for_createprocess(arg));
    }

    let mut cmd_wide: Vec<u16> = cmd_line.encode_utf16().chain(core::iter::once(0)).collect();

    let si = STARTUPINFOW {
        cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();

    // SAFETY: CreateProcessW is a well-defined Win32 API. All pointers are
    // valid: cmd_wide is a mutable null-terminated UTF-16 buffer, si is
    // a zeroed STARTUPINFOW with cb set, pi is zeroed output buffer.
    // We close the returned handles immediately after success.
    #[expect(unsafe_code, reason = "CreateProcessW requires unsafe FFI")]
    let result = unsafe {
        CreateProcessW(
            None,
            Some(windows::core::PWSTR(cmd_wide.as_mut_ptr())),
            None,
            None,
            false, // bInheritHandles = FALSE ← key fix
            DETACHED_PROCESS,
            None,
            None,
            core::ptr::from_ref(&si),
            core::ptr::from_mut(&mut pi),
        )
    };

    match result {
        Ok(()) => {
            tracing::debug!(pid = pi.dwProcessId, "spawn_detached_no_inherit: spawned");
            tracing::info!(
                pid = pi.dwProcessId,
                "Daemon spawned (no handle inheritance)"
            );
            // Close the *thread* handle immediately — we only use the
            // thread handle to unblock the initial process primary thread,
            // which is automatic on spawn.  Keep the *process* handle
            // open so the retry loop can poll for early exit.
            // SAFETY: thread handle was just returned by CreateProcessW
            // and is not aliased elsewhere.
            #[expect(unsafe_code, reason = "closing Win32 thread handle")]
            let thread_close = unsafe { CloseHandle(pi.hThread) };
            drop(thread_close);
            Ok(DaemonChildHandle::from_windows_process(
                pi.hProcess,
                pi.dwProcessId,
            ))
        }
        Err(win_err) => {
            tracing::debug!(error = %win_err, "spawn_detached_no_inherit: FAILED");
            Err(crate::error::ClientError::DaemonStartFailed(format!(
                "CreateProcessW failed for {}: {win_err}",
                exe.display()
            )))
        }
    }
}

// ── Windows elevation helpers ─────────────────────────────────────────────

/// Check if the current process is running with Administrator privileges.
#[cfg(windows)]
fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();

    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that does not
    // need closing.
    #[expect(unsafe_code, reason = "Win32 pseudo-handle accessor")]
    let current_proc = unsafe { GetCurrentProcess() };
    // SAFETY: `OpenProcessToken` writes a valid token handle into `token`
    // on success; `current_proc` is valid.
    #[expect(unsafe_code, reason = "Win32 token FFI")]
    let open_result =
        unsafe { OpenProcessToken(current_proc, TOKEN_QUERY, core::ptr::from_mut(&mut token)) };
    if open_result.is_err() {
        return false;
    }

    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut size = 0_u32;
    // SAFETY: `token` is a valid token handle; the out-pointer points to
    // a stack-owned `TOKEN_ELEVATION` that lives for the whole call.
    #[expect(unsafe_code, reason = "Win32 token information query")]
    let query_result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(core::ptr::from_mut(&mut elevation).cast()),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(u32::MAX),
            core::ptr::from_mut(&mut size),
        )
    };
    // SAFETY: `token` is owned by this function; no other code references it.
    #[expect(unsafe_code, reason = "CloseHandle for owned Win32 handle")]
    let close_result = unsafe { CloseHandle(token) };
    drop(close_result);

    query_result.is_ok() && elevation.TokenIsElevated != 0
}

/// Launch a process elevated via `ShellExecuteW` with the `"runas"` verb.
///
/// This triggers the Windows UAC consent dialog. If the user clicks "Yes",
/// the process starts elevated; if they click "No" or dismiss the dialog,
/// an error is returned.
#[cfg(windows)]
fn shell_execute_elevated(
    exe: &std::path::Path,
    args: &[&str],
) -> Result<(), crate::error::ClientError> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::core::PCWSTR;

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let exe_str = exe.to_string_lossy();
    let file: Vec<u16> = format!("{exe_str}\0").encode_utf16().collect();
    let params_str = args.join(" ");
    let params: Vec<u16> = format!("{params_str}\0").encode_utf16().collect();

    tracing::debug!(
        verb = "runas",
        file = %exe_str,
        params = %params_str,
        "ShellExecuteW"
    );

    // SAFETY: ShellExecuteW is a well-defined Win32 Shell API.
    // All PCWSTR pointers are valid null-terminated UTF-16 buffers
    // that outlive the call (stack-allocated Vecs above).
    #[expect(unsafe_code, reason = "ShellExecuteW requires unsafe FFI")]
    let hinst = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_HIDE,
        )
    };

    // ShellExecuteW returns HINSTANCE — values > 32 indicate success.
    let code = hinst.0 as isize;
    if code > 32 {
        tracing::debug!(code, "ShellExecuteW succeeded");
        Ok(())
    } else {
        let msg = match code {
            0 => "The OS is out of memory or resources",
            2 => "Executable not found (ERROR_FILE_NOT_FOUND)",
            3 => "Path not found (ERROR_PATH_NOT_FOUND)",
            5 => "Access denied (ERROR_ACCESS_DENIED)",
            _ => "Unknown ShellExecuteW error",
        };
        tracing::debug!(code, msg, "ShellExecuteW failed");
        Err(crate::error::ClientError::DaemonStartFailed(format!(
            "ShellExecuteW(runas) failed for {}: code={code} — {msg}",
            exe.display()
        )))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod elevation_policy_tests {
    use super::{ElevationPolicy, elevation_policy_from};

    /// Explicit `force_allow` (e.g. `--elevate`) always wins, even
    /// against an empty or falsy env value.
    #[test]
    fn force_allow_always_permits_uac() {
        assert_eq!(
            elevation_policy_from(true, None),
            ElevationPolicy::AllowUacPrompt,
        );
        assert_eq!(
            elevation_policy_from(true, Some("")),
            ElevationPolicy::AllowUacPrompt,
        );
        assert_eq!(
            elevation_policy_from(true, Some("0")),
            ElevationPolicy::AllowUacPrompt,
        );
    }

    /// Without `force_allow` and without the env var, the default
    /// policy must refuse UAC.  This is the behavioral change v0.5.36
    /// introduces and the linchpin for the whole P7 fix.
    #[test]
    fn missing_env_defaults_to_require_existing_elevation() {
        assert_eq!(
            elevation_policy_from(false, None),
            ElevationPolicy::RequireExistingElevation,
        );
    }

    /// Every documented truthy token must promote to
    /// `AllowUacPrompt`.  Trimming and case-folding are also expected.
    #[test]
    fn truthy_env_values_permit_uac() {
        for token in [
            "1", "true", "TRUE", "True", "yes", "YES", "on", "ON", "  1  ", " yes\n",
        ] {
            assert_eq!(
                elevation_policy_from(false, Some(token)),
                ElevationPolicy::AllowUacPrompt,
                "token {token:?} should enable UAC",
            );
        }
    }

    /// Falsy / unrecognised tokens must keep the conservative default.
    #[test]
    fn falsy_or_unknown_env_values_keep_default() {
        for token in ["0", "false", "no", "off", "", "maybe", "2", "nope"] {
            assert_eq!(
                elevation_policy_from(false, Some(token)),
                ElevationPolicy::RequireExistingElevation,
                "token {token:?} should not enable UAC",
            );
        }
    }

    /// [`ElevationPolicy::default`] must be the safe option.  New
    /// callers that rely on `..Default::default()` must not silently
    /// get the UAC-triggering variant.
    #[test]
    fn default_policy_is_require_existing_elevation() {
        assert_eq!(
            ElevationPolicy::default(),
            ElevationPolicy::RequireExistingElevation,
        );
    }
}

#[cfg(test)]
mod quote_arg_tests {
    use super::quote_arg_for_createprocess;

    /// **Regression (silent `daemon start` failure, `LOG/Output`):** an
    /// empty argument must round-trip as `""` so `CreateProcessW`'s child
    /// sees it as a zero-length argv entry instead of skipping it
    /// entirely.  Before this fix, `["--log-level", "", "--log-file",
    /// "uffsd.log"]` was concatenated as `"... --log-level  --log-file
    /// uffsd.log"`, and the child's argv parser consumed `--log-file` as
    /// the value of `--log-level`, leaving `uffsd.log` as an unknown
    /// positional — clap bailed with exit code 2 before uffsd could bind
    /// its IPC transports.
    #[test]
    fn empty_arg_becomes_explicit_empty_quotes() {
        assert_eq!(quote_arg_for_createprocess(""), "\"\"");
    }

    /// Plain alphanumeric / punctuation arguments pass through unquoted.
    /// This guards the fast path that keeps command lines readable for
    /// `tracing::debug!` consumers.
    #[test]
    fn plain_arg_passes_through_unquoted() {
        assert_eq!(quote_arg_for_createprocess("debug"), "debug");
        assert_eq!(quote_arg_for_createprocess("--log-level"), "--log-level");
        assert_eq!(
            quote_arg_for_createprocess("C:\\Users\\rnio"),
            "C:\\Users\\rnio"
        );
    }

    /// Arguments containing whitespace must be wrapped in double quotes.
    /// This covers the real-world case of a `Program Files` path in
    /// `--data-dir`.
    #[test]
    fn whitespace_arg_gets_quoted() {
        assert_eq!(
            quote_arg_for_createprocess(r"C:\Program Files\uffs"),
            "\"C:\\Program Files\\uffs\"",
        );
    }

    /// Embedded double quotes must be escaped with a backslash so the
    /// child sees the literal quote instead of a premature string
    /// terminator.
    #[test]
    fn embedded_quote_is_escaped() {
        assert_eq!(
            quote_arg_for_createprocess(r#"he said "hi""#),
            r#""he said \"hi\"""#
        );
    }

    /// MSVCRT rule: each backslash that precedes a quote must be
    /// doubled.  Non-quote backslashes pass through literally.  Without
    /// this, a path like `a\"b` would be misparsed by the child.
    #[test]
    fn backslashes_before_quote_are_doubled() {
        // Single backslash followed by a quote → two backslashes then an
        // escaped quote inside the quoted string.
        assert_eq!(quote_arg_for_createprocess(r#"a\"b"#), r#""a\\\"b""#);
        // Two backslashes followed by a quote → four backslashes then an
        // escaped quote.
        assert_eq!(quote_arg_for_createprocess(r#"a\\"b"#), r#""a\\\\\"b""#);
    }

    /// Trailing backslashes inside a quoted arg must be doubled so the
    /// closing quote is not swallowed as an escape target.  A `C:\`
    /// argument with a space elsewhere (forcing quoting) is the canonical
    /// failure case.
    #[test]
    fn trailing_backslash_in_quoted_arg_is_doubled() {
        // "path has\" → needs quoting (space), and the trailing \
        // must be doubled so the closing " stands on its own.
        assert_eq!(
            quote_arg_for_createprocess("path with\\"),
            "\"path with\\\\\"",
        );
    }

    /// End-to-end: simulate the exact args list that caused the bug.
    /// Assembling them with a single space separator must produce the
    /// *correct* command line (empty arg visibly `""`), not the mangled
    /// one from the old naive concatenation.
    #[test]
    fn full_daemon_start_argv_reassembly_preserves_empty_arg() {
        let args = ["--log-level", "", "--log-file", "uffsd.log"];
        let cmd: String = args
            .iter()
            .map(|arg| quote_arg_for_createprocess(arg))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(cmd, "--log-level \"\" --log-file uffsd.log");
    }
}
