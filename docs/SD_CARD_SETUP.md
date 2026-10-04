# SD-card setup

Use a FAT-formatted microSD (FAT32 for cards up to 32 GB; exFAT is not supported). Rustmix Wave mounts it at `/sdcard` with long file names, and keeps everything under `/RUSTMIX`:

```text
/RUSTMIX/
  BOOKS/          books: .txt, .epub (subfolders are not scanned)
  AUDIO/          audiobooks: one .mp3, or one folder of .mp3 tracks, per book
  SLEEP/          sleep images: *.BMP
  APPS/DICT/      dictionary pack for the in-reader lookup (optional)
    INDEX.TXT
    DATA/*.JSN
  WIFI.TXT        saved Wi-Fi networks
  CLOCK.TXT       time zone and interface language
  DISPLAY.TXT     interface text size, sleep screen, automatic standby

  written by the firmware:
  READER/         STATE.TXT, POSITS.TXT, RECENT.TXT, MARKS.TXT, PREFS.TXT
  READER/CACHE/   book text and page indexes, covers, sleep covers, images
  STATS/          one reading-session log per month (YYYYMM.LOG)
  AUDIOPOS.TXT    audiobook positions
  VOLUME.TXT      audio volume (volume=0-100)
  MENU.TXT  SLEEPIDX.TXT  SLEEPAT.TXT
  BOOTTIME.LOG  RESETS.LOG   diagnostics, moved to .OLD at 64 KB
```

Copy files from Home → Carica, over Wi-Fi with the portal or with the USB cable (Connect to PC), or by moving the card to a computer.

## Install bundled examples

```bash
./scripts/install-sd-examples.sh /Volumes/YOUR_SD_CARD
```

Existing paths are preserved by default. Use `--force` only when deliberately replacing bundled example files.

## Books

Copy TXT and EPUB files into `/RUSTMIX/BOOKS`. Text files may be UTF-8 or Windows-1252. Books get their cover and title the first time the Library shows them.

Positions, bookmarks and preferences are written by the Reader with `.TMP` and `.BAK` siblings for recovery. `READER/PREFS.TXT`:

```text
version=1
theme=classic|high-contrast
orientation=portrait|landscape
font_size=large|xlarge|xxlarge|xxxlarge
book_font=literata|atkinson-hyperlegible
paragraph_alignment=justified|left|center|right
show_progress=true|false
full_screen=true|false
```

The four `font_size` values are the four sizes in the Reader preferences, from the smallest.

## Audiobooks

Copy MP3 files into `/RUSTMIX/AUDIO`. A file is one audiobook; a folder is one audiobook whose tracks play in name order, numbers counted as numbers (`2` before `10`), so `01 - …`, `02 - …` or `Capitolo 1`, `Capitolo 2` both work. MPEG-1 or 2 layer III, mono or stereo, any sample rate up to 48 kHz.

## Wi-Fi

The normal way to add networks is from a phone: Home → Carica (or Settings → Rete) opens the device's own hotspot and shows a QR code to join it; the setup page then opens by itself. Enter the code shown on screen and add networks and passwords with the phone's keyboard. Up to 8 networks are saved.

`/RUSTMIX/WIFI.TXT` can also be written by hand:

```text
ssid1=YOUR_NETWORK
password1=YOUR_PASSWORD
ssid2=ANOTHER_NETWORK
password2=ANOTHER_PASSWORD
timezone=Europe/Rome
ntp_server=pool.ntp.org
```

Networks are tried in order at boot; with more than one, those in range are tried first. A single unnumbered `ssid=` / `password=` pair works as `ssid1=` / `password1=`. `timezone` is `Europe/Rome` (the default), `America/New_York` or `UTC`. Do not commit real credentials.

## Clock and language

Settings → Orologio sets date, time and time zone, and Settings → Lingua the language; both are saved in `/RUSTMIX/CLOCK.TXT`, read at boot:

```text
timezone=Europe/Rome
locale=it
```

`locale` is `it` (the default) or `en`.

## Display and standby

`/RUSTMIX/DISPLAY.TXT`, written by Settings → Schermo:

```text
font_size=compact|standard|large
sleep_screen=sequential|random|book-cover
auto_sleep=5|10|15|30|60|never
```

`font_size` is the interface text, not the books'. `sleep_screen` picks what stays on the glass in standby: the images in `SLEEP` in name order (default), at random, or the cover of the book being read. `auto_sleep` is the idle time in minutes before standby (10 by default).

## Sleep images

Files below `/RUSTMIX/SLEEP` must be uncompressed monochrome Windows BMP files of the panel's native size:

```text
800 × 480, 1-bpp, black and white palette
```

Install bundled samples:

```bash
./scripts/install-sleep-images.sh /Volumes/YOUR_SD_CARD
```

## Dictionary pack

The in-reader lookup reads the Rustmix X4 dictionary pack. Install it from a local `rustmix-x4-firmware` checkout:

```bash
./scripts/install-dictionary-x4-pack.sh \
  --force \
  --x4-repo /path/to/rustmix-x4-firmware \
  /Volumes/YOUR_SD_CARD
```

Verify representative lookups:

```bash
./scripts/verify-dictionary-x4-pack.sh /Volumes/YOUR_SD_CARD
```

The verifier also checks that `INDEX.TXT` is byte-sorted (the firmware binary-searches it in place) and that every row points at an existing shard.

### Large packs: bucket the shards

A large pack (e.g. the ~28k-shard Italian pack) is slow when every shard lives in one `DATA/` directory, because FAT scans that directory on every file open. Bucket the shards on a copy on your computer, then copy the whole `DICT` folder to the card. Delete the old `DICT` folder on the card first, so the flat shards are not left behind:

```bash
./scripts/relayout-dictionary-pack.sh /path/to/copy/RUSTMIX/APPS/DICT
```

This moves `DATA/CASA.JSN` to `DATA/CA/CASA.JSN` and rewrites `INDEX.TXT` to match. The firmware reads both layouts.
