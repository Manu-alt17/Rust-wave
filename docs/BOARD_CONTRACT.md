# Board contract

Target board: Waveshare ESP32-S3 e-paper 3.97-inch development board (ESP32-S3R8, 8 MB octal PSRAM, 16 MB flash).

## Display

```text
Controller     SSD1677
Native frame   800 × 480 monochrome
Logical UI     480 × 800 portrait (the Reader can turn to landscape)
SCLK           GPIO11
MOSI           GPIO12
CS             GPIO10
DC             GPIO9
RST            GPIO46
BUSY           GPIO3
Rail           AXP2101 ALDO3, off while the panel sleeps
```

## User inputs

```text
Rocker up / down        Primary UI navigation
Rocker press (SELECT)   GPIO5; a hold of ~900 ms is the route's contextual
                        action (book actions, Reader options, player menu)
BOOT                    GPIO0, single press hierarchical Back. A strapping
                        pin: never a wake source or a restart trigger
Power key               AXP2101 power-key short / long interrupts
```

Power-key product behavior:

```text
Short Power press   Standby: draw the sleep screen, then power off through
                    the AXP2101 PMIC
Long Power press    Open the display-maintenance menu
Power press (off)   Turn the board back on and restore the retained route
~6 s Power hold     Hardware power-off by the PMIC itself
SELECT press        Wakes the board from the MCU deep-sleep fallback below
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
Wake watchdog   Armed (16 s, all rails off) right before the power-off, and
                disarmed on every boot
State on wake   Full reboot, RAM lost, classified as
                `BootCause::PmicPowerKeyOn`; Reader auto-resume and route
                restore behave as after a deep-sleep wake
Fallback        If the marker cannot be written, or the power-off call
                returns instead of cutting power, the board enters the MCU
                deep sleep below
```

If the power-off returns after the marker was written, the watchdog is already armed: the MCU sleeps, and 16 s later the PMIC cuts power anyway. From then on the Power key turns the board on and the boot finds the marker, as after a normal standby; before that, only SELECT wakes it. Without the marker (the write failed) no watchdog is armed and SELECT is the only way back. The line written to `BOOTTIME.LOG` at standby records which case it was.

See `src/power.rs` (`Axp2101::power_off`, `write_shutdown_marker`, `take_shutdown_marker`, `arm_wake_watchdog`).

## MCU deep sleep (fallback)

```text
Wakeup source   ext1 on GPIO5 (SELECT), active low
State on wake   Full reboot, not a resume: RAM is lost, the app restarts and
                restores the retained route
Fallback        If arming the GPIO5 wakeup source fails, the board stays in
                a software-only sleep loop (and disarms the PMIC watchdog)
                instead of a real deep sleep with no way to wake it
```

See `src/mcu_deep_sleep.rs`.

## Storage

```text
Interface      SDMMC 4-bit, 20 MHz; CMD GPIO17, CLK GPIO16, D0 GPIO15,
               D1 GPIO7, D2 GPIO8, D3 GPIO18
SD mount       /sdcard (FAT, long file names in UTF-8, 5 open files)
Product root   /sdcard/RUSTMIX
DMA            Internal RAM only: reads into PSRAM go through an internal
               buffer (src/sd_io.rs); writes never come straight from flash
```

## USB

```text
PHY            One, shared by the USB serial / JTAG console and the USB
               mass-storage device of Connect to PC
PHY selection  RTC_CNTL bits that survive esp_restart(): reset at every boot
               and before the Connect to PC restart
```

## Audio and other chips

```text
Audio codec        ES8311 on I2C (SDA GPIO41, SCL GPIO42) and I2S
Amplifier          NS4150B, speaker header
RTC                PCF85063
Environment, IMU   SHTC3, QMI8658: put to sleep at boot, not used
```

Hardware handles remain native-owned in `src/main.rs`, the audio engine thread and their focused runtime adapters.
