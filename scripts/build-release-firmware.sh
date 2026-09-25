#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

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
  echo 'release-firmware-validation=skipped'
fi

# Known embuild/esp-idf-sys quirk on this toolchain: the custom
# partitions.csv (CONFIG_PARTITION_TABLE_CUSTOM_FILENAME) is sometimes not
# copied into the generated CMake project directory for a release-profile
# build, so `ninja` fails with "partitions.csv ... missing and no known
# rule to make it" even though the exact same source tree builds fine in
# debug. If the first attempt fails, drop our own copy into every pending
# esp-idf-sys OUT_DIR that's missing one and retry once before giving up.
if ! cargo +esp build --release; then
  echo 'release-firmware-build=retrying reason=partitions-csv-workaround'
  healed=0
  for out_dir in target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out; do
    if [[ -d "$out_dir" && ! -f "$out_dir/partitions.csv" ]]; then
      cp partitions.csv "$out_dir/partitions.csv"
      healed=1
    fi
  done
  if [[ "$healed" -eq 0 ]]; then
    echo 'release-firmware-build=failed error=cargo-build-failed-no-partitions-csv-gap-found' >&2
    exit 1
  fi
  cargo +esp build --release
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
  echo 'release-firmware-build=failed error=unable-to-determine-version' >&2
  exit 1
fi

ELF_SOURCE="target/xtensa-esp32s3-espidf/release/waveshare-epd397-rust-app"
if [[ ! -f "$ELF_SOURCE" ]]; then
  echo "release-firmware-build=failed error=missing-release-elf path=$ELF_SOURCE" >&2
  exit 1
fi

mkdir -p dist
PREFIX="dist/waveshare-epd397-rust-app-v${VERSION}"
ELF_OUT="${PREFIX}.elf"
FLASH_HELPER_OUT="${PREFIX}-flash-release.sh"
CHECKSUM_OUT="${PREFIX}-firmware-release.sha256"
FLASHING_OUT="${PREFIX}-FLASHING.txt"
ZIP_OUT="${PREFIX}-firmware-release.zip"

# Remove unsupported legacy raw-address artifacts from earlier release attempts.
rm -f dist/waveshare-epd397-rust-app-v*-flash.bin

cp "$ELF_SOURCE" "$ELF_OUT"
cp scripts/flash-release.sh "$FLASH_HELPER_OUT"
chmod +x "$FLASH_HELPER_OUT"

cat > "$FLASHING_OUT" <<TXT
Rustmix Wave firmware release v${VERSION}

Supported release flashing path (ELF-aware):

  ./$(basename "$FLASH_HELPER_OUT") $(basename "$ELF_OUT")

Equivalent direct command:

  espflash flash --chip esp32s3 --erase-parts otadata --monitor $(basename "$ELF_OUT")

Development flashing with monitor remains available from the source tree:

  ./scripts/flash.sh monitor

SAFETY WARNING
--------------
Do not flash this release with espflash write-bin.
The write-bin command is a raw-address operation. This release bundle intentionally
ships no *-flash.bin artifact and does not define a supported raw address layout.

A merged factory-image workflow may be added later only after the bootloader,
partition-table, and application offsets have been validated on physical hardware.
TXT

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$@"
  else
    sha256sum "$@"
  fi
}

sha256_file "$ELF_OUT" "$FLASH_HELPER_OUT" "$FLASHING_OUT" > "$CHECKSUM_OUT"
rm -f "$ZIP_OUT"
zip -jq "$ZIP_OUT" "$ELF_OUT" "$FLASH_HELPER_OUT" "$CHECKSUM_OUT" "$FLASHING_OUT"

echo "release-firmware-elf=$ELF_OUT"
echo "release-firmware-flash-helper=$FLASH_HELPER_OUT"
echo "release-firmware-checksums=$CHECKSUM_OUT"
echo "release-firmware-flashing=$FLASHING_OUT"
echo "release-firmware-zip=$ZIP_OUT"
echo 'release-firmware-format=elf-only'
echo 'release-firmware-build=ok'
