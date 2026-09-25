#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Produces the standalone OTA application image consumed by the device's own
# GitHub-release update flow (see src/ota.rs). This is intentionally separate
# from scripts/build-release-firmware.sh, which stays ELF-only per
# docs/RELEASE.md and docs/KNOWN_ISSUES.md.
#
# `espflash save-image` WITHOUT `--merge` is not the raw-address merged
# factory image those docs warn against: it carries no fixed flash address at
# all. The device picks the OTA slot to write it to at runtime via
# esp_ota_get_next_update_partition(); nothing about this file's layout is
# guessed or hardcoded by a human running espflash, so the "wrong address
# bricks the device" risk that ban exists for does not apply here.

SKIP_VALIDATE=0
if [[ "${1:-}" == "--skip-validate" ]]; then
  SKIP_VALIDATE=1
  shift
fi
if [[ "$#" -ne 0 ]]; then
  echo "usage: $0 [--skip-validate]" >&2
  exit 1
fi

if [[ "$SKIP_VALIDATE" -eq 0 ]]; then
  ./scripts/validate.sh
else
  echo 'release-ota-image-validation=skipped'
fi

if ! command -v espflash >/dev/null 2>&1; then
  echo 'release-ota-image-build=failed error=espflash-not-found' >&2
  exit 1
fi

# Known embuild/esp-idf-sys quirk on this toolchain: the custom
# partitions.csv (CONFIG_PARTITION_TABLE_CUSTOM_FILENAME) is sometimes not
# copied into the generated CMake project directory for a release-profile
# build, so `ninja` fails with "partitions.csv ... missing and no known
# rule to make it" even though the exact same source tree builds fine in
# debug. If the first attempt fails, drop our own copy into every pending
# esp-idf-sys OUT_DIR that's missing one and retry once before giving up.
if ! cargo +esp build --release; then
  echo 'release-ota-image-build=retrying reason=partitions-csv-workaround'
  healed=0
  for out_dir in target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out; do
    if [[ -d "$out_dir" && ! -f "$out_dir/partitions.csv" ]]; then
      cp partitions.csv "$out_dir/partitions.csv"
      healed=1
    fi
  done
  if [[ "$healed" -eq 0 ]]; then
    echo 'release-ota-image-build=failed error=cargo-build-failed-no-partitions-csv-gap-found' >&2
    exit 1
  fi
  cargo +esp build --release
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
  echo 'release-ota-image-build=failed error=unable-to-determine-version' >&2
  exit 1
fi

ELF_SOURCE="target/xtensa-esp32s3-espidf/release/waveshare-epd397-rust-app"
if [[ ! -f "$ELF_SOURCE" ]]; then
  echo "release-ota-image-build=failed error=missing-release-elf path=$ELF_SOURCE" >&2
  exit 1
fi

mkdir -p dist
PREFIX="dist/waveshare-epd397-rust-app-v${VERSION}"
OTA_BIN_OUT="${PREFIX}-ota-update.bin"
CHECKSUM_OUT="${PREFIX}-ota-update.sha256"

espflash save-image --chip esp32s3 "$ELF_SOURCE" "$OTA_BIN_OUT"

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$@"
  else
    sha256sum "$@"
  fi
}
sha256_file "$OTA_BIN_OUT" > "$CHECKSUM_OUT"

cat <<TXT

Attach this file to the GitHub release tagged for v${VERSION} on the repo
configured in src/build_info.rs (OTA_REPO_OWNER/OTA_REPO_NAME). The device
finds it by asset name ending in ".bin" -- the exact filename otherwise
does not matter.

Do not flash $(basename "$OTA_BIN_OUT") over USB with espflash write-bin or
save-image --merge. It carries no bootloader/partition-table/flash-address
data and is only ever written through the device's own OTA update path.
Use scripts/build-release-firmware.sh + scripts/flash-release.sh for USB
flashing instead.
TXT

echo "release-ota-image=$OTA_BIN_OUT"
echo "release-ota-image-checksum=$CHECKSUM_OUT"
echo 'release-ota-image-build=ok'
