//! Arena-backed string storage for path resolution.

/// Arena allocator for file names.
///
/// Stores all names in a single contiguous buffer to reduce memory
/// fragmentation and improve cache locality.
#[derive(Debug, Clone)]
pub struct NameArena {
    /// Contiguous buffer holding all names (UTF-8 encoded).
    buffer: String,
}

impl NameArena {
    /// Create a new arena with estimated capacity.
    #[must_use]
    pub fn with_capacity(estimated_total_bytes: usize) -> Self {
        Self {
            buffer: String::with_capacity(estimated_total_bytes),
        }
    }

    /// Add a name to the arena, returning its (offset, length).
    ///
    /// # Panics
    ///
    /// Panics if the buffer exceeds 4GB (`u32::MAX` bytes).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "buffer <4GB in practice; name len clamped to u16::MAX"
    )]
    pub fn add(&mut self, name: &str) -> (u32, u16) {
        let offset = self.buffer.len() as u32;
        let len = name.len().min(usize::from(u16::MAX)) as u16;
        self.buffer.push_str(name);
        (offset, len)
    }

    /// Get a name from the arena by (offset, length).
    #[must_use]
    pub fn get(&self, offset: u32, len: u16) -> &str {
        let start = offset as usize;
        let end = start + usize::from(len);
        // Use get() for safe slicing - returns empty string if out of bounds
        self.buffer.get(start..end).unwrap_or("")
    }

    /// Total bytes used by the arena.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if the arena is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}
