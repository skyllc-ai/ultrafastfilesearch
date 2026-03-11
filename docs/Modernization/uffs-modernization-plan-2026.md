<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
Contact: 50460704+githubrobbi@users.noreply.github.com for licensing inquiries
-->

# 🦀 UFFS Rust Master Modernization Plan 2026

> **Status**: 📋 **PLANNING** | **Created**: 2026-01-27 | **Architect**: Rust Master
> **Scope**: Cutting-Edge Rust Excellence | **Target**: World-Class NTFS File Search
>
> 📊 **Progress Tracking**: [`MODERNIZATION_TRACKER.md`](MODERNIZATION_TRACKER.md)

---

## ⚠️ Implementation Rules of Engagement

> **These rules are NON-NEGOTIABLE during all modernization work.**

### 1. No Suppression Hacks
- ❌ **NEVER** add blanket `#[allow(...)]` attributes
- ❌ **NEVER** disable lints to make warnings disappear
- ❌ **NEVER** comment out failing tests
- ❌ **NEVER** hide problems behind `cfg` gates
- ✅ If a targeted `#[allow(...)]` is truly necessary, keep it **minimal**, **scoped**, and **justify it in code comments**

### 2. Surgical, Correct Fixes
- ✅ Prefer **minimal, idiomatic Rust** changes
- ✅ Resolve **root causes** (ownership, types, semantics)
- ❌ Avoid superficial workarounds that mask the real issue
- ✅ Each fix should be the smallest change that correctly solves the problem

### 3. Preserve Behavior & Contracts
- ✅ Maintain **public API** and **observable behavior**
- ✅ Only change behavior if CI failures prove it's wrong or inconsistent
- ✅ When behavior changes are necessary, update **docs AND tests** accordingly
- ❌ Never silently change semantics without documentation

### 4. Improve Tests, Don't Dodge Them
- ✅ **Strengthen** tests to be deterministic and meaningful
- ❌ **NEVER** skip or relax tests just to pass CI
- ✅ If a test is flaky, fix the flakiness (not the test)
- ✅ Add tests for any bugs discovered during modernization

### 5. Tests Should FAIL FAST AND LOUD
- ✅ **`unwrap()`, `expect()`, `panic!()`** are **ENCOURAGED** in test code
- ✅ Tests exist to find bugs quickly - let them crash hard
- ✅ No graceful error handling in tests - we want immediate, obvious failures
- ❌ **NEVER** suppress `unwrap_used` or `expect_used` lints in production code
- ✅ Production code uses proper error handling (`?`, `Result`, `UffsError`)
- ✅ Test code uses `unwrap()` for maximum clarity and fail-fast behavior

### 6. Document & Commit Well
- ✅ Make **small, atomic commits** with clear messages
- ✅ Use format: `fix: concise root cause` or `feat: what was added`
- ✅ **BEFORE** each CI pipeline run, create or update the healing changelog
- ✅ The healing changelog **MUST** be part of the final commit/push

### CI Pipeline Validation Protocol

**When to run**: After completing any major step or wave milestone.

```bash
# Full CI pipeline - creates snapshot with git push, builds binaries, etc.
rust-script scripts/ci-pipeline.rs go -v
```

### Healing Changelog Protocol

**Location**: `LOG/<<YYYY_MM_DD_HH_MM>>_CHANGELOG_HEALING.md`

**Workflow**:
1. **Before CI starts**: Create the healing changelog file
2. **If pipeline fails**: Document what failed, why, and how you're fixing it
3. **Before restart**: Update the changelog with new findings
4. **Before push**: Ensure changelog is committed with all other changes

---

## 📋 Executive Summary

This document outlines a comprehensive modernization roadmap to elevate UFFS to the **highest standards of Rust excellence**. UFFS is a Windows NTFS file search tool that reads the Master File Table (MFT) directly for blazing-fast file enumeration using Polars DataFrames.

### Current State Assessment

| Metric | Current | Target | Status |
|--------|---------|--------|--------|
| **Rust Edition** | 2024 | 2024 | ✅ **Cutting-Edge** |
| **Toolchain** | Nightly 1.85+ | Nightly | ✅ **Latest** |
| **Crate Count** | 8 | 8 | ✅ **Optimized** |
| **Clippy Rules** | ~100 deny/warn | 100+ | ✅ **Comprehensive** |
| **Test Coverage Target** | TBD | 90% | 🎯 **Improvement Needed** |
| **Dependencies** | All current | All current | ✅ **Up-to-date** |

### UFFS Crate Architecture

