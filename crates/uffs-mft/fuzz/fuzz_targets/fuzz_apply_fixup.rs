//! Fuzz target for MFT fixup (Update Sequence Array) application.
//!
//! Tests that `apply_fixup` handles arbitrary input without panicking.
//! This is security-critical as malformed fixup data could cause buffer overflows.
//!
//! Run with: `cargo +nightly fuzz run fuzz_apply_fixup`

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // apply_fixup modifies data in place, so we need a mutable copy
    let mut buffer = data.to_vec();

    // Test fixup application - should never panic
    let _ = uffs_mft::parse::apply_fixup(&mut buffer);
});

