// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Storage serialization/deserialization regression tests.

use super::*;

const RECORD_COUNT_OFFSET: usize = 48;
const NAMES_SIZE_OFFSET: usize = 56;
const LINKS_COUNT_OFFSET: usize = 64;

fn empty_serialized_index() -> Vec<u8> {
    MftIndex::new(crate::platform::DriveLetter::C).serialize(123, 456, crate::usn::Usn::new(789))
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn deserialize_rejects_record_count_that_is_too_large() {
    let mut data = empty_serialized_index();
    write_u64(&mut data, RECORD_COUNT_OFFSET, u64::MAX);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Record section too large")
    ));
}

#[test]
fn deserialize_rejects_names_size_beyond_remaining_bytes() {
    let mut data = empty_serialized_index();
    // Use a size larger than remaining bytes after header+frs_to_idx+records.
    write_u64(&mut data, NAMES_SIZE_OFFSET, 9999);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Names section exceeds remaining data")
    ));
}

#[test]
fn deserialize_rejects_links_count_beyond_remaining_bytes() {
    let mut data = empty_serialized_index();
    write_u64(&mut data, LINKS_COUNT_OFFSET, 1);

    assert!(matches!(
        MftIndex::deserialize(&data),
        Err("Links section exceeds remaining data")
    ));
}
