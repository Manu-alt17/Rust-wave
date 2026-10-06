# Third-party notices

The firmware links these ESP-IDF components besides ESP-IDF itself (Apache-2.0). The ESP-IDF Component Manager downloads them at build time, as declared in `Cargo.toml` (`package.metadata.esp-idf-sys.extra_components`) and `components/usbdisk/idf_component.yml`, and pinned in `components_esp32s3.lock`. None of their source is kept in this repository.

| Component | Version | Used for | License |
| --- | --- | --- | --- |
| [chmorgan/esp-libhelix-mp3](https://github.com/chmorgan/esp-libhelix-mp3) | 1.0.3 | MP3 decoding for audiobooks (`src/audio/mp3.rs`) | Component: Apache-2.0. The Helix MP3 decoder inside it: RealNetworks Public Source License (RPSL), see below |
| [espressif/esp_new_jpeg](https://github.com/espressif/esp-adf-libs/tree/master/esp_new_jpeg) | 1.0.2 | Baseline JPEG covers and images (`src/cover_cache.rs`) | Espressif MIT License: use on Espressif chips |
| [espressif/esp_tinyusb](https://github.com/espressif/esp-usb/tree/master/device/esp_tinyusb) | 1.7.6 | Connect to PC (`components/usbdisk`) | Apache-2.0 |
| [espressif/tinyusb](https://github.com/espressif/tinyusb) | 0.21.0 | USB device stack under esp_tinyusb | MIT, © hathach (tinyusb.org) |

## Helix MP3 decoder

The decoder is Copyright (c) 1995-2004 RealNetworks, Inc., part of the Helix DNA Technology, distributed under the RealNetworks Public Source License (RPSL), or under the RealNetworks Community Source License (RCSL) for RCSL licensees. It is used unmodified. The license texts (`RPSL.txt`, `RCSL.txt`) and the decoder's source ship with the component, and its source is available at <https://github.com/chmorgan/esp-libhelix-mp3>. The code is provided "AS IS", without warranty of any kind.

## Fonts

The interface and book fonts are generated from fonts under the SIL Open Font License 1.1: see [`FONT_NOTICES.md`](FONT_NOTICES.md).

## Icons

The interface glyphs are [Iconoir](https://iconoir.com) drawings, MIT License, Copyright (c) 2021 Luca Burgio. The 24 px and 96 px ones come from the [`embedded-iconoir`](https://crates.io/crates/embedded-iconoir) crate (MIT); the 64 px ones on the icon tiles are rasterised from Iconoir 6.11.0 by `tools/icongen/gen_tile_icons.py` into `src/app/widgets/tile_icons.rs`. The SVG files themselves are not distributed with this repository.
