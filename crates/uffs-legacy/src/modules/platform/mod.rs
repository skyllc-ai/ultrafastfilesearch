// ├── platform/                   # Platform-specific logic
// │   ├── mod.rs                  # Platform module entry point, exposing OS
// traits and platform handling │   ├── windows.rs              #
// Windows-specific platform code (drive listing, etc.) │   ├── unix.rs
// # Unix-specific platform code (handling EXT, symbolic links, etc.)
// │   └── macos.rs                # macOS-specific code (HFS+, APFS, etc.)

mod macos;
mod unix;
mod windows;
