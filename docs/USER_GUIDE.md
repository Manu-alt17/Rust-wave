# Rustmix Wave user guide

This guide describes the Rustmix Wave v1.0.0 user interface as shown by the reference screenshots under [`/screenshots`](../screenshots/). The firmware targets the Waveshare ESP32-S3 3.97-inch e-paper board and uses a rotary-first interaction model.

## Physical controls

| Control | Normal behavior |
| --- | --- |
| Rotary up / down | Move the highlighted row, change a value, turn a reader page, or move within the active keyboard axis |
| Rotary SELECT | Open the highlighted item, activate an action, or advance an editor field |
| Hold SELECT | Contextual secondary action. On grid keyboards it toggles `NAV H` / `NAV V`. In Calendar it opens the daily agenda or creates a personal event from the agenda. In Sudoku and Minesweeper it toggles the active movement axis or cancels an edit/action mode. |
| BOOT | Hierarchical Back. On editors it cancels and returns without saving. |
| Power short | Open the display-maintenance menu. Select **Clear ghosting now** for a full e-paper base refresh, or select **Cancel**. |
| Power long | Enter random sleep-image mode. After the wake guard quiet interval, press Power briefly to restore the previous screen. |

The bottom footer on each screen repeats the controls that are valid in that context.

## 1. Home dashboard

<img src="../screenshots/homepage.jpg" width="360" alt="Rustmix Wave home dashboard">

The home dashboard shows date, time, battery, weather summary, Wi-Fi state, and the five top-level categories.

| Row | Opens |
| --- | --- |
| Reader | Continue Reading, Library, and Bookmarks |
| Productivity | Calendar and Voice Notes |
| Games | SD-loaded apps backed by native Rust game surfaces |
| Tools | File Browser, Dictionary, and Unit Converter |
| Settings | Device services, display, network, sensors, alarms, audio, and weather |

Navigation: rotate to choose a category, then press SELECT. Use BOOT from a category page to return home.

## 2. Reader

### Reader category

<img src="../screenshots/reader-main.jpg" width="360" alt="Reader category menu">

The Reader category contains:

- **Continue Reading**: reopen the most recently saved book position.
- **Library**: browse TXT and EPUB files.
- **Bookmarks**: open saved reading anchors directly.

Navigation: rotate to choose a row, SELECT to open, BOOT to return home.

### Continue Reading

<img src="../screenshots/continue-reading1.jpg" width="360" alt="Continue Reading screen">

This screen shows the last saved book and the saved page. Press SELECT to resume. BOOT to return to the Reader menu.

### Opening a book

<img src="../screenshots/opening_book.jpg" width="360" alt="Opening Book progress screen">

TXT and EPUB opening is staged. The current page becomes available before full indexing completes. BOOT to cancel an in-progress open.

### Library tabs

The Library has four tabs. Use SELECT on the **Change tab** row to cycle tabs, then rotate through rows and press SELECT to open a book or bookmark.

| Tab | Purpose | Screenshot |
| --- | --- | --- |
| Recent | Recently opened books | [library-recent.jpg](../screenshots/library-recent.jpg) |
| Books | Combined TXT and EPUB library | [library-books.jpg](../screenshots/library-books.jpg) |
| Files | File-oriented book listing | [library-files.jpg](../screenshots/library-files.jpg) |
| Bookmarks | Saved anchors with page or chapter labels | [library-bookmarks.jpg](../screenshots/library-bookmarks.jpg) |

### Reading TXT and EPUB books

<table>
<tr><td><img src="../screenshots/txt-reader.jpg" width="300" alt="TXT reader"></td><td><img src="../screenshots/epub-reader.jpg" width="300" alt="EPUB reader"></td></tr>
</table>

Reader page controls:

| Control | Action |
| --- | --- |
| Rotary up | Previous page |
| Rotary down | Next page |
| SELECT | Open Reader Options |
| BOOT | Return to the Reader shell |

TXT pages show encoding, page position, and cache state. EPUB pages additionally show chapter-relative progress.

### Reader Options

