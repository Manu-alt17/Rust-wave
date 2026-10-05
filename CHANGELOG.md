# Changelog

## v1.5.0-beta.4 — First start, and the Wi-Fi page rebuilt

First start:

- A microSD never used before gets its folders by itself (`RUSTMIX/BOOKS`, `AUDIO`, `SLEEP`, made on every boot when missing) and two "read me" files, `LEGGIMI.TXT` and `README.TXT`.
- On such a card the device opens on the first-run pages instead of an empty Home: the language first, in both languages with English under the cursor, then five numbered pages (the keys, Wi-Fi, date and time, how to add books, where to find the pages again). None is mandatory: the first row is what the page proposes, the last moves on without it, BOOT goes back a page. The pages show once; **Primi passi** (First steps), a new tile in Settings, opens them again.
- A card counts as new only when it was read twice without an error and holds neither settings nor books; anything that could not be read counts as used. How far the pages got is kept in `RUSTMIX/SETUP.TXT`, so a restart picks up from the same page. A card already in use gets `done=1` there on its first boot with this firmware.
- Choosing the language puts a short guide among the books as a first one to open, `Guida rapida.txt` or `Quick guide.txt`; a file already there is never replaced.
- With no card, or one that cannot be read, the device opens on a warning in both languages, with **Riavvia** and **Continua senza**, instead of an empty Home.
- Empty lists say what to do and SELECT does it: an empty Library or Audiobooks list opens Upload, and Home's Continue card reads **Aggiungi il primo libro** and opens Upload when the books folder holds nothing. A card that cannot be read is said in plain words instead of the system's error.

Wi-Fi page, rebuilt:

