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
    if [[ "$OSTYPE" == "msys"* ]] || [[ "$OSTYPE" == "cygwin"* ]] || [[ -n "$WINDIR" ]]; then TARGET_BINARY="tt.exe"; printf "\033[0;34m  → Windows detected, using tt.exe\033[0m\n"; else TARGET_BINARY="tt"; printf "\033[0;34m  → Unix-like system detected, using tt\033[0m\n"; fi
    SOURCE_DIR="target/{{ profile }}"
    SOURCE_PATH="$SOURCE_DIR/$TARGET_BINARY"
    if [[ ! -f "$SOURCE_PATH" ]]; then printf "\033[0;31m❌ Binary not found: $SOURCE_PATH\033[0m\n"; printf "\033[1;33m💡 Run 'just build' first\033[0m\n"; exit 1; fi
    mkdir -p ~/bin
    DEST_PATH="$HOME/bin/$TARGET_BINARY"
    printf "\033[0;34m  → Source: $SOURCE_PATH\033[0m\n"
    printf "\033[0;34m  → Destination: $DEST_PATH\033[0m\n"
    if cp "$SOURCE_PATH" "$DEST_PATH" && chmod +x "$DEST_PATH"; then
        printf "\033[0;32m✅ Binary deployed to ~/bin/$TARGET_BINARY\033[0m\n"
        if command -v xattr >/dev/null 2>&1; then xattr -w com.uffs.buildtype "{{ profile }}" "$DEST_PATH" 2>/dev/null || true; printf "\033[0;32m🏷️  Extended attributes set\033[0m\n"; fi
    else
        printf "\033[0;31m❌ Failed to copy binary\033[0m\n"
        exit 1
    fi
    if ! echo "$PATH" | grep -q "$HOME/bin"; then printf "\033[1;33m⚠️  ~/bin not in PATH. Add this to your shell profile:\033[0m\n"; printf "   export PATH=\"\$HOME/bin:\$PATH\"\n"; fi

# Install shipped UFFS binaries to ~/bin (Windows only).
[windows]
use:
    Write-Host "📦 Installing latest UFFS binaries to ~/bin..." -ForegroundColor Blue; \
    Write-Host "ℹ️  Note: UFFS is Windows-only (requires NTFS MFT access)" -ForegroundColor Yellow; \
    $binDir = "$env:USERPROFILE\bin"; if (-not (Test-Path $binDir)) { New-Item -ItemType Directory -Path $binDir -Force | Out-Null }; \
    $distDir = $null; $latestItem = Get-Item "dist\latest" -ErrorAction SilentlyContinue; \
    if ($latestItem -and $latestItem.LinkType -eq 'SymbolicLink') { $distDir = $latestItem.Target } elseif ($latestItem -and -not $latestItem.PSIsContainer) { $target = (Get-Content "dist\latest" -Raw).Trim(); if (Test-Path "dist\$target") { $distDir = "dist\$target" } }; \
    if (-not $distDir) { $versions = Get-ChildItem -Path "dist" -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^v\d+\.\d+\.\d+$' } | Sort-Object { [version]($_.Name -replace '^v','') } -Descending; if ($versions) { $distDir = $versions[0].FullName } }; \
    if (-not $distDir) { Write-Host "❌ No binaries found in dist/. Run 'just phase2-ship' first." -ForegroundColor Red; exit 1 }; \
    Write-Host "  → Using: $distDir" -ForegroundColor Cyan; $binaries = @("uffs", "uffs_mft", "uffs_tui", "uffs_gui"); $installed = 0; $skipped = 0; \
    foreach ($bin in $binaries) { $src = "$distDir\$bin\$bin-windows-x64.exe"; $dest = "$binDir\$bin.exe"; if (Test-Path $src) { Copy-Item $src $dest -Force; Write-Host "  ✅ $bin.exe" -ForegroundColor Green; $installed++ } else { Write-Host "  ⚠️  $bin (not found)" -ForegroundColor Yellow; $skipped++ } }; \
    Write-Host "✅ Installed $installed binaries to ~/bin" -ForegroundColor Green; if ($skipped -gt 0) { Write-Host "⚠️  Skipped $skipped (run 'just phase2-ship' to build all binaries)" -ForegroundColor Yellow }

# Explain that UFFS is Windows-only on Unix hosts.
[unix]
use:
    #!/usr/bin/env bash
    printf "\033[0;31m❌ UFFS only runs on Windows.\033[0m\n"
    printf "\033[0;34m   UFFS reads the NTFS Master File Table directly using Windows APIs.\033[0m\n"
    printf "\033[0;34m   Use 'just phase2-ship' to cross-compile and ship Windows binaries from this host.\033[0m\n"
    exit 1

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

