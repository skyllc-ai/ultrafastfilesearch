#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
# SPDX-License-Identifier: MPL-2.0
#
# Wrap the built uffs binary in a macOS `.app` bundle.
#
# Usage (invoked from repo root):
#
#   packaging/macos/bundle.sh [BIN] [APP]
#
#   BIN   path to the built uffs binary   (default: target/release/uffs)
#   APP   output bundle path              (default: dist/UFFS.app)
#
# The Info.plist version fields are templated from the live workspace
# version via `cargo pkgid -p uffs-cli` so the bundle never drifts from
# Cargo.toml.  Run `just dist-macos` for the full build-and-bundle path.

set -euo pipefail

BIN="${1:-target/release/uffs}"
APP="${2:-dist/UFFS.app}"

if [[ ! -x "$BIN" ]]; then
    printf '\033[1;31merror:\033[0m %s is not an executable binary\n' "$BIN" >&2
    printf 'run `cargo build --release -p uffs-cli` first, or pass an explicit path.\n' >&2
    exit 1
fi

# Cargo reports `path+file:///.../uffs-cli#0.5.67` — strip everything
# before the `#` to get the bare version string.
VERSION="$(cargo pkgid -p uffs-cli | sed 's/.*#//')"

printf '\033[0;34m📦 Building UFFS.app (version %s)\033[0m\n' "$VERSION"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN"                              "$APP/Contents/MacOS/uffs"
cp assets/brand/icons/uffs.icns        "$APP/Contents/Resources/uffs.icns"

# Template the version placeholder in Info.plist.in into the final plist.
sed "s/@@VERSION@@/${VERSION}/g" \
    packaging/macos/Info.plist.in \
    > "$APP/Contents/Info.plist"

chmod +x "$APP/Contents/MacOS/uffs"

printf '\033[0;32m✅ Bundle written to %s\033[0m\n' "$APP"
printf '   Launch with: open %s\n' "$APP"
printf '   (unsigned — Gatekeeper will warn; run `xattr -dr com.apple.quarantine %s` to bypass locally)\n' "$APP"
