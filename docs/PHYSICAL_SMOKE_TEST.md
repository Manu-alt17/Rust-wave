# Consolidated physical smoke test

For screen names, navigation controls, and reference images, see [`USER_GUIDE.md`](USER_GUIDE.md).

Run this checklist after a release build or any cross-cutting runtime change.

## Build and boot

1. Run `./scripts/validate.sh`.
2. Run `cargo +esp build --release`.
3. Flash with `./scripts/flash.sh monitor`.
4. Confirm boot reaches the Home screen without panic or reset loops.
5. Confirm the displayed version is `1.0.0` and the repository-cleanup readiness marker appears.

## Button capture under load

1. Navigate to any menu screen and trigger a global panel refresh (for example, open Library or another route that forces a full refresh).
2. While the refresh is visibly in progress, rapid-press UP/DOWN 5-10 times in quick succession.
3. Confirm every press is eventually reflected once the refresh completes and subsequent refreshes catch up — the final selection position should match the number of presses issued, not fewer, and each press should still produce its own visible step (no presses collapsed or skipped).
4. Repeat holding BOOT (Back) instead of UP/DOWN during a refresh; confirm the Back navigation is still honored afterward, not lost.
5. Repeat with a long SELECT hold started while a refresh is in progress on a route with a contextual long-press action (for example Sudoku or the Calendar agenda); confirm the contextual action still fires afterward.
6. Confirm normal single-press responsiveness (no refresh in progress) feels unchanged from before this change.

## Power key and display refresh

1. Press Power briefly and confirm the display-maintenance menu opens.
2. Select `Clear ghosting now` and confirm a clean global refresh returns to the underlying screen.
3. Press Power briefly, select Cancel, and confirm no sleep transition.
4. Hold Power and confirm random sleep-image mode starts and network services suspend.
5. Wait for the wake quiet guard and press Power to restore the prior route.

## Reader

1. Open one TXT book and one EPUB or `.EPU` book.
2. Confirm staged loading, page navigation, Reader Options, preferences, TOC behavior, and bookmark add/remove.
3. Reboot and confirm Continue Reading restores the prior book and page.
4. Confirm `/RUSTMIX/READER/POSITS.TXT` and `CACHE/<8HEX>.CCH` exist.

## Dictionary

1. Open `Tools > Dictionary`.
2. Confirm `CAB`, `BARN`, and `CALENDAR` exact lookup.
3. Confirm `AAR*` prefix lookup and result cycling.
4. Hold SELECT and confirm `NAV H` / `NAV V` switches without moving the selected key.
5. Press BOOT and confirm hierarchical Back.

## Calendar

1. Open `Productivity > Calendar`.
2. Confirm U.S. event markers and daily agenda rendering.
3. Create, edit, and delete one personal event.
4. Confirm U.S. holiday rows remain read-only.
5. Confirm agenda summary, pagination, first row, and footer do not overlap.
6. Confirm `EVENTS.TMP` is absent after successful write and `EVENTS.BAK` is retained.

## Voice Notes

1. Record a note, pause, resume, and save.
2. Confirm a new `VOICE###.WAV` file persists after reboot.
3. Confirm gain selection persists, metadata is readable, playback works, and delete confirmation works.
4. Confirm LAN export displays a path and protected sidecars are not exposed.

## Network, alarms, and settings

1. Confirm Wi-Fi connection and SNTP status.
2. Open Settings ▸ Network ▸ Configure via phone. Confirm the hotspot SSID/password and the single QR code render. Scan it with a phone camera, confirm it offers to join the hotspot, and confirm the phone then opens the portal page on its own (a "Sign in to network" prompt or an auto-launched browser, depending on the OS) without scanning a second code.
3. From the portal, add a nearby (or manually typed) network with its password and confirm the device reports Connected and returns to the saved-network list; try a wrong password and confirm it reports Failed without saving.
4. Open Settings ▸ Network ▸ Saved networks, confirm the just-added network appears with a Connected badge, then forget it (SELECT twice) and confirm it disappears and `WIFI.TXT` is rewritten.
5. Start the explicit Wi-Fi transfer portal, access it with the displayed code, then stop it.
6. Confirm an alarm can sound, snooze, and dismiss.
7. Confirm alarm behavior is not hidden by the Power-key display menu.
8. Confirm Display settings persist after reboot.

## Games and sensors

1. Open Sudoku and verify rotary movement, held-SELECT axis toggle, edit, and commit.
2. Open one motion game and verify debounced IMU movement.
3. Open Environment and Motion diagnostic screens.
4. Run the audio test chime.

## Text-editor layout alignment

1. Open Voice Notes, select a saved WAV, and choose **Edit friendly title**.
2. Confirm the header reads **VOICE NOTE TITLE / EDIT FRIENDLY TITLE**.
3. Confirm the shared grid keyboard is visible and defaults to `NAV H`.
4. Hold SELECT and confirm `NAV V` appears without moving the selected key.
5. Use `SAVE` to persist a friendly title and confirm the internal `VOICE###.WAV` filename remains unchanged.
6. Reopen the title editor, press BOOT, and confirm the edit is cancelled without saving.
7. Open Calendar, create or edit a personal event, and confirm the status strip shows a compact `YYYY-MM-DD` date plus `NAV H` or `NAV V` without overlap.
8. Confirm the Calendar editor footer is fully visible.
