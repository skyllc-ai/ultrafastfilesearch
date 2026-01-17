//! # UFFS Legacy Library
//!
//! **⚠️ DEPRECATED: This crate contains legacy code kept for reference only.**
//!
//! Use the modern crates instead:
//! - `uffs-core` - Query engine
//! - `uffs-mft` - MFT reading
//! - `uffs-cli` - Command-line interface
//!
//! This legacy code provides:
//! - Cross-platform file search (original implementation)
//! - Windows WMI disk queries
//! - Drive information utilities

pub mod config;
pub mod modules;

// Re-export only the necessary functions or types to be exposed publicly
// pub use crate::modules::directory_reader::;
// pub use crate::modules::directory_reader::read_directory;
// pub use crate::modules::utils::print_directory_tree; // Re-export the
// function
