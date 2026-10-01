# Rustmix Wave screenshots

This directory contains the reference UI images used by [`docs/USER_GUIDE.md`](../docs/USER_GUIDE.md). Runtime firmware does not read files from this directory.

Two kinds of images live here:

- `rendered-*.png`: screens drawn by the firmware's own renderer on the host (`cargo test --lib export_screen_previews -- --ignored`), pixel for pixel what the panel shows, in logical portrait orientation. They cover every screen that changed with the E-ink merge: Home, Library, Reader, Audiobooks, Statistics, Settings and Connect to PC.
- `*.jpg`: photographs of the device taken before the merge (v1.4.8). The screens they show still exist, but the fonts and some details have changed since.

Photographs of removed features (Calendar, Voice Notes, games, alarms, sensors, Unit Converter, the Dictionary app) and of the old Home, Settings and Library layouts were removed with those features.
