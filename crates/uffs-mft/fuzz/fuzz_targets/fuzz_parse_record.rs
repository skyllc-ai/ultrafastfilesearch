//! Fuzz target for MFT record parsing.
//!
//! Tests that `parse_record` handles arbitrary input without panicking.
//! MFT records are typically 1024 bytes but we test all sizes.
//!
//! Run with: `cargo +nightly fuzz run fuzz_parse_record`

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Test parsing with various FRS values
    let _ = uffs_mft::parse::parse_record(data, 0);
    let _ = uffs_mft::parse::parse_record(data, 5); // $ROOT
    let _ = uffs_mft::parse::parse_record(data, u64::MAX);

    // Test full parsing
    let _ = uffs_mft::parse::parse_record_full(data, 0);

    // Test zero-alloc parsing
    let _ = uffs_mft::parse::parse_record_zero_alloc(data, 0);
});

