# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

# UFFS packaging recipes: platform-native bundles + installers.
#
# These recipes wrap the thin shell scripts under `packaging/` with the
# build-release step and the status output the rest of the justfile uses.
# The heavy lifting (plist templating, freedesktop install layout) lives
# in packaging/*/ so the scripts also work outside `just`.

# Build a macOS .app bundle wrapping the release uffs binary.
#
# Output: dist/UFFS.app (dist/ is gitignored — ephemeral build output).
dist-macos:
    @printf "\033[0;34m📦 Building macOS .app bundle...\033[0m\n"
    cargo build --release -p uffs-cli
    ./packaging/macos/bundle.sh target/release/uffs dist/UFFS.app
    @printf "\033[0;32m✅ Bundle ready: dist/UFFS.app\033[0m\n"

# Install UFFS on Linux with desktop integration (requires sudo).
#
# Installs to /usr/local by default; override with `PREFIX=... just install-linux`.
install-linux:
    @printf "\033[0;34m📦 Installing UFFS + desktop integration (sudo required)...\033[0m\n"
    cargo build --release -p uffs-cli
    sudo ./packaging/linux/install.sh target/release/uffs
    @printf "\033[0;32m✅ Install complete\033[0m\n"
