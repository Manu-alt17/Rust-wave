#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

failed=0
check() {
  local label="$1"
  shift
  if "$@"; then
    printf '%s=ok\n' "$label"
  else
    printf '%s=failed\n' "$label" >&2
    failed=1
  fi
}
contains() {
  local path="$1"
  local pattern="$2"
  grep -Fq -- "$pattern" "$path"
}

# One version everywhere: Cargo.toml, Cargo.lock and the ESP-IDF app
# descriptor (a unit test in src/build_info.rs checks the last pair too).
version_contract() {
  python3 - <<'PY'
from pathlib import Path
import re

cargo = Path('Cargo.toml').read_text(encoding='utf-8')
version = re.search(r'^version = "([^"]+)"$', cargo, re.M).group(1)
lock = Path('Cargo.lock').read_text(encoding='utf-8')
entry = re.search(r'name = "waveshare-epd397-rust-app"\nversion = "([^"]+)"', lock)
assert entry and entry.group(1) == version, f'Cargo.lock version differs from {version}'
sdkconfig = Path('sdkconfig.defaults').read_text(encoding='utf-8')
assert f'CONFIG_APP_PROJECT_VER="{version}"' in sdkconfig, f'sdkconfig.defaults app version differs from {version}'
PY
}

clean_repository_contract() {
  python3 - <<'PY'
from pathlib import Path

root = Path('.')

# Extracted patch overlays, generated archives, and cache files are not source.
for child in root.iterdir():
    if child.is_dir() and child.name.startswith('waveshare-epd397-rust-'):
        raise AssertionError(f'extracted overlay directory present: {child}')
ignored_generated_roots = {'.git', 'target', '.embuild', 'dist'}
for path in root.rglob('*'):
    if any(part in ignored_generated_roots for part in path.parts):
        continue
    if path.is_file() and (path.suffix in {'.zip', '.sha256', '.pyc', '.orig', '.rej'} or path.name == '.DS_Store'):
        raise AssertionError(f'local artifact present: {path}')
    if path.is_dir() and path.name == '__pycache__':
        raise AssertionError(f'python cache directory present: {path}')

# Durable documentation is intentionally small and consolidated.
expected = {
    'ARCHITECTURE.md',
    'BOARD_CONTRACT.md',
    'KNOWN_ISSUES.md',
    'PHYSICAL_SMOKE_TEST.md',
    'RELEASE.md',
    'SD_CARD_SETUP.md',
    'USER_GUIDE.md',
}
actual = {p.name for p in Path('docs').iterdir() if p.is_file()}
assert actual == expected, f'durable docs mismatch: actual={sorted(actual)} expected={sorted(expected)}'

readme = Path('README.md').read_text(encoding='utf-8')
arch = Path('docs/ARCHITECTURE.md').read_text(encoding='utf-8')
for fragment in (
    'docs/USER_GUIDE.md',
    'screenshots/',
    'scripts/build-release-firmware.sh',
    'scripts/flash-release.sh',
    'RUSTMIX_DEV_BENCH',
    'THIRD_PARTY_NOTICES.md',
):
    assert fragment in readme, f'README missing: {fragment}'
for fragment in (
    'Design rules',
    'Display and refresh ownership',
    'Power key and standby',
    'Reader',
    'Images',
    'Audio engine',
    'Connect to PC',
    'Wi-Fi transfer and network',
    'Main task and workers',
    'Storage',
    'Validation',
):
    assert fragment in arch, f'architecture missing: {fragment}'

# Documentation of removed features must not come back.
for path in [Path('README.md'), *Path('docs').glob('*.md')]:
    text = path.read_text(encoding='utf-8')
    for removed in ('Voice Notes boundary', 'Calendar boundary', 'Games and Lua boundary', 'Native IMU event pipeline', 'RRBP'):
        assert removed not in text, f'{path} still documents removed feature: {removed}'
PY
}

