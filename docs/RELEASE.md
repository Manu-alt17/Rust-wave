# Release generation

Rustmix Wave has three release helpers:

```text
scripts/build-release-firmware.sh  Build an ELF-only firmware release bundle
scripts/flash-release.sh           Flash an existing release ELF safely
scripts/package-release.sh         Package the cleaned GitHub-ready source tree
```

## Supported release artifact

The supported firmware artifact is the ESP-IDF release ELF:

```text
dist/waveshare-epd397-rust-app-v<VERSION>.elf
```

Flash it with the ELF-aware `espflash flash` command:

```bash
./scripts/flash-release.sh \
  dist/waveshare-epd397-rust-app-v<VERSION>.elf
```

Equivalent direct command:

```bash
espflash flash --chip esp32s3 --monitor \
  dist/waveshare-epd397-rust-app-v<VERSION>.elf
```

Ordinary development flashing remains unchanged:

```bash
./scripts/flash.sh monitor
```

## Safety warning: do not use raw-address flashing

Do **not** use `espflash write-bin` for the release ELF or for any artifact from
this repository. `write-bin` is a raw-address operation. A raw write requires an
explicitly validated flash layout and correct bootloader, partition-table, and
application offsets.

The earlier unverified `*-flash.bin` artifact and the `write-bin ... 0x0`
workflow have been removed.

## Future merged factory image

A merged factory-image workflow remains deferred. It may be added only after all
of the following have been validated on physical hardware:

- Bootloader offset and image
- Partition-table offset and image
- Factory application partition offset
- Flash mode, frequency, and size
- Recovery procedure from ROM download mode

Until then, the ELF-aware `espflash flash` path is the only supported release
installation method.

## Build a firmware release

```bash
./scripts/build-release-firmware.sh
```

The script:

1. Runs `./scripts/validate.sh` unless `--skip-validate` is provided.
2. Builds the embedded release ELF with `cargo +esp build --release`.
3. Copies the ELF into `dist/`.
4. Copies the safe `flash-release.sh` helper into `dist/`.
5. Writes SHA-256 checksums.
6. Generates a release ZIP containing the ELF, flashing helper, checksum
   manifest, and flashing instructions.

Output naming:

```text
dist/waveshare-epd397-rust-app-v<VERSION>.elf
dist/waveshare-epd397-rust-app-v<VERSION>-flash-release.sh
dist/waveshare-epd397-rust-app-v<VERSION>-firmware-release.sha256
dist/waveshare-epd397-rust-app-v<VERSION>-FLASHING.txt
dist/waveshare-epd397-rust-app-v<VERSION>-firmware-release.zip
```

No `*-flash.bin` artifact is generated.

## Skip validation during a repeated local build

```bash
./scripts/build-release-firmware.sh --skip-validate
```

Use this only after the exact source tree has already passed
`./scripts/validate.sh`.

## OTA releases and update channels

Devices update themselves from the GitHub releases of the repository in
`src/build_info.rs` (`OTA_REPO_OWNER`/`OTA_REPO_NAME`), using the `.bin`
asset that `./scripts/build-ota-image.sh` builds. The user picks a channel on
the Software Update screen:

- **Stable** reads `releases/latest`: the newest release that is not a
  pre-release. Publish stable releases from `feature/ota-update`, with a plain
  version (`1.4.9`) in `Cargo.toml` and a matching `v1.4.9` tag.
- **Beta** reads the five most recent releases and takes the highest version,
  pre-releases included. Publish beta builds from the `beta` branch with a
  pre-release version (`1.5.0-beta.1`, then `-beta.2`, ...), and tick
  **Set as a pre-release** on GitHub: without it the build becomes the latest
  release and stable devices install it too.

Versions compare with SemVer precedence: `1.5.0-beta.2` comes after
`1.5.0-beta.1` and before `1.5.0`, so the final `1.5.0` replaces the betas on
both channels. Draft releases are never offered. A device switched back from
Beta to Stable keeps its firmware until a newer stable release appears.

## Bootloader updates

A release can also carry the bootloader: attach
`dist/waveshare-epd397-rust-app-v<VERSION>-bootloader.img`, which
`./scripts/build-ota-image.sh` copies from the build. It is named `.img`, not
`.bin`, because firmware before this feature takes the first `.bin` asset as
the app image. A device whose firmware is already up to date then offers it on
the Software Update screen, if it differs from the bootloader in its flash:
the first SELECT downloads it and checks it against the SHA-256 GitHub
publishes for the asset (no digest, no offer), the second writes it, and the
device restarts into it.

Attach it only when the bootloader really changed (the `BOOTLOADER_` options
in `sdkconfig.defaults`, or a new ESP-IDF). The ESP32-S3 has no backup
bootloader to fall back on: a power cut during the write, which takes under a
second, leaves a device that only a USB reflash can recover. The device
refuses the write below 50% battery without the USB cable, stages the image
in unused flash (never in a firmware slot, which rollback may need), and reads
it back after the copy, copying it again if it differs.

Devices flashed with plain `espflash flash` run espflash's own bootloader, not
this project's (`scripts/flash.sh` passes `--bootloader`): the Software Update
screen shows which one a device has.

## Package cleaned source

```bash
./scripts/package-release.sh
```

This produces:

```text
dist/waveshare-epd397-rust-app-v<VERSION>-github-ready.zip
dist/waveshare-epd397-rust-app-v<VERSION>-github-ready.zip.sha256
```

The source package excludes Git metadata, build outputs, generated release
artifacts, local caches, patch scratch files, and extracted overlay directories.

## Version and OTA updates

Before a release, set the version in `Cargo.toml` and `CONFIG_APP_PROJECT_VER` in `sdkconfig.defaults`: a unit test (`build_info::tests::app_descriptor_version_matches_cargo`) fails while the two differ.

The device updates itself only when asked (Settings → Update): it compares its `Cargo.toml` version with the latest GitHub release of `build_info::OTA_REPO_OWNER` / `OTA_REPO_NAME` and installs that release's `.bin` asset. Build the asset with:

```bash
./scripts/build-ota-image.sh
```

`OTA_REPO_OWNER` / `OTA_REPO_NAME` still point at the `Manu-alt17/Rust-wave` test fork: set them to the repository that will publish this firmware's releases before relying on the update.
