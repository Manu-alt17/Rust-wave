# Known issues and deferred work

## Weather provider reliability

Open-Meteo requests can fail transiently with transport, TLS, timeout, or HTTP service errors. The device already applies bounded retries, delayed backoff, and last-known-good in-memory retention. A cold boot with no successful fetch may still end in a readable `Weather unavailable` state.

## PMIC power-off and MCU deep sleep

A Power short press suspends network services, sleeps the e-paper panel, disables the panel rail, then cuts power at the AXP2101 PMIC (`Axp2101::power_off`). Real MCU hardware deep sleep (`mcu_deep_sleep::espidf::enter`) is the automatic fallback if the PMIC shutdown marker cannot be written or the power-off call does not actually cut power; a software-only sleep-image loop (MCU event loop still active) is the last-resort fallback if that also fails.

A few PMIC behaviors are not yet verified on physical hardware:

- Whether the AXP2101 powers itself back on while VBUS is connected or disconnected during a soft power-off, and whether that self-wake is correctly classified as `BootCause::PowerOnOrReset` (expected, since the shutdown marker would already be cleared) rather than a spurious `BootCause::PmicPowerKeyOn`.
- The exact bit layout of the PMIC's power-key timing register (IRQLEVEL/OFFLEVEL/ONLEVEL, 0x27) and power-off enable register (0x22): the firmware reads and logs both as raw bytes at boot but does not decode or rewrite them, since an incorrect write risks shortening the hardware forced power-off timeout (OFFLEVEL) below the long-press threshold used to open the maintenance menu.
- Holding Power past the PMIC's own hardware forced power-off timeout cuts power immediately with no chance for the firmware to write the shutdown marker or flush anything further; the next boot then falls back to `BootCause::PowerOnOrReset` (no Reader auto-resume), the same way an unexpected reset already does today.

## EPUB scope

Reader supports bounded reflowable text extraction, TOC navigation, bookmarks, and resume. A reopened book now reads its flattened text straight from its `.EPX` SD cache (seek-based, like the TXT reader) instead of holding it all in RAM; a fresh, not-yet-cached open still has to hold the whole flattened text in RAM up to `EPUB_REFLOW_TEXT_LIMIT` before that cache is written in the background. CSS layout, images, hyperlinks, footnotes, fixed-layout EPUB, DRM, ZIP64, and a streamed (RAM-bounded) fresh open for books past that limit remain deferred.

## Calendar scope

Calendar personal events and U.S. holidays are active. U.S. holiday rows remain read-only. Calendar reminders do not automatically create RTC alarms. Non-U.S. calendar packs are intentionally excluded from the native Calendar route.

## Dictionary scope

Dictionary exact and prefix lookup is active through the complete X4 pack. Saved words, search history, and Reader word-selection lookup remain deferred.

## Merged factory-image release artifact

The supported release artifact is the ESP-IDF ELF flashed through `espflash flash`.
Raw-address flashing with `espflash write-bin` is intentionally unsupported. A
merged factory image remains deferred until the bootloader, partition-table, and
application offsets have been validated on physical hardware.
