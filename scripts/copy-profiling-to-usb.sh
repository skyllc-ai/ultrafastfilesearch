#!/bin/bash
# Copy UFFS profiling binaries to USB drive for Windows profiling
# Profiling binaries are built with debug symbols and live in the profiling cache directory
set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
USB_PATH="/Volumes/UFFSPRO"
DEST_DIR="$USB_PATH/uffs_profiling"

# Profiling binaries are in the cargo target cache (not dist/)
# This matches the CARGO_TARGET_DIR used by build-cross-all.rs
SOURCE_DIR="$HOME/Library/Caches/uffs/target-xwin-release/x86_64-pc-windows-msvc/profiling"
SOURCE_EXE="$SOURCE_DIR/uffs.exe"

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}  UFFS Profiling → USB Copy Script${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

# Check if USB is mounted
if [ ! -d "$USB_PATH" ]; then
    echo -e "${RED}❌ USB not found at: $USB_PATH${NC}"
    echo -e "${YELLOW}   Please insert the USB drive and try again.${NC}"
    exit 1
fi
echo -e "${GREEN}✓ USB found at: $USB_PATH${NC}"

# Check if profiling binaries exist
if [ ! -f "$SOURCE_EXE" ]; then
    echo -e "${RED}❌ Profiling binary not found: $SOURCE_EXE${NC}"
    echo -e "${YELLOW}   Run this first: UFFS_PROFILING_BUILD=1 rust-script scripts/build-cross-all.rs${NC}"
    echo -e "${YELLOW}   Or: just profile-usb${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Found profiling binaries${NC}"

# Create destination directory
echo -e "${BLUE}📁 Creating destination: $DEST_DIR${NC}"
mkdir -p "$DEST_DIR"

# Copy files with progress
echo -e "${BLUE}📦 Copying files...${NC}"

echo -e "   Copying uffs.exe ($(du -h "$SOURCE_EXE" | cut -f1))..."
cp "$SOURCE_EXE" "$DEST_DIR/uffs.exe"
echo -e "${GREEN}   ✓ uffs.exe${NC}"

# Note: Skipping PDB - not needed for samply profiling (debug info embedded in binary)

# Copy PowerShell script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/windows-profiling.ps1" ]; then
    cp "$SCRIPT_DIR/windows-profiling.ps1" "$DEST_DIR/run-profiling.ps1"
    echo -e "${GREEN}   ✓ run-profiling.ps1${NC}"
fi

# Create Windows instructions file
cat > "$DEST_DIR/README_WINDOWS.txt" << 'EOF'
UFFS Profiling Instructions for Windows
========================================

EASY WAY: Run the PowerShell script
------------------------------------
Right-click run-profiling.ps1 → "Run with PowerShell"

This will:
  - Install samply (if needed)
  - Copy files to C:\profiling
  - Run profiling
  - Copy profile.json back to USB

MANUAL WAY:
-----------

1. ONE-TIME SETUP: Install samply
   Open PowerShell and run:

   cargo install --locked samply

2. COPY FILES: Copy this folder to a local drive

   mkdir C:\profiling
   copy G:\uffs_profiling\* C:\profiling\

3. RUN PROFILING: Record a profile

   cd C:\profiling
   samply record --save-only -o profile.json -- .\uffs.exe "*" --drives=C

4. COPY BACK: Copy profile.json back to this USB folder

   copy C:\profiling\profile.json G:\uffs_profiling\

5. ON MAC: Analyze with

   samply load /Volumes/UFFSPRO/uffs_profiling/profile.json
EOF
echo -e "${GREEN}   ✓ README_WINDOWS.txt${NC}"

# Show summary
echo ""
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  ✅ DONE! Files copied to USB${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${BLUE}Contents:${NC}"
ls -lh "$DEST_DIR"
echo ""
echo -e "  ${YELLOW}Next steps:${NC}"
echo -e "  1. Eject USB safely"
echo -e "  2. On Windows: Follow instructions in README_WINDOWS.txt"
echo -e "  3. Bring back: profile.json"
echo -e "  4. On Mac: samply load <path>/profile.json"
echo ""