<table>
<tr><td><img src="../screenshots/txt-reader-options.jpg" width="300" alt="TXT reader options"></td><td><img src="../screenshots/epub-reader-options.jpg" width="300" alt="EPUB reader options"></td></tr>
</table>

Available actions include:

- Add or remove a bookmark
- View bookmarks
- Open the EPUB Table of Contents when available
- Open Reading Preferences
- Clear e-paper ghosting
- Go to Library
- Go Home

Navigation: rotate to highlight an action, SELECT to activate, BOOT to return to the page.

### Reading Preferences

<img src="../screenshots/reader-reading-prefs.jpg" width="360" alt="Reading Preferences">

Preferences include theme, orientation, book font size, font family, paragraph alignment, and progress display. Rotate to move; press SELECT to change a setting; BOOT to return.

### EPUB Table of Contents

<img src="../screenshots/reader-toc.jpg" width="360" alt="EPUB table of contents">

The EPUB TOC lists chapter entries. Rotate to choose a chapter, SELECT to open it, and BOOT to return.

### Bookmarks

<table>
<tr><td><img src="../screenshots/reader-bookmarks.jpg" width="300" alt="Reader bookmarks"></td><td><img src="../screenshots/reader-bookmarks-list.jpg" width="300" alt="Reader bookmarks list"></td></tr>
</table>

Bookmarks retain byte-offset anchors as the authoritative jump target. EPUB rows show chapter-relative labels when available; TXT rows show page labels. Rotate to choose a saved anchor, SELECT to open it, and BOOT to return.

## 3. Productivity

### Productivity category

<img src="../screenshots/productivity.jpg" width="360" alt="Productivity menu">

The Productivity category contains Calendar and Voice Notes.

### Calendar month view

<table>
<tr><td><img src="../screenshots/calendar-current-day.jpg" width="300" alt="Calendar current day"></td><td><img src="../screenshots/calendar-us-events.jpg" width="300" alt="Calendar US holiday"></td></tr>
</table>

The native Calendar shows a Gregorian month grid, selected-day summary, personal events from `EVENTS.TXT`, and U.S. holidays from `US2026.TXT`. Days containing events receive markers.

| Control | Action |
| --- | --- |
| Rotary up / down | Move the selected day or month, depending on active mode |
| SELECT | Toggle Day / Month navigation mode |
| Hold SELECT | Open the selected-day agenda |
| BOOT | Return to Productivity |

### Daily agenda

<img src="../screenshots/calendar-date-details.jpg" width="360" alt="Calendar daily agenda">

The agenda lists personal events and U.S. holidays for the selected date. It scrolls when more than six rows exist.

| Control | Action |
| --- | --- |
| Rotary up / down | Move through agenda rows |
| SELECT | Open event details |
| Hold SELECT | Create a new personal event for the selected date |
| BOOT | Return to month view |

Personal events can be edited or deleted. U.S. holiday rows remain read-only.

### Calendar personal-event editor

<img src="../screenshots/calendar-create-note.jpg" width="360" alt="Calendar personal event editor">

The editor writes only `EVENTS.TXT`. U.S. holidays remain untouched.

| Control | Action |
| --- | --- |
| Rotary up / down | Move within the active keyboard axis |
| Hold SELECT | Toggle `NAV H` / `NAV V` without moving the highlighted key |
| SELECT | Activate the highlighted key |
| FIELD | Switch between title and detail |
| SAVE | Commit the personal event |
| CANCEL | Exit without saving |
| BOOT | Cancel and return |

Calendar writes use recovery-safe `EVENTS.TMP -> EVENTS.TXT` replacement with `EVENTS.BAK` fallback.

### Voice Notes list

<img src="../screenshots/voice_notes.jpg" width="360" alt="Voice Notes list">

Voice Notes records FAT 8.3 `VOICE###.WAV` files in PCM16 mono 16 kHz format. The list shows available storage, microphone gain, and saved recordings.