screenshot_user_guide_contract() {
  python3 - <<'PY'
from pathlib import Path
import re

screenshots = Path('screenshots')
guide = Path('docs/USER_GUIDE.md').read_text(encoding='utf-8')
assert (screenshots / 'README.md').is_file(), 'screenshots/README.md missing'

images = {path.name for path in screenshots.iterdir() if path.is_file() and path.name != 'README.md'}
referenced = set(re.findall(r'\.\./screenshots/([A-Za-z0-9_.-]+\.(?:jpg|png))', guide))
assert images == referenced, f'screenshots and user guide disagree: unreferenced={sorted(images - referenced)} missing={sorted(referenced - images)}'
for fragment in (
    '# Rustmix Wave user guide',
    '## Physical controls',
    '## 1. Home',
    '## 2. Library',
    '## 3. Reader',
    '## 4. Audiobooks',
    '## 9. Connect to PC',
    '## 11. Screenshot index',
):
    assert fragment in guide, f'user guide missing: {fragment}'
PY
}

ci_workflow_contract() {
  python3 - <<'PY'
from pathlib import Path
workflow = Path('.github/workflows/ci.yml').read_text(encoding='utf-8')
assert not Path('.github/workflows/source-contract.yml').exists()
for fragment in (
    'actions/checkout@v4',
    'dtolnay/rust-toolchain@stable',
    'components: rustfmt',
    'bash -n scripts/*.sh',
    'cargo +stable fmt --all -- --check',
    './scripts/validate_source_contract.sh',
    './scripts/test-host.sh',
    './scripts/test-release-flash-workflow.sh',
    'workflow_dispatch:',
):
    assert fragment in workflow, f'workflow missing: {fragment}'
PY
}

release_binary_builder_contract() {
  python3 - <<'PY'
from pathlib import Path
builder = Path('scripts/build-release-firmware.sh').read_text(encoding='utf-8')
flasher = Path('scripts/flash-release.sh').read_text(encoding='utf-8')
release_doc = Path('docs/RELEASE.md').read_text(encoding='utf-8')
readme = Path('README.md').read_text(encoding='utf-8')
known = Path('docs/KNOWN_ISSUES.md').read_text(encoding='utf-8')

for fragment in (
    './scripts/validate.sh',
    'cargo +esp build --release',
    'target/xtensa-esp32s3-espidf/release/waveshare-epd397-rust-app',
    '-flash-release.sh',
    '-firmware-release.sha256',
    '-firmware-release.zip',
    "echo 'release-firmware-format=elf-only'",
    "echo 'release-firmware-build=ok'",
):
    assert fragment in builder, f'ELF release builder missing: {fragment}'
for unsafe in (
    'espflash save-image',
    'espflash write-bin --chip esp32s3 0x0',
    'BIN_OUT=',
    'release-firmware-bin=',
):
    assert unsafe not in builder, f'unsafe release builder fragment present: {unsafe}'
assert 'rm -f dist/waveshare-epd397-rust-app-v*-flash.bin' in builder

for fragment in (
    'espflash flash --chip esp32s3',
    '--monitor "$ELF"',
    '--port "$PORT"',
    'release-flash=failed error=missing-release-elf',
):
    assert fragment in flasher, f'release flash helper missing: {fragment}'
assert 'write-bin' not in flasher, 'release flash helper must not use raw writes'

for content, label in ((release_doc, 'release doc'), (readme, 'README'), (known, 'known issues')):
    assert 'espflash write-bin' in content, f'{label} missing raw-address warning'
    assert 'raw-address' in content.lower(), f'{label} missing raw-address explanation'
    assert 'factory' in content.lower(), f'{label} missing deferred factory-image note'
assert 'espflash write-bin --chip esp32s3 0x0' not in release_doc
assert '*-flash.bin' in release_doc and 'No `*-flash.bin` artifact is generated.' in release_doc

# The cleaned source must not carry an unverified legacy raw-address artifact.
assert not list(Path('dist').glob('*-flash.bin')), 'legacy dist/*-flash.bin artifact present'
PY
}

package_release_contract() {
  python3 - <<'PY'
from pathlib import Path
script = Path('scripts/package-release.sh').read_text(encoding='utf-8')
for fragment in (
    './scripts/validate.sh',
    "--exclude 'dist/'",
    "--exclude '*.zip'",
    "--exclude '*.sha256'",
    "--exclude 'waveshare-epd397-rust-*-repair-*/'",
    "--exclude 'waveshare-epd397-rust-*-v*/'",
    'release-source-zip=',
):
    assert fragment in script, f'source packager missing: {fragment}'
PY
}

