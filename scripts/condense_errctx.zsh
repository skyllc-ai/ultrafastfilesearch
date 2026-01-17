#!/usr/bin/env zsh
set -euo pipefail

# Usage: ./condense_errctx.zsh <logfile> [context_lines=1000] [outfile]
LOG="${1:?Usage: $0 <logfile> [context_lines] [outfile]}"
CTX="${2:-1000}"
# ${${LOG:t}} is the basename in zsh; result like "rust_run.condensed.errctx.log"
OUT="${3:-${${LOG:t}}.condensed.errctx.log}"

# Error/Retry + OAuth/Token-focused pattern (case-insensitive)
PATTERN='(error|fatal|panic|fail(?:ed|ure)?|exception|unauthori[sz]ed|forbidden|denied|invalid[_-]?(grant|client|token|scope)|expired|timeout|rate.?limit|circuit.?open|backoff|retry|attempt[[:space:]]*[2-9]|requeue|4(?:00|01|03|08|29)|5[0-9]{2}|oauth|oidc|jwt|bearer|access[_-]?token|refresh[_-]?token)'

# Reader based on extension
READER=(cat)
case "$LOG" in
  *.gz)  READER=(gzip -dc) ;;
  *.bz2) READER=(bzip2 -dc) ;;
  *.xz)  READER=(xz -dc) ;;
esac

if command -v rg >/dev/null 2>&1; then
  # Fast path with ripgrep (automatically merges nearby hits under -C)
  "${READER[@]}" -- "$LOG" \
    | rg -n -i -C "$CTX" -e "$PATTERN" --no-heading --line-number \
    > "$OUT"
else
  # Portable fallback: find match lines, expand to ranges, merge, print
  tmp="$(mktemp)"
  "${READER[@]}" -- "$LOG" \
    | nl -ba -w1 -s: \
    | grep -iE "$PATTERN" \
    | cut -d: -f1 \
    | awk -v c="$CTX" '{s=$1-c; if (s<1) s=1; e=$1+c; print s " " e}' \
    | sort -n \
    | awk 'BEGIN{ps=pe=0} {s=$1; e=$2; if(ps==0){ps=s; pe=e; next} if(s<=pe+1){if(e>pe) pe=e} else {print ps " " pe; ps=s; pe=e}} END{if(ps) print ps " " pe}' \
    > "$tmp"

  awk -v ranges="$tmp" '
    BEGIN {
      while ((getline < ranges) > 0) { n++; S[n]=$1; E[n]=$2 }
      i=1
    }
    {
      while (i<=n && NR > E[i]) { print ""; i++ }
      if (i<=n && NR == S[i]) printf("===== context %d..%d =====\n", S[i], E[i])
      if (i<=n && NR >= S[i] && NR <= E[i]) print
    }
  ' < <("${READER[@]}" -- "$LOG") > "$OUT"
  rm -f "$tmp"
fi

echo "Wrote: $OUT"