| Control | Action |
| --- | --- |
| Rotary up / down | Move between Record new note, microphone gain, and saved rows |
| SELECT on Record new note | Begin recording |
| SELECT on microphone gain | Cycle gain profile |
| SELECT on saved note | Open saved-WAV details |
| BOOT | Return to Productivity |

### Record Voice Note

<img src="../screenshots/voice_notes_record.jpg" width="360" alt="Voice Notes recording screen">

While recording, the screen shows filename, elapsed time, PCM byte count, peak level, microphone gain, clipping count, and streamed `.TMP` status.

| Control | Action |
| --- | --- |
| Rotary up / down | Pause or resume capture |
| SELECT | Stop and save |
| BOOT | Cancel and return |

### Saved-WAV details

<img src="../screenshots/voice_note_detail.jpg" width="360" alt="Voice Note details">

The saved-note page shows the authoritative WAV filename, recorded time, duration, free storage, playback progress, and actions.

Available actions:

- Play note / Stop playback
- Edit friendly title
- Export / download through the LAN portal
- Delete note with confirmation
- Return to Voice Notes

Navigation: rotate to choose an action, SELECT to run it, BOOT to return.

### Edit friendly title

<img src="../screenshots/voice_note_edit.jpg" width="360" alt="Voice Note friendly title editor">

The title editor reuses the shared keyboard-grid navigation model.

| Control | Action |
| --- | --- |
| Rotary up / down | Move within active keyboard axis |
| Hold SELECT | Toggle `NAV H` / `NAV V` |
| SELECT | Activate the highlighted key |
| SAVE | Update friendly title in `META.TXT` |
| CANCEL or BOOT | Return without saving |

The internal `VOICE###.WAV` filename does not change.

## 4. Games

### Games category and SD Lua app catalog

<table>
<tr><td><img src="../screenshots/games.jpg" width="300" alt="Games category"></td><td><img src="../screenshots/games-listing.jpg" width="300" alt="SD Lua apps listing"></td></tr>
</table>

Select **SD Lua Apps** to open the SD-loaded catalog. Rotate to choose a game, SELECT to open it, and BOOT to return.

Lua scripts declare bounded app behavior, but native Rust owns game state, rendering, dirty regions, sensors, and panel refresh policy.

### Hello Grid

<img src="../screenshots/hello-grid.jpg" width="360" alt="Hello Grid sample">

Hello Grid is the basic SD Lua foundation sample. It verifies bounded canvas rendering without exposing the e-paper transport to the script.

### Sudoku

<img src="../screenshots/sudoku.jpg" width="360" alt="Sudoku">

Sudoku is a native board-state bridge driven by an SD app declaration.

| Control | Action |
| --- | --- |
| Rotary up / down | Move cursor in active H/V axis, or cycle candidate in edit mode |
| Hold SELECT | Toggle H/V axis in navigation mode; cancel edit mode when editing |
| SELECT | Enter edit mode or commit candidate |
| BOOT | Return to catalog |

### Minesweeper

<img src="../screenshots/minesweeper.jpg" width="360" alt="Minesweeper">

Minesweeper uses a native beginner board with first-reveal safety, flags, flood reveal, and win/loss status.

| Control | Action |
| --- | --- |
| Rotary up / down | Move in active axis or change the active action |
| Hold SELECT | Toggle axis or cancel action mode |
| SELECT | Reveal or flag according to the current action |
| BOOT | Return to catalog |

### Tilt Maze

<img src="../screenshots/tilt-maze.jpg" width="360" alt="Tilt Maze">

Tilt Maze uses debounced planar tilt events from the QMI8658 IMU. Tilt the device to move the player through the maze. SELECT resets the level; BOOT to return.

### Motion 2048

<img src="../screenshots/motion20248.jpg" width="360" alt="Motion 2048">

Motion 2048 maps debounced tilt events to board swipes. Tilt the device to slide and merge tiles. SELECT resets the board; BOOT to return.

### Sokoban Tilt

<img src="../screenshots/sobokan-tilt.jpg" width="360" alt="Sokoban Tilt">

Sokoban Tilt maps debounced tilt events to player movement and crate pushes. Tilt the device to navigate the puzzle. SELECT resets the level; BOOT to return.

