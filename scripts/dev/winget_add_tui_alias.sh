#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
# SPDX-License-Identifier: MPL-2.0
#
# Idempotently insert the `uffs-tui` PortableCommandAlias into a WinGet
# `SkyLLC.UFFS.installer.yaml` manifest.
#
# WHY THIS EXISTS
# ---------------
# `winget-publish.yml` auto-submits the SkyLLC.UFFS manifest via
# winget-releaser (komac).  komac PRESERVES the previous version's
# NestedInstallerFiles/PortableCommandAlias list across updates
# (komac v2.14.0: "Preserve nested installer metadata"), but it does NOT
# auto-add new executables it finds in the zip.  So the `uffs-tui` alias
# must be seeded into the manifest exactly ONCE — into the auto-generated
# PR for the first release whose `uffs-windows-x64.zip` actually contains
# `uffs-windows-x64/uffs-tui.exe`.  After that, every auto-release carries
# all five aliases forward and this script is no longer needed.
#
# USAGE
# -----
#   scripts/dev/winget_add_tui_alias.sh <path/to/SkyLLC.UFFS.installer.yaml>
#
# Typical flow (one-time):
#   1. After the first TUI-bundled release, winget-releaser opens a PR to
#      microsoft/winget-pkgs from the githubrobbi/winget-pkgs fork.
#   2. Check out that PR branch on the fork.
#   3. Run this script against the new version's installer manifest, e.g.:
#        ./scripts/dev/winget_add_tui_alias.sh \
#          manifests/s/SkyLLC/UFFS/0.5.108/SkyLLC.UFFS.installer.yaml
#   4. Commit + push to the PR branch; let validation pass; merge.
#   5. Verify:  winget install --id SkyLLC.UFFS  &&  uffs-tui --help
#
# The script is safe to re-run: if the alias is already present it exits 0
# without modifying the file.
set -euo pipefail

MANIFEST="${1:-}"
if [[ -z "$MANIFEST" ]]; then
  echo "Usage: $0 <path/to/SkyLLC.UFFS.installer.yaml>" >&2
  exit 2
fi
if [[ ! -f "$MANIFEST" ]]; then
  echo "❌ Manifest not found: $MANIFEST" >&2
  exit 1
fi

# Already seeded? Nothing to do.
if grep -q 'uffs-tui\.exe' "$MANIFEST"; then
  echo "✅ uffs-tui alias already present in $MANIFEST — nothing to do."
  exit 0
fi

# Sanity: this must be the SkyLLC.UFFS portable-in-zip installer manifest.
if ! grep -q '^NestedInstallerFiles:' "$MANIFEST"; then
  echo "❌ $MANIFEST has no 'NestedInstallerFiles:' section — wrong file?" >&2
  exit 1
fi
if ! grep -q 'PackageIdentifier: SkyLLC.UFFS' "$MANIFEST"; then
  echo "❌ $MANIFEST is not the SkyLLC.UFFS manifest." >&2
  exit 1
fi

# winget-pkgs manifests use CRLF line endings; mixing LF into them trips
# the pipeline's "Validation-Line-Endings-Error".  Detect the manifest's
# convention and emit the two inserted lines with a matching terminator so
# the file stays uniform.  (awk preserves the trailing CR on existing
# lines because the default record separator is LF.)
cr=''
if grep -q $'\r$' "$MANIFEST"; then
  cr=$'\r'
fi

# Insert the two-line nested-file block immediately after the
# `NestedInstallerFiles:` key so it joins the existing alias list.
tmp="$(mktemp)"
awk -v cr="$cr" '
  /^NestedInstallerFiles:[[:space:]]*$/ {
    print
    printf "%s%s\n", "- RelativeFilePath: uffs-windows-x64/uffs-tui.exe", cr
    printf "%s%s\n", "  PortableCommandAlias: uffs-tui", cr
    next
  }
  { print }
' "$MANIFEST" > "$tmp"
mv "$tmp" "$MANIFEST"

echo "✅ Added uffs-tui PortableCommandAlias to $MANIFEST"
echo "   Review the diff, commit, and push to the winget-pkgs PR branch."
