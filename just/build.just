# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.

# Show current version.
version:
    @printf "\033[0;34m📋 Current version:\033[0m\n"
    @cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "uffs-cli") | .version'

# Increment version.
version-bump:
    @printf "\033[0;34m📈 Incrementing version...\033[0m\n"
    @if [ -f "./build/update_all_versions.rs" ]; then \
        ./build/update_all_versions.rs patch; \
    else \
        printf "\033[1;33mVersion script not found, using manual increment...\033[0m\n"; \
        echo "Manual version increment needed"; \
    fi

# Build dev binary.
build:
    @printf "\033[0;34m🔨 Building dev binary (FAST-FAIL)...\033[0m\n"
    cargo build --workspace

# Build release binary.
build-release:
    @printf "\033[0;34m🔨 Building optimized release binary...\033[0m\n"
    @printf "\033[1;33m⚠️  This will take significantly longer than dev builds\033[0m\n"
    cargo build --release --workspace

# Build and install all binaries locally.
build-local:
    @printf "\033[0;34m🚀 Building and installing all UFFS binaries locally...\033[0m\n"
    @printf "\033[1;33m📦 Platform-specific build with Fat LTO optimization\033[0m\n"
    @printf "\033[1;33m📁 Installing to ~/bin with PATH setup\033[0m\n"
    @echo "========================================================"
    rust-script scripts/dev/build-local.rs

# Quick alias for local build and install.
install: build-local

# Deploy binary to ~/bin.
deploy:
    @printf "\033[0;34m📦 Deploying dev binary...\033[0m\n"
    just copy-binary debug

# Deploy release binary to ~/bin.
deploy-release:
    @printf "\033[0;34m📦 Deploying release binary...\033[0m\n"
    just copy-binary release

# Cross-platform binary deployment with intelligent path handling.
copy-binary profile:
    #!/usr/bin/env bash
    printf "\033[0;34m📦 Cross-platform binary deployment ({{ profile }})...\033[0m\n"
    if [[ "$OSTYPE" == "msys"* ]] || [[ "$OSTYPE" == "cygwin"* ]] || [[ -n "$WINDIR" ]]; then EXT=".exe"; printf "\033[0;34m  → Windows detected\033[0m\n"; else EXT=""; printf "\033[0;34m  → Unix-like system detected\033[0m\n"; fi
    SOURCE_DIR="target/{{ profile }}"
    mkdir -p ~/bin
    for BIN in uffs uffsd uffsmcp; do
        SOURCE_PATH="$SOURCE_DIR/${BIN}${EXT}"
        DEST_PATH="$HOME/bin/${BIN}${EXT}"
        if [[ ! -f "$SOURCE_PATH" ]]; then
            printf "\033[1;33m⚠️  %s not found: %s (skipping)\033[0m\n" "$BIN" "$SOURCE_PATH"
            continue
        fi
        printf "\033[0;34m  → %s → %s\033[0m\n" "$SOURCE_PATH" "$DEST_PATH"
        # rm -f before cp so the new file gets a fresh inode — overwrites in
        # place share the inode, which lets macOS re-use a path-cached launch
        # services deny verdict against an earlier broken copy at the same path.
        rm -f "$DEST_PATH"
        if cp "$SOURCE_PATH" "$DEST_PATH" && chmod +x "$DEST_PATH"; then
            printf "\033[0;32m✅ %s deployed to ~/bin/\033[0m\n" "${BIN}${EXT}"
        else
            printf "\033[0;31m❌ Failed to copy %s\033[0m\n" "$BIN"
            exit 1
        fi
    done
    if ! echo "$PATH" | grep -q "$HOME/bin"; then printf "\033[1;33m⚠️  ~/bin not in PATH. Add this to your shell profile:\033[0m\n"; printf "   export PATH=\"\$HOME/bin:\$PATH\"\n"; fi