## 5. Tools

### Tools category

<img src="../screenshots/tools.jpg" width="360" alt="Tools menu">

Tools contains File Browser, Dictionary, and Unit Converter.

### File Browser

<table>
<tr><td><img src="../screenshots/directory-listing.jpg" width="300" alt="File browser root"></td><td><img src="../screenshots/files-listing.jpg" width="300" alt="File browser directory listing"></td></tr>
</table>

The File Browser is a bounded read-only SDMMC browser. Directories sort before files. Text files open in bounded preview mode; binary files report that preview is unavailable.

| Control | Action |
| --- | --- |
| Rotary up / down | Move row selection |
| SELECT | Enter directory, open preview, or close preview |
| BOOT | Return to parent or Tools |

### Dictionary

<table>
<tr><td><img src="../screenshots/dictionary.jpg" width="300" alt="Dictionary search"></td><td><img src="../screenshots/dictionary-result.jpg" width="300" alt="Dictionary result"></td></tr>
</table>

The native Dictionary reuses the X4 prefix-shard SD pack. Enter letters, use **GO** for exact lookup with prefix fallback, or use `*` for prefix lookup and repeated result cycling.

| Control | Action |
| --- | --- |
| Rotary up / down | Move within active keyboard axis |
| Hold SELECT | Toggle `NAV H` / `NAV V` |
| SELECT | Activate letter, DEL, CLR, GO, or `*` |
| BOOT | Return to Tools |

### Unit Converter

<table>
<tr><td><img src="../screenshots/unit-converter.jpg" width="300" alt="Unit converter length"></td><td><img src="../screenshots/unit-converter1.jpg" width="300" alt="Unit converter volume"></td></tr>
</table>

The offline fixed-point converter supports categories such as length, mass, temperature, and volume.

| Control | Action |
| --- | --- |
| Rotary up / down | Change the highlighted row value |
| SELECT | Advance to the next editable row |
| BOOT | Return to Tools |

## 6. Settings

<table>
<tr><td><img src="../screenshots/settings.jpg" width="300" alt="Settings page one"></td><td><img src="../screenshots/settings1.jpg" width="300" alt="Settings page two"></td></tr>
</table>

Settings is paginated. Rotate through rows, SELECT to open an item, and BOOT to return home.

### Alarms

<table>
<tr><td><img src="../screenshots/alarms.jpg" width="300" alt="Alarm schedule list"></td><td><img src="../screenshots/alarm-details.jpg" width="300" alt="Alarm editor"></td></tr>
</table>

The alarm list shows configured RTC schedules. The editor changes hour, minute, enable state, recurrence mode, and weekday/day schedule values.

| Control | Action |
| --- | --- |
| List: rotary up / down | Move schedule row |
| List: SELECT | Edit selected alarm |
| Editor: rotary up / down | Change field value |
| Editor: SELECT | Advance field |
| Editor: Hold SELECT | Back |
| BOOT | Return to Settings |

### Audio

<table>
<tr><td><img src="../screenshots/audio.jpg" width="300" alt="Audio overview"></td><td><img src="../screenshots/audio-details.jpg" width="300" alt="Audio details"></td></tr>
</table>

The Audio screen exposes codec state, volume, amplifier state, chime playback, stop, mute/unmute, and detailed ES8311 routing diagnostics.

Navigation: rotate through actions, SELECT to run, BOOT to return.

### Clock and RTC details

<table>
<tr><td><img src="../screenshots/clock.jpg" width="300" alt="Clock overview"></td><td><img src="../screenshots/rtc-details.jpg" width="300" alt="RTC details"></td></tr>
</table>

Clock shows localized RTC time, temperature, humidity, battery, USB, and charge status. RTC Details shows time basis, storage basis, battery voltage, USB state, charge state, and refresh policy.

Navigation: SELECT opens details; BOOT returns.

### Display

<img src="../screenshots/display.jpg" width="360" alt="Display preferences">

Display settings change the global UI font and UI size. Rotate to choose a row, SELECT to change, and BOOT to return.

