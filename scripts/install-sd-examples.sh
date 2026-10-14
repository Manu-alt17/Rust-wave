#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FORCE=0
if [[ "${1:-}" == "--force" ]]; then
  FORCE=1
  shift
fi
VOLUME="${1:-}"
if [[ -z "$VOLUME" ]]; then
  echo "usage: $0 [--force] /Volumes/YOUR_SD_CARD" >&2
  exit 1
fi

SRC="$ROOT/examples/sd-card"
DEST="$VOLUME/System"
mkdir -p "$VOLUME/Wallpapers" "$VOLUME/Books" "$VOLUME/Audiobooks" "$DEST/READER/CACHE" "$DEST/APPS/DICT/DATA"

copy_example() {
  local src="$1"
  local dest="$2"
  if [[ -e "$dest" && "$FORCE" -ne 1 ]]; then
    echo "preserved-existing=$dest"
    return
  fi
  cp "$src" "$dest"
  echo "installed=$dest"
}

copy_example "$SRC/System/WIFI.TXT.example" "$DEST/WIFI.TXT"
copy_example "$SRC/System/DISPLAY.TXT.example" "$DEST/DISPLAY.TXT"
copy_example "$SRC/Wallpapers/SLEEP.BMP" "$VOLUME/Wallpapers/SLEEP.BMP"
copy_example "$SRC/Wallpapers/SLEEP01.BMP" "$VOLUME/Wallpapers/SLEEP01.BMP"
copy_example "$SRC/Books/README.TXT.example" "$VOLUME/Books/README.TXT"
copy_example "$SRC/Audiobooks/README.TXT.example" "$VOLUME/Audiobooks/README.TXT"
copy_example "$SRC/System/READER/PREFS.TXT.example" "$DEST/READER/PREFS.TXT"

echo "rustmix-wave-sd-examples-ready=$VOLUME"

DICT_DEST="$DEST/APPS/DICT"
if find "$DICT_DEST" -mindepth 1 -print -quit | grep -q .; then
  echo "preserved-existing-dictionary-pack=$DICT_DEST"
  echo "dictionary-smoke-pack-install=skipped-existing-use-dedicated-installer"
else
  copy_example "$SRC/System/APPS/DICT/INDEX.TXT" "$DICT_DEST/INDEX.TXT"
  copy_example "$SRC/System/APPS/DICT/README.TXT" "$DICT_DEST/README.TXT"
  copy_example "$SRC/System/APPS/DICT/DATA/AA.JSN" "$DICT_DEST/DATA/AA.JSN"
  copy_example "$SRC/System/APPS/DICT/DATA/AB.JSN" "$DICT_DEST/DATA/AB.JSN"
  copy_example "$SRC/System/APPS/DICT/DATA/CA.JSN" "$DICT_DEST/DATA/CA.JSN"
  echo "rustmix-wave-dictionary-sd-smoke-pack-ready=$DICT_DEST"
fi
echo "dictionary-complete-pack-helper=$ROOT/scripts/install-dictionary-x4-pack.sh"