# Build the entire workspace (release) locally and copy ALL binary artifacts
# to ~/bin.  One `cargo build --release --workspace` does the heavy lifting;
# then every binary in target/release/ is installed.
#
# Use this when you want to test in-development code.  Prefer `just use`
# when you want the reproducible, GitHub-built release binaries — those
# bytes are what your end users download.
#
# Build workspace from source and install all binaries to ~/bin (dev).
[unix]
use-local:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[1;34m📦 Build + install ALL UFFS binaries to ~/bin\033[0m\n"
    echo "========================================================"

    # 1. Single workspace-wide release build
    printf "\n\033[0;34m🔨 Building release workspace...\033[0m\n"
    START=$(date +%s)
    cargo build --release --workspace
    END=$(date +%s)
    printf "\033[0;32m  → Built in %ds\033[0m\n" $((END - START))

    # 2. Copy every binary artifact to ~/bin
    BIN_DIR="$HOME/bin"
    mkdir -p "$BIN_DIR"
    INSTALLED=0
    printf "\n\033[0;34m📦 Installing to %s\033[0m\n" "$BIN_DIR"
    for SRC in target/release/*; do
        [[ -f "$SRC" && -x "$SRC" ]] || continue
        NAME=$(basename "$SRC")
        # Skip non-binaries (shared libs, build scripts, deps fingerprints, etc.)
        [[ "$NAME" == *.d ]] && continue
        [[ "$NAME" == *.dylib ]] && continue
        [[ "$NAME" == *.so ]] && continue
        [[ "$NAME" == *.rlib ]] && continue
        # Skip if it contains a dot (e.g. libfoo.a) — real binaries have no extension on Unix
        [[ "$NAME" == *.* ]] && continue
        DEST="$BIN_DIR/$NAME"
        # If a directory exists with the same name, remove it first
        if [[ -d "$DEST" ]]; then
            printf "  \033[1;33m⚠️  Removing stale directory %s\033[0m\n" "$DEST"
            rm -rf "$DEST"
        fi
        # rm -f before cp so the new file gets a fresh inode — overwrites in
        # place share the inode, which lets macOS re-use a path-cached launch
        # services deny verdict against an earlier broken copy at the same path.
        rm -f "$DEST"
        cp "$SRC" "$DEST"
        chmod +x "$DEST"
        SIZE=$(du -sh "$BIN_DIR/$NAME" | cut -f1)
        printf "  \033[0;32m✅ %-30s %s\033[0m\n" "$NAME" "$SIZE"
        INSTALLED=$((INSTALLED + 1))
    done

    printf "\n\033[0;32m✅ Installed %d binaries to ~/bin\033[0m\n" "$INSTALLED"

    # Check PATH
    if ! echo "$PATH" | grep -q "$HOME/bin"; then
        printf "\n\033[1;33m⚠️  ~/bin not in PATH. Add to your shell profile:\033[0m\n"
        printf "   export PATH=\"\$HOME/bin:\$PATH\"\n"
    fi

# Install UFFS binaries from GitHub Release (cached in dist/) to ~/bin.
#
# This is the default `just use` on both Unix and Windows — the binaries
# are produced by GitHub Actions' release workflow, so the bytes in ~/bin
# are bit-identical to what your end users download.  For local dev builds
# from source, use `just use-local` instead.
#
# Resilience strategy (each tier falls through to the next on miss):
#   A. Exact cache hit on `dist/vX.Y.Z` matching workspace `Cargo.toml`.
#   B. `gh release download vX.Y.Z` for the workspace version.
#   C. `gh release download <latest-published-tag>` — graceful older-version
#      install when the workspace version's release PR is still in CI
#      (the exact failure mode that motivated this recipe: `just ship`
#      bumped Cargo.toml to v0.5.90, but PR #131's release workflow
#      hadn't published the binaries yet, so step B 404'd).
#   D. Latest populated `dist/v*` cache (offline / `gh` missing / no
#      published releases at all).
#
# Each fallback is loud about *why* it fired, so a stale install never
# fails silently.  The exit-1 path is reserved for "no usable binaries
# anywhere" and prints a triage hint pointing at the release PR.
[unix]
use:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📦 Installing UFFS binaries from dist/ cache...\033[0m\n"

    # Read workspace version from Cargo.toml
    CARGO_VER=$(awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print "v"$3; exit}' Cargo.toml | tr -d '"')

    DIST_DIR=""
    STALE_NOTE=""

    # ── A. Exact cache hit on workspace version ──────────────────────
    # Empty `dist/$CARGO_VER` is treated as a miss: `just ship` (and
    # prior failed `just use` runs) can leave the directory behind
    # without any binaries, and accepting it here would skip the
    # GH-release download steps below.
    if [[ -n "$CARGO_VER" ]] \
        && [[ -d "dist/$CARGO_VER" ]] \
        && [[ -n "$(ls -A "dist/$CARGO_VER" 2>/dev/null)" ]]; then
        DIST_DIR="dist/$CARGO_VER"
        printf "\033[0;32m  ✅ dist/%s — exact cache hit\033[0m\n" "$CARGO_VER"
    fi

    # ── B. Download workspace version from GitHub Release ────────────
    if [[ -z "$DIST_DIR" ]] && [[ -n "$CARGO_VER" ]] && command -v gh &>/dev/null; then
        printf "\033[0;34m  ⬇  gh release download %s …\033[0m\n" "$CARGO_VER"
        TARGET="dist/$CARGO_VER"
        mkdir -p "$TARGET"
        if gh release download "$CARGO_VER" --dir "$TARGET" --clobber >/dev/null 2>&1; then
            DIST_DIR="$TARGET"
            printf "\033[0;32m  ✅ Downloaded %s\033[0m\n" "$CARGO_VER"
        else
            # Clean up the empty cache dir created by `mkdir -p` so a
            # subsequent `just use` doesn't false-positive on tier A.
            rmdir "$TARGET" 2>/dev/null || true
            printf "\033[1;33m  ⚠️  Release %s not published yet (release PR likely mid-merge)\033[0m\n" "$CARGO_VER"
        fi
    fi

    # ── C. Download latest published release (older than workspace) ──
    if [[ -z "$DIST_DIR" ]] && command -v gh &>/dev/null; then
        LATEST_PUB=$(gh release view --json tagName -q .tagName 2>/dev/null || true)
        if [[ -n "$LATEST_PUB" ]] && [[ "$LATEST_PUB" != "$CARGO_VER" ]]; then
            TARGET="dist/$LATEST_PUB"
            if [[ -d "$TARGET" ]] && [[ -n "$(ls -A "$TARGET" 2>/dev/null)" ]]; then
                DIST_DIR="$TARGET"
                printf "\033[1;33m  ⚠️  Falling back to cached %s (workspace is %s)\033[0m\n" "$LATEST_PUB" "$CARGO_VER"
            else
                printf "\033[0;34m  ⬇  gh release download %s (latest published) …\033[0m\n" "$LATEST_PUB"
                mkdir -p "$TARGET"
                if gh release download "$LATEST_PUB" --dir "$TARGET" --clobber >/dev/null 2>&1; then
                    DIST_DIR="$TARGET"
                    printf "\033[1;33m  ⚠️  Downloaded %s as fallback (workspace is %s)\033[0m\n" "$LATEST_PUB" "$CARGO_VER"
                else
                    rmdir "$TARGET" 2>/dev/null || true
                fi
            fi
            STALE_NOTE="(stale: workspace at $CARGO_VER, installed binaries are $LATEST_PUB)"
        fi
    fi

    # ── D. Latest local cache (offline / gh missing / no releases) ───
    if [[ -z "$DIST_DIR" ]] && [[ -d "dist" ]]; then
        LATEST_LOCAL=$(ls -d dist/v* 2>/dev/null | sort -V | tail -1 || true)
        if [[ -n "$LATEST_LOCAL" ]] \
            && [[ -d "$LATEST_LOCAL" ]] \
            && [[ -n "$(ls -A "$LATEST_LOCAL" 2>/dev/null)" ]]; then
            DIST_DIR="$LATEST_LOCAL"
            LATEST_VER=$(basename "$LATEST_LOCAL")
            printf "\033[1;33m  ⚠️  Using stale local cache %s (workspace is %s, GH unreachable)\033[0m\n" "$LATEST_VER" "$CARGO_VER"
            STALE_NOTE="(stale: workspace at $CARGO_VER, installed binaries are $LATEST_VER)"
        fi
    fi

    # ── No tier produced a usable cache ──────────────────────────────
    if [[ -z "$DIST_DIR" ]]; then
        printf "\033[0;31m❌ Could not locate UFFS binaries (workspace: %s)\033[0m\n" "${CARGO_VER:-unknown}"
        printf "\n   Triage (in order of likelihood):\n"
        printf "     1. Release PR for this version is still in CI.  Check:\n"
        printf "        \033[0;36mgh pr list --base main --search \"release/v\"\033[0m\n"
        printf "        \033[0;36mgh pr checks <number> --watch\033[0m\n"
        printf "     2. dist/ is empty AND gh is missing/unauthenticated:\n"
        printf "        \033[0;36mbrew install gh && gh auth login\033[0m\n"
        printf "     3. No GitHub Releases exist yet: \033[0;36mjust ship\033[0m to bootstrap.\n"
        exit 1
    fi

    if [[ -n "$STALE_NOTE" ]]; then
        printf "\033[0;34m  → Source: %s \033[1;33m%s\033[0m\n" "$DIST_DIR" "$STALE_NOTE"
    else
        printf "\033[0;34m  → Source: %s\033[0m\n" "$DIST_DIR"
    fi

    BIN_DIR="$HOME/bin"
    mkdir -p "$BIN_DIR"

    # Map release asset names → install names
    declare -A BINARIES=(
        ["uffs-macos-arm64"]="uffs"
        ["uffsd-macos-arm64"]="uffsd"
        ["uffsmcp-macos-arm64"]="uffsmcp"
        ["uffs_mft-macos-arm64"]="uffs_mft"
    )
    INSTALLED=0
    SKIPPED=0

    printf "\033[0;34m📦 Installing to %s\033[0m\n" "$BIN_DIR"
    for SRC_NAME in "${!BINARIES[@]}"; do
        DEST_NAME="${BINARIES[$SRC_NAME]}"
        SRC="$DIST_DIR/$SRC_NAME"
        if [[ -f "$SRC" ]]; then
            # rm -f before cp so the new file gets a fresh inode — overwrites in
            # place share the inode, which lets macOS re-use a path-cached launch
            # services deny verdict against an earlier broken copy at the same path.
            rm -f "$BIN_DIR/$DEST_NAME"
            cp "$SRC" "$BIN_DIR/$DEST_NAME"
            chmod +x "$BIN_DIR/$DEST_NAME"
            SIZE=$(du -sh "$BIN_DIR/$DEST_NAME" | cut -f1)
            printf "  \033[0;32m✅ %-16s → %-16s %s\033[0m\n" "$SRC_NAME" "$DEST_NAME" "$SIZE"
            INSTALLED=$((INSTALLED + 1))
        else
            printf "  \033[1;33m⚠️  %-16s (not found in %s)\033[0m\n" "$SRC_NAME" "$DIST_DIR"
            SKIPPED=$((SKIPPED + 1))
        fi
    done

    printf "\n\033[0;32m✅ Installed %d binaries to ~/bin\033[0m\n" "$INSTALLED"
    if [[ $SKIPPED -gt 0 ]]; then
        printf "\033[1;33m⚠️  Skipped %d\033[0m\n" "$SKIPPED"
    fi

    # Check PATH
    if ! echo "$PATH" | grep -q "$HOME/bin"; then
        printf "\n\033[1;33m⚠️  ~/bin not in PATH. Add to your shell profile:\033[0m\n"
        printf "   export PATH=\"\$HOME/bin:\$PATH\"\n"
    fi

    # Show versions
    printf "\n\033[0;34m📋 Installed versions:\033[0m\n"
    for DEST_NAME in "${BINARIES[@]}"; do
        if [[ -x "$BIN_DIR/$DEST_NAME" ]]; then
            VER=$("$BIN_DIR/$DEST_NAME" --version 2>/dev/null || echo "unknown")
            printf "  %-16s %s\n" "$DEST_NAME" "$VER"
        fi
    done

# Install UFFS binaries from GitHub Release (cached in dist/) to ~/bin (Windows).
#
# Resilience strategy (mirrors the unix recipe — see its docstring for
# the motivating failure mode and the design rationale).  Each tier
# falls through to the next on miss:
#   A. Exact cache hit on `dist/vX.Y.Z` matching workspace `Cargo.toml`.
#   B. `gh release download vX.Y.Z` for the workspace version.
#   C. `gh release download <latest-published-tag>` (older fallback).
#   D. Latest populated `dist/v*` cache (offline / gh missing).
#
# Prerequisite for B/C: `gh` CLI (winget install GitHub.cli).
[windows]
use:
    #!powershell
    Write-Host "[use] Installing UFFS binaries from dist/ cache..." -ForegroundColor Blue

    # Read workspace version from Cargo.toml
    $cargoVer = $null
    $inPkg = $false
    foreach ($line in Get-Content "Cargo.toml") {
        if ($line -match '^\[workspace\.package\]') { $inPkg = $true; continue }
        if ($inPkg -and $line -match '^version\s*=\s*"([^"]+)"') { $cargoVer = "v$($Matches[1])"; break }
        if ($line -match '^\[' -and $inPkg) { break }
    }

    $distDir = $null
    $staleNote = ""
    $ghAvailable = [bool](Get-Command gh -ErrorAction SilentlyContinue)

    # Helper: does `dir` exist AND contain at least one entry?
    function Test-Populated($path) {
        if (-not $path) { return $false }
        if (-not (Test-Path $path)) { return $false }
        return (@(Get-ChildItem -Path $path -Force -ErrorAction SilentlyContinue)).Count -gt 0
    }

    # ── A. Exact cache hit on workspace version ──────────────────────
    $candidate = if ($cargoVer) { Join-Path "dist" $cargoVer } else { $null }
    if (Test-Populated $candidate) {
        $distDir = $candidate
        Write-Host "  OK: dist/$cargoVer - exact cache hit" -ForegroundColor Green
    }

    # ── B. Download workspace version from GitHub Release ────────────
    if (-not $distDir -and $cargoVer -and $ghAvailable) {
        Write-Host "  -> gh release download $cargoVer ..." -ForegroundColor Blue
        $target = Join-Path "dist" $cargoVer
        New-Item -ItemType Directory -Path $target -Force | Out-Null
        gh release download $cargoVer --dir $target --clobber 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0 -and (Test-Populated $target)) {
            $distDir = $target
            Write-Host "  OK: Downloaded $cargoVer" -ForegroundColor Green
        } else {
            # Drop empty cache dir so tier A doesn't false-positive next run.
            if ((Test-Path $target) -and -not (Test-Populated $target)) {
                Remove-Item $target -Force -ErrorAction SilentlyContinue
            }
            Write-Host "  WARNING: Release $cargoVer not published yet (release PR likely mid-merge)" -ForegroundColor Yellow
        }
    }

    # ── C. Download latest published release (older than workspace) ──
    if (-not $distDir -and $ghAvailable) {
        $latestPub = (gh release view --json tagName -q ".tagName" 2>$null)
        if ($latestPub -and $latestPub -ne $cargoVer) {
            $target = Join-Path "dist" $latestPub
            if (Test-Populated $target) {
                $distDir = $target
                Write-Host "  WARNING: Falling back to cached $latestPub (workspace is $cargoVer)" -ForegroundColor Yellow
            } else {
                Write-Host "  -> gh release download $latestPub (latest published) ..." -ForegroundColor Blue
                New-Item -ItemType Directory -Path $target -Force | Out-Null
                gh release download $latestPub --dir $target --clobber 2>$null | Out-Null
                if ($LASTEXITCODE -eq 0 -and (Test-Populated $target)) {
                    $distDir = $target
                    Write-Host "  WARNING: Downloaded $latestPub as fallback (workspace is $cargoVer)" -ForegroundColor Yellow
                } else {
                    if ((Test-Path $target) -and -not (Test-Populated $target)) {
                        Remove-Item $target -Force -ErrorAction SilentlyContinue
                    }
                }
            }
            if ($distDir) {
                $staleNote = "(stale: workspace at $cargoVer, installed binaries are $latestPub)"
            }
        }
    }

    # ── D. Latest local cache (offline / gh missing / no releases) ───
    if (-not $distDir -and (Test-Path "dist")) {
        $versions = Get-ChildItem -Path "dist" -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^v\d+\.\d+\.\d+$' } |
            Where-Object { Test-Populated $_.FullName } |
            Sort-Object { [version]($_.Name -replace '^v','') } -Descending
        if ($versions) {
            $distDir = $versions[0].FullName
            $distVer = $versions[0].Name
            Write-Host "  WARNING: Using stale local cache $distVer (workspace is $cargoVer, GH unreachable)" -ForegroundColor Yellow
            $staleNote = "(stale: workspace at $cargoVer, installed binaries are $distVer)"
        }
    }

    # ── No tier produced a usable cache ──────────────────────────────
    if (-not $distDir) {
        $vDisplay = if ($cargoVer) { $cargoVer } else { "unknown" }
        Write-Host "FAIL: Could not locate UFFS binaries (workspace: $vDisplay)" -ForegroundColor Red
        Write-Host ""
        Write-Host "  Triage (in order of likelihood):"
        Write-Host "    1. Release PR for this version is still in CI.  Check:"
        Write-Host "       gh pr list --base main --search `"release/v`"" -ForegroundColor Cyan
        Write-Host "       gh pr checks <number> --watch" -ForegroundColor Cyan
        Write-Host "    2. dist/ is empty AND gh is missing/unauthenticated:"
        Write-Host "       winget install GitHub.cli ; gh auth login" -ForegroundColor Cyan
        Write-Host "    3. No GitHub Releases exist yet: just ship to bootstrap." -ForegroundColor Cyan
        exit 1
    }

    if ($staleNote) {
        Write-Host "  Source: $distDir $staleNote" -ForegroundColor Yellow
    } else {
        Write-Host "  Source: $distDir" -ForegroundColor Cyan
    }
    $binDir = Join-Path $env:USERPROFILE 'bin'
    if (-not (Test-Path $binDir)) { New-Item -ItemType Directory -Path $binDir -Force | Out-Null }

    # Kill running UFFS processes before copying to avoid file locks.
    foreach ($proc in @('uffsd', 'uffs', 'uffsmcp', 'uffs_mft')) {
        $running = Get-Process -Name $proc -ErrorAction SilentlyContinue
        if ($running) {
            Write-Host "  Stopping $proc (PID $($running.Id -join ', '))..." -ForegroundColor Yellow
            $running | Stop-Process -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 500
        }
    }

    $binaries = @{
        'uffs-windows-x64.exe' = 'uffs.exe'
        'uffsd-windows-x64.exe' = 'uffsd.exe'
        'uffsmcp-windows-x64.exe' = 'uffsmcp.exe'
        'uffs_mft-windows-x64.exe' = 'uffs_mft.exe'
    }
    $installed = 0; $skipped = 0
    foreach ($src in $binaries.Keys) {
        $srcPath = Join-Path $distDir $src
        $destName = $binaries[$src]
        $destPath = Join-Path $binDir $destName
        if (Test-Path $srcPath) {
            try {
                Copy-Item $srcPath $destPath -Force -ErrorAction Stop
                $sz = [math]::Round((Get-Item $destPath).Length / 1MB, 1)
                Write-Host "  OK: $src -> $destName ($sz MB)" -ForegroundColor Green
                $installed++
            } catch {
                Write-Host "  FAIL: $src -> $destName (file locked: $_)" -ForegroundColor Red
                $skipped++
            }
        } else {
            Write-Host "  SKIP: $src not in $distDir" -ForegroundColor Yellow
            $skipped++
        }
    }
    Write-Host ""
    Write-Host "  Installed $installed binaries to ~/bin" -ForegroundColor Green
    if ($skipped -gt 0) { Write-Host "  Skipped $skipped" -ForegroundColor Yellow }
    Write-Host ""
    Write-Host "  Installed versions:" -ForegroundColor Blue
    foreach ($d in $binaries.Values) {
        $p = Join-Path $binDir $d
        if (Test-Path $p) {
            $v = & $p --version 2>$null
            Write-Host "    $d  $v"
        }
    }

# Reset local `main` to `origin/main`, discarding any uncommitted edits.
#
# Motivation: `cargo build` rewrites `Cargo.lock` whenever a transitive
# dep resolves differently (mtime-driven re-resolution, polars git pin
# updates, etc.).  After a `just ship` upstream the lockfile has moved,
# so a plain `git pull` aborts with:
#     "Your local changes to the following files would be overwritten
#      by merge: Cargo.lock"
# `just sync` is the explicit force-discard escape hatch.
#
# Safety: refuses to run on any branch other than `main`, so feature
# branches with real WIP cannot be wiped by accident.  Use plain
# `git pull --rebase` (or commit the WIP first) on those.
[unix]
sync:
    #!/usr/bin/env bash
    set -euo pipefail
    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    if [[ "$BRANCH" != "main" ]]; then
        printf "\033[0;31m❌ just sync only runs on main (current: %s)\033[0m\n" "$BRANCH"
        printf "   Use 'git pull --rebase' (or commit the WIP first) on feature branches.\n"
        exit 1
    fi
    BEFORE=$(git rev-parse --short HEAD)
    git fetch origin main
    AFTER=$(git rev-parse --short FETCH_HEAD)
    DIRTY=""
    if ! git diff --quiet || ! git diff --cached --quiet; then
        DIRTY="1"
        printf "\033[1;33m⚠️  Discarding uncommitted edits to:\033[0m\n"
        git diff --name-only HEAD | sed 's/^/   /'
    fi
    if [[ "$BEFORE" == "$AFTER" && -z "$DIRTY" ]]; then
        printf "\033[0;32m✅ Already at origin/main (%s) — nothing to do\033[0m\n" "$BEFORE"
    else
        if [[ "$BEFORE" != "$AFTER" ]]; then
            printf "\033[1;33m⚠️  Resetting %s -> %s\033[0m\n" "$BEFORE" "$AFTER"
        fi
        git reset --hard FETCH_HEAD
        printf "\033[0;32m✅ Synced to %s\033[0m\n" "$(git rev-parse --short HEAD)"
    fi

# Reset local `main` to `origin/main`, discarding any uncommitted edits
# (Windows / PowerShell variant — same semantics as the unix recipe).
[windows]
sync:
    #!powershell
    $ErrorActionPreference = 'Stop'
    $branch = (git rev-parse --abbrev-ref HEAD).Trim()
    if ($branch -ne 'main') {
        Write-Host "❌ just sync only runs on main (current: $branch)" -ForegroundColor Red
        Write-Host "   Use 'git pull --rebase' (or commit the WIP first) on feature branches."
        exit 1
    }
    $before = (git rev-parse --short HEAD).Trim()
    git fetch origin main
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $after = (git rev-parse --short FETCH_HEAD).Trim()
    $dirty = (git status --porcelain)
    if ($dirty) {
        Write-Host "⚠️  Discarding uncommitted edits to:" -ForegroundColor Yellow
        (git diff --name-only HEAD) -split "`n" | Where-Object { $_ } | ForEach-Object { Write-Host "   $_" }
    }
    if ($before -eq $after -and -not $dirty) {
        Write-Host "✅ Already at origin/main ($before) — nothing to do" -ForegroundColor Green
    } else {
        if ($before -ne $after) {
            Write-Host "⚠️  Resetting $before -> $after" -ForegroundColor Yellow
        }
        git reset --hard FETCH_HEAD
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $head = (git rev-parse --short HEAD).Trim()
        Write-Host "✅ Synced to $head" -ForegroundColor Green
    }

# Force-pull main (discarding local edits) and reinstall release binaries
# from `dist/` to `~/bin`.  Drop-in replacement for the `git pull; just use`
# leg of the daily-driver oneliner — survives a dirty `Cargo.lock`.
refresh: sync use

# Git commit with auto-generated message.
#
# Uses `git commit -S` (force-sign) because the resulting commit
# eventually lands on `main`, which has the GitHub branch-protection
# rule "Require signed commits".  Bypassing signing here used to
# silently break the merge gate downstream — PR #130 was blocked on
# 2026-05-05 because a single unsigned commit slipped through.
# Inherits the user's `user.signingkey`; fails loudly if no key is
# configured (correct: the commit cannot land on `main` without one).
commit:
    @printf "\033[0;34m📝 Creating auto-generated commit...\033[0m\n"
    git add .
    @NEW_VERSION=`awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print $3; exit}' Cargo.toml | tr -d '"'`; \
    git commit -S -m "chore: release v$NEW_VERSION - comprehensive testing complete [auto-commit]"

# Git push to remote.
#
# The implicit-commit branch uses `git commit -S` for the same
# reason as the `commit` recipe above: pushes go straight to `main`,
# which has "Require signed commits" branch-protection.
push:
    @printf "\033[0;34m🚀 Pushing to remote...\033[0m\n"
    @if git diff --quiet && git diff --cached --quiet; then echo "📋 Working directory is clean"; else echo "📋 Committing pending changes..."; git add .; git commit -S -m "chore: commit pending changes before push [auto-commit]"; fi
    git pull origin main --rebase
    git push origin main