- One look for the whole page: a header that says whether the device still answers (**Collegato** / **Non collegato**) and how much space is free, five tabs (**Libri**, **Audiolibri**, **Sfondi**, **File**, **Wi-Fi**), messages in a line at the bottom that goes away by itself. On the device's own hotspot the page opens on Wi-Fi, since that is why one is there.
- **File** works like a file explorer. On a phone a tap opens a folder or previews a text or a picture, the three dots hold a row's actions (download, rename, move, delete), a long press starts a selection with a bar for download, move and delete. With a mouse there is a side panel, sortable columns, click to select, double click to open, right click for the menu, F2, Del, and a row can be dragged onto a folder; files dragged from the computer upload to the folder they are dropped on. Folders come first and carry the names people know (`BOOKS` shows as **Libri**), and the device's own files are hidden until asked for.
- Delete has **Annulla**: the row leaves the list at once and the file is deleted six seconds later, or on leaving the page. A folder is deleted with what it holds (`/api/delete?recursive=1`; the card's top folder is always refused).
- **Scarica** saves the file instead of opening it in the browser: the device now answers a download with the file's type and `Content-Disposition: attachment` under its own name (it sent neither, and the server's default is `text/html`).
- **Sfondi**: pictures are searched inside the page, on Wikimedia Commons (free pictures; Google's results cannot be shown inside another page, and stay one link away), or chosen from the phone, pasted, or dragged in. The picture is framed upright, as the device is held, dragged to position and zoomed with two fingers, the wheel or the slider; the page turns it into the panel's 800×480 when it saves. **Come si vedrà** shows the black and white result first. Brightness, contrast, the rotate buttons and the file name field are gone: tones are stretched by themselves and the file gets the first free `SLEEPnnn.BMP`. The images on the device are listed upright too.
- **Audiolibri**, new: one MP3 is one audiobook; several chosen together are asked for a title and go into a folder of that name. The list shows each title with its tracks and size.
- **Wi-Fi**: signal bars instead of dBm, the whole network name, a **Mostra** button for the password, and a line that says why nearby networks only show from the device's hotspot.
- The page keeps the session alive while it is open and in use (`/api/status?alive=1`), so framing a wallpaper for ten minutes no longer finds the portal closed; a page left open and forgotten still lets it close.
- The device's errors are worded for the user, and a file over 64 MB or a path over 128 bytes is refused before it is sent.
- English: the page follows the device's language (`lang` in `/api/status`).

Display:

- Moving the selection with the rocker no longer switches the panel's analog supply off and on at every step (update control `0xDC`/`0xFC` instead of `0xDF`/`0xFF`, as GxEPD2 does for this panel): a partial refresh takes about 370 ms instead of about 510 ms. The supply is switched off after 1.5 s without a key, and both image memories of the controller are then rewritten with the frame on screen, which is what keeps the next partial refresh from showing the previous screen under the new one.

Development:

- `scripts/flash-and-log.sh` flashes like `scripts/flash.sh` and keeps the serial monitor's output in `dist/monitor.log`; on Windows it first closes a monitor left open by an earlier run.
- `src/input_timing.rs` measures, key by key, the time from contact to the end of the refresh. Off by default (`ENABLED`), and a test fails while it is on.

## v1.5.0-beta.3 — One interface, and the functions it lacked

- Library and Home covers are stretched to fill their cell instead of centre-cropped, so the whole cover shows whatever its shape; thumbnails already on the card are rebuilt once. The full-screen sleep cover is still cropped.

One look for every screen:

- Lists, labels and footers come from shared widgets (`src/app/widgets/text.rs`, `list.rs`, `layout.rs`, `footer.rs`): rows are rounded like the Home tiles, with a thick border on the selected one and the value on the right; the content keeps the Home margins. Settings, Network, Update, Clock, Audio, Info, Files, Audiobooks, Statistics and the Reader's lists were all redrawn on them.
- Text is measured in pixels: what does not fit wraps or ends in "…", instead of running off the screen or over the value beside it. Long titles, network names, file names and error messages are covered.
- Every screen below Home has the same footer, `SELECT <action> · BOOT INDIETRO`, with the page (`2/3`) on the right for lists shown a page at a time.
- A layout audit (`src/app/ux_audit.rs`) renders every screen in both languages and the three text sizes with awkward data, and fails the tests on text off screen, in the footer or on other text.

Added:

- Saved networks: SELECT opens a menu on the network, to connect to it or forget it. If it cannot be joined, the Network screen says so and the device goes back to the saved networks by itself. **Riprova connessione** on the Network screen tries the saved networks again.
- The connected network is marked in the list from the live connection (it was only marked after a change made from the portal).
- Book actions (hold SELECT on a cover): **Segna come non letto** forgets the saved position, **Elimina libro** removes the file from the card after a second SELECT. The screen also shows the format, the size and how much was read.
- Bookmarks: the Reader lists the bookmarks of the open book only, with the chapter and page in words and the percentage; hold SELECT to delete one, there and in a Library book's list. A ribbon in the top right corner marks a page that has a bookmark.
- The table of contents opens on the chapter being read and marks it.
- Update: UP and DOWN move between the action and the channel; SELECT on the channel switches it and checks. Installing takes a second SELECT.
- Dictionary: choosing a line only picks where the word selection starts. Past the last word of a line it goes on to the first word of the next one, and before the first word back to the line above; it stops at the first and last word of the page.
- The Wi-Fi portal no longer asks for the six-digit code, and the Upload screen no longer shows one: the address (or the QR code) is enough. The portal is therefore open to anyone on the same network while the Upload screen is shown; it still closes on leaving the screen and after inactivity. Requests that change something are refused when they come from another site's page (`Origin` check), which the code used to cover.
- Reader options: **Vai a** jumps to a point of the book given as a percentage, in steps of 5%; an EPUB shows the chapter that point falls in.
- Audiobook player menu: **Tracce** lists the tracks and starts the chosen one from its beginning; **Timer di spegnimento** stops playback after 15, 30, 45 or 60 minutes, with the minutes left on the player. The menu and the track list take the whole screen.
- Update: the download shows a bar, the percentage and the megabytes received, redrawn every 10% (`Content-Length` of the download; without it, the megabytes alone).
- Power-key menu: **Riavvia** restarts the device after saving the reading and listening positions. Standby already cuts the power at the PMIC, so there is no separate power off.
- Info: **Ripristina impostazioni** puts the interface text size, standby, sleep screen, "most used" settings and update channel back to their first values after a second SELECT. Language, clock, Wi-Fi, books and reading preferences stay.
- Files: BOOT closes the preview or goes up one folder (the cursor lands on the folder just left) and leaves only from the top folder, so the "Back to Home" and "Parent folder" rows are gone. Hold SELECT on a file to delete it, after a confirming SELECT; folders are not deleted. `src/storage.rs` is no longer read-only.
- Date and time editor: fields in the order time zone, day, month, year, hour, minute; BOOT steps back one field and leaves only from the first.
- **Carica** (Upload) on Home opens a chooser instead of starting the Wi-Fi portal at once: two tiles, **Wi-Fi** (the portal, on the network or on the device's hotspot) and **Cavo USB** (Connect to PC), with what the selected one does written under them. Connect to PC moved there from Settings, which is left with seven tiles; both screens go back to the chooser. The portal opened from Network still goes back to Network.
- UP and DOWN act when the key goes down instead of when it is released, so lists and pages move at once; one press is still one step, and holding the key does not repeat.
- Deleting a book from its options also removes its cache files: the flattened EPUB text, the page index and anchors of the current reading layout, the thumbnail and the sleep cover.

Fixed:

- The Italian Settings screen is titled **Opzioni**, like the Home tile that opens it.
- The percent sign of the 12 px interface font was two smudges without the slash (hinting broke it): redrawn by hand, with an override in `tools/fontgen/gen_bitmap_fonts.py` so a regeneration keeps it.
- Texts in English on the Italian interface (opening a book, the dictionary's "word not found", the portal's last action, Wi-Fi states) and leftovers of removed features on Info, Clock and the Power-key menu.
- The Italian Update tile and header say **Aggiorna**; "Streak" is **Serie**.
- `scripts/validate_source_contract.sh` reads the sources as UTF-8 whatever the system's default encoding is: on Windows its last check stopped on the first accented letter, and with it `build-ota-image.sh`.
- The player's status and volume are no longer drawn under its menu; a dictionary text longer than the page is cut instead of running through the footer.

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
