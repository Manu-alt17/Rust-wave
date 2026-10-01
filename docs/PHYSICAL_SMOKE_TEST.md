# Consolidated physical smoke test

For screen names, navigation controls, and reference images, see [`USER_GUIDE.md`](USER_GUIDE.md).

Run this checklist after a release build or any cross-cutting runtime change. Do not keep a serial monitor attached while testing by hand: reopening the USB serial port resets the board.

## Build and boot

1. Run `./scripts/validate.sh`.
2. Run `cargo +esp build --release`.
3. Flash with `./scripts/flash.sh monitor`.
4. Confirm boot reaches the Home screen without panic or reset loops, and that a release build prints warnings only (no INFO lines from the bootloader or the app).
5. Confirm Settings → Info shows the version in `Cargo.toml`.

## Button capture under load

1. Trigger a global refresh (for example the display maintenance menu's **Clear ghosting now**).
2. While it is in progress, rapid-press Up/Down 5-10 times: every press must still be reflected once the refresh completes.
3. Repeat with BOOT (Back), and with a long SELECT on a page of a book (Reader options).

## Panel idle sleep

1. Open a book and leave it for more than a minute.
2. Turn the page: the new page appears with a partial refresh, no flash, and no trace of the previous page.
3. Do the same on Home and in the Library, going Back instead of turning a page.
4. After 50 partial refreshes a fast global refresh cleans the ghosting by itself.

## Power key and standby

1. Hold Power and confirm the display-maintenance menu opens; **Clear ghosting now** returns to the screen with a clean refresh, Cancel returns without one.
2. Press Power briefly: the sleep screen is drawn, network services suspend, and the board powers off.
3. Press Power again: the board turns on and shows the previous screen.
4. With a book open, power off and on again: the same book reopens at the same page.
5. Set Settings → Schermo → standby to 5 minutes, leave the device: it goes into standby by itself. It must not while an audiobook plays.
6. Set the sleep screen to the book cover and power off with a book open: its cover is on the glass, including progressive-JPEG covers.

## Reader

1. Open one TXT and one EPUB book: an EPUB with a cover opens on it, Down goes on to the text.
2. Check page turns, the options (hold SELECT), preferences, table of contents and bookmark add/remove, and the dictionary lookup (SELECT, choose a word).
3. Reboot and confirm **Continua** on Home restores the book and page.
4. Confirm `/RUSTMIX/READER/POSITS.TXT` and the `READER/CACHE` files exist.

## Library and covers

1. Open the Library with a few new books: covers appear one by one, then stay on later visits.
2. A book without a cover (or with a broken one) shows its title on the placeholder.
3. A book with a progressive JPEG cover shows the cover, after 1 to 2 s the first time.

## Audiobooks

1. Put one MP3 and one folder of numbered MP3 tracks in `/RUSTMIX/AUDIO`.
2. Play each: sound on the speaker, no clicks, the track order follows the numbers.
3. Pause and resume, change the volume, use the player menu (30 s back and forward, previous and next track, stop).
4. Go Back to the list while playing: playback goes on.
5. Reboot and play again: playback resumes where it was.

## Connect to PC

1. Settings → Al PC, connect the cable, press SELECT: the computer sees the microSD as a disk, and the serial port disappears.
2. Copy a book and an audiobook, eject the disk on the computer, press a key on the device (not BOOT): it restarts, the serial port is back, and the new book and audiobook are listed.
3. Repeat, pulling the cable without ejecting: the card must not be formatted.

## Network

1. With no Wi-Fi configured, open Home → Carica: the hotspot and the QR code appear; the phone joins and opens the setup page by itself. Add a network and confirm the device connects.
2. With Wi-Fi configured, open Home → Carica again: the device is reachable at its LAN address. Upload a book; its cover appears in the Library at once.
3. Try to download, overwrite, rename or delete `WIFI.TXT`, `CLOCK.TXT` or `DISPLAY.TXT` through the portal, including as `./WIFI.TXT`, `WIFI.TXT/` and `%2557IFI.TXT`: every attempt is refused.
4. Confirm the clock synchronizes and the statistics count today's reading in local time.

## Idle power profile (diagnostic build)

Measures whether automatic light sleep is actually entered while the device sits idle. Not part of the release checklist.

1. Build and flash with `./scripts/flash-pm-profiling.sh monitor` (same arguments as `flash.sh`). It builds with the profiling overlay and refuses to flash a binary without the profiling telemetry. Boot must log `rustmix-wave=pm-profile status=enabled window-seconds=60`.
2. Every 60 s the log shows `rustmix-wave=pm-profile window-ms=... min-freq-pct=... apb-min-pct=... apb-max-pct=... cpu-max-pct=... light-sleeps=... light-sleep-rejects=... route=... panel-awake=...`, followed by `rustmix-wave=pm-profile-lock` rows (cumulative since boot). The first line after boot only records a baseline.
3. Record at least three windows for each scenario, touching nothing during them:
   - Home, panel asleep (wait past the 60 s panel idle sleep).
   - Reader page, panel asleep, Wi-Fi still connected.
   - Reader page after the reader power-save grace period (`wifi-suspended-for-reading=true`).
   - Audiobook playback (expected to stay awake).
4. `light-sleeps=0` means the chip never actually entered light sleep in that window. `min-freq-pct` is the share of time at the 40 MHz floor where light sleep is permitted (ESP-IDF's `SLEEP` mode), not time spent asleep; the lock rows with a growing `Time(us)` show what keeps it awake or at a raised frequency.
5. While a PC is attached over USB, automatic light sleep is deliberately blocked (`CONFIG_USJ_NO_AUTO_LS_ON_CONNECTION`: the USB Serial/JTAG console cannot survive it), and the lock rows show `usb_serial_jtag NO_LIGHT_SLEEP` held. To measure light sleep, confirm the build over USB first, then run the scenarios on battery or a wall charger with no PC attached. Every window is also appended to `/sdcard/RUSTMIX/PMPROF.TXT` (moved to `PMPROF.OLD` at 64 KB), prefixed with `uptime-ms=`; read it afterwards from the card or through the Wi-Fi transfer portal.
6. Rebuild without the overlay before any battery-life measurement.
