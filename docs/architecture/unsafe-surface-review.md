# Unsafe Surface Review

This note captures the production unsafe boundary after the Wave 3F closure sweep.

## Workspace posture

- Workspace default: `unsafe_code = "deny"` in `Cargo.toml`
- Exceptions are limited to Windows-facing `uffs-mft` modules
- Unsafe blocks are expected to carry scoped rationale via `#[expect(unsafe_code, reason = ...)]`

## Reviewed surfaces

### `crates/uffs-mft/src/platform.rs`

- Raw Windows handle creation and teardown (`CreateFileW`, `CloseHandle`)
- Volume/device control calls (`DeviceIoControl`, `GetVolumeInformationW`)
- Packed-struct reads for NTFS metadata (`ptr::read`)
- Narrow `unsafe impl Send/Sync` for handle wrappers with thread-safety comments

Rationale: this is the required FFI boundary for raw NTFS access and drive capability probing. The code keeps the unsafe boundary localized around Win32 calls and immediately converts failures into typed errors.

### `crates/uffs-mft/src/parse/fixup.rs`

- Packed NTFS header reads during fixup parsing

Rationale: NTFS on-disk structures are packed and require pointer reads. The function validates minimum buffer size before reading and returns corruption status instead of panicking.

### `crates/uffs-mft/src/usn.rs`

- Volume handle opening for USN journal reads

Rationale: Windows journal access is necessarily FFI-based. The unsafe section is limited to handle acquisition and immediately wrapped in Rust error handling.

## Review outcome

- No new unsafe blocks were required for Wave 3F closure work
- The unsafe surface remains concentrated in Windows-only I/O code
- Cross-platform query/pattern work in `uffs-core` stays `#![forbid(unsafe_code)]`

## Follow-up expectation

If new unsafe is introduced, keep it inside the existing Windows boundary modules, document the exact operation being performed, and prefer one operation per unsafe block with an explicit safety comment.