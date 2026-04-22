<p align="center">
  <img src="../../assets/brand/uffs-wordmark.png" alt="UFFS — Ultra Fast File Search" width="560">
</p>

# UFFS User Manual

UFFS (Ultra Fast File Search) is a high-performance file search engine
that reads NTFS Master File Tables directly and queries them in memory.
It indexes millions of files in seconds and searches them in milliseconds.

> **Version:** This manual describes UFFS v0.4.x.

---

## What UFFS Does

- **Instant file search** across every NTFS drive — no Windows Search,
  no crawling, no waiting.
- **40+ filters** — size, date, extension, type, attributes, path
  length, tree size, bulkiness, and more.
- **Background daemon** — loads the MFT once, then answers every query
  in ~200 ms.  Starts automatically on first search.
- **Cross-platform** — native NTFS access on Windows; offline MFT
  analysis on macOS and Linux.
- **AI-agent integration** — MCP server lets Claude, Cursor, Windsurf,
  and other agents search your filesystem directly.

---

## Reading Order

Start here and follow the arrows.  Each page builds on the previous one.

```
 Getting Started
 ┌─────────────────────────────────────────────────────┐
 │  installation.md → getting-started.md               │
 └──────────────────────────┬──────────────────────────┘
                            ▼
 Core Usage
 ┌─────────────────────────────────────────────────────┐
 │  cli-overview.md                                    │
 │    ├── search-modes.md   (glob, regex, literal)     │
 │    ├── filters.md        (40+ filters)              │
 │    ├── sorting.md        (36+ columns)              │
 │    └── output-formats.md (CSV, JSON, table)         │
 └──────────────────────────┬──────────────────────────┘
                            ▼
 Infrastructure
 ┌─────────────────────────────────────────────────────┐
 │  daemon.md → aggregation.md → cache-and-data.md     │
 └──────────────────────────┬──────────────────────────┘
                            ▼
 Deep Knowledge
 ┌─────────────────────────────────────────────────────┐
 │  concepts.md → advanced-diagnostics.md              │
 │    └── performance.md  (benchmarks & profiling)     │
 └──────────────────────────┬──────────────────────────┘
                            ▼
 Reference
 ┌─────────────────────────────────────────────────────┐
 │  troubleshooting.md · faq.md · glossary.md          │
 └─────────────────────────────────────────────────────┘
```

---

## Pages

### Getting Started

| Page | What you learn |
|------|---------------|
| [Installation](installation.md) | Build from source, platform requirements, PATH setup |
| [Getting Started](getting-started.md) | First search, understanding output, 5-minute tutorial |

### Core Usage

| Page | What you learn |
|------|---------------|
| [CLI Overview](cli-overview.md) | Pattern syntax, flags at a glance, subcommands |
| [Search Modes](search-modes.md) | Glob, literal, regex, path-aware patterns |
| [Filters](filters.md) | Size, date, extension, type, attribute, path, tree filters |
| [Sorting](sorting.md) | All 36+ sort columns, multi-tier, deterministic ordering |
| [Output Formats](output-formats.md) | CSV, JSON, table output; column selection; scripting |

### Infrastructure

| Page | What you learn |
|------|---------------|
| [Daemon](daemon.md) | Auto-start, lifecycle, management, platform differences |
| [Aggregation](aggregation.md) | Server-side analytics: presets, custom specs, pagination |
| [Cache & Data Sources](cache-and-data.md) | MFT captures, .uffs cache, --data-dir, platform paths |

### Deep Knowledge

| Page | What you learn |
|------|---------------|
| [Concepts](concepts.md) | Size vs SizeOnDisk, treesize, bulkiness, timestamps |
| [Advanced Diagnostics](advanced-diagnostics.md) | --profile, --benchmark, logging, env vars |
| [Performance](performance.md) | Benchmarks, per-drive profiling, validation throughput |

### Reference

| Page | What you learn |
|------|---------------|
| [Troubleshooting](troubleshooting.md) | Common errors and solutions |
| [FAQ](faq.md) | Quick answers to frequent questions |
| [Glossary](glossary.md) | MFT, FRS, cluster, treesize, and other terms |

### Integrations (separate guides)

| Page | What you learn |
|------|---------------|
| [MCP Server](mcp.md) | AI agent integration via Model Context Protocol |
| [TUI Search](tui-search-box.md) | Terminal UI keybindings, patterns, focus system |

---

## Quick Links — "I Want To…"

| Goal | Where to look |
|------|--------------|
| Find a file by name | [Getting Started](getting-started.md) §1 |
| Search with wildcards or regex | [Search Modes](search-modes.md) |
| Find large / old / hidden files | [Filters](filters.md) |
| Clean up disk space | [Getting Started](getting-started.md) §5 — Triage recipes |
| Get a filesystem overview | [Aggregation](aggregation.md) §1 — `uffs agg overview` |
| Understand why sizes differ from Explorer | [Concepts](concepts.md) §1 |
| See real-world performance numbers | [Performance](performance.md) |
| Set up UFFS for AI agents | [MCP Server](mcp.md) |
| Fix "daemon won't start" | [Troubleshooting](troubleshooting.md) |
