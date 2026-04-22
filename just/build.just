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
# Strategy:
#   1. Prefer `dist/vX.Y.Z/` matching the current `Cargo.toml` version
#   2. Fall back to latest `dist/v*` if matching cache missing
#   3. Fall back to `gh release download` otherwise (requires `gh` CLI)
#
# Install binaries from the matching GitHub Release to ~/bin.
[unix]
use:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📦 Installing UFFS binaries from dist/ cache...\033[0m\n"

    # Read workspace version from Cargo.toml
    CARGO_VER=$(awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print "v"$3; exit}' Cargo.toml | tr -d '"')

    # Find matching dist/ version (prefer workspace version)
    DIST_DIR=""
    if [[ -n "$CARGO_VER" ]] && [[ -d "dist/$CARGO_VER" ]]; then
        DIST_DIR="dist/$CARGO_VER"
        printf "\033[0;32m  Found dist/%s (matches workspace)\033[0m\n" "$CARGO_VER"
    elif [[ -d "dist" ]]; then
        LATEST=$(ls -d dist/v* 2>/dev/null | sort -V | tail -1 || true)
        if [[ -n "$LATEST" ]] && [[ -d "$LATEST" ]]; then
            LATEST_VER=$(basename "$LATEST")
            if [[ -n "$CARGO_VER" ]] && [[ "$LATEST_VER" != "$CARGO_VER" ]]; then
                printf "\033[1;33m⚠️  dist/ has %s but workspace is %s\033[0m\n" "$LATEST_VER" "$CARGO_VER"
                printf "\033[1;33m   Downloading %s from GitHub Release...\033[0m\n" "$CARGO_VER"
            else
                DIST_DIR="$LATEST"
            fi
        fi
    fi

    # Fallback: download from GitHub Release
    if [[ -z "$DIST_DIR" ]] || [[ ! -d "$DIST_DIR" ]]; then
        printf "\033[1;33m⚠️  No matching dist/ cache. Downloading from GitHub Release...\033[0m\n"
        if ! command -v gh &>/dev/null; then
            printf "\033[0;31m❌ gh CLI not found. Install with: brew install gh\033[0m\n"
            exit 1
        fi
        VERSION="${CARGO_VER:-$(gh release view --json tagName -q .tagName 2>/dev/null || true)}"
        if [[ -z "$VERSION" ]]; then
            printf "\033[0;31m❌ No GitHub Release found. Run 'just ship' first.\033[0m\n"
            exit 1
        fi
        DIST_DIR="dist/$VERSION"
        mkdir -p "$DIST_DIR"
        gh release download "$VERSION" --dir "$DIST_DIR" --clobber
        printf "\033[0;32m✅ Downloaded %s to %s\033[0m\n" "$VERSION" "$DIST_DIR"
    fi

    printf "\033[0;34m  → Using: %s\033[0m\n" "$DIST_DIR"

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

# Install UFFS binaries from local dist/ cache to ~/bin (Windows).
# If dist/ is empty, downloads the latest release from GitHub first.
# Prerequisite for fallback: `gh` CLI (winget install GitHub.cli)
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

    # Try to find matching dist/ version (prefer workspace version)
    $distDir = $null
    if ($cargoVer -and (Test-Path (Join-Path "dist" $cargoVer))) {
        $distDir = (Join-Path "dist" $cargoVer)
        Write-Host "  Found dist/$cargoVer (matches workspace)" -ForegroundColor Green
    } elseif (Test-Path "dist") {
        $versions = Get-ChildItem -Path "dist" -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^v\d+\.\d+\.\d+$' } |
            Sort-Object { [version]($_.Name -replace '^v','') } -Descending
        if ($versions) {
            $distDir = $versions[0].FullName
            $distVer = $versions[0].Name
            if ($cargoVer -and $distVer -ne $cargoVer) {
                Write-Host "  WARNING: dist/ has $distVer but workspace is $cargoVer" -ForegroundColor Yellow
                Write-Host "  Downloading $cargoVer from GitHub Release..." -ForegroundColor Yellow
                $distDir = $null  # force download
            }
        }
    }

    # Fallback: download from GitHub Release
    if (-not $distDir) {
        Write-Host "  No matching dist/ cache. Downloading from GitHub Release..." -ForegroundColor Yellow
        if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
            Write-Host "  FAIL: gh CLI not found. Install with: winget install GitHub.cli" -ForegroundColor Red
            exit 1
        }
        $version = if ($cargoVer) { $cargoVer } else { (gh release view --json tagName -q ".tagName" 2>$null) }
        if (-not $version) { Write-Host "  FAIL: No GitHub Release found." -ForegroundColor Red; exit 1 }
        $distDir = Join-Path "dist" $version
        New-Item -ItemType Directory -Path $distDir -Force | Out-Null
        gh release download $version --dir $distDir --clobber
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  FAIL: download failed." -ForegroundColor Red; exit 1
        }
        Write-Host "  OK: Downloaded $version to $distDir" -ForegroundColor Green
    }
    Write-Host "  Source: $distDir" -ForegroundColor Cyan
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

# Git commit with auto-generated message.
commit:
    @printf "\033[0;34m📝 Creating auto-generated commit...\033[0m\n"
    git add .
    @NEW_VERSION=`awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print $3; exit}' Cargo.toml | tr -d '"'`; \
    git -c commit.gpgsign=false commit -m "chore: release v$NEW_VERSION - comprehensive testing complete [auto-commit]"

# Git push to remote.
push:
    @printf "\033[0;34m🚀 Pushing to remote...\033[0m\n"
    @if git diff --quiet && git diff --cached --quiet; then echo "📋 Working directory is clean"; else echo "📋 Committing pending changes..."; git add .; git -c commit.gpgsign=false commit -m "chore: commit pending changes before push [auto-commit]"; fi
    git pull origin main --rebase
    git push origin main

