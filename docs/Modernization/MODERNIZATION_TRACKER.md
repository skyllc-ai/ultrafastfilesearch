<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🦀 UFFS Modernization Tracker

> **Reference**: [`uffs-modernization-plan-2026.md`](uffs-modernization-plan-2026.md)
> **Last Updated**: 2026-01-27
> **Current Phase**: Planning

---

## ⚠️ Rules of Engagement (Summary)

> **Full details in the [modernization plan](uffs-modernization-plan-2026.md#-implementation-rules-of-engagement)**

| Rule | Requirement |
|------|-------------|
| **No Suppression Hacks** | No blanket `#[allow(...)]`, no disabling lints, no commenting out tests |
| **Surgical Fixes** | Minimal, idiomatic Rust; fix root causes, not symptoms |
| **Preserve Contracts** | Maintain public API; update docs/tests if behavior must change |
| **Improve Tests** | Strengthen tests; never skip or relax to pass CI |
| **Document Well** | Atomic commits; healing changelog BEFORE each CI run |

### 🚀 CI Pipeline Validation
```bash
# Run after completing any major step or wave milestone
rust-script scripts/ci-pipeline.rs go -v
```

### 📝 Healing Changelog Protocol
```
Location: LOG/<<YYYY_MM_DD_HH_MM>>_CHANGELOG_HEALING.md

1. CREATE file BEFORE CI pipeline starts
2. UPDATE if pipeline fails (document: what failed, why, fix)
3. COMMIT with all changes before push
```

---

## 📊 Overall Progress

| Wave | Status | Progress | Started | Completed |
|------|--------|----------|---------|-----------|
| **1** Immediate Wins | ⬜ Not Started | 0/5 | - | - |
| **2** Architecture Completion | ⬜ Not Started | 0/4 | - | - |
| **3** Testing Excellence | ⬜ Not Started | 0/4 | - | - |
| **4** Documentation & API | ⬜ Not Started | 0/4 | - | - |
| **5** Performance & Observability | ⬜ Not Started | 0/4 | - | - |
| **6** Advanced Tooling | ⬜ Not Started | 0/4 | - | - |

**Legend**: ⬜ Not Started | 🔧 In Progress | ✅ Complete | ⏸️ Blocked | ❌ Cancelled

---

## 🌊 Wave 1: Immediate Wins

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 1.1 | MSRV Policy Formalization | ✅ | `rust-version = "1.85"` already in Cargo.toml |
| 1.2 | Changelog Automation | ⬜ | Create CHANGELOG.md, configure release-plz |
| 1.3 | Semantic Versioning Checks | ⬜ | Add cargo-semver-checks to CI |
| 1.4 | Fuzz Testing Infrastructure | ⬜ | Setup cargo-fuzz for MFT parsing |
| 1.5 | Mutation Testing | ⬜ | Add cargo-mutants, target ≥70% killed |

---

## 🌊 Wave 2: Architecture Completion

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 2.1 | MFT Pipeline Architecture Audit | 🔧 | Multi-drive parallel, path resolution timing |
| 2.2 | Error Boundary Enforcement | ⬜ | Audit pub fn for UffsError usage |
| 2.3 | Async Architecture Audit | ⬜ | Catalog tokio::spawn, add cancellation |
| 2.4 | Cache Architecture | ⬜ | Default on, zstd compression |

---

## 🌊 Wave 3: Testing Excellence

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 3.1 | Coverage Target | ⬜ | Establish baseline, target 90% |
| 3.2 | MFT Parsing Tests | ⬜ | All attribute types, path resolution |
| 3.3 | Property-Based Testing | ⬜ | Add proptest for edge cases |
| 3.4 | Performance Regression Testing | ⬜ | Add criterion baseline management |

---

## 🌊 Wave 4: Documentation & API

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 4.1 | Rustdoc Coverage | ⬜ | Target 100% public API docs |
| 4.2 | CLI Documentation | ⬜ | Complete --help, man pages |
| 4.3 | MFT Field Documentation | ⬜ | Document all MFT fields |
| 4.4 | Architecture Documentation | ⬜ | Add Mermaid diagrams |

---

## 🌊 Wave 5: Performance & Observability

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 5.1 | Performance Baselines | ⬜ | MFT read speed, path resolution |
| 5.2 | Tracing & Telemetry | ⬜ | Structured spans for MFT ops |
| 5.3 | Memory Profiling | ⬜ | Peak RSS, memory per million files |
| 5.4 | Flamegraph Automation | ⬜ | Add flamegraph recipe to justfile |

---

## 🌊 Wave 6: Advanced Tooling

| ID | Task | Status | Notes |
|----|------|--------|-------|
| 6.1 | tokio-console Integration | ⬜ | Add console-subscriber feature |
| 6.2 | Unused Dependency Detection | ⬜ | Add cargo-machete to CI |
| 6.3 | Build Caching with sccache | ⬜ | Enable for local development |
| 6.4 | cargo-expand Documentation | ⬜ | Document macro debugging workflow |

---

## 📈 Metrics Tracking

### Code Quality

| Metric | Baseline | Current | Target | Trend |
|--------|----------|---------|--------|-------|
| Test Coverage | - | - | 90% | - |
| Mutation Score | - | - | ≥70% | - |
| Clippy Warnings | 0 | 0 | 0 | ✅ |
| Rustdoc Coverage | - | - | 100% | - |

### Performance (vs C++ uffs.com)

| Metric | C++ Baseline | Rust Current | Target | Trend |
|--------|--------------|--------------|--------|-------|
| MFT Read (rec/sec) | ~1M | - | ≥1M | - |
| Path Resolution | - | - | Match C++ | - |
| Peak Memory | - | - | ≤ C++ | - |

---

## 📋 Quick Commands

```bash
# Full CI Pipeline (run after major steps)
rust-script scripts/ci-pipeline.rs go -v

# Local checks (advisory - use between pipeline runs)
just check && just clippy && just test

# Run coverage
cargo llvm-cov --workspace --fail-under 90

# Security audit
cargo audit && cargo deny check
```

---

*Update this tracker as work progresses. Keep the plan document stable.*