### Device Info

<table>
<tr><td><img src="../screenshots/device-info.jpg" width="260" alt="Device info firmware page"></td><td><img src="../screenshots/device-info1.jpg" width="260" alt="Device info board page"></td><td><img src="../screenshots/device-info2.jpg" width="260" alt="Device info runtime page"></td></tr>
</table>

Device Info is a three-page read-only diagnostic surface covering firmware, display, board services, SD storage, runtime services, network, weather, alarm state, display zone, and temperature units.

Navigation: SELECT advances to the next page; BOOT returns.

### Environment

<table>
<tr><td><img src="../screenshots/environment.jpg" width="300" alt="Environment overview"></td><td><img src="../screenshots/environment1.jpg" width="300" alt="Environment sensor details"></td></tr>
</table>

Environment uses the SHTC3 sensor for temperature and relative humidity. SELECT opens sensor details; BOOT returns.

### Motion and Motion Events

<table>
<tr><td><img src="../screenshots/motion.jpg" width="300" alt="Motion overview"></td><td><img src="../screenshots/motion-events.jpg" width="300" alt="Motion event diagnostics"></td></tr>
</table>

Motion uses the QMI8658 accelerometer and gyroscope. The overview shows live axes. Motion Events translates raw samples into debounced native events:

```text
TILT
SHAKE
ROTATE
LEVEL
```

The Motion Events screen exposes thresholds, debounce timing, counters, reset, and sensor details.

| Control | Action |
| --- | --- |
| Overview: SELECT | Open Motion Events |
| Motion Events: rotary up / down | Move through threshold and action rows |
| Motion Events: SELECT | Change threshold, reset counters, or open details |
| BOOT | Return |

### Network and Wi-Fi transfer

<table>
<tr><td><img src="../screenshots/network.jpg" width="300" alt="Network overview"></td><td><img src="../screenshots/network-details.jpg" width="300" alt="Network details"></td></tr>
</table>

Network shows Wi-Fi, SNTP, SSID, IPv4 address, RSSI, how many networks are saved, regional timezone, RTC storage basis, and NTP server.

| Control | Action |
| --- | --- |
| Rotary up / down | Move between Configure via phone, Saved networks and Provisioning details |
| SELECT on Configure via phone | Open the device's own hotspot and Wi-Fi setup portal |
| SELECT on Saved networks | View and forget saved networks |
| SELECT on Provisioning details | Open network details |
| BOOT | Return to Settings |

Configure via phone replaces typing an SSID and password on the device. Selecting it starts a temporary hotspot with a freshly generated name and password, plus a small web portal, both shown on screen along with a single QR code: scan it with a phone camera to join the hotspot automatically. The device also answers as a captive portal, so most phones then open the setup page on their own (as a "Sign in to network" prompt or an automatically launched browser); if a phone doesn't offer that, open its displayed address in a browser instead. The portal lists nearby networks the device found (scanned while the hotspot runs) and any already-saved networks. Tap a network, type its password with the phone's own keyboard, and the device attempts a real connection before saving it — only a network that actually connects gets written to `WIFI.TXT`. Up to 8 networks can be saved this way; the device tries them in order at boot until one connects, so both a home and a travel router can be kept side by side. Changing a saved network's password is done the same way: add it again from the portal with the new password. SELECT on the Configure via phone screen (or BOOT) stops the hotspot and reconnects using the saved list.

Saved networks lists the SSIDs already in `WIFI.TXT`, flagging whichever one is currently connected. Rotary up / down moves through the list; SELECT arms a "forget" confirmation on the highlighted network, and SELECT again removes it. Moving the selection cancels a pending confirmation. This screen is read-only otherwise — adding a network or changing its password happens through Configure via phone.

### Wi-Fi transfer portal

<img src="../screenshots/wifi-transfer.jpg" width="520" alt="Rustmix Wave Wi-Fi transfer browser portal">

