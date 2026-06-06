// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! In-memory [`Host`] for deterministic, OS-independent unit tests.
//!
//! [`MockHost`] keeps an in-memory filesystem, records every call in order (so
//! tests can assert *snapshot-before-mutate* ordering and that the command
//! shown equals the command run), replays scripted keypresses, and returns
//! scripted process outputs. It is ordinary lint-clean library code — not gated
//! behind `#[cfg(test)]` — so integration tests in `tests/` can use it through
//! the public API.

use alloc::collections::{BTreeMap, VecDeque};
use core::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::ProcOutput;

/// A single recorded interaction with the [`MockHost`].
///
/// Ordering in [`MockHost::calls`] is the order the methods were invoked, which
/// is what the "register a restore *before* mutating" assertions inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// [`super::Host::read_file`] for the given path.
    ReadFile(PathBuf),
    /// [`super::Host::write_file`] for the given path.
    WriteFile(PathBuf),
    /// [`super::Host::remove_file`] for the given path.
    RemoveFile(PathBuf),
    /// [`super::Host::rename`] from the first path to the second.
    Rename(PathBuf, PathBuf),
    /// [`super::Host::copy_file`] from the first path to the second.
    Copy(PathBuf, PathBuf),
    /// [`super::Host::create_dir_all`] for the given path.
    CreateDirAll(PathBuf),
    /// [`super::Host::run`] with the executable and its arguments.
    Run(String, Vec<String>),
    /// [`super::Host::read_key`] consumed one scripted keypress.
    ReadKey,
    /// [`super::Host::out`] emitted the given line.
    Out(String),
    /// [`super::Host::sleep_ms`] was asked to pause for the given milliseconds.
    Sleep(u64),
}

/// In-memory, fully scriptable [`Host`](super::Host) implementation.
pub struct MockHost {
    /// In-memory filesystem: absolute path → file bytes.
    files: RefCell<BTreeMap<PathBuf, Vec<u8>>>,
    /// Ordered log of every host interaction.
    calls: RefCell<Vec<Call>>,
    /// Scripted keypresses, consumed front-to-back by `read_key`.
    keys: RefCell<VecDeque<char>>,
    /// Scripted process outputs, consumed front-to-back by `run`.
    run_results: RefCell<VecDeque<ProcOutput>>,
    /// Scripted environment variables.
    env: BTreeMap<String, String>,
    /// Lines emitted via `out`, in order.
    out_lines: RefCell<Vec<String>>,
    /// Current mock clock value returned by `now`.
    clock: Cell<DateTime<Utc>>,
    /// Whether the mock reports an interactive TTY.
    tty: bool,
    /// Whether the mock reports the process as elevated.
    elevated: bool,
}

/// Deterministic default clock (a fixed, arbitrary instant in 2023).
fn default_clock() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap_or_else(Utc::now)
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            files: RefCell::new(BTreeMap::new()),
            calls: RefCell::new(Vec::new()),
            keys: RefCell::new(VecDeque::new()),
            run_results: RefCell::new(VecDeque::new()),
            env: BTreeMap::new(),
            out_lines: RefCell::new(Vec::new()),
            clock: Cell::new(default_clock()),
            tty: true,
            elevated: false,
        }
    }
}

impl MockHost {
    /// Construct an empty [`MockHost`] with an interactive TTY and the default
    /// deterministic clock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: seed an in-memory file.
    #[must_use]
    pub fn with_file<P: Into<PathBuf>, B: Into<Vec<u8>>>(self, path: P, bytes: B) -> Self {
        self.files.borrow_mut().insert(path.into(), bytes.into());
        self
    }

    /// Builder: queue a scripted keypress (consumed in order by `read_key`).
    #[must_use]
    pub fn with_key(self, key: char) -> Self {
        self.keys.borrow_mut().push_back(key);
        self
    }

    /// Builder: queue a scripted process output (consumed in order by `run`).
    #[must_use]
    pub fn with_run_result(self, output: ProcOutput) -> Self {
        self.run_results.borrow_mut().push_back(output);
        self
    }

    /// Builder: set an environment variable visible to `env`.
    #[must_use]
    pub fn with_env<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Builder: set whether the mock reports an interactive TTY.
    #[must_use]
    pub const fn with_tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }

    /// Builder: set whether the mock reports the process as elevated.
    #[must_use]
    pub const fn with_elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Builder: set the deterministic clock value returned by `now`.
    #[must_use]
    pub fn with_now(self, now: DateTime<Utc>) -> Self {
        self.clock.set(now);
        self
    }

    /// Snapshot of the recorded call log, in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    /// Snapshot of the lines emitted via `out`, in order.
    #[must_use]
    pub fn output(&self) -> Vec<String> {
        self.out_lines.borrow().clone()
    }

    /// Current bytes of an in-memory file, if present.
    #[must_use]
    pub fn file(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.borrow().get(path).cloned()
    }

    /// Record a call in the ordered log.
    fn record(&self, call: Call) {
        self.calls.borrow_mut().push(call);
    }
}

impl super::Host for MockHost {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.record(Call::ReadFile(path.to_path_buf()));
        self.files.borrow().get(path).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file: {}", path.display()),
            )
        })
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.record(Call::WriteFile(path.to_path_buf()));
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), bytes.to_vec());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.record(Call::RemoveFile(path.to_path_buf()));
        if self.files.borrow_mut().remove(path).is_some() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file: {}", path.display()),
            ))
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.record(Call::Rename(from.to_path_buf(), to.to_path_buf()));
        let bytes = self.files.borrow_mut().remove(from).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file: {}", from.display()),
            )
        })?;
        self.files.borrow_mut().insert(to.to_path_buf(), bytes);
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.record(Call::Copy(from.to_path_buf(), to.to_path_buf()));
        let bytes = self.files.borrow().get(from).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file: {}", from.display()),
            )
        })?;
        self.files.borrow_mut().insert(to.to_path_buf(), bytes);
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        // The in-memory filesystem is flat (keyed by file path), so directory
        // creation is recorded for assertions but otherwise a no-op.
        self.record(Call::CreateDirAll(path.to_path_buf()));
        Ok(())
    }

    fn path_exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path)
    }

    fn run(&self, exe: &str, args: &[&str]) -> io::Result<ProcOutput> {
        self.record(Call::Run(
            exe.to_owned(),
            args.iter().map(|arg| (*arg).to_owned()).collect(),
        ));
        Ok(self
            .run_results
            .borrow_mut()
            .pop_front()
            .unwrap_or(ProcOutput {
                code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            }))
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.get()
    }

    fn sleep_ms(&self, millis: u64) {
        // No real waiting in tests; the request is recorded so a poll's cadence
        // can be asserted.
        self.record(Call::Sleep(millis));
    }

    fn is_tty(&self) -> bool {
        self.tty
    }

    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn read_key(&self) -> io::Result<char> {
        self.record(Call::ReadKey);
        self.keys
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no scripted keys left"))
    }

    fn out(&self, line: &str) {
        self.record(Call::Out(line.to_owned()));
        self.out_lines.borrow_mut().push(line.to_owned());
    }
}
