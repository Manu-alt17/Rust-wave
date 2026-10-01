# Rustmix Wave architecture

Rustmix Wave is a host-testable Rust library plus a narrow ESP-IDF firmware integration layer. New functionality stays module-based: domain rules out of `main.rs`, hardware handles in native owners, and small explicit state transitions exposed to the UI.

## Design rules

1. `src/main.rs` owns ESP-IDF integration, peripheral handles, the event loop, logging and cross-domain coordination.
2. `src/lib.rs` exports host-testable modules.
3. `src/app/state.rs` owns UI state transitions and routes requests to the firmware loop.
4. `src/app/screens/` renders screens without owning hardware.
5. SD-backed features use bounded reads, FAT-safe names and `.TMP` / `.BAK` recovery where writes are allowed.
6. E-paper refreshes flow through the shared refresh coordinator rather than feature-specific panel writes.
7. Allocation failure aborts in Rust, so anything sized by a file (images, archives, text) checks its budget before allocating, or allocates through `try_reserve`.

## Runtime layers

```text
ESP-IDF event loop and native hardware ownership
  src/main.rs
        |
        +-- AppState requests and snapshots
        |     src/app/state.rs, src/app/router.rs
        |
        +-- Screen rendering
        |     src/app/screens/*, src/app/widgets/*, src/app/typography/*
        |
        +-- Domain modules
              reader.rs / epub.rs / dictionary.rs / reading_stats.rs
              cover_cache.rs / jpeg_luma.rs
              audiobook.rs / audio/*
              usb_disk.rs (components/usbdisk)
              wifi_transfer.rs / network*.rs / dns_captive_portal.rs / ota.rs
              power_key.rs / power_key_menu.rs / sleep_mode.rs / sleep_images.rs / sleep_cover.rs
              panel_refresh.rs / epaper.rs / storage.rs / sd_io.rs / sd_log.rs
```

## Board services

`src/board_services.rs` owns the protocol drivers on one Rust-owned I2C bus (`src/shared_i2c.rs`, an `Arc<Mutex>` so the audio engine thread can reach the codec). Each service initializes independently, so a missing chip cannot stop the e-paper shell from booting.

| Chip | Use |
| --- | --- |
| PCF85063 RTC | Local date and time, reading statistics when no network time is available |
| AXP2101 PMIC | Battery and charge state, Power-key interrupts, panel rail, power-off with a shutdown marker, wake watchdog |
| ES8311 codec | Audiobook playback, audio diagnostics (owned by the audio engine, see below) |
| SHTC3, QMI8658 | Put to sleep at boot and not used: the environment and motion features were removed |

The UI consumes compact snapshots rather than I2C handles.

## Display and refresh ownership

The SSD1677 panel is native `800 × 480`. Screens render to a logical portrait `480 × 800` framebuffer through `src/orientation.rs`; only the Reader can turn to landscape.

`src/panel_refresh.rs` is the single refresh policy. Every frame is a full-screen partial refresh, except a global one (fast waveform) for:

- periodic ghost cleanup, every 50 partial refreshes;
- switching to or from a full-page image (a cover), or leaving a high-contrast page;
- the manual **Clear ghosting now** action, the sleep image, initial boot and safety fallbacks.

After a minute without input the panel controller goes into deep sleep with its rail off (`Epaper397::sleep`), which loses its RAM but not the image on the glass. The next key press re-initializes it and loads the frame still in the framebuffer back into both RAM planes without refreshing (`Epaper397::load_base_silent`), so the next frame is an ordinary partial refresh: no flash on the first page turn after a pause. The first partial after a controller reset reads the temperature sensor (`0x22 0xFF`); the partial counter carries on across the pause.

## Power key and standby

`src/power_key.rs` decodes the AXP2101 power-key interrupts:

```text
Power short press -> standby: sleep screen, then PMIC power-off
Power long press  -> display maintenance menu (src/power_key_menu.rs)
Power press while off -> boot, restoring the route in use
```

