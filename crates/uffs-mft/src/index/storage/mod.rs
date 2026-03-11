//! Persistent on-disk serialization for `MftIndex` snapshots.

/// Binary index deserialization implementation.
mod deserialize;
/// File-based storage wrappers.
mod file_io;
/// Storage header and format version metadata.
mod header;
/// Binary index serialization implementation.
mod serialize;

pub use self::header::IndexHeader;
