// ├── fs/                         # File system-specific handlers
// │   ├── mod.rs                  # File system trait and dispatcher
// │   ├── ntfs.rs                 # NTFS-specific logic and implementation
// │   ├── ext.rs                  # EXT2/3/4-specific logic and implementation
// │   ├── xfs.rs                  # XFS-specific logic and implementation
// │   └── macfs.rs                # Mac file systems logic (HFS/APFS)

mod ext;
mod macfs;
mod ntfs;
mod xfs;
