# Security audit.
audit:
    @printf "\033[0;34m🔒 Security audit...\033[0m\n"
    @if command -v cargo-audit >/dev/null 2>&1; then cargo audit; else printf "\033[1;33mInstalling cargo-audit...\033[0m\n"; just _install-if-missing cargo-audit cargo-audit; cargo audit; fi

# Comprehensive security audit with multiple tools.
audit-comprehensive:
    #!/usr/bin/env bash
    printf "\033[0;34m🔒 Comprehensive security audit...\033[0m\n"
    printf "\033[0;34m  → Running cargo-audit (vulnerability scan)...\033[0m\n"
    if command -v cargo-audit >/dev/null 2>&1; then cargo audit; else printf "\033[1;33m⚠️  cargo-audit not found, run 'just setup' first\033[0m\n"; fi
    printf "\033[0;34m  → Running cargo-deny (dependency analysis)...\033[0m\n"
    if command -v cargo-deny >/dev/null 2>&1; then cargo deny check; else printf "\033[1;33m⚠️  cargo-deny not found, run 'just setup' first\033[0m\n"; fi
    printf "\033[0;34m  → Running cargo-geiger (unsafe code detection)...\033[0m\n"
    if command -v cargo-geiger >/dev/null 2>&1; then cargo geiger; else printf "\033[1;33m⚠️  cargo-geiger not found, run 'just setup' first\033[0m\n"; fi

# Dependency optimization and cleanup.
deps-optimize:
    #!/usr/bin/env bash
    printf "\033[0;34m🔧 Optimizing dependencies...\033[0m\n"
    printf "\033[0;34m  → Finding unused dependencies...\033[0m\n"
    if command -v cargo-udeps >/dev/null 2>&1; then cargo +nightly udeps; else printf "\033[1;33m⚠️  cargo-udeps not found, run 'just setup' first\033[0m\n"; fi
    printf "\033[0;34m  → Checking for outdated dependencies...\033[0m\n"
    if command -v cargo-outdated >/dev/null 2>&1; then cargo outdated; else printf "\033[1;33m⚠️  cargo-outdated not found, run 'just setup' first\033[0m\n"; fi

# Advanced debugging and analysis.
debug-deep:
    #!/usr/bin/env bash
    printf "\033[0;34m🔬 Deep debugging and analysis...\033[0m\n"
    printf "\033[0;34m  → Expanding macros...\033[0m\n"
    if command -v cargo-expand >/dev/null 2>&1; then cargo expand; else printf "\033[1;33m⚠️  cargo-expand not found, run 'just setup' first\033[0m\n"; fi
    printf "\033[0;34m  → Running Miri (undefined behavior detection)...\033[0m\n"
    if rustup component list --installed | grep -q "miri"; then cargo +nightly miri test; else printf "\033[1;33m⚠️  miri component not found, run 'just setup' first\033[0m\n"; fi

# Generate flamegraph for performance analysis.
flamegraph target="uffs-cli" pattern="*.txt":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔥 Generating Flamegraph for {{target}}...\033[0m\n"
    if ! command -v cargo-flamegraph >/dev/null 2>&1; then printf "\033[1;31m❌ cargo-flamegraph not found\033[0m\n"; printf "\033[0;34m   Install with: cargo install flamegraph\033[0m\n"; exit 1; fi
    printf "\033[0;34m  → Building release binary...\033[0m\n"
    cargo build --release -p {{target}} --quiet
    printf "\033[0;34m  → Generating flamegraph (this may require elevated permissions)...\033[0m\n"
    FLAMEGRAPH_OUTPUT="target/flamegraph-$(date +%Y%m%d-%H%M%S).svg"
    cargo flamegraph --bin uffs -o "$FLAMEGRAPH_OUTPUT" -- "{{pattern}}" 2>/dev/null || { printf "\033[1;33m⚠️  Flamegraph requires elevated permissions\033[0m\n"; printf "\033[0;34m   On macOS: sudo just flamegraph\033[0m\n"; printf "\033[0;34m   On Linux: ensure perf is available\033[0m\n"; exit 1; }
    printf "\033[0;32m✅ Flamegraph generated: $FLAMEGRAPH_OUTPUT\033[0m\n"
    printf "\033[0;34m   Open in browser to analyze CPU hotspots\033[0m\n"