The Wi-Fi transfer portal is fully independent of the Network settings: it is started from the Home **Upload** tile, which jumps straight into the portal and starts it. SELECT on the portal screen stops it and returns to Home; BOOT does the same. From a device on the same LAN, open the displayed URL and enter the six-digit session code. The portal lists the `/RUSTMIX` tree and supports bounded upload, download, rename, directory creation, and deletion operations while protecting internal configuration files.

### Weather

<table>
<tr><td><img src="../screenshots/weather.png" width="300" alt="Weather overview"></td><td><img src="../screenshots/weather-1.jpg" width="300" alt="Weather details"></td></tr>
</table>

Weather uses the configured Open-Meteo profile, bounded retries, and a last-known-good cache. The overview shows current conditions and a four-day forecast. Details show provider, timezone, observation time, last success, configuration file, and last error.

| Control | Action |
| --- | --- |
| Rotary up / down | Move between Refresh weather and Weather details |
| SELECT | Run the selected action |
| BOOT | Return to Settings |

## 7. Power-key maintenance and sleep

A short Power press opens a display-maintenance menu from any ordinary UI route. Select **Clear ghosting now** to request the shared global-base refresh path. Select **Cancel** or BOOT to return without refreshing.

A long Power press enters sleep-image mode:

<img src="../screenshots/sleep.jpg" width="360" alt="Sleep image mode">

The firmware selects a random image from `/sdcard/RUSTMIX/SLEEP`, suspends network activity, sleeps the panel, retains the prior route, and uses a wake guard so the entry press is not mistaken for an immediate wake press.

## 8. Screenshot index

Every supplied screenshot is stored in the repository so the guide and README can link stable reference images.