host_test_native_target_contract() {
  python3 - <<'PY'
from pathlib import Path
script = Path('scripts/test-host.sh').read_text(encoding='utf-8')
for fragment in (
    'HOST_TRIPLE="$(rustc +stable -vV',
    "sed -n 's/^host: //p'",
    'cargo +stable test --target "$HOST_TRIPLE" --lib',
    "echo 'host-test-native-target-isolation=ok'",
):
    assert fragment in script, f'host test helper missing: {fragment}'
PY
}

runtime_contract() {
  python3 - <<'PY'
from pathlib import Path

lib = Path('src/lib.rs').read_text(encoding='utf-8')
main = Path('src/main.rs').read_text(encoding='utf-8')
state = Path('src/app/state.rs').read_text(encoding='utf-8')
power = Path('src/power_key.rs').read_text(encoding='utf-8')
wifi = Path('src/wifi_transfer.rs').read_text(encoding='utf-8')

for module in (
    'reader', 'epub', 'dictionary', 'reading_stats', 'cover_cache', 'jpeg_luma',
    'audio', 'audiobook', 'usb_disk', 'wifi_transfer', 'power_key', 'power_key_menu',
    'sleep_mode', 'sleep_images', 'sleep_network', 'sd_io', 'sd_log',
):
    assert f'pub mod {module};' in lib, f'library module missing: {module}'
for removed in (
    'calendar', 'voice_notes', 'voice_note_metadata', 'keyboard_navigation', 'alarm',
    'lua_runtime', 'games', 'weather', 'weather_config', 'unit_converter', 'imu_events',
    'imu_tap_diagnostics', 'magic_tokens', 'rtc_alarm_interrupt', 'rustmix_remote',
):
    assert f'pub mod {removed};' not in lib, f'removed module is back: {removed}'
    assert not Path(f'src/{removed}.rs').exists() and not Path(f'src/{removed}').exists(), f'removed module source is back: {removed}'

# Power key: short press standby, long press display-maintenance menu.
assert 'pub fn power_key_sleep_restore_route(&self) -> ScreenRoute {' in state
assert 'state.open_power_key_menu();' in main
assert 'event == PowerKeyEvent::LongPress' in main
assert 'power_key_clear_ghost' in main
for fragment in ('power_key_event_from_irq_status', 'POWER_KEY_LONG_PRESS_MASK', 'POWER_KEY_SHORT_PRESS_MASK'):
    assert fragment in power, f'power key decoder missing: {fragment}'

# Connect to PC: the PHY is released first thing at boot, and no format ever.
assert 'usb_disk::espidf::release_phy();' in main
assert '-Wl,--wrap=f_mkfs' in Path('components/usbdisk/CMakeLists.txt').read_text(encoding='utf-8')

# The in-reader lookup reuses the bounded X4 dictionary pack.
dictionary = Path('src/dictionary.rs').read_text(encoding='utf-8')
for fragment in (
    'DICTIONARY_ROOT: &str = "/sdcard/RUSTMIX/APPS/DICT"',
    'DICTIONARY_INDEX_FILE: &str = "INDEX.TXT"',
    'DICTIONARY_SHARD_MAX_BYTES: usize = 16 * 1024',
):
    assert fragment in dictionary, f'dictionary contract missing: {fragment}'

# Wi-Fi portal protects the configuration files by their resolved path.
for protected in ('WIFI.TXT', 'CLOCK.TXT', 'DISPLAY.TXT'):
    assert f'"{protected}"' in wifi, f'protected portal path missing: {protected}'
assert 'fn resolve_portal_path' in wifi
PY
}

sd_examples_contract() {
  python3 - <<'PY'
from pathlib import Path
required = (
    'examples/sd-card/RUSTMIX/WIFI.TXT.example',
    'examples/sd-card/RUSTMIX/DISPLAY.TXT.example',
    'examples/sd-card/RUSTMIX/READER/PREFS.TXT.example',
    'examples/sd-card/RUSTMIX/BOOKS/README.TXT.example',
    'examples/sd-card/RUSTMIX/AUDIO/README.TXT.example',
    'examples/sd-card/RUSTMIX/SLEEP/SLEEP.BMP',
    'examples/sd-card/RUSTMIX/APPS/DICT/INDEX.TXT',
    'examples/sd-card/RUSTMIX/APPS/DICT/DATA/AA.JSN',
)
for path in required:
    assert Path(path).is_file(), f'SD example missing: {path}'
for removed in ('APPS/DICT/MAIN.LUA', 'APPS/DICT/APP.TOM', 'APPS/CALENDAR', 'WEATHER.TXT.example', 'ALARMS.TXT.example'):
    assert not Path('examples/sd-card/RUSTMIX', removed).exists(), f'example of a removed feature present: {removed}'
PY
}

