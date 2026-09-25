# Board contract

Target board: Waveshare ESP32-S3 e-paper 3.97-inch development board.

## Display

```text
Controller     SSD1677
Native frame   800 × 480 monochrome
Logical UI     480 × 800 portrait
SCLK           GPIO11
MOSI           GPIO12
CS             GPIO10
DC             GPIO9
RST            GPIO46
BUSY           GPIO3
```

## User inputs

```text
Rotary wheel / Select   Primary UI navigation
BOOT                    GPIO0, single press hierarchical Back
Hold SELECT             GPIO5, ~900ms hold triggers a route-specific
                        contextual action (keyboard-grid axis toggle,
                        calendar agenda/create, Sudoku/Minesweeper
                        axis-toggle-or-cancel) instead of its normal
                        short-press meaning
Power key               AXP2101 PEK short / long interrupts
```

Power-key product behavior:

```text
Short Power press   Enter random sleep-image mode, then power off via the
                     AXP2101 PMIC (real MCU deep sleep is the automatic
                     fallback if the PMIC power-off cannot be armed)
Long Power press    Open display-maintenance menu
Wake Power press    Turn the board back on (PMIC power-key wake) or restore
                     the retained route after the quiet guard (software-only
                     fallback path)
Wake rotary SELECT  GPIO5 ext1 wakeup from the real MCU deep-sleep fallback
                     (reboots the board)
```

## PMIC power-off

```text
Mechanism       AXP2101 COMMON_CONFIG (register 0x10) soft power-off bit;
                cuts every PMIC rail immediately, no MCU involvement
Boot marker     One PMIC scratch register (DATA_BUFFER1, 0x04) is written
                right before power-off and read + cleared right after boot,
                so a Power-key wake from PMIC power-off can be told apart
                from an ordinary power-on/reset even though the ESP32-S3's
                own wakeup-cause register cannot see the difference
State on wake   Same as a real MCU deep-sleep wake: full reboot, RAM lost,
                classified as `BootCause::PmicPowerKeyOn` -- product-facing
                behavior (Reader auto-resume, route restore) is identical to
                `BootCause::DeepSleepGpioWake` via `BootCause::is_sleep_resume`
Fallback        If the shutdown marker cannot be written, or the PMIC
                power-off call returns instead of cutting power, the board
                falls through to the existing MCU deep-sleep path below
```

See `src/power.rs` (`Axp2101::power_off`, `write_shutdown_marker`, `take_shutdown_marker`).

## MCU deep sleep (fallback)

```text
Wakeup source   ext1 on GPIO5 (rotary SELECT), active low
RTC alarm wake  Not available: GPIO45 is outside the ESP32-S3 RTC IO range
                (GPIO0-21), so it cannot arm an ext1/ext0 deep-sleep wakeup
State on wake   Full reboot, not a resume: RAM is lost, the app restarts and
                lands on Home like any cold boot
Fallback        If arming the GPIO5 wakeup source fails, the board stays in
                the pre-existing software-only sleep-image loop (mcu-sleep
                false) instead of a real deep sleep with no way to wake it
```

See `src/mcu_deep_sleep.rs`.

## Storage

```text
SD mount       /sdcard
Product root   /sdcard/RUSTMIX
Filesystem     FAT; generated writable names must remain FAT 8.3-safe
```

## Audio and sensors

```text
Audio codec        ES8311
RTC alarm input    GPIO45 active low
Environment        SHTC3
IMU                QMI8658
```

Hardware handles remain native-owned in `src/main.rs` and its focused runtime adapters. Lua apps do not receive raw peripheral access.
