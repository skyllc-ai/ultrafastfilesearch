# Load and analyze a profile from USB.
profile-load:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔬 Loading profiles from USB...\033[0m\n"
    USB_DIR=""
    for dir in /Volumes/*/uffs_profiling; do if [ -d "$dir" ]; then USB_DIR="$dir"; break; fi; done
    if [ -z "$USB_DIR" ]; then printf "\033[0;31m❌ No uffs_profiling folder found on USB\033[0m\n"; printf "\033[0;34m   Looking for USB drives...\033[0m\n"; ls -la /Volumes/ 2>/dev/null || true; exit 1; fi
    printf "\033[0;32m✅ Found USB at: $USB_DIR\033[0m\n"
    PROFILES_DIR="docs/profiles"
    mkdir -p "$PROFILES_DIR"
    COPIED=0
    if [ -f "$USB_DIR/uffs.pdb" ]; then printf "\033[0;34m   Copying uffs.pdb (for symbolication)...\033[0m\n"; cp "$USB_DIR/uffs.pdb" "$PROFILES_DIR/uffs.pdb"; COPIED=$((COPIED + 1)); printf "\033[0;32m   ✓ uffs.pdb copied\033[0m\n"; else printf "\033[0;33m   ⚠ uffs.pdb not found - profile will lack uffs.exe symbols\033[0m\n"; fi
    for profile in "$USB_DIR"/profile_*.json; do if [ -f "$profile" ]; then BASENAME=$(basename "$profile"); DEST="$PROFILES_DIR/$BASENAME"; if [ ! -f "$DEST" ]; then printf "\033[0;34m   Copying $BASENAME...\033[0m\n"; cp "$profile" "$DEST"; COPIED=$((COPIED + 1)); else printf "\033[0;33m   Skipping $BASENAME (already exists)\033[0m\n"; fi; fi; done
    for summary in "$USB_DIR"/profile_summary_*.txt; do if [ -f "$summary" ]; then BASENAME=$(basename "$summary"); DEST="$PROFILES_DIR/$BASENAME"; if [ ! -f "$DEST" ]; then printf "\033[0;34m   Copying $BASENAME...\033[0m\n"; cp "$summary" "$DEST"; COPIED=$((COPIED + 1)); else printf "\033[0;33m   Skipping $BASENAME (already exists)\033[0m\n"; fi; fi; done
    if [ -f "$USB_DIR/profile.json" ]; then TIMESTAMP=$(date +"%Y-%m-%d_%H-%M-%S"); DEST="$PROFILES_DIR/profile_imported_$TIMESTAMP.json"; printf "\033[0;34m   Copying profile.json as profile_imported_$TIMESTAMP.json...\033[0m\n"; cp "$USB_DIR/profile.json" "$DEST"; COPIED=$((COPIED + 1)); fi
    printf "\033[0;32m✅ Copied $COPIED file(s) to $PROFILES_DIR/\033[0m\n"
    LATEST=$(ls -t "$PROFILES_DIR"/profile_*.json 2>/dev/null | head -1)
    if [ -z "$LATEST" ]; then printf "\033[0;31m❌ No profiles found in $PROFILES_DIR\033[0m\n"; exit 1; fi
    printf "\033[0;32m✅ Latest profile: $LATEST\033[0m\n"
    echo ""
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;34m  📁 Profiles saved to: docs/profiles/\033[0m\n"
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""
    ls -lh "$PROFILES_DIR"/profile_*.json "$PROFILES_DIR"/uffs.pdb "$PROFILES_DIR"/profile_summary_*.txt 2>/dev/null || true
    echo ""
    LATEST_SUMMARY=$(ls -t "$PROFILES_DIR"/profile_summary_*.txt 2>/dev/null | head -1)
    if [ -n "$LATEST_SUMMARY" ] && [ -f "$LATEST_SUMMARY" ]; then printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"; printf "\033[0;34m  📋 Latest Summary: $(basename "$LATEST_SUMMARY")\033[0m\n"; printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"; cat "$LATEST_SUMMARY"; echo ""; fi
    PDB_PATH="$PROFILES_DIR/uffs.pdb"
    if [ -f "$PDB_PATH" ]; then printf "\033[0;32m✓ PDB file available for symbolication\033[0m\n"; else printf "\033[0;33m⚠ No PDB file - uffs.exe functions will show as addresses\033[0m\n"; fi
    printf "\033[0;34m   Opening latest profile in Firefox Profiler...\033[0m\n"
    printf "\033[0;34m   (Using Microsoft Symbol Server for Windows libraries)\033[0m\n"
    echo ""
    cd "$PROFILES_DIR"
    samply load --windows-symbol-server "https://msdl.microsoft.com/download/symbols" "$(basename "$LATEST")"

