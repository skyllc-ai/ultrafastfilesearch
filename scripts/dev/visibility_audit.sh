#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Visibility audit helper for Phase 2.5 — visibility minimization.
#
# Usage: scripts/dev/visibility_audit.sh <crate-name>
#
# Outputs a Markdown report listing every bare `pub` item in the
# given crate, with its external (cross-crate) usage count.  Items
# with `external_count == 0` are demote candidates.
#
# Output goes to stdout; redirect to capture.

set -uo pipefail
# Note: NOT using -e because we rely on grep returning 1 when no matches
# are found (e.g., pub(crate) count in a crate with no pub(crate) items).

CRATE="${1:-}"
if [[ -z "$CRATE" ]]; then
  echo "Usage: $0 <crate-name>" >&2
  echo "  e.g.: $0 uffs-format" >&2
  exit 1
fi

CRATE_DIR="crates/$CRATE/src"
if [[ ! -d "$CRATE_DIR" ]]; then
  echo "ERROR: crate directory '$CRATE_DIR' not found" >&2
  exit 1
fi

# Convert crate name to its Rust path (uffs-mft -> uffs_mft)
CRATE_RUST="${CRATE//-/_}"

cat <<EOF
# Visibility audit — \`$CRATE\`

**Captured:** $(date -u +%Y-%m-%dT%H:%M:%SZ)
**SHA:** $(git rev-parse HEAD)
**Methodology:** for each bare \`pub\` item in the **library** portion
of \`$CRATE_DIR/\` (excluding \`src/bin/\`), count:

- \`refs\` — total line occurrences of the identifier across all
  workspace \`crates/\` (excluding the owner library's lib-portion files)
- \`consumers\` — count of **distinct downstream crates** that
  reference the identifier
- \`own_bin\` — 1 if any \`src/bin/*.rs\` or \`tests/*.rs\` of the
  **same crate** references the identifier (these are separate
  compilation units that need \`pub\` access)

**Action thresholds (aggressive but compile-safe policy):**

- \`consumers == 0\` AND \`own_bin == 0\` → **DEMOTE** (no external use anywhere — safe to demote)
- \`consumers == 1\` AND \`own_bin == 0\` → **RELOCATE** (single workspace consumer — candidate to move into consumer crate; demotion alone breaks compile)
- \`own_bin == 1\` → keep \`pub\` (own crate's bin/test crates need cross-crate-style access)
- \`consumers == 2\` → verify (review manually — could consolidate or stay)
- \`consumers >= 3\` → keep \`pub\` (genuine workspace-level API)

---

## Summary

EOF

# Tally totals
TOTAL_PUB=$(grep -rE '^\s*pub\s+(mod|fn|async\s+fn|struct|enum|trait|type|const|static|use)' "$CRATE_DIR" 2>/dev/null | wc -l | tr -d ' ')
TOTAL_PUB_CRATE=$(grep -rE '^\s*pub\(crate\)\s+' "$CRATE_DIR" 2>/dev/null | wc -l | tr -d ' ')
TOTAL_PUB_SUPER=$(grep -rE '^\s*pub\(super\)\s+' "$CRATE_DIR" 2>/dev/null | wc -l | tr -d ' ')

echo "- Total bare \`pub\` items: **$TOTAL_PUB**"
echo "- Total \`pub(crate)\` items: $TOTAL_PUB_CRATE"
echo "- Total \`pub(super)\` items: $TOTAL_PUB_SUPER"
echo

# Per-item analysis
echo "## Per-item analysis"
echo
echo "| File | Line | Kind | Identifier | refs | consumers | own_bin | Action |"
echo "|---|---:|---|---|---:|---:|---:|---|"

# Find all bare-pub declarations
# Match: ^<whitespace>pub <KIND> <IDENT>
# Skip: pub(crate), pub(super), pub(in ...), pub use (different handling)
while IFS=: read -r FILE LINE CONTENT; do
  # Skip if pub(crate), pub(super), pub(in
  if [[ "$CONTENT" =~ ^[[:space:]]*pub\([a-z]+ ]] || [[ "$CONTENT" =~ ^[[:space:]]*pub\(in ]]; then
    continue
  fi

  # Extract kind and identifier
  KIND=""
  IDENT=""
  if [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+(async[[:space:]]+)?fn[[:space:]]+([a-z_][a-zA-Z0-9_]*) ]]; then
    KIND="fn"
    IDENT="${BASH_REMATCH[2]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+mod[[:space:]]+([a-z_][a-zA-Z0-9_]*) ]]; then
    KIND="mod"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+struct[[:space:]]+([A-Z][a-zA-Z0-9_]*) ]]; then
    KIND="struct"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+enum[[:space:]]+([A-Z][a-zA-Z0-9_]*) ]]; then
    KIND="enum"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+trait[[:space:]]+([A-Z][a-zA-Z0-9_]*) ]]; then
    KIND="trait"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+type[[:space:]]+([A-Z][a-zA-Z0-9_]*) ]]; then
    KIND="type"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+const[[:space:]]+([A-Z_][A-Z0-9_]*) ]]; then
    KIND="const"
    IDENT="${BASH_REMATCH[1]}"
  elif [[ "$CONTENT" =~ ^[[:space:]]*pub[[:space:]]+static[[:space:]]+([A-Z_][A-Z0-9_]*) ]]; then
    KIND="static"
    IDENT="${BASH_REMATCH[1]}"
  else
    continue
  fi

  # Count external references in other crates and own-crate bins/tests.
  # The owner crate's library code (non-bin, non-test) is EXCLUDED because
  # those are the same compilation unit; pub(crate) suffices there.
  # The owner crate's src/bin/ and tests/ are SEPARATE compilation units
  # that need pub access — so they count as external consumers.
  EXTERNAL_HITS=$(grep -rE "\\b${IDENT}\\b" crates/ \
    --include='*.rs' \
    2>/dev/null \
    | grep -v "^crates/${CRATE}/src/" \
    || true)

  if [[ -z "$EXTERNAL_HITS" ]]; then
    EXTERNAL_REFS=0
    EXTERNAL_CONSUMERS=0
  else
    EXTERNAL_REFS=$(echo "$EXTERNAL_HITS" | wc -l | tr -d ' ')
    EXTERNAL_CONSUMERS=$(echo "$EXTERNAL_HITS" | cut -d/ -f2 | sort -u | wc -l | tr -d ' ')
  fi

  # Separately check own-crate bin/test usage (these are separate
  # compilation units that need `pub` to access lib items).
  OWN_BIN_HITS=$(grep -rE "\\b${IDENT}\\b" \
    "crates/${CRATE}/src/bin" \
    "crates/${CRATE}/tests" \
    "crates/${CRATE}/benches" \
    "crates/${CRATE}/examples" \
    --include='*.rs' \
    2>/dev/null \
    || true)
  if [[ -z "$OWN_BIN_HITS" ]]; then
    OWN_BIN=0
  else
    OWN_BIN=1
  fi

  # Determine action using both metrics.
  # Policy:
  #   own_bin == 1               -> keep pub (separate compilation unit needs it)
  #   consumers == 0 + own_bin 0 -> DEMOTE (definitively unused; safe demotion)
  #   consumers == 1 + own_bin 0 -> RELOCATE (single workspace consumer;
  #                                proper fix is to MOVE item into consumer
  #                                crate, not blanket-demote, because demotion
  #                                alone would break the single consumer)
  #   consumers == 2             -> verify (manual review)
  #   consumers >= 3             -> keep pub (legitimate workspace API)
  ACTION=""
  if [[ "$OWN_BIN" -eq 1 ]]; then
    ACTION="keep \`pub\` (own bin)"
  elif [[ "$EXTERNAL_CONSUMERS" -eq 0 ]]; then
    ACTION="**DEMOTE** (0 consumers)"
  elif [[ "$EXTERNAL_CONSUMERS" -eq 1 ]]; then
    ACTION="**RELOCATE** (1 consumer — move item into consumer crate)"
  elif [[ "$EXTERNAL_CONSUMERS" -eq 2 ]]; then
    ACTION="verify (2 consumers)"
  else
    ACTION="keep \`pub\`"
  fi

  # Get relative path
  REL_FILE="${FILE#crates/$CRATE/src/}"

  echo "| \`$REL_FILE\` | $LINE | $KIND | \`$IDENT\` | $EXTERNAL_REFS | $EXTERNAL_CONSUMERS | $OWN_BIN | $ACTION |"
done < <(grep -rEn '^\s*pub\s+(mod|async\s+fn|fn|struct|enum|trait|type|const|static)' "$CRATE_DIR" 2>/dev/null | grep -v "^${CRATE_DIR}/bin/" | grep -v 'pub(crate)' | grep -v 'pub(super)' | grep -v 'pub(in')

echo
echo "## Demote summary"
echo
echo "Run \`scripts/dev/visibility_audit.sh $CRATE | grep DEMOTE | wc -l\` for count."
