# Install and configure sccache for distributed compilation caching.
setup-sccache:
    #!/usr/bin/env bash
    printf "\033[0;34m🚀 Setting up sccache for faster compilation...\033[0m\n"
    if ! command -v sccache >/dev/null 2>&1; then printf "\033[0;34m  → Installing sccache...\033[0m\n"; just _install-if-missing sccache sccache; else printf "\033[0;32m  ✅ sccache already installed\033[0m\n"; fi
    SCCACHE_DIR="$HOME/.cache/sccache"
    mkdir -p "$SCCACHE_DIR"
    printf "\033[0;34m  → Cache directory: $SCCACHE_DIR\033[0m\n"
    printf "\033[0;34m  → Setting RUSTC_WRAPPER=sccache\033[0m\n"
    if command -v sccache >/dev/null 2>&1; then printf "\033[0;34m  → Current cache statistics:\033[0m\n"; RUSTC_WRAPPER=sccache sccache --show-stats || true; fi
    printf "\033[0;32m✅ sccache setup complete!\033[0m\n"
    printf "\033[0;34m💡 To enable sccache, set: export RUSTC_WRAPPER=sccache\033[0m\n"
    printf "\033[0;34m💡 Add to your shell profile for permanent activation\033[0m\n"

# Show sccache statistics and performance impact.
sccache-stats:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 sccache compilation cache statistics...\033[0m\n"
    if ! command -v sccache >/dev/null 2>&1; then printf "\033[1;33m⚠️  sccache not installed. Run 'just setup-sccache' first.\033[0m\n"; exit 1; fi
    printf "\033[0;34m  → Cache statistics:\033[0m\n"
    RUSTC_WRAPPER=sccache sccache --show-stats
    printf "\033[0;34m  → Cache location:\033[0m\n"
    printf "    $HOME/.cache/sccache\n"
    if [ -d "$HOME/.cache/sccache" ]; then CACHE_SIZE=$(du -sh "$HOME/.cache/sccache" 2>/dev/null | cut -f1 || echo "unknown"); printf "\033[0;34m  → Cache size: $CACHE_SIZE\033[0m\n"; fi
    printf "\033[0;32m✅ sccache statistics complete!\033[0m\n"

# Clear sccache to free up disk space.
sccache-clear:
    #!/usr/bin/env bash
    printf "\033[0;34m🧹 Clearing sccache compilation cache...\033[0m\n"
    if ! command -v sccache >/dev/null 2>&1; then printf "\033[1;33m⚠️  sccache not installed. Nothing to clear.\033[0m\n"; exit 0; fi
    if [ -d "$HOME/.cache/sccache" ]; then CACHE_SIZE=$(du -sh "$HOME/.cache/sccache" 2>/dev/null | cut -f1 || echo "unknown"); printf "\033[0;34m  → Current cache size: $CACHE_SIZE\033[0m\n"; fi
    RUSTC_WRAPPER=sccache sccache --zero-stats
    rm -rf "$HOME/.cache/sccache"
    mkdir -p "$HOME/.cache/sccache"
    printf "\033[0;32m✅ sccache cache cleared!\033[0m\n"

