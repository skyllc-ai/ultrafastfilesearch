#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
# SPDX-License-Identifier: MPL-2.0
#
# Install UFFS on Linux with full freedesktop.org desktop integration.
# Run as root (sudo).
#
# Usage (invoked from repo root):
#
#   sudo packaging/linux/install.sh [BIN]
#   sudo PREFIX=/opt/uffs packaging/linux/install.sh   # custom prefix
#
#   BIN     path to the built uffs binary   (default: target/release/uffs)
#   PREFIX  install prefix                   (default: /usr/local)

set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BIN="${1:-target/release/uffs}"

if [[ ! -x "$BIN" ]]; then
    printf '\033[1;31merror:\033[0m %s is not an executable binary\n' "$BIN" >&2
    printf 'run `cargo build --release -p uffs-cli` first, or pass an explicit path.\n' >&2
    exit 1
fi

printf '\033[0;34m📦 Installing UFFS to %s\033[0m\n' "$PREFIX"

# Portable install helper: GNU install's `-D` flag (create parent dirs)
# is a GNU extension and is NOT supported by the BSD `install` on macOS.
# `mkdir -p` + `install -m MODE` works identically on both, which lets
# this script smoke-test from a macOS dev box as well as run for real
# on Linux targets.
_install() {
    local mode="$1" src="$2" dst="$3"
    mkdir -p "$(dirname "$dst")"
    install -m "$mode" "$src" "$dst"
}

_install 755 "$BIN"                        "$PREFIX/bin/uffs"
_install 644 packaging/linux/uffs.desktop  "$PREFIX/share/applications/uffs.desktop"

# Hicolor icon set — every size the assets/brand/ tree provides.
#
# The tree lives in two places depending on where install.sh is run from:
#   • repo root      → assets/brand/icons/hicolor/…
#   • extracted ZIP  → assets/hicolor/…   (release.yml flattens the path
#                                          so the staged bundle is shallower)
# Auto-detect so the same script works in both contexts.
if   [[ -d "assets/brand/icons/hicolor" ]]; then
    ICON_ROOT="assets/brand/icons/hicolor"
elif [[ -d "assets/hicolor" ]]; then
    ICON_ROOT="assets/hicolor"
else
    printf '\033[1;31merror:\033[0m hicolor icon tree not found under assets/\n' >&2
    exit 1
fi

for size in 16 32 48 64 128 256 512; do
    _install 644 "$ICON_ROOT/${size}x${size}/uffs.png" \
                 "$PREFIX/share/icons/hicolor/${size}x${size}/apps/uffs.png"
done

# Refresh icon + desktop caches.  Both commands are optional — they
# don't exist on every distro and are no-ops when run against a prefix
# the host doesn't index.  Failures are benign: the next login or
# `gtk-update-icon-cache`/`update-desktop-database` invocation picks
# everything up.
gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
update-desktop-database        "$PREFIX/share/applications" 2>/dev/null || true

printf '\033[0;32m✅ UFFS installed under %s\033[0m\n' "$PREFIX"
printf '   Binary:   %s/bin/uffs\n'                   "$PREFIX"
printf '   Desktop:  %s/share/applications/uffs.desktop\n' "$PREFIX"
printf '   Icons:    %s/share/icons/hicolor/*/apps/uffs.png\n' "$PREFIX"
