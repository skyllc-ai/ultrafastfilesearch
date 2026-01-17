// src/modules/directory/
// ├── mod.rs             # Module entry point for directory functions
// ├── reader.rs          # Functions for reading directories
// ├── metadata.rs        # Functions for fetching directory metadata
// └── link_manager.rs    # Managing symbolic links or other link types

pub(crate) mod link_manager;
pub(crate) mod metadata;
pub(crate) mod reader;
