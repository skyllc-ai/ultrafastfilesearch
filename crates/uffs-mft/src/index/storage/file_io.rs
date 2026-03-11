//! File-based persistence wrappers for `MftIndex` snapshots.

use super::IndexHeader;
use crate::index::MftIndex;

impl MftIndex {
    /// Saves the index to a file.
    ///
    /// # Errors
    ///
    /// Returns an error if file writing fails.
    pub fn save_to_file(
        &self,
        path: &std::path::Path,
        volume_serial: u64,
        usn_journal_id: u64,
        next_usn: i64,
    ) -> std::io::Result<()> {
        use std::io::Write;

        let data = self.serialize(volume_serial, usn_journal_id, next_usn);
        let mut file = std::fs::File::create(path)?;
        file.write_all(&data)?;
        Ok(())
    }

    /// Loads an index from a file.
    ///
    /// # Errors
    ///
    /// Returns an error if file reading fails or data is corrupted.
    pub fn load_from_file(
        path: &std::path::Path,
    ) -> Result<(Self, IndexHeader), Box<dyn core::error::Error>> {
        let data = std::fs::read(path)?;
        let (index, header) = Self::deserialize(&data)?;
        Ok((index, header))
    }
}
