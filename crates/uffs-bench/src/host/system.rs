// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Real-OS implementation of [`Host`].

use std::io::{self, IsTerminal as _, Write as _};
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Utc};

use super::{Host, ProcOutput};

/// Production [`Host`] that talks to the real operating system.
///
/// Stateless: holds no handles, so it is freely `Copy`/`Clone` and can be
/// constructed wherever a `&dyn Host` is required.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemHost;

impl SystemHost {
    /// Construct a new [`SystemHost`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Decode captured child-process output for display and host fingerprinting.
///
/// Child processes (competitor tools, the daemon status probe) may emit bytes
/// that are not valid UTF-8. That output is only ever shown to the operator or
/// recorded verbatim in the host fingerprint — it is never parsed for a
/// security or control-flow decision — so a lossy decode is the correct,
/// non-failing choice and stays deterministic for fingerprint diffing.
fn decode_console(bytes: &[u8]) -> String {
    // AUDIT-OK(bytes): display/log-only capture of arbitrary child output; never a
    // parse or security decision.
    String::from_utf8_lossy(bytes).into_owned()
}

impl Host for SystemHost {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        // `std::fs::copy` returns the byte count; the orchestrator only needs
        // success/failure, so the count is intentionally discarded here.
        std::fs::copy(from, to).map(|_bytes| ())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn run(&self, exe: &str, args: &[&str]) -> io::Result<ProcOutput> {
        let output = Command::new(exe).args(args).output()?;
        Ok(ProcOutput {
            code: output.status.code(),
            stdout: decode_console(&output.stdout),
            stderr: decode_console(&output.stderr),
        })
    }

    fn run_streaming(&self, exe: &str, args: &[&str]) -> io::Result<Option<i32>> {
        Command::new(exe)
            .args(args)
            .status()
            .map(|status| status.code())
    }

    fn spawn(&self, exe: &str, args: &[&str]) -> io::Result<()> {
        use std::process::Stdio;
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_child| ())
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep_ms(&self, millis: u64) {
        std::thread::sleep(core::time::Duration::from_millis(millis));
    }

    fn is_tty(&self) -> bool {
        io::stdin().is_terminal() && io::stdout().is_terminal()
    }

    fn is_elevated(&self) -> bool {
        // Best-effort, dependency-free probe through the same process seam the
        // host already uses. Honest token/uid inspection would need `unsafe`
        // FFI (which the workspace denies) or a heavyweight crate; this value is
        // only recorded in the environment fingerprint, never used to gate a
        // decision, so a process probe is the correct, minimal trade-off.
        #[cfg(windows)]
        {
            // The High Mandatory Level group SID (`S-1-16-12288`) appears in the
            // token only when the process is running elevated.
            self.run("whoami", &["/groups"])
                .is_ok_and(|out| out.stdout.contains("S-1-16-12288"))
        }
        #[cfg(unix)]
        {
            self.run("id", &["-u"])
                .is_ok_and(|out| out.stdout.trim() == "0")
        }
        #[cfg(not(any(windows, unix)))]
        {
            false
        }
    }

    fn read_key(&self) -> io::Result<char> {
        // A full-line read is a deliberately simple stand-in for raw single-key
        // input: it needs no extra dependency or `unsafe` termios handling and
        // is sufficient for the gate prompts (the operator presses a letter then
        // Enter). True raw single-key capture is a later UX refinement.
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(line.trim().chars().next().unwrap_or('\n'))
    }

    fn out(&self, line: &str) {
        // Console output goes through `io::stdout().write_all` rather than the
        // `println!` macro because the workspace denies `clippy::print_stdout`.
        // UI rendering is cosmetic, so write errors are intentionally ignored.
        let mut stdout = io::stdout().lock();
        _ = stdout.write_all(line.as_bytes());
        _ = stdout.write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::SystemHost;
    use crate::host::Host as _;

    #[test]
    fn write_read_rename_remove_roundtrip() {
        let host = SystemHost::new();
        let dir = tempdir().expect("create tempdir");
        let src = dir.path().join("a.txt");
        let dst = dir.path().join("b.txt");

        host.write_file(&src, b"hello").expect("write");
        assert!(host.path_exists(&src));
        assert_eq!(host.read_file(&src).expect("read"), b"hello");

        host.rename(&src, &dst).expect("rename");
        assert!(!host.path_exists(&src));
        assert!(host.path_exists(&dst));

        host.remove_file(&dst).expect("remove");
        assert!(!host.path_exists(&dst));
    }

    #[test]
    fn create_dir_all_creates_nested() {
        let host = SystemHost::new();
        let dir = tempdir().expect("create tempdir");
        let nested = dir.path().join("x").join("y").join("z");
        host.create_dir_all(&nested).expect("create_dir_all");
        assert!(host.path_exists(&nested));
    }

    #[test]
    fn env_absent_var_is_none() {
        let host = SystemHost::new();
        assert!(host.env("UFFS_BENCH_DEFINITELY_ABSENT_VAR").is_none());
    }

    #[test]
    fn now_is_after_year_2020() {
        let host = SystemHost::new();
        let year_2020 = chrono::DateTime::<chrono::Utc>::from_timestamp(1_577_836_800, 0)
            .expect("valid 2020 timestamp");
        assert!(host.now() > year_2020);
    }

    #[cfg(unix)]
    #[test]
    fn run_captures_stdout() {
        let host = SystemHost::new();
        let output = host.run("printf", &["hello"]).expect("run printf");
        assert_eq!(output.stdout, "hello");
        assert!(output.success());
    }
}
