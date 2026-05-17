// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Compiled name-patterns for the aggregate engine's per-record filter.
//!
//! Only `compile_parsed_pattern` (and its private `_with_fold` peer) is
//! called from outside this submodule — from `crate::aggregate` — to build
//! an `IndexPattern` that the aggregate engine then drives via
//! `IndexPattern::matches`.  Everything else (`IndexQuery`, `SearchResult`,
//! `QueryMode`, routing helpers, the standalone `compile_index_pattern` /
//! `compile_extensions` entry points) was removed in #263 after a
//! workspace-wide audit confirmed zero callers anywhere.  The submodule is
//! `pub(crate)` (see `crate::lib.rs`) because the aggregate engine is the
//! only caller.

pub(crate) use self::pattern::compile_parsed_pattern;

mod pattern;

#[cfg(test)]
mod tests;
