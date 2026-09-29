#!/usr/bin/env bash
set -euo pipefail

VOLUME="${1:-}"
if [[ -z "$VOLUME" || ! -d "$VOLUME" ]]; then
  echo "usage: scripts/verify-dictionary-x4-pack.sh /Volumes/YOUR_SD_CARD" >&2
  exit 1
fi

DICT="$VOLUME/RUSTMIX/APPS/DICT"
INDEX="$DICT/INDEX.TXT"
if [[ ! -f "$INDEX" ]]; then
  echo "dictionary-x4-pack-verification=failed missing=$INDEX" >&2
  exit 1
fi

ROWS="$(grep -Ev '^[[:space:]]*(#|$)' "$INDEX" | wc -l | tr -d ' ')"
SHARDS=0
[[ -d "$DICT/DATA" ]] && SHARDS="$(find "$DICT/DATA" -type f -name '*.JSN' | wc -l | tr -d ' ')"

# The firmware binary-searches INDEX.TXT in place, so it must be byte-sorted.
if ! grep -Ev '^[[:space:]]*(#|$)' "$INDEX" | tr -d '\r' | cut -d'|' -f1 | LC_ALL=C sort -c 2>/dev/null; then
  echo "dictionary-x4-pack-verification=failed index-not-sorted rows=$ROWS shards=$SHARDS" >&2
  exit 1
fi

# Every row must point at a real shard, flat (DATA/X.JSN) or bucketed
# (DATA/XY/X.JSN).
MISSING=0
while IFS='|' read -r name path; do
  path="${path%$'\r'}"
  [[ -z "$name" || "$name" == \#* ]] && continue
  if [[ ! "$path" =~ ^DATA/([A-Za-z0-9]{1,8}/)?[^/]+\.JSN$ || ! -f "$DICT/$path" ]]; then
    echo "dictionary-x4-pack-verification=missing-shard row=$name path=$path" >&2
    MISSING=$((MISSING + 1))
  fi
done < "$INDEX"
if [[ "$MISSING" -ne 0 ]]; then
  echo "dictionary-x4-pack-verification=failed missing-shards=$MISSING rows=$ROWS shards=$SHARDS" >&2
  exit 1
fi

# Probe words: English X4 pack or Italian pack; at least one must resolve.
shard_paths_for() {
  local word="$1" length base
  for ((length = 5; length >= 1; length--)); do
    base="${word:0:length}"
    grep -E "^${base}[0-9]*\|" "$INDEX" | tr -d '\r' | cut -d'|' -f2 || true
  done
}
FOUND=()
for word in CALENDAR CAB BARN CASA ANDARE; do
  # Collect first: breaking out of a live pipe makes Git Bash's cut report
  # a spurious write error.
  mapfile -t paths < <(shard_paths_for "$word")
  for path in "${paths[@]}"; do
    [[ -n "$path" ]] || continue
    if grep -Fq "\"$word\"" "$DICT/$path"; then
      FOUND+=("$word")
      break
    fi
  done
done
if [[ ${#FOUND[@]} -eq 0 ]]; then
  echo "dictionary-x4-pack-verification=failed no-probe-word-found rows=$ROWS shards=$SHARDS" >&2
  exit 1
fi

PROBES="$(IFS=,; echo "${FOUND[*]}")"
echo "dictionary-x4-pack-verification=ok rows=$ROWS shards=$SHARDS probes=$PROBES"