rust_lexical_delimiter_scan() {
  python3 - <<'PY'
from pathlib import Path

pairs = {'(': ')', '[': ']', '{': '}'}
closing = {v: k for k, v in pairs.items()}
for path in sorted(Path('src').rglob('*.rs')):
    text = path.read_text(encoding='utf-8')
    stack = []
    i = 0
    state = 'code'
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ''
        if state == 'code':
            # Raw strings (r"...", r#"..."#, br#"..."#) hold the portal's
            # HTML and JavaScript: skip them whole.
            prev = text[i - 1] if i else ''
            if ch == 'r' and (not (prev.isalnum() or prev == '_') or (prev == 'b' and not (text[i - 2: i - 1].isalnum() or text[i - 2: i - 1] == '_'))):
                j = i + 1
                while j < len(text) and text[j] == '#':
                    j += 1
                if j < len(text) and text[j] == '"':
                    hashes = j - i - 1
                    end = text.find('"' + '#' * hashes, j + 1)
                    if end != -1:
                        i = end + 1 + hashes; continue
            if ch == '/' and nxt == '/':
                state = 'line_comment'; i += 2; continue
            if ch == '/' and nxt == '*':
                state = 'block_comment'; i += 2; continue
            if ch == '"':
                state = 'string'; i += 1; continue
            if ch == "'":
                # Rust lifetimes are not character literals. Treat as a char only
                # when a closing quote is nearby.
                end = text.find("'", i + 1, min(len(text), i + 6))
                if end != -1:
                    i = end + 1; continue
            if ch in pairs:
                stack.append((ch, i))
            elif ch in closing:
                if not stack or stack[-1][0] != closing[ch]:
                    raise AssertionError(f'{path}: unmatched {ch} at byte {i}')
                stack.pop()
        elif state == 'line_comment':
            if ch == '\n': state = 'code'
        elif state == 'block_comment':
            if ch == '*' and nxt == '/': state = 'code'; i += 2; continue
        elif state == 'string':
            if ch == '\\': i += 2; continue
            if ch == '"': state = 'code'
        i += 1
    if stack:
        raise AssertionError(f'{path}: unclosed delimiters: {stack[-3:]}')
PY
}

check version-consistency version_contract
check cleaned-repository-contract clean_repository_contract
check screenshot-user-guide-contract screenshot_user_guide_contract
check ci-workflow-contract ci_workflow_contract
check release-elf-builder release_binary_builder_contract
check release-flash-workflow-selftest-script contains scripts/test-release-flash-workflow.sh 'release-flash-workflow-selftest=ok'
check package-release-contract package_release_contract
check host-test-native-target-isolation host_test_native_target_contract
check runtime-contract runtime_contract
check sd-examples-contract sd_examples_contract
check font-notice-inter contains docs/licenses/FONT_NOTICES.md 'Inter'
check font-notice-literata contains docs/licenses/FONT_NOTICES.md 'Literata'
check font-notice-atkinson contains docs/licenses/FONT_NOTICES.md 'Atkinson Hyperlegible Next'
check third-party-notice-helix contains docs/licenses/THIRD_PARTY_NOTICES.md 'esp-libhelix-mp3'
check third-party-notice-tinyusb contains docs/licenses/THIRD_PARTY_NOTICES.md 'esp_tinyusb'
check third-party-notice-jpeg contains docs/licenses/THIRD_PARTY_NOTICES.md 'esp_new_jpeg'
check no-raw-font-files bash -c '! find . -path ./target -prune -o -type f \( -iname "*.ttf" -o -iname "*.otf" -o -iname "*.woff" -o -iname "*.woff2" \) -print | grep -q .'
for script in scripts/*.sh; do
  check "bash-syntax-$(basename "$script")" bash -n "$script"
done
check rust-lexical-delimiter-scan rust_lexical_delimiter_scan

if [[ "$failed" -ne 0 ]]; then
  echo 'source-contract-validation=failed' >&2
  exit 1
fi

echo 'source-contract-validation=ok'
