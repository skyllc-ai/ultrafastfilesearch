//! Tree column selection and parsing.

/// Tree-derived columns that can be computed on-demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeColumn {
    /// Count of all items (files + directories) under this directory.
    Descendants,
    /// Sum of logical file sizes under this directory.
    TreeSize,
    /// Sum of allocated sizes under this directory.
    TreeAllocated,
    /// Fragmentation metric: `tree_allocated` / `treesize` ratio.
    /// Higher values indicate more fragmentation/overhead.
    Bulkiness,
}

impl TreeColumn {
    /// Get the `DataFrame` column name for this tree column.
    #[must_use]
    pub const fn column_name(&self) -> &'static str {
        match self {
            Self::Descendants => "descendants",
            Self::TreeSize => "treesize",
            Self::TreeAllocated => "tree_allocated",
            Self::Bulkiness => "bulkiness",
        }
    }

    /// Parse a column name into a `TreeColumn`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "descendants" | "decendents" => Some(Self::Descendants),
            "treesize" | "tree_size" => Some(Self::TreeSize),
            "treeallocated" | "tree_allocated" => Some(Self::TreeAllocated),
            "bulkiness" => Some(Self::Bulkiness),
            _ => None,
        }
    }
}
