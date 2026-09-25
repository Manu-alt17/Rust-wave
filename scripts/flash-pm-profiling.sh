#!/usr/bin/env bash
# Diagnostic flash: identical to flash.sh, but builds the firmware with the
# PM-profiling overlay (sdkconfig.defaults.pm-profiling) and refuses to
# flash unless the resulting binary really carries the profiling telemetry.
# Takes the same arguments as flash.sh. A later plain ./scripts/flash.sh
# rebuilds the normal firmware again on its own.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults;sdkconfig.defaults.pm-profiling"
BIN="target/xtensa-esp32s3-espidf/release/waveshare-epd397-rust-app"

# Build here first (same partitions.csv retry as flash.sh) so the binary can
# be checked before anything is written to the device.
if ! cargo +esp build --release; then
  echo 'pm-profiling-build=retrying reason=partitions-csv-workaround'
  for out_dir in target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out; do
    if [[ -d "$out_dir" && ! -f "$out_dir/partitions.csv" ]]; then
      cp partitions.csv "$out_dir/partitions.csv"
    fi
  done
  cargo +esp build --release
fi

if ! grep -q 'pm-profile status=enabled' "$BIN"; then
  echo 'pm-profiling-build=failed error=binary-lacks-pm-profile-telemetry' >&2
  exit 1
fi
echo 'pm-profiling-build=ok'

# flash.sh's own build step is now a no-op (same environment, same inputs).
exec "$ROOT/scripts/flash.sh" "$@"
