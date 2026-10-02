# Changelog

## v1.5.0-beta.2 — Bootloader updates over the air

- Bootloader update over the air: a release can carry the bootloader as `*-bootloader.img`, which `scripts/build-ota-image.sh` now writes next to the app image. When the firmware is up to date and the release's bootloader differs from the one in flash, Settings → Update offers it: it is downloaded and checked first (the SHA-256 GitHub publishes, the image's own appended hash, chip and size), then written on a second SELECT with the battery at 50% or the USB cable, staged in unused flash, read back and copied again if it differs; the device restarts into it. The ESP32-S3 has no backup bootloader, so a power cut during the write (under a second) still needs a USB reflash.
- Update shows the installed bootloader (ESP-IDF version and build date), to tell this project's from espflash's generic one.
- A small SHA-256 (`src/sha256.rs`), checked against the FIPS 180-4 vectors.

## v1.5.0-beta.1 — E-ink merge (`beta`)

Rustmix Wave v1.4.8 merged with the E-ink firmware, a C / ESP-IDF e-reader for the same board. Each item below is one commit or a few. The first version of the `beta` branch: a pre-release version, so beta devices are never offered a stable 1.4.x release, which would replace this firmware.

Removed:

- Weather, the SD Lua app runtime and native games, Magic Tokens, the Rustmix Remote BLE build, the Calendar, RTC alarms, the environment and motion sensors (tap page-turn included; both chips are put to sleep at boot), the standalone Dictionary app (the in-reader lookup stays), Voice Notes, the Unit Converter and the Tools category (Files moves to Home).
- Most fonts: the interface uses Inter, books Literata or Atkinson Hyperlegible, all with Latin-1 and Windows-1252 typography.
- The automatic OTA check; updates run only from Settings → Update.

Added:

- Audiobooks: MP3 files or folders of tracks from `RUSTMIX/AUDIO`, played by a dedicated audio engine thread through libhelix, with pause, 30 s skips, track changes, volume and saved positions.
- Connect to PC: the microSD as a USB disk (`components/usbdisk` over esp_tinyusb), with the USB PHY reset at every boot and any format of the card refused.
- Long file names on the microSD.
- The automatic standby timeout as a Display setting.
- Progressive JPEG covers, through a luma-only decoder (`src/jpeg_luma.rs`, ported from E-ink).
- The title written on placeholder covers.
- Italian and Europe/Rome by default.
- Update channel in Settings → Update, as on `feature/ota-update`: Stable follows the latest release, Beta the highest version among the five most recent releases, pre-releases included, with SemVer pre-release ordering. Kept in `RUSTMIX/UPDATE.TXT`; a beta firmware follows Beta until it is changed.

Fixed:

- Wi-Fi portal: protected files are checked by their resolved path, so `./WIFI.TXT`, `WIFI.TXT/` or `%2557IFI.TXT` no longer reach them.
- EPUB: no silent truncation of long spines, manifests or archives; nested NCX entries and the EPUB3 `toc` nav only; named entities decoded.
- Covers are centre-cropped instead of stretched, and JPEGs that would not fit in memory are refused instead of aborting.
- A PNG cover could run the decoder out of memory at every boot (the Home Continue card decodes it), so the device restarted endlessly: PNG frames are reserved fallibly, and a decode that crashed the device is not tried again.
- PNGs are read row by row and reduced while read, so a large one never sits in memory whole; transparency is drawn on white.
- Panel: waking from the idle sleep no longer flashes; the frame on the glass is reloaded silently.
- Reading statistics count days in local time.
- SD logs move to `.OLD` at 64 KB; release builds log warnings only, from the bootloader on; the app descriptor version follows `Cargo.toml`.
- The fallback-sleep log line no longer says `wake=select-only` when the PMIC watchdog will power the board off.

Faster:

- SD at 20 MHz again, and large reads into PSRAM through an internal buffer (2.3 times faster).
- SD reads that start mid-sector go up to the sector boundary first: the SD driver read them one sector per command (a 1.46 MB cover: 4.7 s, now 1.3 s).
- The library visible set, sleep-image checks, the kept `.EPX` handle, and no Wi-Fi scan with a single saved network.

## v1.4.x — PMIC Power-Off Shutdown

- Replace the Reader's two smallest Book Font Size options (`Small`, `Medium`) with two new larger tiers above the old `XLarge` ceiling; the size picker now reads `Little` / `Medium` / `Large` / `XLarge` on screen (internally still the `Large` / `XLarge` / `XXLarge` / `XXXLarge` variants and persisted markers, to keep old preference files loading correctly), with the internal `XLarge` tier (on-screen "Medium") as the new default. Adds matching generated bitmap strikes (Atkinson Hyperlegible Next Medium, DejaVu Serif, Literata Medium) and recalibrated `lines_per_page` pagination for both new sizes. Saved preference files from older firmware that still say `small` or `medium` load as the smallest tier instead of failing to parse.
- Replace MCU deep sleep with a real AXP2101 PMIC software power-off as the primary Power short-press shutdown path; real MCU deep sleep is now the automatic fallback if the PMIC power-off cannot be armed, and the pre-existing software-only sleep loop remains the last-resort fallback.
- Add `Axp2101::power_off`, `write_shutdown_marker`, and `take_shutdown_marker`, using an AXP2101 scratch register to flag a firmware-requested shutdown so the next boot can tell a PMIC Power-key wake apart from an ordinary power-on/reset.
- Add `BootCause::PmicPowerKeyOn` and `BootCause::is_sleep_resume()`; Reader auto-resume and other sleep-wake boot behavior now trigger identically for a PMIC wake and a real MCU deep-sleep GPIO wake.
- Add a post-boot guard that suppresses residual Power-key events in the first moments after boot, so the same press that turns the board back on can never immediately shut it back off.
- Finish the Power-key short-press/long-press swap (short press now shuts down, long press opens the display-maintenance menu) across code comments, logs, `scripts/validate_source_contract.sh`, and the documentation set below -- the runtime behavior had already changed; several docs and one validation assertion had not caught up.
- Update `README.md`, `docs/USER_GUIDE.md`, `docs/BOARD_CONTRACT.md`, `docs/KNOWN_ISSUES.md`, and `docs/PHYSICAL_SMOKE_TEST.md` for the new shutdown path.

