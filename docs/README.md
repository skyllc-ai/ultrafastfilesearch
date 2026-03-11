# Documentation Map

This directory is being normalized toward a small set of durable buckets:

- [`architecture/`](architecture/README.md) — design notes and canonical subsystem docs
- [`dev/`](dev/README.md) — contributor workflow and implementation planning docs
- [`user/`](user/README.md) — operator guides and testing walkthroughs
- [`performance/`](performance/README.md) — benchmarks, performance analysis, and validation

## CRUCIBLE baseline artifacts

- [`ARCHITECTURE.md`](ARCHITECTURE.md)
- [`PERFORMANCE.md`](PERFORMANCE.md)
- [`RISKS.md`](RISKS.md)
- [`../LOG/CHANGELOG_HEALING_CRUCIBLE.md`](../LOG/CHANGELOG_HEALING_CRUCIBLE.md)

## Notes for the Wave 3A sweep

- Stable architecture documents stay canonical in `docs/architecture/`.
- `docs/architecture/Investigation/` remains a working-notes area.
- Exact duplicate investigation copies have been retired in favor of their stable canonical docs.
- Reference-oriented committed docs are linked from the architecture and dev landing pages.
- Any optional private legacy reference tree should use the repo-root, gitignored `old_cpp_reference/` layout, stay local-only, and must not be pushed.
- `docs/reference/` is also local-only and stays ignored.
- Some older root-level documents still exist while links are being consolidated into the buckets above.

