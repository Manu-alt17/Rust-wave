#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PORT=""
case "$#" in
  0)
    ;;
  1)
    if [[ "$1" != "monitor" ]]; then
      PORT="$1"
    fi
    ;;
  2)
    if [[ "$1" != "--port" ]]; then
      echo "usage: $0 [monitor|PORT|--port PORT]" >&2
      exit 1
    fi
    PORT="$2"
    ;;
  *)
    echo "usage: $0 [monitor|PORT|--port PORT]" >&2
    exit 1
    ;;
esac

BIN="target/xtensa-esp32s3-espidf/release/waveshare-epd397-rust-app"

# Known embuild/esp-idf-sys quirk on this toolchain: the custom
# partitions.csv (CONFIG_PARTITION_TABLE_CUSTOM_FILENAME) is sometimes not
# copied into the generated CMake project directory for a release-profile
# build, so `ninja` fails with "partitions.csv ... missing and no known
# rule to make it" even though the exact same source tree builds fine in
# debug. If the first attempt fails, drop our own copy into every pending
# esp-idf-sys OUT_DIR that's missing one and retry once before giving up.
if ! cargo +esp build --release; then
  echo 'release-build=retrying reason=partitions-csv-workaround'
  healed=0
  for out_dir in target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out; do
    if [[ -d "$out_dir" && ! -f "$out_dir/partitions.csv" ]]; then
      cp partitions.csv "$out_dir/partitions.csv"
      healed=1
    fi
  done
  if [[ "$healed" -eq 0 ]]; then
    echo 'release-build=failed error=cargo-build-failed-no-partitions-csv-gap-found' >&2
    exit 1
  fi
  cargo +esp build --release
fi

ARGS=(flash --chip esp32s3)
if [[ -n "$PORT" ]]; then
  ARGS+=(--port "$PORT")
fi

# The bootloader built from sdkconfig.defaults (warnings-only log, no image
# check at power-on, see there): without --bootloader espflash writes its
# own, built with ESP-IDF's defaults.
BOOTLOADER="$(ls -t target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin 2>/dev/null | head -n1 || true)"
if [[ -n "$BOOTLOADER" ]]; then
  ARGS+=(--bootloader "$BOOTLOADER")
else
  echo 'flash=warning bootloader=espflash-default reason=esp-idf-bootloader-not-found' >&2
fi

# Reset otadata so the bootloader falls back to booting ota_0 -- the slot
# espflash always writes to on this partition table (no "factory" partition,
# see partitions.csv). Without this, a device that has ever completed an OTA
# update keeps booting whatever slot otadata points at, silently ignoring a
# fresh USB flash into ota_0.
exec espflash "${ARGS[@]}" --partition-table partitions.csv --erase-parts otadata --monitor "$BIN"