| Crate | Purpose | Priority |
|-------|---------|----------|
| `uffs-polars` | Polars facade (compilation isolation) | 🔴 Critical |
| `uffs-mft` | MFT reading → Polars DataFrame | 🔴 Critical |
| `uffs-core` | Query engine using Polars lazy API | 🔴 Critical |
| `uffs-cli` | Command-line interface | 🟠 Major |
| `uffs-tui` | Terminal UI | 🟡 Moderate |
| `uffs-gui` | Graphical UI (future) | 🟢 Enhancement |
| `uffs-diag` | Diagnostic tools | 🟡 Moderate |

### What's Already Excellent ✅

1. **Edition 2024** - Latest Rust edition
2. **Nightly Toolchain** - With miri, llvm-tools, rust-analyzer
3. **Super-Strict Linting** - ~100 clippy rules at deny/warn level
4. **Dependency Management** - deny.toml with license/security checks
5. **CI/CD Pipeline** - rust-script based with optimized resource usage
6. **Error Handling** - thiserror + miette diagnostics
7. **Test Infrastructure** - nextest, llvm-cov
8. **Benchmarking** - criterion + divan (modern)
9. **Security Auditing** - cargo-audit, cargo-deny
10. **Cross-Compilation** - 5 target platforms defined
11. **Build Profiles** - 7 profiles (dev, debug-optimized, release, profiling, bench, dist, xwin-dev)
12. **MFT Performance** - Direct NTFS MFT reading with async I/O

---

## 🎯 Modernization Waves

### Wave Overview

| Wave | Focus | Effort | Impact | Priority | Implementation Guide |
|------|-------|--------|--------|----------|---------------------|
| **1** | Immediate Wins | 1-2 days | High | 🔴 Critical | [📘 Guide](modernization-guides/wave-1-immediate-wins.md) |
| **2** | Architecture Completion | 3-5 days | High | 🔴 Critical | [📘 Guide](modernization-guides/wave-2-architecture-completion.md) |
| **2.5** | Module Restructuring | 3-5 days | High | 🔴 Critical | [📘 Guide](modernization-guides/wave-2.5-module-restructuring.md) |
| **3** | Testing Excellence | 2-3 days | High | 🟠 Major | [📘 Guide](modernization-guides/wave-3-testing-excellence.md) |
| **4** | Documentation & API | 2-3 days | Medium | 🟠 Major | [📘 Guide](modernization-guides/wave-4-documentation-api.md) |
| **5** | Performance & Observability | 2-3 days | Medium | 🟡 Moderate | [📘 Guide](modernization-guides/wave-5-performance-observability.md) |
| **6** | Advanced Tooling | 1-2 days | Low | 🟢 Enhancement | [📘 Guide](modernization-guides/wave-6-advanced-tooling.md) |

> 📚 **Implementation Guides**: Each wave has a detailed step-by-step guide in [`modernization-guides/`](modernization-guides/README.md)

---

## 🌊 Wave 1: Immediate Wins (1-2 Days)

> 📘 **Detailed Guide**: [wave-1-immediate-wins.md](modernization-guides/wave-1-immediate-wins.md)

**Goal**: Quick, high-impact improvements with minimal effort

### 1.1 MSRV Policy Formalization
**Status**: ✅ Implemented | **Priority**: 🔴 Critical

Already have `rust-version = "1.85"` in Cargo.toml.

### 1.2 Changelog Automation
**Status**: ❌ Not Implemented | **Priority**: 🔴 Critical