| Screenshot | Screen |
| --- | --- |
| [homepage.jpg](../screenshots/homepage.jpg) | Home dashboard |
| [reader-main.jpg](../screenshots/reader-main.jpg) | Reader category |
| [continue-reading1.jpg](../screenshots/continue-reading1.jpg) | Continue Reading |
| [opening_book.jpg](../screenshots/opening_book.jpg) | Opening Book progress |
| [library-recent.jpg](../screenshots/library-recent.jpg) | Library Recent tab |
| [library-books.jpg](../screenshots/library-books.jpg) | Library Books tab |
| [library-files.jpg](../screenshots/library-files.jpg) | Library Files tab |
| [library-bookmarks.jpg](../screenshots/library-bookmarks.jpg) | Library Bookmarks tab |
| [txt-reader.jpg](../screenshots/txt-reader.jpg) | TXT Reader page |
| [epub-reader.jpg](../screenshots/epub-reader.jpg) | EPUB Reader page |
| [txt-reader-options.jpg](../screenshots/txt-reader-options.jpg) | TXT Reader Options |
| [epub-reader-options.jpg](../screenshots/epub-reader-options.jpg) | EPUB Reader Options |
| [reader-reading-prefs.jpg](../screenshots/reader-reading-prefs.jpg) | Reading Preferences |
| [reader-toc.jpg](../screenshots/reader-toc.jpg) | EPUB Table of Contents |
| [reader-bookmarks.jpg](../screenshots/reader-bookmarks.jpg) | Persistent Bookmarks |
| [reader-bookmarks-list.jpg](../screenshots/reader-bookmarks-list.jpg) | Bookmarks list |
| [productivity.jpg](../screenshots/productivity.jpg) | Productivity category |
| [calendar-current-day.jpg](../screenshots/calendar-current-day.jpg) | Calendar month view, selected day |
| [calendar-us-events.jpg](../screenshots/calendar-us-events.jpg) | Calendar month view, U.S. event |
| [calendar-date-details.jpg](../screenshots/calendar-date-details.jpg) | Calendar daily agenda |
| [calendar-create-note.jpg](../screenshots/calendar-create-note.jpg) | Calendar personal-event editor |
| [voice_notes.jpg](../screenshots/voice_notes.jpg) | Voice Notes list |
| [voice_notes_record.jpg](../screenshots/voice_notes_record.jpg) | Voice Notes recording |
| [voice_note_detail.jpg](../screenshots/voice_note_detail.jpg) | Saved-WAV details |
| [voice_note_edit.jpg](../screenshots/voice_note_edit.jpg) | Voice Notes friendly-title editor |
| [games.jpg](../screenshots/games.jpg) | Games category |
| [games-listing.jpg](../screenshots/games-listing.jpg) | SD Lua apps catalog |
| [hello-grid.jpg](../screenshots/hello-grid.jpg) | Hello Grid |
| [sudoku.jpg](../screenshots/sudoku.jpg) | Sudoku |
| [minesweeper.jpg](../screenshots/minesweeper.jpg) | Minesweeper |
| [tilt-maze.jpg](../screenshots/tilt-maze.jpg) | Tilt Maze |
| [motion20248.jpg](../screenshots/motion20248.jpg) | Motion 2048 |
| [sobokan-tilt.jpg](../screenshots/sobokan-tilt.jpg) | Sokoban Tilt |
| [tools.jpg](../screenshots/tools.jpg) | Tools category |
| [directory-listing.jpg](../screenshots/directory-listing.jpg) | File Browser root |
| [files-listing.jpg](../screenshots/files-listing.jpg) | File Browser directory listing |
| [dictionary.jpg](../screenshots/dictionary.jpg) | Dictionary input |
| [dictionary-result.jpg](../screenshots/dictionary-result.jpg) | Dictionary result |
| [unit-converter.jpg](../screenshots/unit-converter.jpg) | Unit Converter length example |
| [unit-converter1.jpg](../screenshots/unit-converter1.jpg) | Unit Converter volume example |
| [settings.jpg](../screenshots/settings.jpg) | Settings page one |
| [settings1.jpg](../screenshots/settings1.jpg) | Settings page two |
| [alarms.jpg](../screenshots/alarms.jpg) | Alarm schedules |
| [alarm-details.jpg](../screenshots/alarm-details.jpg) | Alarm editor |
| [audio.jpg](../screenshots/audio.jpg) | Audio overview |
| [audio-details.jpg](../screenshots/audio-details.jpg) | ES8311 audio details |
| [clock.jpg](../screenshots/clock.jpg) | Clock overview |
| [rtc-details.jpg](../screenshots/rtc-details.jpg) | RTC details |
| [display.jpg](../screenshots/display.jpg) | Display preferences |
| [device-info.jpg](../screenshots/device-info.jpg) | Device Info firmware page |
| [device-info1.jpg](../screenshots/device-info1.jpg) | Device Info board page |
| [device-info2.jpg](../screenshots/device-info2.jpg) | Device Info runtime page |
| [environment.jpg](../screenshots/environment.jpg) | Environment overview |
| [environment1.jpg](../screenshots/environment1.jpg) | SHTC3 details |
| [motion.jpg](../screenshots/motion.jpg) | Motion overview |
| [motion-events.jpg](../screenshots/motion-events.jpg) | Motion event diagnostics |
| [network.jpg](../screenshots/network.jpg) | Network overview |
| [network-details.jpg](../screenshots/network-details.jpg) | Network details |
| [wifi-transfer.jpg](../screenshots/wifi-transfer.jpg) | Browser Wi-Fi transfer portal |
| [weather.png](../screenshots/weather.png) | Weather overview |
| [weather-1.jpg](../screenshots/weather-1.jpg) | Weather details |
| [sleep.jpg](../screenshots/sleep.jpg) | Sleep-image mode |

## 9. Related documentation

- [`README.md`](../README.md): project overview, setup, validation, build, and release commands
- [`ARCHITECTURE.md`](ARCHITECTURE.md): module ownership, sensor pipeline, workers, and memory-safety design
- [`BOARD_CONTRACT.md`](BOARD_CONTRACT.md): stable hardware contract
- [`SD_CARD_SETUP.md`](SD_CARD_SETUP.md): SD-card layout and installers
- [`PHYSICAL_SMOKE_TEST.md`](PHYSICAL_SMOKE_TEST.md): consolidated hardware verification checklist
- [`RELEASE.md`](RELEASE.md): ELF-only release workflow
