<!--
SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
SPDX-License-Identifier: MPL-2.0
-->
# WinGet manifest — `uffs-tui` alias contract

The `SkyLLC.UFFS` WinGet package is **auto-submitted** to `microsoft/winget-pkgs`
by [`.github/workflows/winget-publish.yml`](../../.github/workflows/winget-publish.yml)
on every release, via `winget-releaser` (komac under the hood). The package is a
**zip → portable** installer that exposes its bundled binaries as typed commands
through `NestedInstallerFiles` / `PortableCommandAlias`.

The `normal`-tier zip (`uffs-windows-x64.zip`) now also bundles the free
**`uffs-tui` demo**, and we want `winget install SkyLLC.UFFS` to expose it as a
typed `uffs-tui` command alongside `uffs`, `uffsd`, `uffsmcp`, and `uffs-mft`.

## Why this needs a one-time seed

komac **preserves** the previous version's `NestedInstallerFiles` list when it
bumps the package to a new version (komac v2.14.0 — *"Preserve nested installer
metadata during version updates to preserve existing PortableCommandAlias"*).

It does **not** scan the zip and auto-add newly-bundled executables. So the
`uffs-tui` alias will never appear on its own — it must be added to the manifest
**once**. After that, komac carries all five aliases forward on every
auto-submitted release, and nothing further is required.

## Hard precondition

Only add the alias to a release whose `uffs-windows-x64.zip` **actually contains**
`uffs-windows-x64/uffs-tui.exe` — i.e. the **first release built after** the
min/normal/full tiering landed in `release.yml`. Adding it to an earlier version
(whose zip has no `uffs-tui.exe`) makes the winget validation pipeline fail the
install test.

Confirm before seeding:

```bash
TAG=v0.5.XXX   # the first TUI-bundled release
curl -fsSL -o /tmp/uffs.zip \
  "https://github.com/skyllc-ai/UltraFastFileSearch/releases/download/${TAG}/uffs-windows-x64.zip"
unzip -l /tmp/uffs.zip | grep -F 'uffs-windows-x64/uffs-tui.exe'   # must print a line
```

## One-time procedure

1. After the first TUI-bundled release, `winget-publish.yml` dispatches
   `winget-releaser`, which opens a PR to `microsoft/winget-pkgs` from the
   `githubrobbi/winget-pkgs` fork bumping `SkyLLC.UFFS` to the new version (with
   the existing four aliases).
2. Check out that PR branch on the fork and run the idempotent patcher against
   the new version's installer manifest:

   ```bash
   ./scripts/dev/winget_add_tui_alias.sh \
     manifests/s/SkyLLC/UFFS/0.5.XXX/SkyLLC.UFFS.installer.yaml
   ```

   (Or hand-add the block from [`uffs-tui-nested-file.yaml`](uffs-tui-nested-file.yaml).)
3. Commit + push to the PR branch, let the winget validation bot pass, and merge.
4. Verify once the package indexes:

   ```powershell
   winget install --id SkyLLC.UFFS
   uffs-tui --help
   ```

From then on, every release auto-keeps all five aliases — no manual step.

## Pending seed: `uffs-broker`

The 2026-06-12 fresh-VM dry run found that the CLI's elevation help advertises
`uffs-broker --install` ("one-time setup, no future UAC prompts") but the
binary was missing from the winget zip. `release.yml` now bundles
`uffs-broker.exe` into the Windows normal/full tiers — so the **first release
after that change** needs the same one-time seed as `uffs-tui` above, with a
`uffs-windows-x64/uffs-broker.exe` → `uffs-broker` entry. Same hard
precondition: verify the zip actually contains the exe before seeding.

## Future: `uffs-gui`

When the GUI demo is bundled into the zip, repeat the same one-time seed with a
`uffs-windows-x64/uffs-gui.exe` → `uffs-gui` entry.
