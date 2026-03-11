<!--
SPDX-License-Identifier: MPL-2.0
Copyright (c) 2025-2026 Robert Nio

UFFS - Ultra Fast File Search
-->

# 🦀 UFFS Modernization Implementation Guides

> **For**: Developers implementing the modernization plan
> **Reference**: [`uffs-modernization-plan-2026.md`](../uffs-modernization-plan-2026.md)
> **Tracker**: [`MODERNIZATION_TRACKER.md`](../MODERNIZATION_TRACKER.md)

---

## 📚 Guide Index

| Wave | Guide | Effort | Priority |
|------|-------|--------|----------|
| 1 | [Immediate Wins](wave-1-immediate-wins.md) | 1-2 days | 🔴 Critical |
| 2 | [Architecture Completion](wave-2-architecture-completion.md) | 3-5 days | 🔴 Critical |
| 2.5 | [Module Restructuring](wave-2.5-module-restructuring.md) | 3-5 days | 🔴 Critical |
| 3 | [Testing Excellence](wave-3-testing-excellence.md) | 2-3 days | 🟠 Major |
| 4 | [Documentation & API](wave-4-documentation-api.md) | 2-3 days | 🟠 Major |
| 5 | [Performance & Observability](wave-5-performance-observability.md) | 2-3 days | 🟡 Moderate |
| 6 | [Advanced Tooling](wave-6-advanced-tooling.md) | 1-2 days | 🟢 Enhancement |

**Total Estimated Effort**: 15-23 days

---

## 🚀 Quick Start

### Before Starting Any Wave

1. **Create healing changelog**:
   ```bash
   touch LOG/$(date +%Y_%m_%d_%H_%M)_CHANGELOG_HEALING.md
   ```

2. **Verify clean state**:
   ```bash
   just check && just clippy && just test
   ```

3. **Read the Rules of Engagement** in the [main plan](../uffs-modernization-plan-2026.md#-implementation-rules-of-engagement)

### ⚡ MANDATORY Wave Completion Flow

> **This flow is NON-NEGOTIABLE after completing each wave.**

```
┌─────────────────────────────────────────────────────────────┐
│                  WAVE COMPLETION FLOW                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  1. ✅ Complete wave tasks                                   │
│           │                                                  │
│           ▼                                                  │
│  2. 📝 Update MODERNIZATION_TRACKER.md                       │
│     • Mark tasks complete                                    │
│     • Update wave status                                     │
│     • Add completion date                                    │
│           │                                                  │
│           ▼                                                  │
│  3. 🚀 Run FULL CI Pipeline                                  │
│     rust-script scripts/ci/ci-pipeline.rs go -v                 │
│           │                                                  │
│           ▼                                                  │
│  4. ➡️  Continue to next wave (DO NOT ASK - JUST PROCEED)    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Key Rule**: After CI passes, immediately proceed to the next wave. No confirmation needed.

---

## ⚠️ Rules of Engagement (Summary)

| Rule | Requirement |
|------|-------------|
| **No Suppression Hacks** | No blanket `#[allow(...)]`, no disabling lints |
| **Surgical Fixes** | Minimal, idiomatic Rust; fix root causes |
| **Preserve Contracts** | Maintain public API; update docs if behavior changes |
| **Improve Tests** | Strengthen tests; never skip to pass CI |
| **Document Well** | Atomic commits; healing changelog before CI |

---

## 📋 What Each Guide Contains

Every guide includes:

- ✅ **Task Checklist** - Track progress within the wave
- 📝 **Step-by-Step Instructions** - Exact commands and file edits
- 💻 **Code Examples** - Copy-paste ready snippets
- ✔️ **Verification Steps** - How to confirm each task is done
- 🚨 **Troubleshooting** - Common issues and solutions
- 🔗 **Navigation** - Links to previous/next waves

---

## 🔧 Prerequisites

Before starting, ensure you have:

```bash
# Rust toolchain
rustc --version  # Should be 1.85+ or nightly

# Required tools
cargo --version
just --version

# Recommended tools (install as needed per wave)
cargo install cargo-nextest
cargo install cargo-llvm-cov
cargo install cargo-audit
cargo install cargo-deny
```

---

## 📊 Progress Tracking

Use the [MODERNIZATION_TRACKER.md](../MODERNIZATION_TRACKER.md) to:

- Track overall wave progress
- Mark individual tasks complete
- Log blockers and issues
- Record decisions made
- Document weekly updates

---

## 🆘 Getting Help

If you get stuck:

1. **Check the troubleshooting section** in each guide
2. **Search existing healing changelogs** in `LOG/` for similar issues
3. **Review the main plan** for context and rationale
4. **Ask for help** - document what you tried

---

*Start with [Wave 1: Immediate Wins](wave-1-immediate-wins.md)*

