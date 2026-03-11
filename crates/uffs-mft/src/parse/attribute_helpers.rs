//! Helpers for parsing core NTFS attributes from MFT record bytes.

use crate::ntfs::{ExtendedStandardInfo, NameInfo, StreamInfo};

/// Parses `$STANDARD_INFORMATION` into `ExtendedStandardInfo`.
///
/// Handles both NTFS 1.2 (36 bytes) and NTFS 3.0+ (72 bytes) formats.
/// For NTFS 3.0+, also extracts `usn`, `security_id`, and `owner_id`.
#[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
pub(super) fn parse_standard_info_full(
    data: &[u8],
    attr_offset: usize,
    result: &mut ExtendedStandardInfo,
) {
    use core::mem::size_of;

    use crate::ntfs::{
        STANDARD_INFO_SIZE_V12, STANDARD_INFO_SIZE_V30, StandardInformation,
        StandardInformationExtended, filetime_to_unix_micros,
    };

    let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
    let value_length =
        u32::from_le_bytes(value_length_bytes.try_into().unwrap_or([0, 0, 0, 0])) as usize;
    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let si_offset = attr_offset + value_offset;

    if value_length >= STANDARD_INFO_SIZE_V30
        && si_offset + size_of::<StandardInformationExtended>() <= data.len()
    {
        // SAFETY: `si_offset` bounds are validated above and the struct is read from packed NTFS bytes.
        let si: StandardInformationExtended =
            unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

        *result = ExtendedStandardInfo {
            created: filetime_to_unix_micros(si.creation_time),
            modified: filetime_to_unix_micros(si.modification_time),
            accessed: filetime_to_unix_micros(si.access_time),
            mft_changed: filetime_to_unix_micros(si.mft_change_time),
            usn: si.usn,
            security_id: si.security_id,
            owner_id: si.owner_id,
            ..ExtendedStandardInfo::from_attributes(si.file_attributes)
        };
    } else if value_length >= STANDARD_INFO_SIZE_V12
        && si_offset + size_of::<StandardInformation>() <= data.len()
    {
        // SAFETY: `si_offset` bounds are validated above and the struct is read from packed NTFS bytes.
        let si: StandardInformation = unsafe { core::ptr::read(data[si_offset..].as_ptr().cast()) };

        *result = ExtendedStandardInfo {
            created: filetime_to_unix_micros(si.creation_time),
            modified: filetime_to_unix_micros(si.modification_time),
            accessed: filetime_to_unix_micros(si.access_time),
            mft_changed: filetime_to_unix_micros(si.mft_change_time),
            usn: 0,
            security_id: 0,
            owner_id: 0,
            ..ExtendedStandardInfo::from_attributes(si.file_attributes)
        };
    }
}

/// Parses `$FILE_NAME` and returns a `NameInfo` with timestamps.
#[expect(unsafe_code, reason = "FFI: ptr::read for packed NTFS struct")]
pub(super) fn parse_file_name_full(
    data: &[u8],
    attr_offset: usize,
    source_frs: u64,
) -> Option<NameInfo> {
    use core::mem::size_of;

    use smallvec::SmallVec;

    use crate::ntfs::{FileNameAttribute, file_reference_to_frs, filetime_to_unix_micros};

    let value_offset_bytes = &data[attr_offset + 20..attr_offset + 22];
    let value_offset = u16::from_le_bytes(value_offset_bytes.try_into().unwrap_or([0, 0])) as usize;

    let fn_offset = attr_offset + value_offset;
    if fn_offset + size_of::<FileNameAttribute>() > data.len() {
        return None;
    }

    // SAFETY: `fn_offset` bounds are validated above and the struct is read from packed NTFS bytes.
    let fn_attr: FileNameAttribute = unsafe { core::ptr::read(data[fn_offset..].as_ptr().cast()) };

    let name_len = fn_attr.file_name_length as usize;
    let name_offset = fn_offset + size_of::<FileNameAttribute>();

    if name_offset + name_len * 2 > data.len() {
        return None;
    }

    let name_bytes = &data[name_offset..name_offset + name_len * 2];
    #[expect(
        clippy::missing_asserts_for_indexing,
        reason = "chunks_exact(2) guarantees chunk.len() == 2"
    )]
    let name_u16: SmallVec<[u16; 128]> = name_bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let name = String::from_utf16(&name_u16).ok()?;

    Some(NameInfo {
        name,
        parent_frs: file_reference_to_frs(fn_attr.parent_directory),
        namespace: fn_attr.file_name_namespace,
        fn_created: filetime_to_unix_micros(fn_attr.creation_time),
        fn_modified: filetime_to_unix_micros(fn_attr.modification_time),
        fn_accessed: filetime_to_unix_micros(fn_attr.access_time),
        fn_mft_changed: filetime_to_unix_micros(fn_attr.mft_change_time),
        source_frs,
    })
}