## v1.0.0-r3 — Screenshot User Guide and Architecture Documentation

- Add `screenshots/` with the physically verified UI screenshot set.
- Add `docs/USER_GUIDE.md` with screen-by-screen navigation for Home, Reader, Productivity, Games, Tools, Settings, Wi-Fi transfer, and sleep-image mode.
- Expand `README.md` with sensor-driven utility coverage, motion-game behavior, and main-task worker-isolation policy.
- Expand `docs/ARCHITECTURE.md` with board-service ownership, the native QMI8658 motion-event pipeline, Lua/native game boundaries, runtime memory telemetry, and named worker stack budgets.
- Preserve firmware runtime behavior and the ELF-only release workflow.

## v1.0.0-r2 — Text Editor Layout Alignment

- Move the Voice Notes friendly-title editor onto the shared grid keyboard with BOOT-short NAV H / NAV V toggling.
- Give the Voice Notes title editor its own header, width-safe status strip, keyboard SAVE/CANCEL actions, and long-BOOT cancel/back behavior.
- Compact the Calendar personal-event editor status date and footer so NAV H / NAV V and instructions remain readable on e-paper.
- Preserve the ELF-only release flash workflow and all accepted runtime paths.

## v1.0.0-r1 — Release Flash Workflow Safety Repair

- Remove the unsafe raw-address `espflash write-bin ... 0x0` release instructions and the unverified `*-flash.bin` artifact.
- Publish the ESP-IDF ELF as the supported firmware release artifact.
- Add `scripts/flash-release.sh` to flash an existing release ELF through `espflash flash --chip esp32s3 --monitor`.
- Preserve the ordinary `./scripts/flash.sh monitor` development path.
- Defer any merged factory-image workflow until bootloader, partition-table, and application offsets are validated on physical hardware.

## v1.0.0 — First Stable Rustmix Wave Release

- Promote the physically accepted Rustmix Wave firmware baseline to the first stable release.
- Preserve Reader, Voice Notes, Dictionary, Calendar, Wi-Fi transfer, RTC alarms, weather, audio, sensors, sleep-image mode, Power-key maintenance menu, Lua apps, and motion games.
- Preserve short Power press for manual ghost-clearing maintenance and long Power press for sleep-image mode.

## v0.20.5 — Repository Cleanup, Consolidated Documentation, CI, and Release Binary Builder

- Remove extracted patch-overlay folders, generated archives, patch scripts, repair documents, milestone smoke-test documents, and cache artifacts from the source tree.
- Consolidate durable project documentation into `README.md`, `docs/ARCHITECTURE.md`, `docs/BOARD_CONTRACT.md`, `docs/SD_CARD_SETUP.md`, `docs/PHYSICAL_SMOKE_TEST.md`, `docs/KNOWN_ISSUES.md`, and `docs/RELEASE.md`.
- Replace the stale GitHub workflow with `.github/workflows/ci.yml`, which checks stable formatting, the cleaned source contract, shell syntax, and native-target host tests.
- Add `scripts/build-release-firmware.sh` to produce release artifacts. The later v1.0.0-r1 safety repair restricts supported distribution to the ELF-aware flashing path.
- Tighten `scripts/package-release.sh` so generated source archives exclude overlay directories and local artifacts.
- Preserve the physically accepted runtime: Reader, Voice Notes, Dictionary, Calendar, Power-key maintenance menu and long-press sleep, Wi-Fi transfer, alarms, weather, audio, sensors, Lua apps, and motion games.

## Accepted runtime baseline before cleanup

- v0.20.4: short Power press display-maintenance menu and long Power press sleep-image mode.
- v0.20.3: Calendar personal-event editor with U.S. holidays read-only and atomic `EVENTS.TMP -> EVENTS.TXT` persistence with `EVENTS.BAK` rollback.
- v0.20.1: Calendar U.S. events, month markers, daily agenda, and details.
- v0.20.0: native X4-pack Dictionary with exact, prefix, wildcard, and BOOT-short `NAV H` / `NAV V` keyboard navigation.
- v0.19.x: Voice Notes recording, gain, pause/resume, playback, metadata, titles, storage telemetry, delete confirmation, and LAN export.
- v0.18.x: explicit Wi-Fi transfer portal, bounded Lua app foundation, native game bridges, and IMU motion games.
- v0.17.x: bounded reflowable EPUB Reader foundation and TOC navigation.
- v0.16.x: TXT Reader persistence, bookmarks, preferences, FAT 8.3 runtime names, and Library bookmark alignment.
