#!/usr/bin/env bash
set -euo pipefail

# Flash exactly like scripts/flash.sh and keep a copy of everything the
# serial monitor prints in dist/monitor.log (dist/ is ignored by git), so a
# diagnostic run can be read back as a file instead of from the terminal.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# A monitor left open by an earlier run still holds the serial port, and the
# flash would stop at "Failed to open serial port". Close it first (Windows
# only; elsewhere there is no taskkill and nothing happens).
if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe //F //IM espflash.exe >/dev/null 2>&1 || true
  sleep 1
fi

mkdir -p dist
./scripts/flash.sh "$@" 2>&1 | tee dist/monitor.log
