#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: scripts/relayout-dictionary-pack.sh /path/to/RUSTMIX/APPS/DICT

Moves every flat DATA/NAME.JSN shard into a two-character bucket directory
(DATA/CA/CASA.JSN) and rewrites INDEX.TXT to match, keeping its byte order.
A full pack keeps ~28k shards in one directory, and FAT scans that whole
directory on every file open; buckets keep each directory small.

Run it on a copy on your computer, then copy the result to the SD card.
Safe to re-run: rows already in a bucket are left alone, and an interrupted
run resumes where it stopped.
USAGE
}

DICT="${1:-}"
if [[ -z "$DICT" || "$DICT" == "-h" || "$DICT" == "--help" ]]; then usage; exit 1; fi
INDEX="$DICT/INDEX.TXT"
if [[ ! -f "$INDEX" || ! -d "$DICT/DATA" ]]; then
  echo "dictionary-relayout=failed missing=$INDEX-or-DATA" >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/rustmix-dict-relayout.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# Pass 1: new index plus "BUCKET FILE" move list for flat rows.
awk -v moves="$WORK/moves.txt" '
  { sub(/\r$/, "") }
  /^[[:space:]]*(#|$)/ { print; next }
  {
    split($0, parts, "|")
    name = parts[1]; path = parts[2]
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", path)
    if (path ~ /^DATA\/[^\/]+$/) {
      bucket = toupper(substr(name, 1, 2))
      if (bucket !~ /^[A-Z0-9]+$/) {
        print "dictionary-relayout=failed bad-shard-name=" name > "/dev/stderr"
        exit 2
      }
      file = substr(path, 6)
      print bucket " " file > moves
      print name "|DATA/" bucket "/" file
    } else {
      print name "|" path
    }
  }
' "$INDEX" > "$WORK/INDEX.TXT"
touch "$WORK/moves.txt"

MOVED=0
BUCKETS=0
# Pass 2: one mkdir + one batched mv per bucket instead of one per shard.
while read -r bucket; do
  mkdir -p "$DICT/DATA/$bucket"
  BUCKETS=$((BUCKETS + 1))
  files=()
  while read -r file; do
    [[ -f "$DICT/DATA/$file" ]] && files+=("$file")
  done < <(awk -v b="$bucket" '$1 == b { print $2 }' "$WORK/moves.txt")
  if [[ ${#files[@]} -gt 0 ]]; then
    (cd "$DICT/DATA" && mv -- "${files[@]}" "$bucket/")
    MOVED=$((MOVED + ${#files[@]}))
  fi
done < <(cut -d' ' -f1 "$WORK/moves.txt" | sort -u)

# Pass 3: only swap the index in once every row resolves to a real file.
MISSING=0
while IFS='|' read -r name path; do
  [[ -z "$name" || "$name" == \#* ]] && continue
  if [[ ! -f "$DICT/$path" ]]; then
    echo "dictionary-relayout=missing-shard row=$name path=$path" >&2
    MISSING=$((MISSING + 1))
  fi
done < "$WORK/INDEX.TXT"
if [[ "$MISSING" -ne 0 ]]; then
  echo "dictionary-relayout=failed missing-shards=$MISSING index=unchanged" >&2
  exit 1
fi

cp "$WORK/INDEX.TXT" "$INDEX"
LEFT="$(find "$DICT/DATA" -maxdepth 1 -type f -name '*.JSN' | wc -l | tr -d ' ')"
echo "dictionary-relayout=ok moved=$MOVED buckets=$BUCKETS flat-shards-left=$LEFT"