- Add `CHANGELOG.md` following [Keep a Changelog](https://keepachangelog.com/) format
- Configure release-plz for automatic changelog updates

### 1.3 Semantic Versioning Checks
**Status**: ❌ Not Implemented | **Priority**: 🔴 Critical

Add `cargo-semver-checks` to CI pipeline for breaking change detection.

### 1.4 Fuzz Testing Infrastructure
**Status**: ❌ Not Implemented | **Priority**: 🟠 Major

Set up cargo-fuzz for security-critical MFT parsing code:
- MFT record parsing
- Attribute parsing ($FILE_NAME, $DATA, etc.)
- Path resolution logic

### 1.5 Mutation Testing
**Status**: ❌ Not Implemented | **Priority**: 🟠 Major

Add cargo-mutants for test quality validation. Target: ≥70% mutants killed.

---

## 🌊 Wave 2: Architecture Completion (3-5 Days)

> 📘 **Detailed Guide**: [wave-2-architecture-completion.md](modernization-guides/wave-2-architecture-completion.md)

**Goal**: Complete MFT pipeline architecture and error handling

### 2.1 MFT Pipeline Architecture Audit
**Status**: 🔧 In Progress | **Priority**: 🔴 Critical

Ensure the MFT reading pipeline is optimized:
- [ ] Multi-drive parallel indexing (match C++ IOCP performance)
- [ ] Path resolution during MFT digestion (not after)
- [ ] Tree metrics calculation optimization
- [ ] Hard link expansion (default on, power-user switch to disable)

### 2.2 Error Boundary Enforcement
**Status**: 🔧 Partial | **Priority**: 🔴 Critical

Ensure consistent error handling at crate boundaries:
- Public APIs return `UffsError` (not `anyhow::Error`)
- Rich error context with `.context()` / `.with_context()`
- Proper `From` implementations for error conversion

### 2.3 Async Architecture Audit
**Status**: ❌ Not Complete | **Priority**: 🟠 Major

Review async patterns in MFT reading:
- [ ] Catalog all `tokio::spawn` calls
- [ ] Implement cancellation tokens for long-running operations
- [ ] Add graceful shutdown handlers

### 2.4 Cache Architecture
**Status**: 🔧 Partial | **Priority**: 🟠 Major

Optimize caching strategy:
- [ ] Default cache enabled (opt-out with `--no-cache`)
- [ ] Zstd compression for cache files
- [ ] Cache invalidation based on MFT sequence numbers

---

## 🌊 Wave 2.5: Module Restructuring (3-5 Days)

> 📘 **Detailed Guide**: [wave-2.5-module-restructuring.md](modernization-guides/wave-2.5-module-restructuring.md)

**Goal**: Each file = one primary type or responsibility, 200-500 lines max

### 2.5.1 Critical Files (>2000 lines)
**Status**: ⬜ Not Started | **Priority**: 🔴 Critical

Split oversized files into focused modules:
- [ ] `io.rs` (6,623 lines) → `io/` directory
- [ ] `index.rs` (5,690 lines) → `index/` directory
- [ ] `main.rs` (4,543 lines) → Extract CLI logic
- [ ] `reader.rs` (4,475 lines) → `reader/` directory
- [ ] `commands.rs` (2,520 lines) → `commands/` directory

### 2.5.2 Large Files (1000-2000 lines)
**Status**: ⬜ Not Started | **Priority**: 🟠 Major

Split remaining large files:
- [ ] `platform.rs` (1,872 lines)
- [ ] `parse.rs` (1,861 lines)
- [ ] `ntfs.rs` (1,457 lines)
- [ ] `index_search.rs` (1,334 lines)
- [ ] `path_resolver.rs` (1,107 lines)
- [ ] `raw.rs` (1,041 lines)

### 2.5.3 Medium Files (500-1000 lines)
**Status**: ⬜ Not Started | **Priority**: 🟡 Moderate

Review and split if multiple types:
- [ ] `compiled_pattern.rs` (974 lines)
- [ ] `tree.rs` (854 lines)
- [ ] `output.rs` (790 lines)
- [ ] `query.rs` (679 lines)
- [ ] `extensions.rs` (616 lines)

---

## 🌊 Wave 3: Testing Excellence (2-3 Days)

> 📘 **Detailed Guide**: [wave-3-testing-excellence.md](modernization-guides/wave-3-testing-excellence.md)

**Goal**: Achieve world-class test coverage and quality

### 3.1 Coverage Target
**Status**: 🎯 TBD → 90% | **Priority**: 🟠 Major

Establish baseline and increase to 90% project target.

### 3.2 MFT Parsing Tests
**Status**: 🔧 Partial | **Priority**: 🔴 Critical

Comprehensive tests for:
- [ ] MFT record parsing (all attribute types)
- [ ] Path resolution accuracy
- [ ] Hard link detection and expansion
- [ ] ADS (Alternate Data Streams) handling
- [ ] Tree metrics calculation

### 3.3 Property-Based Testing
**Status**: ❌ Not Implemented | **Priority**: 🟠 Major

Add proptest for:
- Path resolution edge cases
- Filter expression parsing
- Size/date range queries

### 3.4 Performance Regression Testing
**Status**: ❌ Not Implemented | **Priority**: 🟠 Major

Add criterion baseline management for:
- MFT reading speed (records/sec)
- Path resolution throughput
- Query execution time

---

## 🌊 Wave 4: Documentation & API Excellence (2-3 Days)

> 📘 **Detailed Guide**: [wave-4-documentation-api.md](modernization-guides/wave-4-documentation-api.md)

**Goal**: Comprehensive, professional documentation

### 4.1 Rustdoc Coverage
**Status**: ❌ Not Enforced | **Priority**: 🟠 Major

Target: 100% public API documentation with examples.

### 4.2 CLI Documentation
**Status**: 🔧 Partial | **Priority**: 🟠 Major

- [ ] Complete `--help` for all commands
- [ ] Man page generation
- [ ] Usage examples in README

### 4.3 MFT Field Documentation
**Status**: ❌ Not Complete | **Priority**: 🟡 Moderate

Document all MFT fields and their meanings:
- Standard Information attribute
- File Name attribute
- Data attribute
- Index allocation

### 4.4 Architecture Documentation
**Status**: 🔧 Partial | **Priority**: 🟡 Moderate

Add Mermaid diagrams for:
- [ ] Crate dependency graph
- [ ] MFT reading pipeline
- [ ] Path resolution algorithm
- [ ] Query execution flow

---

## 🌊 Wave 5: Performance & Observability (2-3 Days)

> 📘 **Detailed Guide**: [wave-5-performance-observability.md](modernization-guides/wave-5-performance-observability.md)

**Goal**: Establish performance baselines and monitoring

### 5.1 Performance Baselines
**Status**: 🔧 Partial | **Priority**: 🟡 Moderate

Establish and track:
- MFT reading: records/sec per drive
- Path resolution: paths/sec
- Query execution: queries/sec
- Memory usage: peak RSS during full index

### 5.2 Tracing & Telemetry
**Status**: 🔧 Partial | **Priority**: 🟡 Moderate

Enhance tracing with structured spans:
- MFT reading progress
- Path resolution timing
- Query execution breakdown

### 5.3 Memory Profiling
**Status**: ❌ Not Formalized | **Priority**: 🟡 Moderate

Targets:
- Peak RSS during indexing
- Memory per million files
- DataFrame memory efficiency

### 5.4 Flamegraph Automation
**Status**: ❌ Not Automated | **Priority**: 🟡 Moderate

Add flamegraph generation to justfile for profiling hot paths.

---

## 🌊 Wave 6: Advanced Tooling (1-2 Days)

> 📘 **Detailed Guide**: [wave-6-advanced-tooling.md](modernization-guides/wave-6-advanced-tooling.md)

**Goal**: Leverage cutting-edge Rust tooling

### 6.1 tokio-console Integration
**Status**: ❌ Not Implemented | **Priority**: 🟢 Enhancement

Add tokio-console for async debugging during development.

### 6.2 Unused Dependency Detection
**Status**: ❌ Not Automated | **Priority**: 🟢 Enhancement

Add cargo-machete to CI for automatic unused dependency detection.

### 6.3 Build Caching with sccache
**Status**: 🔧 Partial | **Priority**: 🟢 Enhancement

Enable sccache for faster local development builds.

### 6.4 cargo-expand for Macro Debugging
**Status**: ❌ Not Documented | **Priority**: 🟢 Enhancement

Document macro debugging workflow for complex derive macros.

---

## 📊 Success Metrics

### Code Quality Metrics

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Test Coverage | TBD | 90% | `cargo llvm-cov` |
| Mutation Score | Unknown | ≥70% | `cargo mutants` |
| Clippy Warnings | 0 | 0 | `cargo clippy` |
| Rustdoc Coverage | Unknown | 100% | `cargo doc --show-coverage` |

### Performance Metrics (vs C++ uffs.com)

| Metric | C++ Baseline | Rust Target | Measurement |
|--------|--------------|-------------|-------------|
| MFT Read Speed | ~1M rec/sec | ≥1M rec/sec | Criterion benchmark |
| Path Resolution | TBD | Match or exceed | Criterion benchmark |
| Memory Usage | TBD | ≤ C++ | Peak RSS measurement |
| Startup Time | TBD | ≤ 100ms | hyperfine |

---

## 🔧 Tool Installation Summary

```bash
# Core tools (already installed)
cargo install cargo-nextest
cargo install cargo-llvm-cov
cargo install cargo-audit
cargo install cargo-deny

# New tools for modernization
cargo install cargo-semver-checks    # Wave 1.3
cargo install cargo-fuzz             # Wave 1.4
cargo install cargo-mutants          # Wave 1.5
cargo install cargo-machete          # Wave 6.2
cargo install cargo-expand           # Wave 6.4
cargo install flamegraph             # Wave 5.4
cargo install tokio-console          # Wave 6.1
```

---

**Document Version**: 1.0.0
**Last Updated**: 2026-01-27
**Author**: Rust Master

