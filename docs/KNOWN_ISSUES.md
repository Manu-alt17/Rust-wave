# Known issues and deferred work

## Not yet verified on the device

- **Silent panel wake**: the log shows the silent reload and a partial refresh on the first key press after a minute's pause, with no panel error, but only the eye can confirm that no trace of the previous page is left.
- **Wi-Fi with a single saved network** now connects without a scan first; the test card had no `WIFI.TXT`.
- **Audiobook volume** is not saved: every boot starts from the default.
- Whether the AXP2101 powers itself back on while VBUS is connected during a soft power-off, and how that boot is classified.

## Upgrading from earlier versions

- Positions and Recent entries saved before long file names were enabled use 8.3 names (`OSSESS~1.EPU`); they no longer match the long names the library now shows, so those books start again from the beginning once.
- Audiobooks go in `RUSTMIX/AUDIO`; files left elsewhere (for example the E-ink firmware's `/audiobooks`) are not listed.
- `CLOCK.TXT` keeps the time zone chosen before: the Europe/Rome default applies only where none was saved.

## PMIC power-off

A Power short press suspends network services, sleeps the panel, cuts its rail and then powers off at the AXP2101 (`Axp2101::power_off`). MCU deep sleep is the fallback if the shutdown marker cannot be written or the power-off does not cut power; a software-only sleep loop is the last resort if arming the MCU wakeup also fails. See [`BOARD_CONTRACT.md`](BOARD_CONTRACT.md).

The power-key timing register (0x27) reads `0x14` on the board: long press after 1.5 s, hardware power-off after 6 s, power-on after 0.128 s. The firmware logs it at boot and never rewrites it. Holding Power past the hardware power-off cuts power with no chance to save anything; the next boot is then an ordinary power-on, without Reader auto-resume.

## EPUB scope

Reflowable text, covers, inline images, table of contents, bookmarks and resume. A reopened book reads its text from the `.EPX` cache; a first open still holds the whole flattened text in RAM, up to `EPUB_REFLOW_TEXT_LIMIT`, until that cache is written. CSS layout, hyperlinks, footnotes, fixed-layout EPUB, DRM and ZIP64 are not supported.

## Covers and images

Progressive JPEGs decode luma only, at up to half size for a 1165×1800 cover, in 1 to 2 s the first time (then cached). PNG has no scaled decode: a large PNG is refused past its decoded-size budget, and its cover stays a placeholder with the title.

## USB disk speed

Connect to PC runs at USB Full Speed (12 Mbit/s). Windows reports a copy finished when the data reaches its cache; the transfer really ends at the eject.

## Merged factory-image release artifact

The supported release artifact is the ESP-IDF ELF flashed through `espflash flash`. Raw-address flashing with `espflash write-bin` is intentionally unsupported. A merged factory image remains deferred until the bootloader, partition-table, and application offsets have been validated on physical hardware.
