#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
# Copyright (c) 2025-2026 SKY, LLC.
#
# Phase 2.5 visibility-demotion applier.
#
# Reads a per-crate audit report produced by `visibility_audit.sh` and
# applies `pub` -> `pub(crate)` demotions for every row marked DEMOTE.
#
# Usage:
#   scripts/dev/apply_visibility_demotions.sh <crate-name>
#
# Workflow:
#   1. Read docs/dev/baseline/2026-05-12/phase_2_5_audits/<crate>.md
#   2. For each DEMOTE table row, parse (file, line, kind, identifier)
#   3. Use sed to demote `pub <kind> <ident>` -> `pub(crate) <kind> <ident>`
#   4. Print summary of changes
#
# Does NOT auto-compile or auto-commit; caller must verify.

set -uo pipefail

CRATE="${1:-}"
if [[ -z "$CRATE" ]]; then
  echo "Usage: $0 <crate-name>" >&2
  exit 1
fi

REPORT="${REPORT_DIR:-docs/dev/baseline/2026-05-12/phase_2_5_audits}/${CRATE}.md"
if [[ ! -f "$REPORT" ]]; then
  echo "ERROR: audit report not found: $REPORT" >&2
  echo "Run scripts/dev/visibility_audit.sh $CRATE first." >&2
  exit 1
fi

CRATE_DIR="crates/$CRATE/src"
APPLIED=0
SKIPPED=0
ERRORED=0

# Parse DEMOTE rows; each row is:
#   | `<file>` | <line> | <kind> | `<ident>` | <refs> | <consumers> | <own_bin> | **DEMOTE** ... |
while IFS= read -r ROW; do
  # Extract fields between backticks/pipes
  FILE=$(echo "$ROW" | awk -F'`' '{print $2}')
  LINE=$(echo "$ROW" | awk -F'|' '{gsub(/^ +| +$/, "", $3); print $3}')
  KIND=$(echo "$ROW" | awk -F'|' '{gsub(/^ +| +$/, "", $4); print $4}')
  IDENT=$(echo "$ROW" | awk -F'`' '{print $4}')

  if [[ -z "$FILE" ]] || [[ -z "$LINE" ]] || [[ -z "$IDENT" ]]; then
    echo "SKIP malformed row: $ROW" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  ABS_FILE="${CRATE_DIR}/${FILE}"
  if [[ ! -f "$ABS_FILE" ]]; then
    echo "SKIP missing file: $ABS_FILE" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  # Build sed expression for the specific line.
  # Match optional leading whitespace + 'pub' + space-or-tab.
  # Replace with the same whitespace + 'pub(crate)' + same separator.
  # Use GNU/BSD-compatible sed via macOS-friendly invocation.
  SED_EXPR="${LINE}s/^\\([[:space:]]*\\)pub /\\1pub(crate) /"

  # Capture before/after to verify
  BEFORE=$(sed -n "${LINE}p" "$ABS_FILE")
  if [[ -z "$BEFORE" ]]; then
    echo "SKIP empty line $LINE in $ABS_FILE" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  # Sanity check: line must contain `pub ` (not `pub(`) before our edit
  if ! echo "$BEFORE" | grep -qE '^\s*pub\s'; then
    echo "SKIP line $LINE doesn't match expected pub pattern: $BEFORE" >&2
    SKIPPED=$((SKIPPED + 1))
    continue
  fi

  # Apply edit (macOS BSD sed needs '-i ""')
  if sed -i '' "${SED_EXPR}" "$ABS_FILE" 2>/dev/null; then
    AFTER=$(sed -n "${LINE}p" "$ABS_FILE")
    if [[ "$BEFORE" != "$AFTER" ]]; then
      APPLIED=$((APPLIED + 1))
    else
      echo "WARN no change for $ABS_FILE:$LINE ($IDENT)" >&2
      ERRORED=$((ERRORED + 1))
    fi
  else
    echo "ERROR sed failed for $ABS_FILE:$LINE ($IDENT)" >&2
    ERRORED=$((ERRORED + 1))
  fi
done < <(grep -E '^\|.*\*\*DEMOTE\*\*' "$REPORT")

echo
echo "=== Demotion summary for $CRATE ==="
echo "  Applied:  $APPLIED"
echo "  Skipped:  $SKIPPED"
echo "  Errored:  $ERRORED"
echo
echo "Next: cargo check -p $CRATE --all-targets --frozen"