/// Parses `$DATA` attribute and returns a `StreamInfo`.
///
/// # Special handling for `$BadClus:$Bad`
/// The `$BadClus` file (FRS 8) has a `$Bad` stream that is a sparse file
/// spanning the entire volume. C++ uses `InitializedSize` instead of `DataSize`
/// for this stream to avoid reporting the full volume size.
pub(super) fn parse_data_attribute_full(
    data: &[u8],
    attr_offset: usize,
    header: &crate::ntfs::AttributeRecordHeader,
    frs: u64,
) -> Option<StreamInfo> {
    use smallvec::SmallVec;

    let stream_name = if header.name_length > 0 {
        let name_offset = attr_offset + header.name_offset as usize;
        let name_len = header.name_length as usize;
        if name_offset + name_len * 2 > data.len() {
            return None;
        }
        let name_bytes = &data[name_offset..name_offset + name_len * 2];
        #[expect(
            clippy::missing_asserts_for_indexing,
            reason = "chunks_exact(2) guarantees chunk.len() == 2"
        )]
        let name_u16: SmallVec<[u16; 64]> = name_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&name_u16).unwrap_or_default()
    } else {
        String::new()
    };

    let is_resident = header.is_non_resident == 0;

    if !is_resident {
        let nr_offset = attr_offset + 16;
        if nr_offset + 8 > data.len() {
            return None;
        }
        let lowest_vcn = i64::from_le_bytes(data[nr_offset..nr_offset + 8].try_into().ok()?);
        if lowest_vcn != 0 {
            return None;
        }
    }

    let (size, allocated_size, is_sparse, is_compressed) = if is_resident {
        let value_length_bytes = &data[attr_offset + 16..attr_offset + 20];
        let value_length = u32::from_le_bytes(value_length_bytes.try_into().ok()?);
        (value_length as u64, 0, false, false)
    } else {
        let nr_offset = attr_offset + 16;
        if nr_offset + 48 > data.len() {
            return None;
        }

        let allocated_size =
            i64::from_le_bytes(data[nr_offset + 24..nr_offset + 32].try_into().ok()?);
        let data_size = i64::from_le_bytes(data[nr_offset + 32..nr_offset + 40].try_into().ok()?);
        let initialized_size =
            i64::from_le_bytes(data[nr_offset + 40..nr_offset + 48].try_into().ok()?);

        let compression_unit = data[nr_offset + 18];
        let is_compressed = compression_unit > 0;
        let is_sparse = (header.flags & 0x8000) != 0;

        let effective_allocated_raw = if is_compressed {
            if nr_offset + 56 <= data.len() {
                i64::from_le_bytes(data[nr_offset + 48..nr_offset + 56].try_into().ok()?)
            } else {
                allocated_size
            }
        } else {
            allocated_size
        };

        let is_badclus_bad = frs == 8 && stream_name == "$Bad";
        let effective_size = if is_badclus_bad {
            initialized_size.max(0) as u64
        } else {
            data_size.max(0) as u64
        };
        let effective_allocated = if is_badclus_bad {
            initialized_size.max(0) as u64
        } else {
            effective_allocated_raw.max(0) as u64
        };

        (
            effective_size,
            effective_allocated,
            is_sparse,
            is_compressed,
        )
    };

    Some(StreamInfo {
        name: stream_name,
        size,
        allocated_size,
        is_sparse,
        is_compressed,
        is_resident,
    })
}
