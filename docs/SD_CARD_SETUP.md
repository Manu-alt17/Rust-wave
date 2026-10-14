# SD-card setup

Use a FAT-formatted microSD (FAT32 for cards up to 32 GB; exFAT is not supported). Rustmix Wave mounts it at `/sdcard` with long file names. Books, audiobooks and wallpapers each have a folder at the top of the card, and what the device keeps for itself is in `/System`:

```text
/Books/           books: .txt, .epub (subfolders are not scanned)
/Audiobooks/      audiobooks: one .mp3, or one folder of .mp3 tracks, per book
/Wallpapers/      sleep images: *.BMP
/System/          the device's own files
  WIFI.TXT        saved Wi-Fi networks
  CLOCK.TXT       time zone and interface language
  DISPLAY.TXT     interface text size, sleep screen, automatic standby
  APPS/DICT/      dictionary pack for the in-reader lookup (optional)
    INDEX.TXT
    DATA/*.JSN

  written by the firmware:
  READER/         STATE.TXT, POSITS.TXT, RECENT.TXT, MARKS.TXT, PREFS.TXT
  READER/CACHE/   book text and page indexes, covers, sleep covers, images
  STATS/          one reading-session log per month (YYYYMM.LOG)
  AUDIOPOS.TXT    audiobook positions
  VOLUME.TXT      audio volume (volume=0-100)
  UPDATE.TXT      update channel (channel=stable or beta)
  SLEEPIDX.TXT  SLEEPAT.TXT
  SETUP.TXT       first-start pages: page=N while under way, done=1 after
  BOOTTIME.LOG  RESETS.LOG   diagnostics, moved to .OLD at 64 KB
/LEGGIMI.TXT  /README.TXT   what the folders are for, written on a new card
```

`Books`, `Audiobooks`, `Wallpapers` and `System` are made by the firmware at power-on when they are missing, so a freshly formatted card needs no preparation.

Firmware up to 1.5.0-beta.5 kept everything in one `/RUSTMIX` folder. The first start with a later one moves the card to this layout before reading anything: `RUSTMIX/BOOKS`, `AUDIO` and `SLEEP` become `Books`, `Audiobooks` and `Wallpapers`, the rest goes into `System`, the reading positions and statistics follow their books, and the caches are made again as books are opened. Nothing is overwritten: a file that meets one of the same name with the same contents takes its place, one with different contents is kept as `name (2)`, and `RUSTMIX` goes once it is empty. A folder of an older firmware such as `books` is renamed `Books` (FAT takes the two for one folder, so their contents merge the same way). On a card with no settings and no books the device then shows its first-start pages (see the user guide); deleting `SETUP.TXT` alone does not bring them back on a card that holds settings or books.

Copy files from Home → Carica, over Wi-Fi with the portal or with the USB cable (Connect to PC), or by moving the card to a computer.

## Install bundled examples

```bash
./scripts/install-sd-examples.sh /Volumes/YOUR_SD_CARD
```

Existing paths are preserved by default. Use `--force` only when deliberately replacing bundled example files.

## Books

Copy TXT and EPUB files into `/Books`. Text files may be UTF-8 or Windows-1252. Books get their cover and title the first time the Library shows them.

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

Copy MP3 files into `/Audiobooks`. A file is one audiobook; a folder is one audiobook whose tracks play in name order, numbers counted as numbers (`2` before `10`), so `01 - …`, `02 - …` or `Capitolo 1`, `Capitolo 2` both work. MPEG-1 or 2 layer III, mono or stereo, any sample rate up to 48 kHz.

## Wi-Fi

The normal way to add networks is from a phone: Home → Carica (or Settings → Rete) opens the device's own hotspot and shows a QR code to join it; the setup page then opens by itself. Enter the code shown on screen and add networks and passwords with the phone's keyboard. Up to 8 networks are saved.

`/System/WIFI.TXT` can also be written by hand:

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

Settings → Orologio sets date, time and time zone, and Settings → Lingua the language; both are saved in `/System/CLOCK.TXT`, read at boot:

```text
timezone=Europe/Rome
locale=it
```

`locale` is `it` (the default) or `en`.

## Display and standby

`/System/DISPLAY.TXT`, written by Settings → Schermo:

```text
font_size=compact|standard|large
sleep_screen=sequential|random|fixed|book-cover
auto_sleep=5|10|15|30|60|never
```

`font_size` is the interface text, not the books'. `sleep_screen` picks what stays on the glass in standby: the images in `SLEEP` in name order (default), at random, always the same one (`fixed`: the image named in `SLEEPIDX.TXT`, which is the last one shown or the one chosen in Settings or on the Wi-Fi page), or the cover of the book being read. Firmware older than this value does not know `fixed` and starts with the default display settings. `auto_sleep` is the idle time in minutes before standby (10 by default).

## Sleep images

Files below `/Wallpapers` must be uncompressed monochrome Windows BMP files of the panel's native size:

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
./scripts/relayout-dictionary-pack.sh /path/to/copy/System/APPS/DICT
```

This moves `DATA/CASA.JSN` to `DATA/CA/CASA.JSN` and rewrites `INDEX.TXT` to match. The firmware reads both layouts.
