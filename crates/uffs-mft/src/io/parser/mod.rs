//! Windows-specific parsing bridges plus direct-to-index helpers.
//! Split into focused submodules while preserving the legacy `io` parser
//! surface.

mod fragment;
mod fragment_extension;
mod index;
mod index_extension;

#[expect(deprecated, reason = "re-exporting deprecated API for backward compatibility")]
pub use fragment::parse_record_to_fragment;
pub use index::parse_record_to_index;

pub use crate::parse::{
    ExtensionAttributes, ParseResult, ParsedColumns, ParsedRecord,
    add_missing_parent_placeholders_to_vec, create_placeholder_record, parse_record,
    parse_record_full, parse_record_zero_alloc,
};

#[cfg(test)]
mod tests {
    #[expect(deprecated, reason = "testing deprecated parse_record_to_index API")]
    use super::parse_record_to_fragment;
    use super::parse_record_to_index;
    use crate::index::{MftIndex, MftIndexFragment};

    #[test]
    fn parse_record_to_index_rejects_short_buffers() {
        let mut index = MftIndex::new('C');
        assert!(!parse_record_to_index(&[0_u8; 3], 42, &mut index));
    }

    #[test]
    #[expect(deprecated, reason = "testing deprecated parse_record_to_fragment API")]
    fn parse_record_to_fragment_rejects_short_buffers() {
        let mut fragment = MftIndexFragment::with_capacity(1);
        assert!(!parse_record_to_fragment(&[0_u8; 3], 42, &mut fragment));
    }
}
