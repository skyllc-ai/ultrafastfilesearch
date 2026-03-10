//! Focused test groups for the `index` module.

pub(super) use super::*;

#[path = "tests_helpers.rs"]
mod tests_helpers;

#[path = "tests_core.rs"]
mod tests_core;

#[path = "tests_extensions.rs"]
mod tests_extensions;

#[path = "tests_children.rs"]
mod tests_children;

#[path = "tests_tree.rs"]
mod tests_tree;

#[path = "tests_perf.rs"]
mod tests_perf;

#[path = "tests_merge.rs"]
mod tests_merge;

#[path = "tests_storage.rs"]
mod tests_storage;