Standby draws the sleep screen (`src/sleep_images.rs` for the BMPs in `RUSTMIX/SLEEP`, `src/sleep_cover.rs` for the current book's cover), saves state, writes a shutdown marker to the PMIC and powers the board off. Should the power-off not happen, a PMIC watchdog armed just before it cuts power 16 s later, and meanwhile the MCU is in deep sleep; either way the next boot finds the marker and resumes. Automatic standby follows the idle time in the Display settings; it is held off while an audiobook plays or the microSD is connected to a PC.

GPIO0 BOOT is Back. It never wakes or restarts the board, being a strapping pin.

## Reader

`src/reader.rs` owns the library, per-book state, bookmarks, preferences and pagination. `src/epub.rs` is an isolated EPUB parser: it reads the ZIP central directory (up to 4096 entries), extracts stored or DEFLATE members, resolves `META-INF/container.xml`, parses the OPF manifest and spine (up to 2048 spine items, with an explicit error beyond), flattens XHTML to text with its entities, and builds the table of contents from the EPUB3 `toc` nav or the NCX, nested points included.

A parsed book's flattened text is written once to an `.EPX` cache file and read back in windows; the file read last stays open for the next page (`epub::release_kept_text_file` closes it before a rewrite or a library scan).

The in-reader dictionary lookup (`src/dictionary.rs`) reads the bounded Rustmix X4 prefix-shard pack from `RUSTMIX/APPS/DICT`.

```text
/sdcard/RUSTMIX/READER/
  STATE.TXT  POSITS.TXT  RECENT.TXT  MARKS.TXT  PREFS.TXT
/sdcard/RUSTMIX/READER/CACHE/
  <8HEX>.EPX .EPP .CCH   text, page index, TXT cache
  <8HEX>.THB .SLC .EPI   thumbnails, sleep covers, inline images
```

`src/reading_stats.rs` appends one line per reading session to a monthly log in `RUSTMIX/STATS` and aggregates days, weeks, months and streaks in the device's time zone.

## Images

`src/cover_cache.rs` turns covers and inline images into dithered 1-bpp bitmaps, cached on SD by a fingerprint of the book and the target size:

- baseline JPEG: `esp_new_jpeg`, with its downscale;
- progressive JPEG, or anything `esp_new_jpeg` rejects: `src/jpeg_luma.rs`, a luma-only decoder that keeps only the coefficients the output scale needs (about 2.2 MB for a 1165×1800 cover at half size, where a general decoder needs 6.3 MB), within a 4 MB budget;
- CMYK and RGB JPEGs: `jpeg-decoder`, refused beyond an estimated 2 MB;
- PNG: the `png` crate, within a decoded-size budget.

Covers are centre-cropped to the cell's shape, never stretched. The Wi-Fi portal renders the thumbnails of uploaded books in the browser with the same fingerprint, so the device finds them ready.

## Audio engine

`src/audio/engine.rs` runs a dedicated thread (priority 6, core 1, 12 KiB stack) that owns the ES8311 codec, the NS4150B amplifier and the I2S channel. The main loop sends it commands and polls a compact state; the thread decodes MP3 with libhelix (`src/audio/mp3.rs`), recreating the I2S channel at each file's sample rate (MCLK = 256 × fs, so the codec's divider setup never changes). `src/audiobook.rs` scans `RUSTMIX/AUDIO`, sorts tracks naturally and keeps positions in `RUSTMIX/AUDIOPOS.TXT`. Around standby the engine is suspended and positions saved.

## Connect to PC

`components/usbdisk` is a small C component over `esp_tinyusb` (mass-storage class), wrapped by `src/usb_disk.rs`. The ESP32-S3 has one USB PHY, shared between the serial / JTAG port and the USB disk, so:

- the mode starts only from the menu, never at boot;
- the firmware stops audio, closes the portal and unmounts the card before handing it over;
- it ends only with a restart, which also rereads the library;
- the PHY selection bits live in RTC registers that survive a restart: they are reset at every boot and before the restart, or the serial port would stay gone;
- esp_tinyusb formats the card when it cannot remount it after an eject: the link wraps `f_mkfs` (`-Wl,--wrap=f_mkfs`) so that any format is refused.

## Wi-Fi transfer and network

`src/wifi_transfer.rs` serves a portal rooted at `/sdcard/RUSTMIX`, started from Home → Upload and stopped after inactivity. Requests carry the session code shown on screen, paths are decoded once and checked after resolution (`resolve_portal_path`), names must be FAT-safe, and replacements are written atomically. `WIFI.TXT`, `CLOCK.TXT` and `DISPLAY.TXT` are protected against download, upload, rename and delete.

Without a saved network the same screen opens a hotspot (`Configuration::Mixed`, AP + STA) with a QR code to join it; `src/dns_captive_portal.rs` answers every DNS query with the device's address so the phone opens the setup page by itself. Up to 8 networks are saved. With several saved networks the boot scans first and tries the ones in range; with one it connects straight away.

## Main task and workers

The main task has a 96 KiB stack (`CONFIG_ESP_MAIN_TASK_STACK_SIZE`), including room for an OTA install. `AppState` is heap-boxed. Heavy work runs on short-lived workers with PSRAM stacks (`src/runtime_worker.rs`), which log memory before and after and return compact results:

| Operation | Worker stack |
| --- | --- |
| EPUB parse | 64 KiB |
| EPUB title lookup | 32 KiB |
| Cover and image decode | 64 KiB |
| Wi-Fi portal | ESP-IDF HTTP server task, 24 KiB, 4 KiB chunks, 64 MiB upload cap |

Panel SPI never leaves the main task.

## Storage

`src/storage.rs` mounts the card (SDMMC 4-bit, 20 MHz, 5 open files) and provides the bounded read-only browser. Long file names are on (UTF-8, heap buffer).

- `src/sd_io.rs`: the SDMMC driver can DMA only into internal RAM, and reads into PSRAM one 512-byte sector per command. Large reads into PSRAM (EPUB archives, text windows, sleep covers and images) go through an 8 KB internal buffer instead: 4 MB in 1.5 s rather than 3.6 s.
- `src/sd_log.rs`: diagnostic logs (`BOOTTIME.LOG`, `RESETS.LOG`, `PMPROF.TXT`) move to `.OLD` at 64 KB.
- Data written to SD always comes from RAM: the SD DMA cannot read from flash, and a `const` array written directly lands as zeros.

Writes are limited to preferences, Reader state, audiobook positions, statistics, caches, logs and explicit portal operations.

## Validation

```text
scripts/validate.sh
  cargo +stable fmt --all -- --check
  scripts/validate_source_contract.sh
  scripts/test-host.sh
```

`test-host.sh` resolves the stable native target so `.cargo/config.toml` cannot leak the Xtensa default into host compilation. GitHub Actions runs the same flow on Ubuntu; firmware builds remain a local ESP toolchain operation.

Hardware checks that the host tests cannot make go through bench builds (`RUSTMIX_DEV_BENCH=1`: no standby, full-speed CPU, INFO logs) and the checklist in [`PHYSICAL_SMOKE_TEST.md`](PHYSICAL_SMOKE_TEST.md). Screen previews for visual review: `cargo test --lib export_screen_previews -- --ignored`.
