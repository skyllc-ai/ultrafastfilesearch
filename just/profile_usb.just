# Build profiling binaries and copy to USB for Windows profiling.
profile-usb:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;34m  🔬 UFFS Profiling → USB Workflow\033[0m\n"
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""
    printf "\033[0;34mStep 1: Building profiling binaries...\033[0m\n"
    printf "\033[0;36m  → UFFS_PROFILING_BUILD=1 rust-script scripts/ci/build-cross-all.rs\033[0m\n"
    UFFS_PROFILING_BUILD=1 rust-script scripts/ci/build-cross-all.rs
    printf "\033[0;32m✅ Profiling binaries built\033[0m\n"
    echo ""
    printf "\033[0;34mStep 2: Checking for USB drive...\033[0m\n"
    USB_PATHS=("/Volumes/UFFSPRO" "/Volumes/USB" "/Volumes/UNTITLED")
    USB_PATH=""
    for path in "${USB_PATHS[@]}"; do if [ -d "$path" ]; then USB_PATH="$path"; break; fi; done
    if [ -z "$USB_PATH" ]; then
        printf "\033[1;33m⚠️  No USB drive found at common mount points.\033[0m\n"
        printf "\033[0;34m   Looking for mounted volumes...\033[0m\n"
        echo ""
        ls -la /Volumes/ 2>/dev/null || true
        echo ""
        printf "\033[1;33m   Please enter the USB mount path (e.g., /Volumes/UFFSPRO): \033[0m"
        read -r USB_PATH
        if [ ! -d "$USB_PATH" ]; then printf "\033[0;31m❌ USB path not found: $USB_PATH\033[0m\n"; exit 1; fi
    fi
    printf "\033[0;32m✅ USB found at: $USB_PATH\033[0m\n"
    echo ""
    printf "\033[0;34mStep 3: Copying files to USB...\033[0m\n"
    DEST_DIR="$USB_PATH/uffs_profiling"
    mkdir -p "$DEST_DIR"
    SOURCE_DIR="$HOME/Library/Caches/uffs/target-xwin-release/x86_64-pc-windows-msvc/profiling"
    SOURCE_EXE="$SOURCE_DIR/uffs.exe"
    if [ ! -f "$SOURCE_EXE" ]; then printf "\033[0;31m   ❌ Profiling binary not found: $SOURCE_EXE\033[0m\n"; printf "\033[0;33m   The build may have failed - check output above\033[0m\n"; exit 1; fi
    printf "\033[0;36m   Source: $SOURCE_DIR\033[0m\n"
    echo "   Copying uffs.exe ($(du -h "$SOURCE_EXE" | cut -f1))..."
    cp "$SOURCE_EXE" "$DEST_DIR/uffs.exe"
    printf "\033[0;32m   ✓ uffs.exe\033[0m\n"
    if [ -f "scripts/windows/windows-profiling.ps1" ]; then cp "scripts/windows/windows-profiling.ps1" "$DEST_DIR/run-profiling.ps1"; printf "\033[0;32m   ✓ run-profiling.ps1\033[0m\n"; fi
    echo ""
    printf "\033[0;32m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;32m  ✅ DONE! USB is ready for Windows profiling\033[0m\n"
    printf "\033[0;32m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""
    printf "\033[0;34m  Contents of $DEST_DIR:\033[0m\n"
    ls -lh "$DEST_DIR"
    echo ""
    printf "\033[1;33m  📋 ON WINDOWS:\033[0m\n"
    printf "\033[0;36m     Option 1: Right-click run-profiling.ps1 → 'Run with PowerShell'\033[0m\n"
    printf "\033[0;36m     Option 2: In PowerShell: G:\\uffs_profiling\\run-profiling.ps1\033[0m\n"
    echo ""
    printf "\033[1;33m  📋 BACK ON MAC:\033[0m\n"
    printf "\033[0;36m     samply load $USB_PATH/uffs_profiling/profile.json\033[0m\n"

