<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# Changelog

All notable changes to UFFS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Modernization documentation and tracking (Wave 1-6 guides)
- UFFS-specific modernization plan for 2026
- Mandatory wave completion flow documentation

### Changed
- Cleaned up all TTAPI references from justfile and build scripts
- Updated justfile header and recipes for UFFS

## [0.2.116] - 2026-01-27

### Added
- Baseline CI validation for modernization effort
- Windows cross-compilation for all binaries (uffs, uffs_mft, uffs_tui, uffs_gui)
- Modernization tracker and wave guides

### Changed
- Updated Polars to commit 8b99db82

## [0.2.114] - 2026-01-26

### Added
- Initial UFFS Rust implementation
- MFT reading and parsing with Polars DataFrames
- Path resolution during MFT digestion
- Hard link expansion (default on)
- Multi-drive parallel indexing support
- Cache architecture with zstd compression

### Fixed
- Various MFT parsing edge cases

[Unreleased]: https://github.com/githubrobbi/UltraFastFileSearch/compare/v0.2.116...HEAD
[0.2.116]: https://github.com/githubrobbi/UltraFastFileSearch/compare/v0.2.114...v0.2.116
[0.2.114]: https://github.com/githubrobbi/UltraFastFileSearch/releases/tag/v0.2.114

