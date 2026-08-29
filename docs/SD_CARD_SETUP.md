# SD-card setup

Use a FAT-formatted SD card. Rustmix Wave mounts it at `/sdcard` and expects the following product tree:

```text
/RUSTMIX/
  WIFI.TXT
  CLOCK.TXT
  WEATHER.TXT
  ALARMS.TXT
  DISPLAY.TXT
  BOOKS/
  READER/
    CACHE/
  VOICE/
  SLEEP/
    *.BMP
  MAGIC/
    INDEX.TXT
    ACTIVE.TXT
    *_F.TOK
    *_H.TOK
  APPS/
    HGRID/
    SUDOKU/
    MINES/
    TILTMAZE/
    M2048/
    SOKOBAN/
    DICT/
      INDEX.TXT
      DATA/*.JSN
    CALENDAR/
      EVENTS.TXT
      US2026.TXT
```

## Install bundled examples

```bash
./scripts/install-sd-examples.sh /Volumes/YOUR_SD_CARD
```

Existing paths are preserved by default. Use `--force` only when deliberately replacing bundled example files:

```bash
./scripts/install-sd-examples.sh --force /Volumes/YOUR_SD_CARD
```

The generic installer preserves an existing Dictionary and Calendar tree. Use the dedicated installers for intentional complete-pack replacement.

## Wi-Fi

The normal way to add and manage Wi-Fi networks is on-device: Settings ▸ Network ▸ Configure via phone opens the device's own hotspot and a small web page, reached by scanning a single on-screen QR code to join the hotspot — the page then opens on its own via captive-portal detection. From there, pick a network the device found nearby (or type one in) and enter its password with your phone's keyboard — no typing on the device itself. Up to 8 networks can be saved; Settings ▸ Network ▸ Saved networks lists them and can forget one. See `docs/USER_GUIDE.md` for the full walkthrough.

Editing `/RUSTMIX/WIFI.TXT` by hand remains a supported fallback for headless or bulk provisioning:

```text
ssid1=YOUR_NETWORK
password1=YOUR_PASSWORD
ssid2=ANOTHER_NETWORK
password2=ANOTHER_PASSWORD
timezone=America/New_York
ntp_server=pool.ntp.org
```

Networks are numbered `ssid1=`/`password1=`, `ssid2=`/`password2=`, and so on (up to 8), and are tried in order at boot until one connects. A single unnumbered `ssid=`/`password=` pair is equivalent to `ssid1=`/`password1=`, so an existing single-network file keeps working unchanged.

Supported `timezone` values: `America/New_York`, `Europe/Rome`, `UTC`. All three apply automatic DST transitions except UTC.

The timezone can also be changed on-device from Settings ▸ Clock ▸ Set date & time (first field, cycled with Up/Down). Saving there updates the clock immediately and writes the choice to its own `/RUSTMIX/CLOCK.TXT` file (`timezone=Europe/Rome`), independent of Wi-Fi provisioning, so it survives a reboot — including a real deep-sleep wake, which is a full reboot — even when `WIFI.TXT` does not exist yet. When `WIFI.TXT` is already present, its `timezone=` line is kept in sync too, but `CLOCK.TXT` is the source of truth read at boot.

Do not commit real credentials.

## Weather

Optional `/RUSTMIX/WEATHER.TXT` example:

```text
provider=open-meteo
location=New York, NY
latitude=40.7128
longitude=-74.0060
timezone=America/New_York
refresh_minutes=30
```

## Alarms

Optional `/RUSTMIX/ALARMS.TXT` example:

```text
snooze_minutes=10
alarm=Workday,07:30,weekdays,on,recurring
alarm=Weekend,09:00,weekends,off,recurring
alarm=Appointment,16:45,2026-06-10,on,once
```

Calendar personal events remain separate from alarms.

## Display preferences

`/RUSTMIX/DISPLAY.TXT` supports:

```text
font_family=inter|atkinson-hyperlegible
font_size=compact|standard|large
```

## Sleep images

Files below `/RUSTMIX/SLEEP` must be uncompressed monochrome Windows BMP files:

```text
800 × 480
1-bpp
```

Install bundled samples:

```bash
./scripts/install-sleep-images.sh /Volumes/YOUR_SD_CARD
```

## Reader books and state

Copy TXT, EPUB, or FAT-friendly `.EPU` books into:

```text
/RUSTMIX/BOOKS
```

The device creates Reader state automatically:

```text
/RUSTMIX/READER/STATE.TXT
/RUSTMIX/READER/POSITS.TXT
/RUSTMIX/READER/RECENT.TXT
/RUSTMIX/READER/MARKS.TXT
/RUSTMIX/READER/PREFS.TXT
/RUSTMIX/READER/CACHE/<8HEX>.CCH
```

Reader writes use `.TMP` and `.BAK` siblings for recovery.

## Voice Notes

The device creates:

```text
/RUSTMIX/VOICE/VOICE###.WAV
/RUSTMIX/VOICE/INDEX.TXT
/RUSTMIX/VOICE/META.TXT
/RUSTMIX/VOICE/SETTINGS.TXT
```

Do not hand-edit sidecars while the device is active.

## Complete Dictionary pack

Install from a local `rustmix-x4-firmware` checkout:

```bash
./scripts/install-dictionary-x4-pack.sh \
  --force \
  --x4-repo /Users/piyushdaiya/Documents/projects/rustmix-x4-firmware \
  /Volumes/YOUR_SD_CARD
```

Verify representative lookups:

```bash
./scripts/verify-dictionary-x4-pack.sh /Volumes/YOUR_SD_CARD
```

## U.S.-only Calendar pack

Install from a local X4 checkout:

```bash
./scripts/install-calendar-x4-pack.sh \
  --force \
  --x4-repo /Users/piyushdaiya/Documents/projects/rustmix-x4-firmware \
  /Volumes/YOUR_SD_CARD
```

The installer includes `EVENTS.TXT` and `US2026.TXT`, and explicitly excludes `HINDU26.TXT`.

Calendar personal-event writes use:

```text
EVENTS.TMP -> EVENTS.TXT
EVENTS.BAK retained for rollback
```
