#!/bin/bash
# =============================================================================
# scripts/dev/create-profiling-bundle.sh - Create a profiling bundle for Windows analysis
# =============================================================================
#
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 Robert Nio
#
# Creates a self-contained bundle with:
# - Profiling binaries (uffs.exe + uffs.pdb)
# - Build metadata (git sha, rustc version, build flags)
# - Directory structure for profiles to be added after Windows profiling
#
# Usage:
#   ./scripts/dev/create-profiling-bundle.sh [scenario_name]
#
# Example:
#   ./scripts/dev/create-profiling-bundle.sh mft_large_cold_cache
#
# After running on Windows, copy profile.json back to bundles/<name>/profiles/

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Get script directory and project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Configuration
TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Library/Caches/uffs/target}"
TARGET_TRIPLE="x86_64-pc-windows-msvc"
BUNDLES_DIR="$PROJECT_ROOT/bundles"

# Get scenario name from argument or generate default
SCENARIO="${1:-baseline}"
DATE=$(date +%Y-%m-%d)
GIT_SHA=$(git -C "$PROJECT_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")
BUNDLE_NAME="${DATE}-${GIT_SHA}-${SCENARIO}"
BUNDLE_PATH="$BUNDLES_DIR/$BUNDLE_NAME"

echo -e "${BLUE}📦 Creating Profiling Bundle${NC}"
echo -e "   Bundle: ${GREEN}$BUNDLE_NAME${NC}"
echo ""

# Step 1: Build with profiling profile
echo -e "${YELLOW}🔨 Step 1: Building with profiling profile...${NC}"
export UFFS_PROFILING_BUILD=1
cd "$PROJECT_ROOT"

if ! rust-script scripts/ci/build-cross-all.rs; then
    echo -e "${RED}❌ Build failed!${NC}"
    exit 1
fi

echo -e "${GREEN}✅ Build complete${NC}"
echo ""

# Step 2: Create bundle directory structure
echo -e "${YELLOW}📁 Step 2: Creating bundle directory structure...${NC}"
mkdir -p "$BUNDLE_PATH/dist"
mkdir -p "$BUNDLE_PATH/profiles"

# Step 3: Copy binaries and PDBs
echo -e "${YELLOW}📋 Step 3: Copying binaries and symbols...${NC}"
PROFILING_DIR="$TARGET_DIR/$TARGET_TRIPLE/profiling"

# Copy main CLI binary
if [[ -f "$PROFILING_DIR/uffs.exe" ]]; then
    cp "$PROFILING_DIR/uffs.exe" "$BUNDLE_PATH/dist/"
    echo -e "   ${GREEN}✅${NC} uffs.exe"
else
    echo -e "   ${RED}❌${NC} uffs.exe not found at $PROFILING_DIR/uffs.exe"
    exit 1
fi

if [[ -f "$PROFILING_DIR/uffs.pdb" ]]; then
    cp "$PROFILING_DIR/uffs.pdb" "$BUNDLE_PATH/dist/"
    echo -e "   ${GREEN}✅${NC} uffs.pdb"
else
    echo -e "   ${YELLOW}⚠️${NC} uffs.pdb not found (symbols may not resolve)"
fi

# Step 4: Create build metadata
echo -e "${YELLOW}📝 Step 4: Creating build metadata...${NC}"
cat > "$BUNDLE_PATH/dist/build_meta.json" << EOF
{
  "git_sha": "$(git -C "$PROJECT_ROOT" rev-parse HEAD 2>/dev/null || echo "unknown")",
  "git_sha_short": "$GIT_SHA",
  "git_branch": "$(git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")",
  "build_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "rustc_version": "$(rustc --version)",
  "cargo_version": "$(cargo --version)",
  "profile": "profiling",
  "target": "$TARGET_TRIPLE",
  "scenario": "$SCENARIO",
  "host_os": "$(uname -s)",
  "host_arch": "$(uname -m)"
}
EOF
echo -e "   ${GREEN}✅${NC} build_meta.json"

# Step 5: Create run instructions
echo -e "${YELLOW}📖 Step 5: Creating run instructions...${NC}"
cat > "$BUNDLE_PATH/README.md" << 'EOF'
# Profiling Bundle

## On Windows

### 1. Install samply (one-time)
```powershell
cargo install --locked samply
```

### 2. Run profiling
```powershell
cd dist
samply record --save-only -o ..\profiles\profile.json -- .\uffs.exe "*" --drives=C
```

### 3. Copy back to Mac
Copy the entire `profiles/` folder back via USB.

## On Mac

### Analyze with samply
```bash
samply load profiles/profile.json
```

### Or use SpeedScope (if you exported from PerfView)
Open https://www.speedscope.app/ and drag-drop the .speedscope.json file.
EOF
echo -e "   ${GREEN}✅${NC} README.md"

# Step 6: Create notes template
cat > "$BUNDLE_PATH/notes.md" << EOF
# Profiling Notes: $BUNDLE_NAME

## What I'm Testing
<!-- Describe what optimization or change you're profiling -->

## Scenario
- **Name**: $SCENARIO
- **Dataset**: <!-- e.g., "Full C: drive, ~2M files" -->
- **Cache State**: <!-- cold / warm -->

## Results
<!-- Add observations after profiling -->

## Comparison
<!-- Compare with previous runs if applicable -->
EOF
echo -e "   ${GREEN}✅${NC} notes.md"

echo ""
echo -e "${GREEN}✅ Bundle created successfully!${NC}"
echo ""
echo -e "${BLUE}📍 Bundle location:${NC} $BUNDLE_PATH"
echo ""
echo -e "${YELLOW}Next steps:${NC}"
echo "  1. Copy $BUNDLE_PATH/dist/ to Windows (USB or network)"
echo "  2. On Windows, run: samply record --save-only -o profile.json -- .\\uffs.exe \"*\" --drives=C"
echo "  3. Copy profile.json back to $BUNDLE_PATH/profiles/"
echo "  4. On Mac, run: samply load $BUNDLE_PATH/profiles/profile.json"

