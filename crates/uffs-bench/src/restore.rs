// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! The "no crumb left behind" restore machinery.
//!
//! [`RestoreRegistry`] is a LIFO stack of undo closures. The orchestrator
//! registers an undo **before** performing each mutation, so [`drain`] replays
//! them in reverse order (last mutation undone first). [`RunGuard`] wraps a
//! registry together with the [`Host`] it should restore through, and its
//! [`Drop`] implementation drains on early return or panic — the Rust analogue
//! of a `try/finally` block.
//!
//! [`drain`]: RestoreRegistry::drain

use crate::error::{CrumbError, Result};
use crate::host::Host;

/// A boxed undo action: given the host, reverse one mutation.
type Undo = Box<dyn FnOnce(&dyn Host) -> Result<()>>;

/// LIFO stack of labelled undo closures.
#[derive(Default)]
pub struct RestoreRegistry {
    /// Registered `(label, undo)` pairs; popped back-to-front on drain.
    actions: Vec<(String, Undo)>,
}

impl RestoreRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an undo action, to be run before any earlier-registered one.
    ///
    /// Always call this *before* performing the corresponding mutation, so the
    /// registry is correct even if the mutation itself fails part-way.
    pub fn register<L, F>(&mut self, label: L, undo: F)
    where
        L: Into<String>,
        F: FnOnce(&dyn Host) -> Result<()> + 'static,
    {
        self.actions.push((label.into(), Box::new(undo)));
    }

    /// Number of pending undo actions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether there are no pending undo actions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Run every pending undo in LIFO order, collecting (never propagating)
    /// failures so one bad undo cannot block the rest.
    ///
    /// Returns one [`CrumbError`] per failed undo; an empty vector means the
    /// host was fully restored. This method never panics.
    pub fn drain(&mut self, host: &dyn Host) -> Vec<CrumbError> {
        let mut crumbs = Vec::new();
        while let Some((label, undo)) = self.actions.pop() {
            if let Err(source) = undo(host) {
                crumbs.push(CrumbError { label, source });
            }
        }
        crumbs
    }
}

/// Drop-guarded pairing of a [`RestoreRegistry`] with the [`Host`] it restores.
///
/// On the happy path call [`finish`](RunGuard::finish) to drain explicitly and
/// inspect the crumbs. If a stage returns early or panics first, [`Drop`]
/// drains as a safety net — crumbs cannot be returned from `drop`, so they are
/// discarded there; the explicit `finish` path is how callers surface them.
pub struct RunGuard<'h> {
    /// The wrapped registry.
    registry: RestoreRegistry,
    /// The host every undo is replayed against.
    host: &'h dyn Host,
    /// Set once [`finish`](RunGuard::finish) has drained, to suppress the
    /// safety-net drain in [`Drop`].
    finished: bool,
}

impl<'h> RunGuard<'h> {
    /// Wrap a fresh registry around `host`.
    #[must_use]
    pub fn new(host: &'h dyn Host) -> Self {
        Self {
            registry: RestoreRegistry::new(),
            host,
            finished: false,
        }
    }

    /// Register an undo action (delegates to the wrapped registry).
    pub fn register<L, F>(&mut self, label: L, undo: F)
    where
        L: Into<String>,
        F: FnOnce(&dyn Host) -> Result<()> + 'static,
    {
        self.registry.register(label, undo);
    }

    /// Number of pending undo actions in the wrapped registry.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.registry.len()
    }

    /// Explicitly drain on the happy path, returning any restore failures and
    /// suppressing the [`Drop`] safety net.
    #[must_use]
    pub fn finish(mut self) -> Vec<CrumbError> {
        self.finished = true;
        self.registry.drain(self.host)
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Safety-net drain on early return / panic. Crumbs cannot escape a
            // `drop`, so they are intentionally discarded here; the happy path
            // uses `finish()` to surface them.
            let _crumbs = self.registry.drain(self.host);
        }
    }
}
