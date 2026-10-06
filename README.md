# Rustmix Wave for Waveshare ESP32-S3 E-Paper 3.97

Rustmix Wave is a Rust / ESP-IDF firmware that turns the Waveshare ESP32-S3 3.97-inch e-paper board into an e-reader and audiobook player. The native panel is `800 × 480`; the interface renders on a logical `480 × 800` portrait canvas.

This branch (`feature/merge-eink`) merges Rustmix Wave v1.4.8 with the E-ink firmware, a C / ESP-IDF e-reader for the same board: the reading-focused parts of Rustmix Wave stay, the rest was removed, and the E-ink firmware contributed audiobooks, the microSD as a USB disk, and a number of fixes found on its hardware. Firmware version: **1.5.0-beta.4** (`Cargo.toml`). It is published as the `beta` branch: the update channels in `docs/RELEASE.md` explain how beta and stable releases reach the devices.

A screen-by-screen guide is in [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md), with images under [`screenshots/`](screenshots/).

## Features

- **Reader** for TXT and reflowable EPUB: covers as first page, inline images, table of contents, bookmarks, per-book resume, Literata or Atkinson Hyperlegible in four sizes with Latin-1 and typographic characters, justification, landscape, high contrast, and word lookup in an offline dictionary pack.
- **Library** as a grid of covers, prepared once per book; progressive JPEG covers included, through a luma-only decoder that fits in this board's memory.
- **Audiobooks**: MP3 files or folders of tracks from the microSD, played on the board's speaker, with pause, 30 s skips, track changes, volume, and the position of every book saved.
- **Reading statistics**: time by week or by month, day by day against a time axis, going back through the earlier ones; streak, speed, time left in the book.
- **Wi-Fi transfer** from a browser, protected configuration files, and Wi-Fi setup from a phone through the device's own hotspot.
- **Connect to PC**: the microSD as a USB disk, to copy books and audiobooks with a cable.
- **Standby** through the AXP2101 PMIC, with a sleep image or the current book's cover on the glass, automatic after an idle time you choose.
- Italian interface and Europe/Rome time by default, English available.

Removed from Rustmix Wave on this branch: weather, games and Lua apps, BLE remote, calendar, Magic tokens, alarms, environment and motion sensors (tap page-turn included), voice notes, unit converter, the Dictionary app (the in-reader lookup stays) and most fonts.

## Hardware target

| Component | Contract |
| --- | --- |
| MCU | ESP32-S3R8: 240 MHz dual core, 8 MB octal PSRAM, 16 MB flash |
| Display | Waveshare 3.97-inch SSD1677 e-paper, native `800 × 480` |
| Display SPI | SCLK GPIO11, MOSI GPIO12, CS GPIO10, DC GPIO9, RST GPIO46, BUSY GPIO3 |
| Storage | microSD on SDMMC 4-bit at 20 MHz, FAT, mounted at `/sdcard` |
| Input | Rocker up / down / press, BOOT (GPIO0) as Back, Power key through the AXP2101 |
| Audio | ES8311 codec and NS4150B amplifier on I2S, speaker header |
| USB | Native USB: serial / JTAG normally, mass storage in Connect to PC |

See [`docs/BOARD_CONTRACT.md`](docs/BOARD_CONTRACT.md) for the stable board boundary.

## microSD layout

```text
/sdcard/RUSTMIX/
  BOOKS/        books: .txt and .epub
  AUDIO/        audiobooks: one .mp3, or a folder of .mp3 tracks per book
  SLEEP/        sleep images: 800 × 480 1-bpp BMP
  APPS/DICT/    dictionary pack for the in-reader lookup (optional)
  WIFI.TXT      saved Wi-Fi networks (the phone setup writes it)
  READER/ STATS/ ...   kept by the firmware (READER/CACHE holds covers and caches)
```

See [`docs/SD_CARD_SETUP.md`](docs/SD_CARD_SETUP.md) for every file.

## Build

The firmware builds with the `esp` Rust toolchain (selected by [`rust-toolchain.toml`](rust-toolchain.toml)) and ESP-IDF 5.5, which `esp-idf-sys` downloads on the first build. The ESP Rust toolchain ships for Linux, macOS and Windows with MSVC; on Windows without MSVC, build inside WSL.

```bash
./scripts/validate.sh        # format check, source contract, host tests
./scripts/build.sh           # validated release build
cargo +esp build --release   # the same build, without validation
./scripts/flash.sh           # build, flash and monitor (espflash)
```

Host tests run on the native target with stable Rust: `./scripts/test-host.sh`.

A development bench build keeps the board awake with a full-speed CPU, for flashing and logging without touching it: build with `RUSTMIX_DEV_BENCH=1`. Never ship one.

Release builds log warnings and errors only; bench builds log at INFO.

## Releases

`./scripts/build-release-firmware.sh` validates the source, builds the release ELF and packs it under `dist/`; `./scripts/flash-release.sh` flashes that ELF. Do not use `espflash write-bin`: it is a raw-address operation, and a merged factory image is deferred. See [`docs/RELEASE.md`](docs/RELEASE.md).

The OTA update checks the GitHub releases of `build_info::OTA_REPO_OWNER` / `OTA_REPO_NAME` only when asked, from Settings → Update.

## Documentation

- [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md): screens and controls
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): module boundaries and runtime ownership
- [`docs/BOARD_CONTRACT.md`](docs/BOARD_CONTRACT.md): pins and hardware behavior
- [`docs/SD_CARD_SETUP.md`](docs/SD_CARD_SETUP.md): microSD paths and installers
- [`docs/PHYSICAL_SMOKE_TEST.md`](docs/PHYSICAL_SMOKE_TEST.md): hardware verification
- [`docs/RELEASE.md`](docs/RELEASE.md): source and firmware releases
- [`docs/KNOWN_ISSUES.md`](docs/KNOWN_ISSUES.md): known limits and deferred work
- [`CHANGELOG.md`](CHANGELOG.md): history

## Credits and license

Rustmix Wave is by Piyush Daiya; v1.4.8 is Manu-alt17's fork of it. The E-ink firmware contributed audiobooks, Connect to PC, the luma-only JPEG decoder, the silent panel wake and the microSD fixes.

MIT, see [`LICENSE`](LICENSE). Third-party components and fonts keep their own licenses: see [`docs/licenses/THIRD_PARTY_NOTICES.md`](docs/licenses/THIRD_PARTY_NOTICES.md) and [`docs/licenses/FONT_NOTICES.md`](docs/licenses/FONT_NOTICES.md).
