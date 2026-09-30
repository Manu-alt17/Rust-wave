#[cfg(target_os = "espidf")]
mod firmware {
    use std::{
        time::{Duration, Instant},
    };

    use anyhow::Result;
    use embedded_hal::delay::DelayNs;
    use esp_idf_svc::{
        fs::fatfs::Fatfs,
        hal::{
            delay::{Ets, FreeRtos},
            gpio::{AnyIOPin, PinDriver, Pull},
            i2c::{I2cConfig, I2cDriver},
            peripherals::Peripherals,
            reset::restart,
            sd::{
                mmc::{SdMmcHostConfiguration, SdMmcHostDriver},
                SdCardConfiguration, SdCardDriver,
            },
            spi::{
                config::Config as SpiConfig, Dma, SpiDeviceDriver, SpiDriver, SpiDriverConfig,
                SpiError,
            },
            units::*,
        },
        io::vfs::MountedFatfs,
        log::EspLogger,
        sys,
    };
    use log::{debug, info, warn};
    use waveshare_epd397_rust_app::{
        boot_profile,
        sd_log,
        app::{
            display::{DisplayPreferences, SleepScreenMode, DISPLAY_CONFIG_PATH},
            menu::{CategoryUsage, MENU_USAGE_CONFIG_PATH},
            render_current_screen,
            screens::reader::library_visible_books,
            AppState, ScreenRoute, AUTO_DEEP_SLEEP_ENABLED,
            DEV_BENCH_BUILD, CHARGING_STATUS_POLL_SECONDS, LIBRARY_THUMBNAIL_REFRESH_SECONDS,
            NETWORK_LIVE_REFRESH_SECONDS, NETWORK_LOG_HEARTBEAT_SECONDS, PANEL_IDLE_SLEEP_SECONDS,
            READER_POWER_SAVE_GRACE_SECONDS, SAMPLE_LIVE_REFRESH_SECONDS,
        },
        audio::{
            engine::{AudioCommand, AudioEngine, PlayerCommand},
            espidf::AudioRuntime,
            AudioPlaybackState, AudioSnapshot, AudioUiRequest, AUDIO_MCLK_MULTIPLE,
            DEFAULT_AUDIO_VOLUME_PERCENT,
        },
        audiobook::{
            scan_audiobooks, AudiobookPositions, PlayerRequest, PlayerState, AUDIOBOOK_POSITIONS_PATH,
            AUDIOBOOK_ROOT,
        },
        board_services::{BoardServices, BoardSnapshot},
        build_info::{FIRMWARE_VERSION, PRODUCT_SLUG},
        buttons::{
            BootBackButton, ButtonEvent, Buttons, SelectHoldButton, SelectPressEvent,
            set_select_long_press_ms, READER_SELECT_LONG_PRESS_MS, SELECT_LONG_PRESS_MS,
        },
        cover_cache::CoverCache,
        epaper::{self, Epaper397},
        framebuffer::FrameBuffer,
        input_events::{InputEvent, InputEventQueue},
        mcu_deep_sleep,
        network::{
            espidf::NetworkRuntime, NetworkLogFingerprint, NetworkSnapshot, WifiConnectionState,
        },
        network_config::{
            NetworkConfig, SavedNetwork, DEFAULT_NTP_SERVER, DEFAULT_TIMEZONE, WIFI_CONFIG_PATH,
        },
        network_saved::SavedNetworkEntry,
        ota::{
            espidf::{
                install_update_on_main_task, mark_running_slot_valid, poll_latest_release_check,
                spawn_latest_release_check,
            },
            OtaCheckState, OtaUiRequest, ReleaseCheckError, ReleaseInfo,
            OTA_AUTO_CHECK_ENABLED, OTA_CHECK_INTERVAL_SECONDS,
        },
        panel_refresh::{
            parse_sleep_timestamp, wake_uses_fast_waveform, PanelGlobalReason,
            PanelRefreshCoordinator, PanelRefreshPlan, PanelRefreshRequest,
            FAST_WAKE_MAX_SLEEP_SECONDS, PANEL_PARTIAL_REFRESH_LIMIT,
        },
        power::{self, Axp2101},
        power_key::{
            BootPowerKeyGuard, PowerKeyEvent, SleepWakeGuard, SleepWakeGuardDecision,
            POWER_KEY_BOOT_GUARD_QUIET_MS, POWER_KEY_POLL_MS, POWER_KEY_WAKE_GUARD_QUIET_MS,
        },
        reader::{ReaderDictionaryMode, ReaderTickOutcome, ReadingTheme},
        reading_stats::{
            book_id_for, compute_snapshot, resolve_unix_timestamp, ReadingStatsSnapshot,
            ReadingStatsTracker, STATS_DIRECTORY,
        },
        regional::{Locale, RegionalPreferences, CLOCK_CONFIG_PATH},
        power_profile::{self, PowerProfileTracker, POWER_PROFILE_LOG_SECONDS},
        runtime_memory::{debug_runtime_memory, log_runtime_memory},
        shared_i2c::SharedI2cBus,
        sleep_cover::{compose_cover_sleep_frame, SLEEP_COVER_HEIGHT, SLEEP_COVER_WIDTH},
        sleep_images::{SleepImageCatalog, SleepImageSelection, SLEEP_IMAGE_DIRECTORY},
        sleep_mode::{SleepModeState, SleepWakeCause},
        sleep_network::SleepNetworkState,
        storage::{
            StorageBrowser, StorageSnapshot, StorageUiOutcome, SDMMC_COMMAND_TIMEOUT_MS,
            SDMMC_STABLE_SPEED_KHZ, SD_MOUNT_POINT, STORAGE_IO_RETRY_ATTEMPTS,
        },
        usb_disk::{self, UsbDiskPhase},
        wifi_transfer::{
            espidf::WifiTransferServer, WifiTransferSnapshot, WifiTransferUiRequest,
            NETWORK_PROVISION_RESCAN_SECONDS, WIFI_TRANSFER_ROOT, WIFI_TRANSFER_SERVER_STACK_BYTES,
        },
    };

    /// Temporary diagnostic: appends the `wake-overlay-timing` /
    /// `wake-global-refresh` lines to SD so their timing survives a real
    /// deep-sleep wake even when a USB-serial monitor can't stay attached
    /// across the power cycle (the board's own power path drops the USB
    /// bridge along with everything else on this hardware, unlike a plain
    /// ESP32 deep sleep). Best-effort, like every other SD write in this
    /// file: logs a warning (still visible if a serial monitor happens to be
    /// attached) rather than failing boot if the card isn't mounted or the
    /// write fails. Pull the SD card and open BOOTTIME.LOG in a text editor
    /// to read it back.
    // "BOOTTIME" is exactly 8 characters. The firmware's own files keep
    // 8.3 names, valid with or without FAT long names; before long names
    // were enabled, the first name tried here ("BOOT_TIMING.LOG") failed to
    // create silently.
    const BOOT_TIMING_LOG_PATH: &str = "/sdcard/RUSTMIX/BOOTTIME.LOG";

    /// UTC unix time of the last sleep entry, read back on wake to decide
    /// between the fast and the full waveform (see
    /// `panel_refresh::wake_uses_fast_waveform`). FAT 8.3 name.
    const SLEEP_TIMESTAMP_PATH: &str = "/sdcard/RUSTMIX/SLEEPAT.TXT";

    /// SD record of every reset that is not a plain power-on or deep-sleep
    /// wake, plus the error text of any fatal `firmware::run` exit (see
    /// `main`). A hang or reboot on battery leaves no serial log, so this is
    /// what survives to explain it. FAT 8.3 name, same as `BOOTTIME.LOG`.
    pub(crate) const RESET_LOG_PATH: &str = "/sdcard/RUSTMIX/RESETS.LOG";

    /// Best-effort append of one line to [`RESET_LOG_PATH`].
    pub(crate) fn append_reset_log(line: &str) {
        if let Err(error) = sd_log::append(RESET_LOG_PATH, &format!("{line}\n")) {
            warn!("rustmix-wave=reset-log status=write-failed path={RESET_LOG_PATH} error={error}");
        }
    }

    /// Log why this boot happened and, unless it is an ordinary power-on or
    /// deep-sleep wake, keep it on SD: brownout, watchdog, panic and the
    /// software restart `main` performs after a fatal error each point at a
    /// different cause.
    fn record_reset_reason(sd_mounted: bool) {
        let reason = unsafe { sys::esp_reset_reason() };
        let marker = match reason {
            sys::esp_reset_reason_t_ESP_RST_POWERON => "power-on",
            sys::esp_reset_reason_t_ESP_RST_EXT => "external-pin",
            sys::esp_reset_reason_t_ESP_RST_SW => "software-restart",
            sys::esp_reset_reason_t_ESP_RST_PANIC => "panic",
            sys::esp_reset_reason_t_ESP_RST_INT_WDT => "interrupt-watchdog",
            sys::esp_reset_reason_t_ESP_RST_TASK_WDT => "task-watchdog",
            sys::esp_reset_reason_t_ESP_RST_WDT => "other-watchdog",
            sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deep-sleep-wake",
            sys::esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout",
            sys::esp_reset_reason_t_ESP_RST_USB => "usb",
            sys::esp_reset_reason_t_ESP_RST_JTAG => "jtag",
            sys::esp_reset_reason_t_ESP_RST_PWR_GLITCH => "power-glitch",
            sys::esp_reset_reason_t_ESP_RST_CPU_LOCKUP => "cpu-lockup",
            _ => "unknown",
        };
        let line = format!(
            "rustmix-wave=reset-reason reason={marker} code={reason} version={FIRMWARE_VERSION}"
        );
        info!("{line}");
        let routine = matches!(
            reason,
            sys::esp_reset_reason_t_ESP_RST_POWERON | sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP
        );
        if sd_mounted && !routine {
            append_reset_log(&line);
        }
    }

    /// Append the post-boot phase of the boot profile to SD and print the
    /// whole report to serial. Printing is deferred to here because the
    /// report itself is ~100 lines: logging it during boot would add most of
    /// a second of blocking UART time to what it measures.
    fn finish_boot_profile(reason: &str, sd_mounted: bool) {
        boot_profile::mark_with("boot-profile-finish", Some(reason));
        if sd_mounted {
            let header = format!("=== boot-profile phase=post-boot reason={reason}");
            if let Err(error) = boot_profile::flush_to_file(BOOT_TIMING_LOG_PATH, &header) {
                warn!("rustmix-wave=boot-profile status=flush-failed error={error}");
            }
        }
        for line in boot_profile::finish() {
            info!("{line}");
        }
        info!("rustmix-wave=boot-profile status=finished reason={reason}");
    }

    fn append_boot_timing_log(line: &str) {
        if let Err(error) = sd_log::append(BOOT_TIMING_LOG_PATH, &format!("{line}\n")) {
            warn!(
                "rustmix-wave=boot-timing-log status=write-failed path={BOOT_TIMING_LOG_PATH} error={error:#}"
            );
        }
    }

    pub fn run() -> Result<()> {
        // Wall-clock anchor for `rustmix-wave=wake-overlay-timing` and the
        // `global-refresh-ms` log at the final wake paint, so boot-to-ready
        // time can be measured end to end (e.g. to compare with the overlay
        // disabled) without needing an external stopwatch on the UART log.
        let boot_started = Instant::now();
        // Its timestamp is also the time spent in ROM, bootloader and
        // ESP-IDF startup before this function was entered.
        boot_profile::mark("run-entered");
        let early_span = boot_profile::span("logger-pm-bootcause");
        sys::link_patches();
        // First thing: a restart out of "Connect to PC" -- or a crash while
        // in it -- can leave the USB PHY wired to the disk, and then neither
        // the serial port nor the disk works until power is removed.
        usb_disk::espidf::release_phy();
        EspLogger::initialize_default();
        // sdkconfig.defaults starts every log at WARN: each line also goes
        // out on UART0 at 115200 baud, about 87 us of busy wait per
        // character, and there were info lines for every key press. Bench
        // builds put ESP-IDF's and the firmware's info lines back.
        if DEV_BENCH_BUILD {
            unsafe { sys::esp_log_level_set(c"*".as_ptr(), sys::esp_log_level_t_ESP_LOG_INFO) };
        } else {
            log::set_max_level(log::LevelFilter::Warn);
        }
        info!("rustmix-wave=epd397-rust-app-start");
        if DEV_BENCH_BUILD {
            warn!("rustmix-wave=dev-bench-build auto-sleep=off dfs=off light-sleep=off");
        }

        // Battery optimization: let the CPU drop to XTAL frequency (40 MHz)
        // and, whenever every FreeRTOS task is blocked/suspended for long
        // enough, into automatic light sleep, instead of always running at
        // the configured 240 MHz ceiling. Enabled here for the ordinary
        // active-use main loop; `mcu_deep_sleep::espidf::enter` disables it
        // again immediately before arming GPIO5's real deep-sleep EXT1
        // wakeup (the two were observed to conflict when both were active
        // at once -- see that function's docs) and restores it if the
        // real-deep-sleep attempt fails and the software-only fallback loop
        // keeps running instead. Wi-Fi's own modem sleep already defaults to
        // WIFI_PS_MIN_MODEM and is unaffected by this call.
        // A development bench build (see `DEV_BENCH_BUILD`) keeps the CPU at
        // full speed and out of light sleep instead, for a reliable USB log.
        let pm_config = sys::esp_pm_config_t {
            max_freq_mhz: mcu_deep_sleep::CPU_MAX_FREQ_MHZ,
            min_freq_mhz: if DEV_BENCH_BUILD {
                mcu_deep_sleep::CPU_MAX_FREQ_MHZ
            } else {
                mcu_deep_sleep::CPU_MIN_FREQ_MHZ
            },
            light_sleep_enable: !DEV_BENCH_BUILD,
        };
        match unsafe { sys::esp_pm_configure((&raw const pm_config).cast::<core::ffi::c_void>()) } {
            sys::ESP_OK => debug!(
                "rustmix-wave=power-management status=enabled max-mhz={} min-mhz={} light-sleep={}",
                pm_config.max_freq_mhz, pm_config.min_freq_mhz, pm_config.light_sleep_enable
            ),
            error => warn!("rustmix-wave=power-management status=failed error-code={error}"),
        }
        power_profile::log_build_status();
        debug!(
            "rustmix-wave=product-ui-shell-start product={PRODUCT_SLUG} version={FIRMWARE_VERSION}"
        );
        let mut boot_cause = mcu_deep_sleep::espidf::boot_cause();
        debug!(
            "rustmix-wave=boot-cause status=classified cause={} wake-gpio={}",
            boot_cause.marker(),
            mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO
        );

        early_span.end();
        let peripherals = Peripherals::take()?;


        // The uploaded Waveshare sample uses SDMMC in 4-bit mode:
        // CMD GPIO17, CLK GPIO16, D0 GPIO15, D1 GPIO7, D2 GPIO8, D3 GPIO18.
        // Mount failure is non-fatal so the verified product shell still boots
        // when no card is inserted.
        let sd_span = boot_profile::span("sd-mount");
        let mounted_sd = (|| {
            let host = SdMmcHostDriver::new_4bits(
                peripherals.sdmmc1,
                peripherals.pins.gpio17,
                peripherals.pins.gpio16,
                peripherals.pins.gpio15,
                peripherals.pins.gpio7,
                peripherals.pins.gpio8,
                peripherals.pins.gpio18,
                None::<AnyIOPin>,
                None::<AnyIOPin>,
                &SdMmcHostConfiguration::new(),
            )?;
            let mut card_config = SdCardConfiguration::new();
            card_config.speed_khz = SDMMC_STABLE_SPEED_KHZ;
            card_config.command_timeout_ms = SDMMC_COMMAND_TIMEOUT_MS;
            let card = SdCardDriver::new_mmc(host, &card_config)?;
            let fatfs = Fatfs::new_sdcard(0, card)?;
            MountedFatfs::mount(fatfs, SD_MOUNT_POINT, 5)
        })();
        let mounted_sd = match mounted_sd {
            Ok(mounted) => {
                debug!(
                    "rustmix-wave=sdmmc-mount status=ready mount={SD_MOUNT_POINT} mode=4bit-fat access=ui-readonly speed-khz={SDMMC_STABLE_SPEED_KHZ} timeout-ms={SDMMC_COMMAND_TIMEOUT_MS} retry-attempts={STORAGE_IO_RETRY_ATTEMPTS}"
                );
                Some(mounted)
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=sdmmc-mount status=unavailable mount={SD_MOUNT_POINT} mode=4bit-fat access=ui-readonly speed-khz={SDMMC_STABLE_SPEED_KHZ} timeout-ms={SDMMC_COMMAND_TIMEOUT_MS} retry-attempts={STORAGE_IO_RETRY_ATTEMPTS} error={error:#}"
                );
                None
            }
        };
        sd_span.end();
        let reset_reason_span = boot_profile::span("reset-reason-record");
        record_reset_reason(mounted_sd.is_some());
        reset_reason_span.end();
        let mut storage_browser = StorageBrowser::new(SD_MOUNT_POINT, mounted_sd.is_some());
        // Held for the whole run, and dropped (unmounting the card) only to
        // hand the card to a PC in "Connect to PC" mode.
        let mut mounted_sd = mounted_sd;

        // The e-paper panel and the I2C-driven PMIC rail that powers it are
        // constructed here (cheap: `Epaper397::new` only sets pin state, no
        // I/O), well ahead of the display/network config loads and sensor
        // bring-up below. Actually powering/initializing the
        // panel is deferred to right before the first real paint, same as
        // any other boot -- see the merged paint block further down.
        //
        // PMIC (power key) and RTC share this bus and are
        // polled continuously by the main loop regardless of which screen is
        // active. Without an explicit hardware timeout, esp-idf-hal's
        // embedded_hal::i2c::I2c impl blocks each transaction for BLOCK
        // (TickType_t::MAX, i.e. forever): a single stuck SCL line (a slave
        // glitch, electrical noise) would then freeze the entire
        // single-threaded event loop, including power-key polling itself,
        // with no way to recover short of a reset. This bounds every
        // transaction so a wedged bus surfaces as a recoverable I2C error
        // instead of a silent, total lockup.
        const I2C_BUS_TIMEOUT_MS: u64 = 20;
        let i2c_span = boot_profile::span("i2c-init");
        let i2c_config = I2cConfig::new()
            .baudrate(400.kHz().into())
            .timeout(Duration::from_millis(I2C_BUS_TIMEOUT_MS).into());
        let i2c = I2cDriver::new(
            peripherals.i2c0,
            peripherals.pins.gpio41,
            peripherals.pins.gpio42,
            &i2c_config,
        )?;
        let shared_i2c = SharedI2cBus::new(i2c);
        let mut panel_power = Axp2101::new(shared_i2c.clone());
        i2c_span.end();
        let pmic_span = boot_profile::span("pmic-boot-reads");

        // PMIC-side corroboration for `boot_cause`: the ESP32-S3's own
        // wakeup-cause register cannot tell a PMIC power-key wake (a real
        // power-on from the PMIC's point of view) apart from an external
        // reset or a fresh power-on -- see
        // `mcu_deep_sleep::BootCause::from_raw_wakeup_cause`. Reading and
        // immediately clearing the shutdown marker must happen here, before
        // `panel_power` moves into `Epaper397::new` below and before
        // anything else can touch that register, so a stale marker can never
        // survive into a later, unrelated reset.
        let pmic_shutdown_marker = match panel_power.take_shutdown_marker() {
            Ok(marker) => marker,
            Err(error) => {
                warn!("rustmix-wave=pmic-shutdown-marker status=read-failed error={error:#}");
                false
            }
        };
        // The sleep path arms the PMIC watchdog before powering off, so a
        // wake that ends up in ROM download mode (Back/GPIO0 held) powers
        // itself off instead of hanging. Firmware is running, so disarm it.
        if let Err(error) = panel_power.disarm_wake_watchdog() {
            warn!("rustmix-wave=pmic-wake-watchdog status=disarm-failed error={error:#}");
        }
        // Where the previous run stopped (see `power::LifecycleStage`), read
        // before this run overwrites it.
        let previous_stage = match panel_power.read_lifecycle_stage() {
            Ok(raw) => power::lifecycle_stage_name(raw),
            Err(error) => {
                warn!("rustmix-wave=pmic-lifecycle-stage status=read-failed error={error:#}");
                "read-failed"
            }
        };
        if let Err(error) = panel_power.write_lifecycle_stage(power::LifecycleStage::BootPmicUp) {
            warn!("rustmix-wave=pmic-lifecycle-stage status=write-failed error={error:#}");
        }
        if pmic_shutdown_marker && boot_cause == mcu_deep_sleep::BootCause::PowerOnOrReset {
            boot_cause = mcu_deep_sleep::BootCause::PmicPowerKeyOn;
        }
        let pwron_status = match panel_power.read_power_on_off_source() {
            Ok((pwron, pwroff)) => {
                info!(
                    "rustmix-wave=pmic-power-source status=logged pwron=0x{pwron:02X} pwroff=0x{pwroff:02X}"
                );
                boot_profile::mark_with(
                    "pmic-power-source",
                    Some(format!(
                        "pwron=0x{pwron:02X} pwroff=0x{pwroff:02X} shutdown-marker={pmic_shutdown_marker} previous-stage={previous_stage}"
                    )),
                );
                Some(pwron)
            }
            Err(error) => {
                warn!("rustmix-wave=pmic-power-source status=read-failed error={error:#}");
                None
            }
        };

        // Plugging a USB cable into a device the firmware powered off makes
        // the AXP2101 power the board back on by itself (VBUS insert is a
        // hardware power-on source). The user did not ask for that, so go
        // straight back off before the panel is touched: the sleep image
        // stays on the glass and charging continues with the rails down.
        // The marker is rewritten first so the next real Power-key press
        // still resumes like any other PMIC wake. Limited to a genuine
        // ESP32-S3 power-on reset so a flash/monitor reset over USB (which
        // leaves `PWRON_STATUS` untouched) never bounces the board off.
        if pmic_shutdown_marker
            && pwron_status.is_some_and(power::is_vbus_insert_power_on)
            && unsafe { sys::esp_reset_reason() } == sys::esp_reset_reason_t_ESP_RST_POWERON
        {
            info!("rustmix-wave=pmic-vbus-power-on status=returning-off shutdown=pmic");
            match panel_power.write_shutdown_marker() {
                Ok(()) => {
                    if let Err(error) = panel_power.power_off() {
                        warn!("rustmix-wave=pmic-power-off status=failed reason=vbus-insert error={error:#}");
                    } else {
                        warn!("rustmix-wave=pmic-power-off status=returned-unexpectedly reason=vbus-insert");
                    }
                    // Same fallback as the sleep path: real MCU deep sleep,
                    // woken by SELECT. Only returns if it could not be armed,
                    // in which case boot simply continues as normal.
                    if let Err(error) = mcu_deep_sleep::espidf::enter() {
                        warn!("rustmix-wave=mcu-deep-sleep status=failed reason=vbus-insert error={error:#}");
                        if let Err(error) = mcu_deep_sleep::espidf::set_light_sleep_enabled(true) {
                            warn!(
                                "rustmix-wave=power-management status=light-sleep-restore-failed error={error:#}"
                            );
                        }
                    }
                }
                Err(error) => warn!(
                    "rustmix-wave=pmic-shutdown-marker status=write-failed reason=vbus-insert error={error:#}"
                ),
            }
        }
        match panel_power.read_power_key_timing_config() {
            Ok((pwroff_en, irq_off_on_level)) => {
                info!(
                    "rustmix-wave=pmic-power-key-timing status=logged pwroff-en=0x{pwroff_en:02X} irq-off-on-level=0x{irq_off_on_level:02X}"
                );
                boot_profile::mark_with(
                    "pmic-power-key-timing",
                    Some(format!(
                        "pwroff-en=0x{pwroff_en:02X} irq-off-on-level=0x{irq_off_on_level:02X}"
                    )),
                );
            }
            Err(error) => warn!(
                "rustmix-wave=pmic-power-key-timing status=read-failed error={error:#}"
            ),
        }
        info!(
            "rustmix-wave=boot-cause status=classified-final cause={} wake-gpio={} shutdown=pmic fallback=deep-sleep",
            boot_cause.marker(),
            mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO
        );

        pmic_span.end();
        let panel_construct_span = boot_profile::span("spi-gpio-panel-construct");
        let spi_driver_config = SpiDriverConfig::new().dma(Dma::Auto(4096));
        let spi_driver = SpiDriver::new(
            peripherals.spi3,
            peripherals.pins.gpio11,
            peripherals.pins.gpio12,
            None::<AnyIOPin>,
            &spi_driver_config,
        )?;
        let spi_config = SpiConfig::new().baudrate(20.MHz().into()).write_only(true);
        let spi = PanelSpi(SpiDeviceDriver::new(
            spi_driver,
            None::<AnyIOPin>,
            &spi_config,
        )?);

        let dc = PinDriver::output(peripherals.pins.gpio9)?;
        let reset = PinDriver::output(peripherals.pins.gpio46)?;
        let cs = PinDriver::output(peripherals.pins.gpio10)?;
        // GPIO3 is display busy. Do not reuse it for rotary or app input.
        let busy = PinDriver::input(peripherals.pins.gpio3, Pull::Up)?;

        let mut panel = Epaper397::new(spi, dc, reset, cs, busy, FreeRtosDelay, panel_power)?;
        // GPIO5 may still be RTC-owned from an `ext1` deep-sleep wakeup armed
        // by `mcu_deep_sleep::espidf::enter` on the previous cycle. Release it
        // back to the digital domain before the SELECT PinDriver claims it.
        mcu_deep_sleep::espidf::release_wake_pin()?;
        panel_construct_span.end();

        // Real deep sleep is a full reboot: nothing in RAM survived, but the
        // e-paper image itself needs no redraw to "stay" since it is still
        // physically on the glass with no power applied. Boot proceeds
        // exactly the same as any other boot from here (SD-backed config
        // loads, sensor bring-up), ending in the same single clean global
        // refresh once everything is ready -- see the merged paint block
        // further down. There used to be an intermediate "RIATTIVAZIONE"
        // overlay shown here to cover that gap; measured on real hardware it
        // cost ~100-120ms once its own wait was already overlapped with the
        // rest of boot (see the removed `wake-overlay-timing` log), not
        // worth keeping for that little.

        let config_span = boot_profile::span("config-display");
        let display_preferences = match DisplayPreferences::load_from_path(DISPLAY_CONFIG_PATH) {
            Ok(preferences) => {
                debug!(
                    "rustmix-wave=display-config status=ready path={DISPLAY_CONFIG_PATH} font-size={} auto-sleep={}",
                    preferences.font_size.marker(),
                    preferences.auto_sleep.marker()
                );
                preferences
            }
            Err(error) => {
                let preferences = DisplayPreferences::default();
                warn!(
                    "rustmix-wave=display-config status=default path={DISPLAY_CONFIG_PATH} font-size={} auto-sleep={} error={error:#}",
                    preferences.font_size.marker(),
                    preferences.auto_sleep.marker()
                );
                preferences
            }
        };

        // Credentials are read from removable storage. Never log the password.
        // Mutable so a save from the on-device Wi-Fi setup screen keeps this
        // cache in sync for later sleep/wake reconnects.
        config_span.end();
        let config_span = boot_profile::span("config-wifi");
        let mut network_config = match NetworkConfig::load_from_path(WIFI_CONFIG_PATH) {
            Ok(config) => {
                debug!(
                    "rustmix-wave=wifi-config status=ready path={WIFI_CONFIG_PATH} ssid={} saved-networks={} timezone={} ntp-server={}",
                    first_saved_ssid(&config), config.networks.len(), config.timezone, config.ntp_server
                );
                Some(config)
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=wifi-config status=unavailable path={WIFI_CONFIG_PATH} error={error:#}"
                );
                None
            }
        };

        config_span.end();
        let mut board_services = BoardServices::new(shared_i2c.clone());

        // Schematic trace confirmed ALDO1, ALDO4, BLDO1, BLDO2, CPUSLDO,
        // DLDO1, DLDO2, and DCDC2-4 have no downstream load on this board
        // revision (unmounted resistor, no net, or no inductor on LX), so
        // disable them once at boot regardless of the PMIC's power-on
        // default. DCDC1 (system VCC3V3) and DCDC5 are never touched.
        let mut misc_power = Axp2101::new(shared_i2c.clone());
        let rails_span = boot_profile::span("pmic-unused-rails-disable");
        match misc_power.disable_unused_pmic_rails() {
            Ok(()) => debug!("rustmix-wave=pmic-unused-rails-disable status=done rails=aldo1,aldo4,bldo1,bldo2,cpusldo,dldo1,dldo2,dcdc2,dcdc3,dcdc4"),
            Err(error) => {
                warn!("rustmix-wave=pmic-unused-rails-disable status=failed error={error:#}")
            }
        }

        rails_span.end();
        let inputs_span = boot_profile::span("gpio-inputs-and-input-thread");
        let buttons = Buttons::new(
            PinDriver::input(peripherals.pins.gpio4, Pull::Up)?,
            PinDriver::input(peripherals.pins.gpio6, Pull::Up)?,
        );
        let select_button =
            SelectHoldButton::new(PinDriver::input(peripherals.pins.gpio5, Pull::Up)?);
        let back_button = BootBackButton::new(PinDriver::input(peripherals.pins.gpio0, Pull::Up)?);
        debug!(
            "rustmix-wave=boot-button-back status=ready gpio=0 active-low=true press=short action=back"
        );
        debug!(
            "rustmix-wave=select-button-hold status=ready gpio=5 active-low=true short-press=confirm hold-ms={SELECT_LONG_PRESS_MS} long-press=contextual-navigation"
        );
        // The PCF85063 active-low alarm output is wired to GPIO45, a boot
        // strapping pin. Nothing reads it any more (the RTC alarm stays
        // disabled, see `disable_rtc_alarm` below), but the line keeps its
        // pull-up so it never floats while the firmware runs.
        let _rtc_alarm_int_line = PinDriver::input(peripherals.pins.gpio45, Pull::Up)?;
        // Every boot-time pin driver is configured by now (audio's are only
        // claimed later, lazily). Automatic light sleep has been allowed
        // since `esp_pm_configure` at the top of `run`, so keep the awake
        // pin configuration from here on rather than only once the main loop
        // starts: the panel's RST line is held from the next statement on,
        // across the rest of boot.
        keep_gpio_state_in_light_sleep();

        // Power the panel and pulse its hardware reset now, then let the
        // controller finish resetting while the config loads and sensor
        // bring-up below run, instead of sleeping through that time right
        // before the first paint (`finish_initialize` further down).
        let panel_begin_span = boot_profile::span("panel-begin-initialize");
        panel.begin_initialize()?;
        let panel_reset_released_at = Instant::now();
        panel_begin_span.end();

        // Button GPIOs are polled on a dedicated background thread so a
        // press is never dropped while the main loop is stuck busy-waiting
        // inside a slow e-paper panel refresh (`Epaper397::wait_until_idle`
        // can block for hundreds of ms up to ~1.5s). The thread only detects
        // and enqueues events; the main loop remains the sole owner of
        // `AppState` and all rendering, draining one event per tick in FIFO
        // order.
        const INPUT_POLL_STACK_BYTES: usize = 8 * 1024;
        // Sampling period while no key is down. Only the *first* sample of a
        // press depends on it -- each `poll` follows a detected press at
        // 10 ms steps until release -- so it bounds detection latency, not
        // debounce or long-press timing. 50 ms rather than the former 10 ms:
        // ESP-IDF only enters automatic light sleep when every task stays
        // idle for at least CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP (3 ticks
        // = 30 ms), and the PM-profiling build measured zero light-sleep
        // entries with this thread waking 100 times a second. A real key
        // press lasts well over 50 ms, so it is still seen.
        const INPUT_POLL_IDLE_SLEEP_MS: u64 = 50;
        let input_queue = InputEventQueue::default();
        {
            let input_queue = input_queue.clone();
            std::thread::Builder::new()
                .name("input-poll".into())
                .stack_size(INPUT_POLL_STACK_BYTES)
                .spawn(move || {
                    let (mut buttons, mut back_button, mut select_button) =
                        (buttons, back_button, select_button);
                    let mut delay = FreeRtosDelay;
                    loop {
                        let mut activity = false;
                        match back_button.poll(&mut delay) {
                            Ok(true) => {
                                boot_profile::mark_with("input-detected", Some("back"));
                                input_queue.push(InputEvent::Back);
                                activity = true;
                            }
                            Ok(false) => {}
                            Err(error) => warn!(
                                "rustmix-wave=input-poll-thread component=back status=read-failed error={error:#}"
                            ),
                        }
                        match select_button.poll(&mut delay) {
                            Ok(Some(SelectPressEvent::LongPress)) => {
                                boot_profile::mark_with("input-detected", Some("select-long"));
                                input_queue.push(InputEvent::SelectLongPress);
                                activity = true;
                            }
                            Ok(Some(SelectPressEvent::ShortPress)) => {
                                boot_profile::mark_with("input-detected", Some("select"));
                                input_queue.push(InputEvent::Button(ButtonEvent::Select));
                                activity = true;
                            }
                            Ok(None) => {}
                            Err(error) => warn!(
                                "rustmix-wave=input-poll-thread component=select status=read-failed error={error:#}"
                            ),
                        }
                        match buttons.poll(&mut delay) {
                            Ok(Some(event)) => {
                                boot_profile::mark_with("input-detected", Some("up-down"));
                                input_queue.push(InputEvent::Button(event));
                                activity = true;
                            }
                            Ok(None) => {}
                            Err(error) => warn!(
                                "rustmix-wave=input-poll-thread component=up-down status=read-failed error={error:#}"
                            ),
                        }
                        if !activity {
                            std::thread::sleep(std::time::Duration::from_millis(
                                INPUT_POLL_IDLE_SLEEP_MS,
                            ));
                        }
                    }
                })?;
        }
        debug!(
            "rustmix-wave=input-poll-thread status=ready stack-bytes={INPUT_POLL_STACK_BYTES} idle-sleep-ms={INPUT_POLL_IDLE_SLEEP_MS}"
        );

        inputs_span.end();
        let appstate_span = boot_profile::span("appstate-and-regional-init");
        let mut service_delay = FreeRtosDelay;
        let mut frame = FrameBuffer::new_white();
        // Keep the growing product UI state off the firmware main-task stack.
        // HTTPS requests and display refreshes still execute from the
        // same orchestrator, but their stack budget is no longer reduced by a
        // long-lived inline AppState allocation.
        let mut state = Box::new(AppState::default());
        let mut panel_refresh = PanelRefreshCoordinator::default();
        sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
        state.display = display_preferences;
        // A missing file (first boot) just keeps the default history.
        if let Ok(usage) = CategoryUsage::load_from_path(MENU_USAGE_CONFIG_PATH) {
            state.category_usage = usage;
        }
        // Reader SD catalog scans, and the audio codec
        // bring-up right after them, are deferred until after the first
        // e-paper frame is visible (see below the panel draw). The Home
        // screen's menu tiles are a static const list and never read these,
        // so nothing before the first frame needs them.
        let mut sleep_images = SleepImageCatalog::default();
        let mut sleep_mode = SleepModeState::default();
        let mut sleep_wake_guard = SleepWakeGuard::default();
        let mut sleep_wake_guard_started_at: Option<Instant> = None;
        // Defense in depth against the physical press that just woke the
        // board (PMIC power-key or GPIO5) leaving (or re-latching) a Power-key
        // event right as the loop below starts polling; see
        // `power_key::BootPowerKeyGuard`.
        let mut boot_power_key_guard = BootPowerKeyGuard::default();
        let mut sleep_network = SleepNetworkState::default();
        if let Some(config) = network_config.as_ref() {
            state.regional = state.regional.with_timezone_name(&config.timezone)?;
            state.update_network_snapshot(NetworkSnapshot::provisioned(config));
        }
        // A timezone chosen on-device from Clock > Set date & time is saved
        // to its own file rather than only to WIFI.TXT, so it survives a
        // reboot (a real deep-sleep wake is a full reboot) even when Wi-Fi
        // has never been configured. When present, it overrides whatever
        // WIFI.TXT's `timezone=` line above set, since it reflects the more
        // recent explicit on-device choice.
        match RegionalPreferences::load_from_path(CLOCK_CONFIG_PATH) {
            Ok(saved) => {
                state.regional.timezone = saved.timezone;
                state.regional.locale = saved.locale;
                debug!(
                    "rustmix-wave=clock-config status=ready path={CLOCK_CONFIG_PATH} timezone={} locale={}",
                    saved.timezone_name(),
                    saved.locale.name()
                );
            }
            Err(error) => {
                debug!(
                    "rustmix-wave=clock-config status=unavailable path={CLOCK_CONFIG_PATH} error={error:#}"
                );
            }
        }
        state.update_storage_snapshot(storage_browser.snapshot());
        log_storage_snapshot(&state.storage);
        debug!(
            "rustmix-wave=regional-profile timezone={} display-offset={} rtc-storage-offset={}",
            state.regional.timezone_name(),
            state.regional.timezone_label_for_rtc(state.board.rtc),
            state.regional.rtc_storage_label()
        );

        appstate_span.end();
        let board_init_span = boot_profile::span("board-services-init");
        let init = board_services.initialize(&mut service_delay);
        // This firmware has no alarms, but an earlier one (or the factory
        // image) may have left the PCF85063 alarm armed. Its interrupt line
        // is GPIO45, a boot strapping pin, so keep it disabled and its flag
        // clear from the first boot on.
        if let Err(error) = board_services.disable_rtc_alarm() {
            warn!("rustmix-wave=rtc-alarm-disable status=failed reason=boot error={error:#}");
        }
        board_init_span.end();
        debug!(
            "rustmix-wave=board-services-init rtc={} power={} rtc-integrity-lost={} shtc3=sleep:{} qmi8658=off:{} qmi8658-revision={}",
            init.rtc_available,
            init.power_monitoring_available,
            init.rtc_clock_integrity_was_lost,
            init.environment_sensor_id
                .map_or_else(|| "unavailable".into(), |id| format!("0x{id:04X}")),
            init.imu_address
                .map_or_else(|| "unavailable".into(), |value| format!("0x{value:02X}")),
            init.imu_revision
                .map_or_else(|| "unavailable".into(), |value| format!("0x{value:02X}"))
        );
        let power_key_span = boot_profile::span("power-key-init");
        let mut power_key_available = match board_services.initialize_power_key_events() {
            Ok(()) => {
                debug!("rustmix-wave=power-key status=ready source=axp2101-pek events=short-sleep,long-menu poll-ms={POWER_KEY_POLL_MS}");
                true
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=power-key status=unavailable source=axp2101-pek error={error:#}"
                );
                false
            }
        };
        power_key_span.end();
        let snapshot_span = boot_profile::span("board-snapshot-read");
        state.update_board_snapshot(board_services.read_light_snapshot());
        snapshot_span.end();
        log_board_snapshot(state.board, state.regional);

        // A real hardware deep-sleep wake is a full reboot: nothing in RAM,
        // including the router's route, survived. Reader persistence is
        // loaded before the first frame on every boot: ~45 ms of small text
        // reads, against the ~530 ms partial refresh (plus a visible flash)
        // it took to correct Home's Continue Reading card when it was loaded
        // after the first frame instead. When the durable marker recorded at
        // the last deep-sleep entry (see the short-press handler below) says
        // the Reader was active, the book is also reopened here so the very
        // first frame routes straight into it instead of Home.
        let marker_span = boot_profile::span("reader-deep-sleep-marker-check");
        let resume_reader_on_wake = boot_cause.is_sleep_resume()
            && mounted_sd.is_some()
            && state.reader.deep_sleep_marker_indicates_active();
        marker_span.end();
        let reader_persistence = state.reader.load_persistent_state();
        if resume_reader_on_wake {
            let _reopen_span = boot_profile::span("reader-reopen");
            if state.reader.request_continue() {
                // Drive the reopen to completion right here instead of just
                // queuing it for the main loop: the book being resumed was
                // the one just read before sleep, so its `.EPX`/`.EPP` cache
                // is normally warm and this finishes in well under a second
                // (see `reader-stage-timing` in the boot logs), comfortably
                // inside the 2s bound below. Finishing here means the first
                // real frame draws the book page directly instead of a
                // loading-bar screen that a second refresh would then
                // replace. Bounded so a genuine cold reopen (stale/missing
                // cache) still falls back to the ordinary, visible
                // `ReaderLoading` screen rather than stalling this paint --
                // audio codec, Wi-Fi and the SD catalogs run after it now,
                // so they no longer add to that wait either way.
                let deadline = Instant::now() + Duration::from_millis(2000);
                loop {
                    if state.reader.loading.is_none() || Instant::now() >= deadline {
                        break;
                    }
                    let mut tick_span = boot_profile::span("reader-tick");
                    let outcome = state.tick_reader();
                    tick_span.detail(format_args!("{outcome:?}"));
                    tick_span.end();
                    if outcome == ReaderTickOutcome::Failed {
                        break;
                    }
                }
                if state.reader.loading.is_some() {
                    state.router.navigate_to(ScreenRoute::ReaderLoading);
                    info!(
                        "rustmix-wave=deep-sleep-restore status=reader-resume-requested policy=visible-loading-screen"
                    );
                } else {
                    info!(
                        "rustmix-wave=deep-sleep-restore status=reader-resume-requested policy=silent-warm-resume"
                    );
                }
            } else {
                info!("rustmix-wave=deep-sleep-restore status=no-resumable-book");
            }
        }

        // Everything Home's Continue Reading card draws, so the first frame
        // is already complete: the reading stats behind its remaining-time
        // clause, and the book cover. Only an already-cached thumbnail is
        // used here -- generating one can take seconds -- and a missing one
        // is built by the main loop's Continue Reading block, which repaints
        // the card once it exists.
        let stats_span = boot_profile::span("reading-stats-snapshot");
        refresh_reading_stats_snapshot_now(&mut state);
        stats_span.end();
        if state.active_route() == ScreenRoute::Home {
            let cover_cache = CoverCache::new(state.reader.cache_directory());
            sync_continue_reading_thumbnail(&mut state, &cover_cache);
        }

        let panel_finish_span = boot_profile::span("panel-finish-initialize");
        let reset_recovery = Duration::from_millis(u64::from(epaper::RESET_RECOVERY_MS));
        if let Some(remaining) = reset_recovery.checked_sub(panel_reset_released_at.elapsed()) {
            std::thread::sleep(remaining);
        }
        panel.finish_initialize()?;
        panel_finish_span.end();
        // This is the single global refresh that shows the real first
        // screen, on every boot cause. None of the work above (display/
        // network config loads, board services, the
        // reader-resume decision) touches the panel, and none of it affects
        // what Home/Reader draws either (Home's menu tiles are a static
        // const list, and reader/audio state isn't read by Home), so
        // painting here matches the timing cold boot always used. The
        // button-polling loop still doesn't start draining `input_queue`
        // until the deferred work further below finishes (same as it always
        // has), so a press between this paint and then is queued, not
        // dropped or silently ignored.
        if boot_cause.is_sleep_resume() {
            let render_span = boot_profile::span("render-first-frame");
            render_current_screen(&mut frame, &state)?;
            render_span.end();
            // Timed the same way as `wake-overlay-timing`'s partial-refresh
            // phase, so the two can be compared directly: this is the global
            // refresh the overlay's cheaper partial refresh stands in for
            // until everything else is ready.
            // Fast waveform unless the device was off for 12 h or more (or
            // that can't be established): sleep entry always wrote the sleep
            // image with the full waveform, and only an image held that long
            // needs the full one again to clear without a trace.
            let slept_at = std::fs::read_to_string(SLEEP_TIMESTAMP_PATH)
                .ok()
                .and_then(|text| parse_sleep_timestamp(&text));
            let woke_at = reading_stats_now(&state);
            let fast_wake = wake_uses_fast_waveform(slept_at, woke_at);
            let slept_seconds = slept_at
                .zip(woke_at)
                .map(|(slept_at, woke_at)| woke_at.saturating_sub(slept_at));
            let wake_waveform = if fast_wake { "fast" } else { "full" };
            boot_profile::mark_with(
                "wake-waveform",
                Some(format!(
                    "{wake_waveform} slept-s={}",
                    slept_seconds.map_or_else(|| "unknown".to_string(), |s| s.to_string())
                )),
            );
            let global_refresh_started = Instant::now();
            let show_span = boot_profile::span("show-base-first-frame");
            if fast_wake {
                panel.show_base_fast(frame.as_bytes())?;
            } else {
                panel.show_base(frame.as_bytes())?;
            }
            show_span.end();
            let global_refresh_ms = global_refresh_started.elapsed().as_millis();
            panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
            info!(
                "rustmix-wave=panel-refresh plan=global-base reason=after-wake transport=global-base"
            );
            let boot_to_ready_line = format!(
                "rustmix-wave=wake-global-refresh reason=deep-sleep-gpio-wake-boot-complete waveform={wake_waveform} slept-s={} fast-max-s={FAST_WAKE_MAX_SLEEP_SECONDS} global-refresh-ms={global_refresh_ms} boot-to-ready-ms={}",
                slept_seconds.map_or_else(|| "unknown".to_string(), |s| s.to_string()),
                boot_started.elapsed().as_millis()
            );
            info!("{boot_to_ready_line}");
            let timing_log_span = boot_profile::span("boot-timing-log-append");
            append_boot_timing_log(&boot_to_ready_line);
            timing_log_span.end();
        } else {
            let render_span = boot_profile::span("render-first-frame");
            render_current_screen(&mut frame, &state)?;
            render_span.end();
            let show_span = boot_profile::span("show-base-first-frame");
            panel.show_base(frame.as_bytes())?;
            show_span.end();
            panel_refresh.reset_after_external_global(PanelGlobalReason::InitialBoot);
            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
            info!(
                "rustmix-wave=panel-refresh plan=global-base reason=initial-boot transport=global-base"
            );
        }
        info!("rustmix-wave=epd397-rust-display-ready");
        boot_profile::mark_with("first-frame-visible", Some(state.active_route().marker()));
        let _ = misc_power.write_lifecycle_stage(power::LifecycleStage::BootFirstFrame);

        // Reader SD catalog scans and the audio codec
        // bring-up happen only after the first e-paper frame is visible.
        // None of them are needed to draw the Home screen (its menu tiles
        // are a static const list), and on a real deep-sleep wake this is a
        // full reboot, so keeping them off the path to the first frame and
        // the button-polling main loop matters every time the device wakes.
        // Queue Recent's other books for silent background warm-up into
        // `session_cache` (see `tick_background_warmup` below), so switching
        // to one of them later in this session is an instant swap instead of
        // a fresh SD reopen. Skips whichever book the block above just
        // queued for immediate foreground resume, if any, so it isn't warmed
        // twice.
        let warmup_span = boot_profile::span("reader-seed-background-warmup");
        let active_on_open = state
            .reader
            .loading
            .as_ref()
            .map(|loading| loading.book.path.clone());
        state
            .reader
            .seed_background_warmup(active_on_open.as_deref());
        warmup_span.end();
        // The Library scan itself (as opposed to the cheap
        // STATE/POSITS/RECENT text-file reads above) is not run here at all:
        // `apply_home` already runs it the moment the user actually opens
        // Library (and `activate_continue_reading` does the same for a direct
        // Continue-Reading resume), and neither Home nor the category menus
        // ever read the catalog. On both a cold boot and a deep-sleep wake (a
        // full reboot -- nothing in RAM survives it) this used to mean
        // re-scanning every book directory unconditionally before the
        // button-polling loop could even start.
        info!(
            "rustmix-wave=reader-persistence-load state-loaded={} preferences-loaded={} positions={} recent={} bookmarks={} warning={}",
            reader_persistence.state_loaded,
            reader_persistence.preferences_loaded,
            reader_persistence.position_count,
            reader_persistence.recent_count,
            reader_persistence.bookmark_count,
            reader_persistence.warning.as_deref().unwrap_or("none")
        );

        // ES8311 codec. The uploaded BSP uses I2S0 with MCLK GPIO13, BCLK
        // GPIO14, WS GPIO47, ESP-to-codec DOUT GPIO48 and amplifier GPIO39.
        // The codec-to-ESP DIN line (GPIO21) is left unclaimed: nothing
        // records, so the channel is TX-only.
        //
        // Brought up lazily, on the first visit to Audio or the first
        // audiobook: most boots never play anything, and the codec probe
        // costs several I2C transactions plus the audio rail's settle time.
        // The I2S0 peripheral and its pins are moved out of `peripherals`
        // here and never used: the audio engine re-creates the channel from
        // stolen handles for every sample rate, which is sound only because
        // nothing else can reach them. `state.audio` starts at
        // `AudioSnapshot::default()`, which reads "has not been initialized".
        let _audio_i2s_reserved = (
            peripherals.i2s0,
            peripherals.pins.gpio13,
            peripherals.pins.gpio14,
            peripherals.pins.gpio47,
            peripherals.pins.gpio48,
        );
        let mut audio_amplifier_pin = Some(peripherals.pins.gpio39);
        let shared_i2c_for_audio = shared_i2c.clone();
        let mut audio_engine: Option<AudioEngine> = None;
        // One attempt per boot, the first time audio is needed: the
        // amplifier pin is spent by it either way, so a codec that failed to
        // answer stays reported as unavailable instead of being re-probed.
        let mut ensure_audio_engine = move |audio_engine: &mut Option<AudioEngine>,
                                            state: &mut AppState,
                                            misc_power: &mut Axp2101<_>| {
            if audio_engine.is_some() {
                return;
            }
            let Some(amplifier_pin) = audio_amplifier_pin.take() else {
                return;
            };
            if let Err(error) = misc_power.enable_audio_rail() {
                warn!("rustmix-wave=pmic-audio-rail status=enable-failed error={error:#}");
            }
            info!(
                "rustmix-wave=audio-init status=starting codec=es8311 address=0x18 wire-write=0x30"
            );
            let attempt = (|| -> Result<AudioEngine> {
                let amplifier = PinDriver::output(amplifier_pin)?;
                let runtime = AudioRuntime::initialize(
                    shared_i2c_for_audio.clone(),
                    amplifier,
                    &mut FreeRtosDelay,
                )?;
                let snapshot = runtime.snapshot();
                info!(
                    "rustmix-wave=audio-codec status=ready codec=es8311 address={} wire-write={} mclk-multiple={AUDIO_MCLK_MULTIPLE}",
                    snapshot.codec_address_label(),
                    snapshot.codec_address.map_or_else(|| "--".into(), |address| format!("0x{:02X}", address << 1))
                );
                let profile = runtime.profile();
                info!(
                    "rustmix-wave=audio-codec-profile status=ready source=waveshare-esp-codec-dev-parity gpio44=0x{:02X} dac-reference=ready system14=0x{:02X} adc15=0x{:02X} adc17=0x{:02X} gp45=0x{:02X}",
                    profile.gpio44,
                    profile.system14,
                    profile.adc15,
                    profile.adc17,
                    profile.gp45
                );
                AudioEngine::start(runtime)
            })();
            match attempt {
                Ok(engine) => {
                    info!("rustmix-wave=audio-subsystem-ready mute=true volume={DEFAULT_AUDIO_VOLUME_PERCENT}");
                    state.update_audio_snapshot(engine.current().audio);
                    *audio_engine = Some(engine);
                }
                Err(error) => {
                    warn!("rustmix-wave=audio-init status=unavailable codec=es8311 error={error:#}");
                    state.update_audio_snapshot(AudioSnapshot::unavailable(format!("{error:#}")));
                }
            }
            log_audio_snapshot(&state.audio);
        };

        // Start optional networking only after the first e-paper frame is
        // visible. A missing config or failed association never blocks shell
        // startup. Keep the runtime alive so Wi-Fi and SNTP remain active.
        let network_span = boot_profile::span("network-setup");
        let mut network_runtime = if let Some(config) = network_config.as_ref() {
            info!(
                "rustmix-wave=wifi-connect status=starting ssid={} saved-networks={}",
                first_saved_ssid(config),
                config.networks.len()
            );
            match NetworkRuntime::connect(peripherals.modem, config) {
                Ok(runtime) => {
                    // Association and DHCP are not awaited here: connect()
                    // only queues the driver start so the main loop (and
                    // button polling) can start immediately. Completion is
                    // logged from the loop once network.tick() observes it.
                    info!(
                        "rustmix-wave=wifi-connect status=starting-async ssid={}",
                        first_saved_ssid(config)
                    );
                    runtime
                }
                Err(error) => {
                    warn!(
                        "rustmix-wave=wifi-connect status=failed ssid={} error={error:#}",
                        first_saved_ssid(config)
                    );
                    NetworkRuntime::failed(config, format!("{error:#}"))
                }
            }
        } else {
            // No `WIFI.TXT` yet: still bring up a Wi-Fi driver (radio kept
            // off until Wi-Fi setup is actually opened) so phone
            // provisioning works on a first-ever boot without a reboot.
            match NetworkRuntime::provision(peripherals.modem) {
                Ok(runtime) => runtime,
                Err(error) => {
                    warn!("rustmix-wave=wifi-provision status=failed error={error:#}");
                    NetworkRuntime::configuration_missing()
                }
            }
        };
        state.update_network_snapshot(network_runtime.snapshot());
        log_network_snapshot(&state.network);
        network_span.end();
        let mut last_network_log = Instant::now();
        let mut last_network_fingerprint = state.network.log_fingerprint();
        // Explicitly activated only. Normal boot never starts the portal.
        // Reachable via the already-connected LAN once Wi-Fi is configured,
        // or via the device's own bootstrap hotspot otherwise (see
        // `wifi_transfer`'s module docs); `portal_via_hotspot` records which
        // one the currently running `wifi_transfer_server`, if any, used.
        let mut wifi_transfer_server: Option<WifiTransferServer> = None;
        let mut portal_via_hotspot = false;
        // Suppresses the LAN path's wifi-loss auto-stop while
        // `maintain_portal_server` is reconnecting back to the saved-network
        // list after a failed Wi-Fi-tab join attempt (see
        // `NetworkRuntime::try_join_candidate`'s LAN fallback), so the
        // portal is not torn down mid-recovery.
        let mut portal_lan_recovering = false;
        state.update_wifi_transfer_snapshot(WifiTransferSnapshot::default());
        let mut network_provision_join_pending: Option<(String, String)> = None;
        let mut network_provision_last_rescan = Instant::now();
        state.set_saved_networks(saved_network_entries(&network_config, None));
        debug_runtime_memory("boot-complete");

        state.audiobooks.positions =
            AudiobookPositions::load(std::path::Path::new(AUDIOBOOK_POSITIONS_PATH));
        let mut audiobook_positions_saved_at = Instant::now();
        let mut last_activity = Instant::now();
        let mut last_status_refresh = Instant::now();
        let mut last_charging_poll = Instant::now();
        let mut last_displayed_charging = state.battery_charging();
        let mut last_power_key_poll = Instant::now();
        let mut last_ota_check_attempt: Option<Instant> = None;
        let mut ota_self_test_confirmed = false;
        let mut last_reader_tick = Instant::now();
        // Reading-stats session tracker: owns the currently-open reading
        // session (if any) and the append-only SD log, fed one page turn or
        // inactivity check at a time. Kept outside `AppState` like the other
        // hardware-adjacent trackers above -- `AppState` stays independent
        // of the wall clock and SD I/O this needs.
        let mut reading_stats_tracker = ReadingStatsTracker::new();
        // Dispatch/poll pair for the OTA release check's own worker thread:
        // `Some` from the tick that starts a request until the tick that
        // observes its result on the channel, so the loop keeps draining
        // `input_queue` and redrawing while the HTTPS round-trip is in
        // flight -- see `spawn_latest_release_check`'s doc comment for the
        // freeze this replaces.
        let mut ota_check_in_flight: Option<
            std::sync::mpsc::Receiver<Result<ReleaseInfo, ReleaseCheckError>>,
        > = None;
        // Amortized EPUB cover-thumbnail generation: no dedicated thread (the
        // main loop is single-threaded and this is the project's only SD
        // consumer, so there is nothing to contend with). At most one
        // thumbnail is built per loop iteration while the Library screen is
        // on-panel, bounded to whatever is actually visible on the current
        // page — see `cover_cache::CoverCache::pump_pending`. The resulting
        // panel refresh is throttled separately (`last_library_thumbnail_refresh`):
        // generation is cheap every tick, but an actual e-paper update is not,
        // and firing one per newly generated thumbnail back-to-back would
        // freeze button polling behind a burst of real panel refreshes.
        let cover_cache = CoverCache::new(state.reader.cache_directory());
        let mut last_library_thumbnail_refresh = Instant::now();
        let mut library_thumbnail_refresh_pending = false;
        // Reader battery power-save: Wi-Fi and the ES8311 audio rail are
        // otherwise held on for the whole session regardless of screen route.
        // Track continuous dwell time on a reader-active route separately from
        // sleep-image suspension so the two mechanisms don't fight each other.
        let mut reader_route_active_since: Option<Instant> = None;
        let mut wifi_suspended_for_reading = false;
        let mut audio_suspended_for_reading = false;
        // Main-loop pacing. Every iteration ends in
        // `input_queue.wait_timeout`, which returns the moment a key event is
        // queued, so input latency does not depend on these values. While
        // something needs frequent service (audio streaming,
        // the transfer portal, a Reader open in progress) the loop keeps the
        // historical 20 ms cadence; otherwise it waits 100 ms, long enough
        // for automatic light sleep (>= 30 ms of idle) and still well inside
        // the 100 ms power-key poll and the 250 ms Reader tick it paces.
        const MAIN_LOOP_ACTIVE_TICK_MS: u64 = 20;
        const MAIN_LOOP_IDLE_WAIT_MS: u64 = 100;
        let mut light_sleep_guard = LightSleepGuard::new();
        // Diagnostic PM-profiling build only (see `power_profile`).
        let mut power_profile_tracker = PowerProfileTracker::default();
        let mut last_power_profile_log = Instant::now();
        boot_profile::mark("main-loop-entered");
        let _ = misc_power.write_lifecycle_stage(power::LifecycleStage::MainLoop);
        if mounted_sd.is_some() {
            let header = format!(
                "=== boot-profile phase=boot version={FIRMWARE_VERSION} cause={} reset-reason={} boot-to-loop-ms={}",
                boot_cause.marker(),
                unsafe { sys::esp_reset_reason() },
                boot_started.elapsed().as_millis()
            );
            if let Err(error) = boot_profile::flush_to_file(BOOT_TIMING_LOG_PATH, &header) {
                warn!("rustmix-wave=boot-profile status=flush-failed error={error}");
            }
        }
        info!("rustmix-wave=boot-profile status=boot-phase-recorded path={BOOT_TIMING_LOG_PATH}");
        // The report is finished (post-boot phase appended to SD, full report
        // printed to serial) a few seconds after Wi-Fi resolves, after
        // `BOOT_PROFILE_MAX_SECONDS` without that, or at sleep entry.
        const BOOT_PROFILE_SETTLE_SECONDS: u64 = 5;
        const BOOT_PROFILE_MAX_SECONDS: u64 = 60;
        let boot_profile_loop_started = Instant::now();
        let mut boot_profile_wifi_resolved_at: Option<Instant> = None;
        let mut first_loop_idle_marked = false;
        // Nothing reads the PMIC Power-key status during boot, so anything
        // latched between `initialize_power_key_events` and here is the
        // press that just powered the board on (its release lands after
        // that early clear whenever the finger stays down a little longer
        // than boot takes to get there) or a press made while the first
        // frame was still drawing. Acting on it would send the device
        // straight back to sleep right after waking -- observed in the field
        // once boot got fast enough to clear the status before a normal
        // release. Drop it, and start the boot guard's quiet window from
        // here, where polling actually begins.
        if power_key_available {
            match board_services.take_power_key_event() {
                Ok(Some(event)) => {
                    info!(
                        "rustmix-wave=power-key-boot-guard event=boot-latched-press-discarded press={}",
                        event.marker()
                    );
                    boot_profile::mark_with("power-key-boot-latched-discarded", Some(event.marker()));
                }
                Ok(None) => {}
                Err(error) => warn!(
                    "rustmix-wave=power-key-boot-guard status=read-failed error={error:#}"
                ),
            }
        }
        let power_key_polling_started = Instant::now();
        loop {
            if power_profile::ENABLED
                && last_power_profile_log.elapsed()
                    >= Duration::from_secs(POWER_PROFILE_LOG_SECONDS)
            {
                power_profile_tracker.log_window(&format!(
                    "route={} panel-awake={} wifi={:?} wifi-suspended-for-reading={} audio-suspended-for-reading={}",
                    state.active_route().marker(),
                    state.panel_awake,
                    state.network.wifi_state,
                    wifi_suspended_for_reading,
                    audio_suspended_for_reading,
                ));
                last_power_profile_log = Instant::now();
            }
            // Post-boot OTA rollback self-test: the panel is already known
            // good (rendered at least one frame to reach this loop at all),
            // so the remaining condition is Wi-Fi actually resolving one way
            // or the other -- either no network is configured, or the boot
            // association attempt has concluded (connected or given up).
            // Runs once; confirming a slot that is already valid is a
            // harmless no-op in `mark_running_slot_valid`.
            if !ota_self_test_confirmed
                && (network_config.is_none()
                    || matches!(
                        state.network.wifi_state,
                        WifiConnectionState::Connected | WifiConnectionState::Failed
                    ))
            {
                mark_running_slot_valid();
                ota_self_test_confirmed = true;
                boot_profile::mark_with(
                    "wifi-resolved",
                    Some(format!("{:?}", state.network.wifi_state)),
                );
                boot_profile_wifi_resolved_at = Some(Instant::now());
            }
            if boot_profile::is_active()
                && (boot_profile_wifi_resolved_at.is_some_and(|at| {
                    at.elapsed() >= Duration::from_secs(BOOT_PROFILE_SETTLE_SECONDS)
                }) || boot_profile_loop_started.elapsed()
                    >= Duration::from_secs(BOOT_PROFILE_MAX_SECONDS))
            {
                finish_boot_profile("settled", mounted_sd.is_some());
            }

            let portal_snapshot_before = state.wifi_transfer.clone();
            maintain_portal_server(
                &mut network_runtime,
                &mut wifi_transfer_server,
                &mut network_config,
                &mut state,
                &mut network_provision_join_pending,
                &mut network_provision_last_rescan,
                portal_via_hotspot,
                &mut portal_lan_recovering,
                &mut storage_browser,
            );
            if state.panel_awake
                && state.wifi_transfer != portal_snapshot_before
                && matches!(
                    state.active_route(),
                    ScreenRoute::WifiTransfer | ScreenRoute::Network
                )
            {
                refresh_screen(
                    &mut panel,
                    &mut frame,
                    &mut state,
                    &mut panel_refresh,
                    RefreshRequest::Normal,
                )?;
            }
            // Reading-stats housekeeping: close a session left open past the
            // inactivity timeout even without a further page turn (the
            // event loop keeps running through idle -- see this project's
            // known light-sleep power issue -- so this poll, not screen
            // state, is what actually bounds a session), and flush it the
            // moment the Reader screen itself is left ("book closed").
            // Cheap when nothing is open: both calls are a plain `Option`
            // check unless there is a session to actually flush.
            if let Some(now) = reading_stats_now(&state) {
                reading_stats_tracker.poll_inactivity(now, STATS_DIRECTORY);
            }
            if !state.active_route().is_reader_active() {
                reading_stats_tracker.close_session(STATS_DIRECTORY);
            }
            if state.panel_awake
                && last_activity.elapsed() >= Duration::from_secs(PANEL_IDLE_SLEEP_SECONDS)
            {
                panel.sleep()?;
                state.panel_awake = false;
                info!("rustmix-wave=epd397-panel-sleep");
            }

            if state.panel_awake && state.active_route() == ScreenRoute::Library {
                // Cheap step: bring already-cached-on-SD thumbnails for the
                // current page into the render-side map (no-op once synced,
                // since a HashMap lookup skips the read for anything already
                // present).
                let visible_books = library_visible_books(&state);
                for book in &visible_books {
                    if !state.reader.library_thumbnails.contains_key(&book.path) {
                        if let Some(thumbnail) = cover_cache.load_cached_thumbnail(book) {
                            state
                                .reader
                                .library_thumbnails
                                .insert(book.path.clone(), thumbnail);
                            library_thumbnail_refresh_pending = true;
                        }
                    }
                }
                // Bounded step: build at most one still-missing thumbnail
                // this tick (~20-40ms budget on its own dedicated worker
                // stack — see `CoverCache::generate_thumbnail`). Narrowed to
                // books the sync step above didn't just resolve, so
                // `pump_pending` doesn't re-read a cache file from SD every
                // tick for entries already confirmed fresh in RAM.
                let still_missing = visible_books
                    .iter()
                    .filter(|book| !state.reader.library_thumbnails.contains_key(&book.path))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some((book, thumbnail)) = cover_cache.pump_pending(&still_missing) {
                    state.reader.library_thumbnails.insert(book.path, thumbnail);
                    library_thumbnail_refresh_pending = true;
                }
                // Only actually drive the panel on the throttled cadence:
                // button polling above stays reactive every tick regardless
                // of how many thumbnails are still pending.
                if library_thumbnail_refresh_pending
                    && last_library_thumbnail_refresh.elapsed()
                        >= Duration::from_secs(LIBRARY_THUMBNAIL_REFRESH_SECONDS)
                {
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                    last_library_thumbnail_refresh = Instant::now();
                    library_thumbnail_refresh_pending = false;
                }
            } else if !state.reader.library_thumbnails.is_empty() {
                // Leaving Library (or the panel went to sleep): drop the
                // render-side map so it stays bounded to roughly one
                // screenful instead of accumulating across a full scroll
                // through a large library.
                state.reader.library_thumbnails.clear();
                library_thumbnail_refresh_pending = false;
            }

            // Continue Reading tile cover: a single book's thumbnail, so
            // unlike the Library grid above this is loaded (or generated,
            // since there's only ever one book to build) outright rather
            // than amortized across ticks, and kept around across route
            // changes instead of cleared on exit (see
            // `screens::home::render_home`).
            if state.panel_awake && state.active_route() == ScreenRoute::Home {
                if let Some(book) = state.reader.continue_reading_book() {
                    let needs_reload = state
                        .reader
                        .continue_reading_thumbnail
                        .as_ref()
                        .map_or(true, |(path, _)| *path != book.path);
                    if needs_reload {
                        let thumbnail = cover_cache
                            .load_cached_thumbnail(&book)
                            .unwrap_or_else(|| cover_cache.generate_thumbnail(&book));
                        state.reader.continue_reading_thumbnail =
                            Some((book.path.clone(), thumbnail));
                        refresh_screen(
                            &mut panel,
                            &mut frame,
                            &mut state,
                            &mut panel_refresh,
                            RefreshRequest::Normal,
                        )?;
                    }
                } else if state.reader.continue_reading_thumbnail.is_some() {
                    state.reader.continue_reading_thumbnail = None;
                }
            }

            // The audio engine plays on its own thread; here its state is
            // brought into the UI, positions are saved, and the audio
            // screens redraw when what they show changed.
            if let Some(engine) = audio_engine.as_mut() {
                if let Some(status) = engine.poll() {
                    let audio_changed = status.audio != state.audio;
                    if audio_changed {
                        state.update_audio_snapshot(status.audio);
                        log_audio_snapshot(&state.audio);
                    }
                    let previous_state = state.audiobooks.now_playing.state;
                    let player_changed = state.audiobooks.update_now_playing(status.now_playing);
                    let now_state = state.audiobooks.now_playing.state;
                    if now_state != previous_state
                        || audiobook_positions_saved_at.elapsed() >= AUDIOBOOK_POSITION_SAVE_INTERVAL
                    {
                        save_audiobook_positions(&state);
                        audiobook_positions_saved_at = Instant::now();
                    }
                    let redraw = match state.active_route() {
                        ScreenRoute::Audio | ScreenRoute::AudioDetails => audio_changed,
                        ScreenRoute::AudiobookPlayer => player_changed || audio_changed,
                        ScreenRoute::AudiobookLibrary => now_state != previous_state,
                        _ => false,
                    };
                    if redraw && state.panel_awake {
                        refresh_screen(
                            &mut panel,
                            &mut frame,
                            &mut state,
                            &mut panel_refresh,
                            RefreshRequest::Normal,
                        )?;
                    }
                }
            }

            if !sleep_network.is_suspended() {
                if let Some(utc) = network_runtime.tick() {
                    info!(
                        "rustmix-wave=sntp-sync status=completed utc={}",
                        utc.date_time()
                    );
                    match board_services.sync_rtc_from_utc(utc) {
                        Ok(stored) => info!(
                            "rustmix-wave=rtc-sync status=updated storage-basis={} stored={}",
                            state.regional.rtc_storage_label(),
                            stored.date_time()
                        ),
                        Err(error) => warn!("rustmix-wave=rtc-sync status=failed error={error:#}"),
                    }
                    state.update_board_snapshot(board_services.read_light_snapshot());
                    log_board_snapshot(state.board, state.regional);
                    // Diagnostic for the OTA-worker internal-RAM fragmentation
                    // investigation: captures the heap right as Wi-Fi finishes
                    // associating and SNTP completes, to see how much of the
                    // fragmentation is already present by this point versus
                    // accumulating from later traffic/usage.
                    log_runtime_memory("wifi-connected-and-synced");
                }
                let latest_network = network_runtime.snapshot();
                if latest_network != state.network {
                    state.update_network_snapshot(latest_network);
                }
                if let Some(scan_result) = network_runtime.poll_scan() {
                    if let Some(server) = wifi_transfer_server.as_ref() {
                        server.set_scan_results(scan_result.unwrap_or_default());
                    }
                }
                let latest_fingerprint = state.network.log_fingerprint();
                if latest_fingerprint != last_network_fingerprint && boot_profile::is_active() {
                    boot_profile::mark_with(
                        "network-state",
                        Some(format!("{:?}", state.network.wifi_state)),
                    );
                }
                if latest_fingerprint != last_network_fingerprint
                    || last_network_log.elapsed()
                        >= Duration::from_secs(NETWORK_LOG_HEARTBEAT_SECONDS)
                {
                    log_network_snapshot(&state.network);
                    last_network_fingerprint = latest_fingerprint;
                    last_network_log = Instant::now();
                }
            }

            // Reader battery power-save: Wi-Fi and the audio rail are cut
            // after a grace period of continuous dwell on a reader-active
            // route, and restored the moment the user leaves it.
            let route_is_reader_active = state.active_route().is_reader_active();
            if route_is_reader_active {
                if reader_route_active_since.is_none() {
                    reader_route_active_since = Some(Instant::now());
                }
            } else {
                if reader_route_active_since.is_some() {
                    // Leaving the Reader route: the ~250ms poll that would
                    // otherwise flush a debounced page-turn save (see
                    // `ReaderUiState::pending_persist`) only runs while a
                    // reader route is active, so force it now instead of
                    // leaving it pending indefinitely.
                    state.reader.flush_pending_persist();
                }
                reader_route_active_since = None;
            }
            let reader_power_save_ready = reader_route_active_since.is_some_and(|since| {
                since.elapsed() >= Duration::from_secs(READER_POWER_SAVE_GRACE_SECONDS)
            });

            if reader_power_save_ready
                && !wifi_suspended_for_reading
                && !sleep_network.is_suspended()
                && !network_runtime.is_provisioning()
                && !state.wifi_transfer.is_active()
            {
                if suspend_network(
                    &mut network_runtime,
                    &mut state,
                    &mut sleep_network,
                    &mut last_network_fingerprint,
                    &mut last_network_log,
                    "reader-power-save",
                ) {
                    wifi_suspended_for_reading = true;
                }
            } else if wifi_suspended_for_reading
                && (!route_is_reader_active || state.wifi_transfer.is_active())
            {
                resume_network(
                    &mut network_runtime,
                    network_config.as_ref(),
                    &mut state,
                    &mut sleep_network,
                    &mut last_network_fingerprint,
                    &mut last_network_log,
                    "reader-power-save",
                );
                wifi_suspended_for_reading = false;
            }

            // Only an idle codec may lose its rail: a book playing (or
            // opening) in the background keeps it, reader page or not.
            let audio_idle = !state.audiobooks.now_playing.is_active()
                && state.audio.playback_state != AudioPlaybackState::PlayingTestTone;
            if reader_power_save_ready && !audio_suspended_for_reading && audio_idle {
                suspend_audio_for_reading(audio_engine.as_ref(), &mut misc_power);
                audio_suspended_for_reading = true;
            } else if audio_suspended_for_reading && !route_is_reader_active {
                resume_audio_after_reading(audio_engine.as_ref(), &mut misc_power, &mut state);
                audio_suspended_for_reading = false;
            }

            if sleep_mode.is_sleeping() {
                if let Some(started_at) = sleep_wake_guard_started_at.as_ref() {
                    let elapsed_ms = started_at.elapsed().as_millis() as u64;
                    if sleep_wake_guard.arm_after_quiet_window(elapsed_ms) {
                        info!(
                            "rustmix-wave=sleep-wake-guard status=ready-for-new-wake-press minimum-quiet-ms={POWER_KEY_WAKE_GUARD_QUIET_MS}"
                        );
                    }
                }
            }

            if power_key_available
                && last_power_key_poll.elapsed() >= Duration::from_millis(POWER_KEY_POLL_MS)
            {
                match board_services.take_power_key_event() {
                    Ok(Some(event)) => {
                        info!(
                            "rustmix-wave=power-key event={} source=axp2101-pek",
                            event.marker()
                        );
                        boot_profile::mark_with("power-key-event", Some(event.marker()));
                        let elapsed_since_polling_ms =
                            power_key_polling_started.elapsed().as_millis() as u64;
                        if boot_power_key_guard.should_ignore(elapsed_since_polling_ms) {
                            info!(
                                "rustmix-wave=power-key-boot-guard event=residual-press-suppressed elapsed-ms={elapsed_since_polling_ms} minimum-quiet-ms={POWER_KEY_BOOT_GUARD_QUIET_MS}"
                            );
                            boot_profile::mark_with(
                                "power-key-boot-guard-suppressed",
                                Some(event.marker()),
                            );
                            last_power_key_poll = Instant::now();
                            continue;
                        }
                        if sleep_mode.is_sleeping() {
                            let elapsed_ms = sleep_wake_guard_started_at
                                .as_ref()
                                .map_or(0, |started_at| started_at.elapsed().as_millis() as u64);
                            if sleep_wake_guard.on_power_press(elapsed_ms)
                                == SleepWakeGuardDecision::SuppressStalePress
                            {
                                info!(
                                    "rustmix-wave=sleep-wake-guard event=stale-power-key-suppressed source=axp2101-pek elapsed-ms={elapsed_ms} minimum-quiet-ms={POWER_KEY_WAKE_GUARD_QUIET_MS}"
                                );
                                last_power_key_poll = Instant::now();
                                continue;
                            }
                            sleep_wake_guard.reset_after_wake();
                            sleep_wake_guard_started_at = None;
                            let restore_route = sleep_mode.exit(SleepWakeCause::PowerKey);
                            panel.initialize()?;
                            state.panel_awake = true;
                            state.router.navigate_to(restore_route);
                            if restore_route == ScreenRoute::Home {
                                // Same Continue Reading card as the deep-sleep
                                // boot path above -- the remaining-time clause
                                // must reflect `now` (today's totals, streak)
                                // on the frame drawn right after waking, not
                                // whatever was last computed before sleeping.
                                refresh_reading_stats_snapshot_now(&mut state);
                            }
                            render_current_screen(&mut frame, &state)?;
                            panel.show_base(frame.as_bytes())?;
                            panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
                            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                            info!("rustmix-wave=panel-refresh plan=global-base reason=after-wake transport=global-base");
                            info!(
                                "rustmix-wave=sleep-mode-exit cause=power-key restore-route={}",
                                restore_route.marker()
                            );
                            info!("rustmix-wave=wake-global-refresh reason=power-key-sleep-image");
                            if sleep_network.is_suspended() {
                                resume_network(
                                    &mut network_runtime,
                                    network_config.as_ref(),
                                    &mut state,
                                    &mut sleep_network,
                                    &mut last_network_fingerprint,
                                    &mut last_network_log,
                                    "sleep-image",
                                );
                            }
                            wifi_suspended_for_reading = false;
                            reader_route_active_since = None;
                            last_activity = Instant::now();
                            last_status_refresh = Instant::now();
                        } else if event == PowerKeyEvent::LongPress {
                            state.open_power_key_menu();
                            refresh_screen(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                RefreshRequest::Normal,
                            )?;
                            info!(
                                "rustmix-wave=power-key-menu outcome=opened return-route={}",
                                state.power_key_sleep_restore_route().marker()
                            );
                            last_activity = Instant::now();
                            last_status_refresh = Instant::now();
                        } else {
                            enter_deep_sleep_mode(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                &mut sleep_images,
                                &mut sleep_mode,
                                &mut sleep_wake_guard,
                                &mut sleep_wake_guard_started_at,
                                &mut wifi_transfer_server,
                                &network_config,
                                &mut network_provision_join_pending,
                                portal_via_hotspot,
                                &mut storage_browser,
                                audio_engine.as_ref(),
                                &mut network_runtime,
                                &mut sleep_network,
                                &mut last_network_fingerprint,
                                &mut last_network_log,
                                &mut wifi_suspended_for_reading,
                                &mut reader_route_active_since,
                                &mut board_services,
                                &mut misc_power,
                                &mut audio_suspended_for_reading,
                                &mut reading_stats_tracker,
                            )?;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        power_key_available = false;
                        warn!("rustmix-wave=power-key status=unavailable source=axp2101-pek error={error:#}");
                    }
                }
                last_power_key_poll = Instant::now();
            }

            // Auto deep sleep: the same real MCU hardware deep sleep a
            // power-key short press arms, triggered instead by inactivity.
            // Uniform across every screen, Reader included -- a reader who
            // sits on one page without pressing a key for the full timeout
            // is, by this measure, indistinguishable from an idle menu, and
            // is deep-slept the same way. `sleep_mode.is_sleeping()` already
            // being true (a prior sleep-image entry, whether from this timer
            // or a power-key press, that has not yet resumed) blocks a
            // repeat call every subsequent loop tick. The timeout is the
            // Settings > Display "Auto standby" choice, which can be Never.
            let auto_sleep_idle_seconds = if AUTO_DEEP_SLEEP_ENABLED {
                state.display.auto_sleep.idle_seconds()
            } else {
                None
            };
            // Never while a book plays: standby is a reboot, the listening
            // would stop.
            if let Some(idle_seconds) = auto_sleep_idle_seconds.filter(|idle_seconds| {
                !sleep_mode.is_sleeping()
                    && !state.audiobooks.now_playing.is_active()
                    && last_activity.elapsed() >= Duration::from_secs(*idle_seconds)
            }) {
                info!(
                    "rustmix-wave=sleep-mode-enter trigger=idle-timeout idle-seconds={idle_seconds}"
                );
                enter_deep_sleep_mode(
                    &mut panel,
                    &mut frame,
                    &mut state,
                    &mut panel_refresh,
                    &mut sleep_images,
                    &mut sleep_mode,
                    &mut sleep_wake_guard,
                    &mut sleep_wake_guard_started_at,
                    &mut wifi_transfer_server,
                    &network_config,
                    &mut network_provision_join_pending,
                    portal_via_hotspot,
                    &mut storage_browser,
                    audio_engine.as_ref(),
                    &mut network_runtime,
                    &mut sleep_network,
                    &mut last_network_fingerprint,
                    &mut last_network_log,
                    &mut wifi_suspended_for_reading,
                    &mut reader_route_active_since,
                    &mut board_services,
                    &mut misc_power,
                    &mut audio_suspended_for_reading,
                    &mut reading_stats_tracker,
                )?;
            }

            if state.take_reading_stats_refresh_request() {
                let snapshot =
                    reading_stats_now(&state).map_or_else(ReadingStatsSnapshot::default, |now| {
                        compute_snapshot(
                            STATS_DIRECTORY,
                            now,
                            state.reader.continue_reading_progress(),
                            state.regional.timezone,
                        )
                    });
                state.update_reading_stats_snapshot(snapshot);
            }

            // Drain a completed check, if any, before deciding whether to
            // start another one this tick. A plain non-blocking channel
            // check: the loop never waits on the HTTPS round-trip.
            if let Some(receiver) = ota_check_in_flight.as_ref() {
                if let Some(outcome) = poll_latest_release_check(receiver) {
                    ota_check_in_flight = None;
                    let update_found = outcome.is_update_available();
                    state.update_ota_state(outcome);
                    // Only interrupt an idle Home screen; a user already
                    // browsing elsewhere sees the result the next time they
                    // open Settings > Software Update.
                    if update_found
                        && state.panel_awake
                        && state.active_route() == ScreenRoute::Home
                    {
                        state.router.navigate_to(ScreenRoute::OtaUpdate);
                    }
                    if state.panel_awake
                        && matches!(
                            state.active_route(),
                            ScreenRoute::Home | ScreenRoute::OtaUpdate
                        )
                    {
                        refresh_screen(
                            &mut panel,
                            &mut frame,
                            &mut state,
                            &mut panel_refresh,
                            RefreshRequest::Normal,
                        )?;
                    }
                }
            }

            // `!is_update_available()`: once the periodic check has found a
            // pending update, stop re-checking every `OTA_CHECK_INTERVAL_SECONDS`
            // -- notify once and wait for the user to act, rather than
            // re-fetching the same GitHub release info daily until they
            // install it. The Settings > Software Update screen's manual
            // "Check Now" button is a separate request path and is
            // unaffected, so the user can still re-check on demand.
            if OTA_AUTO_CHECK_ENABLED
                && !sleep_network.is_suspended()
                && state.ota.can_check()
                && !state.ota.is_update_available()
                && state.network.wifi_state == WifiConnectionState::Connected
                && last_ota_check_attempt.map_or(true, |last| {
                    last.elapsed() >= Duration::from_secs(OTA_CHECK_INTERVAL_SECONDS)
                })
            {
                state.request_ota_background_check();
                last_ota_check_attempt = Some(Instant::now());
            }
            if let Some(request) = state.take_ota_request() {
                match request {
                    OtaUiRequest::CheckNow => {
                        // `state.ota.can_check()` already keeps a manual
                        // "Check Now" from firing while one is in flight (it
                        // flips to `Checking` the moment a request is
                        // queued), so this is only ever `Some` here if two
                        // requests raced onto the same tick -- drop the
                        // second rather than starting an overlapping fetch.
                        if ota_check_in_flight.is_none() {
                            match spawn_latest_release_check() {
                                Ok(receiver) => ota_check_in_flight = Some(receiver),
                                // Thread creation itself failed synchronously
                                // -- there is nothing to poll for, so resolve
                                // this attempt immediately.
                                Err(error) => {
                                    warn!(
                                        "rustmix-wave=ota-check status=start-failed error={error}"
                                    );
                                    state.update_ota_state(OtaCheckState::CheckFailed(
                                        error.to_string(),
                                    ));
                                    if state.panel_awake
                                        && matches!(
                                            state.active_route(),
                                            ScreenRoute::Home | ScreenRoute::OtaUpdate
                                        )
                                    {
                                        refresh_screen(
                                            &mut panel,
                                            &mut frame,
                                            &mut state,
                                            &mut panel_refresh,
                                            RefreshRequest::Normal,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                    OtaUiRequest::InstallNow {
                        version,
                        download_url,
                    } => {
                        info!("rustmix-wave=ota-install status=starting version={version}");
                        // Free the 3 background-warmed book sessions right
                        // before installing: an install either reboots
                        // momentarily (warm-up rebuilds them fresh on the
                        // next boot anyway) or fails and warm-up simply
                        // rebuilds them again shortly after -- so there is
                        // nothing to lose. The install itself no longer
                        // needs a fresh contiguous stack block (it now runs
                        // on the main task's own stack -- see
                        // `install_update_on_main_task`'s doc comment), but
                        // this still gives the download/flash heap traffic
                        // more general-purpose headroom to work with.
                        log_runtime_memory("before-release-parked-sessions");
                        state.reader.release_parked_sessions_for_install();
                        log_runtime_memory("after-release-parked-sessions");
                        match install_update_on_main_task(download_url) {
                            Ok(()) => {
                                info!(
                                    "rustmix-wave=ota-install status=rebooting version={version}"
                                );
                                restart();
                            }
                            Err(error) => {
                                warn!("rustmix-wave=ota-install status=failed error={error}");
                                state.update_ota_state(OtaCheckState::InstallFailed(error));
                                if state.panel_awake
                                    && state.active_route() == ScreenRoute::OtaUpdate
                                {
                                    refresh_screen(
                                        &mut panel,
                                        &mut frame,
                                        &mut state,
                                        &mut panel_refresh,
                                        RefreshRequest::Normal,
                                    )?;
                                }
                            }
                        }
                    }
                }
            }

            if !sleep_mode.is_sleeping()
                && matches!(
                    state.active_route(),
                    ScreenRoute::ReaderLoading | ScreenRoute::ReaderPage
                )
                && (state.active_route() == ScreenRoute::ReaderLoading
                    || last_reader_tick.elapsed() >= Duration::from_millis(250))
            {
                let previous_route = state.active_route();
                let outcome = state.tick_reader();
                match outcome {
                    ReaderTickOutcome::LoadingStageChanged => {
                        info!(
                            "rustmix-wave=reader-cache-stage route={} stage={}",
                            state.active_route().marker(),
                            state
                                .reader
                                .loading_stage()
                                .map_or("none", |stage| stage.label())
                        );
                    }
                    ReaderTickOutcome::FirstPageReady => {
                        info!("rustmix-wave=reader-first-page-ready route={} cache-policy=lazy-nearby-pages", state.active_route().marker());
                    }
                    ReaderTickOutcome::BackgroundCacheAdvanced => {
                        if let Some(session) = state.reader.session.as_ref() {
                            info!("rustmix-wave=reader-background-cache indexed-percent={} pages={} complete={}", session.progress_percent(), session.page_offsets.len(), session.index_complete);
                        }
                    }
                    ReaderTickOutcome::Failed => {
                        warn!(
                            "rustmix-wave=reader-cache-stage status=failed route={}",
                            state.active_route().marker()
                        );
                    }
                    ReaderTickOutcome::None => {}
                }
                apply_portal_ui_request(
                    &mut network_runtime,
                    &mut wifi_transfer_server,
                    &mut network_config,
                    &mut state,
                    &mut network_provision_join_pending,
                    &mut portal_via_hotspot,
                    &mut portal_lan_recovering,
                    &mut storage_browser,
                );
                apply_network_saved_ui_request(
                    &mut network_config,
                    &mut state,
                    &wifi_transfer_server,
                );
                log_reader_persistence_event(&mut state);
                // Intermediate LoadingStageChanged transitions are deliberately not
                // redrawn: each e-paper partial refresh costs far more wall time than
                // the (now metadata-only, page-granular) stage work it would be
                // reporting, so redrawing every stage turned a sub-second cache-hit
                // reopen into several stacked panel refreshes. Stage transitions are
                // still logged above; only the terminal outcomes get pixels.
                if state.panel_awake
                    && (outcome == ReaderTickOutcome::FirstPageReady
                        || outcome == ReaderTickOutcome::Failed
                        || state.active_route() != previous_route)
                {
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                    last_activity = Instant::now();
                }
                last_reader_tick = Instant::now();
            }

            // Silent background pre-warm of Recent's other books into
            // `session_cache` (queued at boot; see `seed_background_warmup`).
            // One book per idle loop iteration, and only cache-hit reads
            // (never a cold parse) -- see `tick_background_warmup`. Gated on
            // `loading.is_none()` so it never competes with a book the user
            // is actually waiting on right now. Also held off until the user
            // has been idle for `READER_WARMUP_IDLE_SECONDS` with no key
            // queued: one book costs 0.6-0.85 s of blocked loop, which the
            // boot profile measured running back to back right after boot
            // (~2.2 s for three books) exactly when the user starts pressing
            // keys.
            const READER_WARMUP_IDLE_SECONDS: u64 = 10;
            if !sleep_mode.is_sleeping()
                && mounted_sd.is_some()
                && state.reader.loading.is_none()
                && last_activity.elapsed() >= Duration::from_secs(READER_WARMUP_IDLE_SECONDS)
                && input_queue.is_empty()
                && state.wifi_transfer.state
                    == waveshare_epd397_rust_app::wifi_transfer::WifiTransferState::Off
            {
                let warmed_a_book = state.reader.tick_background_warmup();
                if warmed_a_book {
                    // OTA-worker internal-RAM fragmentation investigation:
                    // one snapshot per book warmed, so the delta between
                    // consecutive lines attributes fragmentation to this
                    // specific book's cache-hit reopen rather than to
                    // Wi-Fi, which the association-requested/connected
                    // traces already showed isn't the initial cause.
                    debug_runtime_memory("after-reader-background-warmup");
                }
            }

            let live_refresh_seconds = match state.active_route() {
                ScreenRoute::Network | ScreenRoute::NetworkDetails => NETWORK_LIVE_REFRESH_SECONDS,
                _ => SAMPLE_LIVE_REFRESH_SECONDS,
            };
            if state.panel_awake
                && state.active_route().uses_live_status()
                && last_status_refresh.elapsed() >= Duration::from_secs(live_refresh_seconds)
            {
                state.update_board_snapshot(board_services.read_light_snapshot());
                log_board_snapshot(state.board, state.regional);
                refresh_screen(
                    &mut panel,
                    &mut frame,
                    &mut state,
                    &mut panel_refresh,
                    RefreshRequest::Normal,
                )?;
                info!(
                    "rustmix-wave=sample-board-status-auto-refresh route={}",
                    state.active_route().marker()
                );
                last_status_refresh = Instant::now();
            }

            // Route-independent PMIC charging poll -- see
            // `CHARGING_STATUS_POLL_SECONDS` for why Home (the usual
            // post-boot and idle screen) needs this instead of relying on
            // the periodic live-status refresh above, which skips Home
            // entirely. Only reads (cheap: RTC + PMIC, no SHTC3 wake) every
            // tick; only repaints when the charging flag actually flips, so
            // an unplugged, un-plugged-in device sitting on Home causes no
            // extra e-paper wear.
            if state.panel_awake
                && last_charging_poll.elapsed() >= Duration::from_secs(CHARGING_STATUS_POLL_SECONDS)
            {
                state.update_board_snapshot(board_services.read_light_snapshot());
                // Matches `AppState::battery_charging`'s own definition
                // exactly, so this only repaints when what the header
                // actually draws would change.
                let now_charging = state.battery_charging();
                // A full-screen Reader page draws no battery glyph at all,
                // so a charging flip there changes nothing on screen.
                let hides_status = state.active_route() == ScreenRoute::ReaderPage
                    && state.reader.preferences.full_screen;
                if now_charging != last_displayed_charging && hides_status {
                    last_displayed_charging = now_charging;
                }
                if now_charging != last_displayed_charging {
                    log_board_snapshot(state.board, state.regional);
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                    info!(
                        "rustmix-wave=charging-status-changed charging={now_charging} route={}",
                        state.active_route().marker()
                    );
                    last_displayed_charging = now_charging;
                }
                last_charging_poll = Instant::now();
            }

            // The Reader page uses a longer SELECT hold (options) than every
            // other route; publish the threshold for the input thread.
            set_select_long_press_ms(
                if state.active_route() == ScreenRoute::ReaderPage {
                    READER_SELECT_LONG_PRESS_MS
                } else {
                    SELECT_LONG_PRESS_MS
                },
            );
            if let Some(input_event) = input_queue.pop() {
                // Whole handling of one key, including the refresh it causes:
                // any gap between this span's start and its nested
                // `epd-*` span is work done before the panel is touched.
                let mut dispatch_span = boot_profile::span("input-dispatch");
                if boot_profile::is_active() {
                    boot_profile::mark_with("input-handled", Some(format!("{input_event:?}")));
                    dispatch_span.detail(format_args!("{input_event:?}"));
                }
                match input_event {
                    InputEvent::Back => {
                        info!("rustmix-wave=boot-button event=press action=back");
                        if sleep_mode.is_sleeping() {
                            info!("rustmix-wave=sleep-mode-input-suppressed event=boot-press-back");
                            FreeRtos::delay_ms(20);
                            continue;
                        }
                        let woke_from_sleep = !state.panel_awake;
                        if woke_from_sleep {
                            panel.initialize()?;
                            state.panel_awake = true;
                            panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
                            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                        }
                        state.update_board_snapshot(board_services.read_light_snapshot());
                        let previous_route = state.active_route();
                        if previous_route == ScreenRoute::Home {
                            info!("rustmix-wave=hierarchical-back outcome=ignored route=home");
                        } else {
                            state.back();
                            apply_portal_ui_request(
                                &mut network_runtime,
                                &mut wifi_transfer_server,
                                &mut network_config,
                                &mut state,
                                &mut network_provision_join_pending,
                                &mut portal_via_hotspot,
                                &mut portal_lan_recovering,
                                &mut storage_browser,
                            );
                            info!(
                                "rustmix-wave=hierarchical-back outcome=navigated from={} to={}",
                                previous_route.marker(),
                                state.active_route().marker()
                            );
                            info!(
                                "rustmix-wave=screen-route route={}",
                                state.active_route().marker()
                            );
                        }
                        // Compare against `previous_route == Home` (whether `state.back()`
                        // ran at all) rather than `state.active_route() != previous_route`:
                        // several back-handled sub-states (the reader preferences row
                        // editor, the in-page dictionary steps) undo themselves
                        // without changing `ScreenRoute`, so a route-equality check would
                        // skip the redraw and leave the stale screen on the panel until
                        // some later input forced one.
                        if woke_from_sleep || previous_route != ScreenRoute::Home {
                            if state.active_route() == ScreenRoute::Library {
                                // Same reasoning as the Button handler's identical
                                // block below: sync any thumbnails already cached
                                // on SD before this paint, otherwise leaving a
                                // book back into Library shows blank cells for
                                // covers that were already generated on an
                                // earlier visit, until the throttled periodic
                                // redraw at the top of the loop catches up (up to
                                // `LIBRARY_THUMBNAIL_REFRESH_SECONDS` later).
                                for book in library_visible_books(&state) {
                                    if !state.reader.library_thumbnails.contains_key(&book.path) {
                                        if let Some(thumbnail) =
                                            cover_cache.load_cached_thumbnail(&book)
                                        {
                                            state
                                                .reader
                                                .library_thumbnails
                                                .insert(book.path, thumbnail);
                                        }
                                    }
                                }
                            }
                            if state.active_route() == ScreenRoute::Home {
                                // Same idea for Home's Continue Reading card:
                                // after a deep-sleep wake into the reader the
                                // thumbnail was never loaded (boot only preloads
                                // it when landing on Home), so backing out of the
                                // book would paint an empty cover frame and then
                                // repaint once the main loop loaded it.
                                sync_continue_reading_thumbnail(&mut state, &cover_cache);
                            }
                            log_board_snapshot(state.board, state.regional);
                            let request = if woke_from_sleep {
                                RefreshRequest::ForceGlobalAfterWake
                            } else {
                                RefreshRequest::Normal
                            };
                            refresh_screen(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                request,
                            )?;
                        }
                        last_activity = Instant::now();
                        last_status_refresh = Instant::now();
                    }
                    InputEvent::SelectLongPress => {
                        info!(
                        "rustmix-wave=select-button event=long-press action=contextual-navigation"
                    );
                        if sleep_mode.is_sleeping() {
                            info!("rustmix-wave=sleep-mode-input-suppressed event=select-long-press-contextual-navigation");
                            FreeRtos::delay_ms(20);
                            continue;
                        }
                        // Each handler only acts on its own route, so at most
                        // one of them claims the press.
                        let reader_dictionary_context =
                            state.apply_reader_dictionary_select_long_press();
                        let network_saved_context = !reader_dictionary_context
                            && state.apply_network_saved_select_long_press();
                        let library_book_actions_context = !reader_dictionary_context
                            && !network_saved_context
                            && state.apply_library_select_long_press();
                        let audiobook_menu_context = !reader_dictionary_context
                            && !network_saved_context
                            && !library_book_actions_context
                            && state.apply_audiobook_select_long_press();
                        if reader_dictionary_context
                            || network_saved_context
                            || library_book_actions_context
                            || audiobook_menu_context
                        {
                            if reader_dictionary_context {
                                info!(
                                    "rustmix-wave=reader-dictionary-mode outcome=toggled active={}",
                                    !matches!(
                                        state.reader.dictionary_mode,
                                        ReaderDictionaryMode::Off
                                    )
                                );
                            }
                            if network_saved_context {
                                info!(
                                "rustmix-wave=network-saved-forget-confirm outcome=toggled armed={}",
                                state.network_saved.confirming_forget
                            );
                            }
                            if library_book_actions_context {
                                info!(
                                    "rustmix-wave=library-book-actions outcome=opened route={}",
                                    state.active_route().marker()
                                );
                            }
                            let woke_from_sleep = !state.panel_awake;
                            if woke_from_sleep {
                                panel.initialize()?;
                                state.panel_awake = true;
                                panel_refresh
                                    .reset_after_external_global(PanelGlobalReason::AfterWake);
                                sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                            }
                            state.update_board_snapshot(board_services.read_light_snapshot());
                            log_board_snapshot(state.board, state.regional);
                            let request = if woke_from_sleep {
                                RefreshRequest::ForceGlobalAfterWake
                            } else {
                                RefreshRequest::Normal
                            };
                            refresh_screen(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                request,
                            )?;
                            last_activity = Instant::now();
                            last_status_refresh = Instant::now();
                        } else {
                            info!(
                            "rustmix-wave=select-button event=long-press action=ignored route={}",
                            state.active_route().marker()
                        );
                        }
                    }
                    InputEvent::Button(event) => {
                        info!("rustmix-wave=button-event event={event:?}");
                        if sleep_mode.is_sleeping() {
                            info!("rustmix-wave=sleep-mode-input-suppressed event={event:?}");
                            FreeRtos::delay_ms(20);
                            continue;
                        }
                        let woke_from_sleep = !state.panel_awake;
                        if woke_from_sleep {
                            panel.initialize()?;
                            state.panel_awake = true;
                            panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
                            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                        }

                        state.update_board_snapshot(board_services.read_light_snapshot());
                        let previous_route = state.active_route();
                        let previous_display = state.display;
                        let previous_category_usage = state.category_usage.clone();
                        let previous_regional = state.regional;
                        if previous_route == ScreenRoute::Files {
                            apply_storage_event(&mut storage_browser, &mut state, event);
                        } else if previous_route == ScreenRoute::Audio {
                            if let Some(request) = state.apply_audio_button(event) {
                                apply_audio_request(audio_engine.as_ref(), &mut state, request);
                            }
                        } else {
                            state.apply(event);
                            record_reader_page_turn(&mut state, &mut reading_stats_tracker);
                            if state.active_route() == ScreenRoute::Files {
                                storage_browser.refresh();
                                state.update_storage_snapshot(storage_browser.snapshot());
                                log_storage_snapshot(&state.storage);
                            }
                            // Lazy audio codec bring-up on first entry into the
                            // Audio screen (not on every button press within it
                            // -- `previous_route` already matching means this
                            // already ran).
                            if previous_route != ScreenRoute::Audio
                                && state.active_route() == ScreenRoute::Audio
                            {
                                ensure_audio_engine(&mut audio_engine, &mut state, &mut misc_power);
                            }
                            // Entering the audiobook list rescans the card, so
                            // files copied meanwhile show up.
                            if previous_route == ScreenRoute::Home
                                && state.active_route() == ScreenRoute::AudiobookLibrary
                            {
                                refresh_audiobook_library(&mut state);
                            }
                            if let Some(request) = state.audiobooks.take_request() {
                                if matches!(request, PlayerRequest::Open { .. }) {
                                    ensure_audio_engine(&mut audio_engine, &mut state, &mut misc_power);
                                }
                                apply_player_request(audio_engine.as_ref(), &mut state, request);
                            }
                            // "Connect to PC": close everything that uses
                            // the card, unmount it and hand it to the PC.
                            // One way: this ends in a restart.
                            if state.take_usb_disk_request() {
                                if mounted_sd.is_none() {
                                    state.usb_disk = UsbDiskPhase::Failed(
                                        match state.regional.locale {
                                            Locale::English => "The microSD card is not readable.",
                                            Locale::Italian => "La microSD non è leggibile.",
                                        }
                                        .into(),
                                    );
                                } else {
                                    if let Some(engine) = audio_engine.as_ref() {
                                        engine.send(AudioCommand::Player(PlayerCommand::Stop));
                                        engine.suspend();
                                        state
                                            .audiobooks
                                            .update_now_playing(engine.current().now_playing);
                                    }
                                    save_audiobook_positions(&state);
                                    stop_portal_server(
                                        &mut network_runtime,
                                        &mut wifi_transfer_server,
                                        &network_config,
                                        &mut state,
                                        &mut network_provision_join_pending,
                                        &mut storage_browser,
                                        portal_via_hotspot,
                                        "usb-disk",
                                    );
                                    drop(mounted_sd.take());
                                    info!("rustmix-wave=usb-disk status=sd-unmounted");
                                    state.usb_disk = UsbDiskPhase::Active;
                                }
                                refresh_screen(
                                    &mut panel,
                                    &mut frame,
                                    &mut state,
                                    &mut panel_refresh,
                                    RefreshRequest::Normal,
                                )?;
                                if state.usb_disk == UsbDiskPhase::Active {
                                    if let Err(error) = usb_disk::espidf::start(USB_DISK_SD_PINS) {
                                        warn!("rustmix-wave=usb-disk status=start-failed error={error}");
                                        state.usb_disk = UsbDiskPhase::Failed(error);
                                        refresh_screen(
                                            &mut panel,
                                            &mut frame,
                                            &mut state,
                                            &mut panel_refresh,
                                            RefreshRequest::Normal,
                                        )?;
                                    }
                                }
                                wait_for_usb_disk_exit(&input_queue);
                            }
                        }
                        // Consume Settings > Network transfer start/stop intents before
                        // rendering the next frame.  This guarantees that the transfer
                        // route shows READY plus its LAN URL and code on the same normal
                        // partial refresh that follows the SELECT event.
                        apply_network_saved_ui_request(
                            &mut network_config,
                            &mut state,
                            &wifi_transfer_server,
                        );
                        apply_portal_ui_request(
                            &mut network_runtime,
                            &mut wifi_transfer_server,
                            &mut network_config,
                            &mut state,
                            &mut network_provision_join_pending,
                            &mut portal_via_hotspot,
                            &mut portal_lan_recovering,
                            &mut storage_browser,
                        );
                        apply_clock_set_time_ui_request(
                            &mut board_services,
                            &mut state,
                        );
                        apply_clock_set_timezone_ui_request(&mut network_config, &mut state);
                        log_reader_persistence_event(&mut state);
                        if state.display != previous_display {
                            match state.display.save_to_path(DISPLAY_CONFIG_PATH) {
                        Ok(()) => info!(
                            "rustmix-wave=display-config-write status=saved path={DISPLAY_CONFIG_PATH}"
                        ),
                        Err(error) => warn!(
                            "rustmix-wave=display-config-write status=failed path={DISPLAY_CONFIG_PATH} error={error:#}"
                        ),
                    }
                            info!(
                        "rustmix-wave=display-settings-updated font-size={} sleep-screen={} auto-sleep={} persistence=sd-file path={DISPLAY_CONFIG_PATH}",
                        state.display.font_size.marker(),
                        state.display.sleep_screen.marker(),
                        state.display.auto_sleep.marker()
                    );
                        }
                        if state.category_usage != previous_category_usage {
                            if let Err(error) =
                                state.category_usage.save_to_path(MENU_USAGE_CONFIG_PATH)
                            {
                                warn!(
                                    "rustmix-wave=menu-usage-write status=failed path={MENU_USAGE_CONFIG_PATH} error={error:#}"
                                );
                            }
                        }
                        if state.regional != previous_regional {
                            match state.regional.save_to_path(CLOCK_CONFIG_PATH) {
                                Ok(()) => info!(
                                    "rustmix-wave=regional-config-write status=saved path={CLOCK_CONFIG_PATH} timezone={} locale={}",
                                    state.regional.timezone_name(),
                                    state.regional.locale.name()
                                ),
                                Err(error) => warn!(
                                    "rustmix-wave=regional-config-write status=failed path={CLOCK_CONFIG_PATH} error={error:#}"
                                ),
                            }
                        }
                        if state.active_route() != previous_route {
                            info!(
                                "rustmix-wave=screen-route route={}",
                                state.active_route().marker()
                            );
                        }
                        if state.active_route() == ScreenRoute::Library {
                            // Sync any thumbnails already cached on SD before this
                            // event's paint below, instead of leaving the periodic
                            // sync at the top of the loop to pick them up on some
                            // later tick (a cache hit is just a small file read,
                            // cheap enough to do inline here) — otherwise the next
                            // frame shows blank cells with titles for covers that
                            // were already generated on a previous visit. This runs
                            // on every Library button event, not just on entering
                            // the route: scrolling to a new page swaps in a whole
                            // new visible window just as much as opening the screen
                            // does, and without this it would sit on stale blank
                            // cells until the throttled periodic redraw caught up.
                            for book in library_visible_books(&state) {
                                if !state.reader.library_thumbnails.contains_key(&book.path) {
                                    if let Some(thumbnail) =
                                        cover_cache.load_cached_thumbnail(&book)
                                    {
                                        state
                                            .reader
                                            .library_thumbnails
                                            .insert(book.path, thumbnail);
                                    }
                                }
                            }
                        }
                        if state.active_route() == ScreenRoute::Home {
                            // See the Back handler: have the Continue Reading
                            // cover in RAM before this paint, not a tick later.
                            sync_continue_reading_thumbnail(&mut state, &cover_cache);
                        }
                        let reader_clear_ghost = state.take_reader_clear_ghost_request();
                        let power_key_clear_ghost = state.take_power_key_manual_refresh_request();
                        log_board_snapshot(state.board, state.regional);
                        let request = if woke_from_sleep {
                            RefreshRequest::ForceGlobalAfterWake
                        } else if reader_clear_ghost || power_key_clear_ghost {
                            RefreshRequest::ForceGlobalManual
                        } else {
                            RefreshRequest::Normal
                        };
                        refresh_screen(
                            &mut panel,
                            &mut frame,
                            &mut state,
                            &mut panel_refresh,
                            request,
                        )?;
                        last_activity = Instant::now();
                        last_status_refresh = Instant::now();
                    }
                }
            }

            let needs_fast_tick = !matches!(
                    state.audio.playback_state,
                    AudioPlaybackState::Muted
                        | AudioPlaybackState::Ready
                        | AudioPlaybackState::Unavailable
                        | AudioPlaybackState::Error
                )
                || state.wifi_transfer.state
                    != waveshare_epd397_rust_app::wifi_transfer::WifiTransferState::Off
                || state.reader.loading.is_some()
                || state.active_route() == ScreenRoute::ReaderLoading
                || portal_via_hotspot;
            // Whatever needs the fast tick also must not be interrupted by
            // automatic light sleep: audio/I2S streaming and above all the
            // transfer portal, whose SoftAP hotspot ESP-IDF
            // does not support across light sleep (opening Upload with no
            // Wi-Fi configured hung the device in the field).
            light_sleep_guard.set(needs_fast_tick);
            if !first_loop_idle_marked {
                boot_profile::mark("first-loop-iteration-done");
                first_loop_idle_marked = true;
            }
            input_queue.wait_timeout(Duration::from_millis(if needs_fast_tick {
                MAIN_LOOP_ACTIVE_TICK_MS
            } else {
                MAIN_LOOP_IDLE_WAIT_MS
            }));
        }
    }

    /// Best-effort current unix timestamp for reading-stats bookkeeping:
    /// SNTP-synced system time when plausible, otherwise the hardware RTC
    /// (converted to true UTC first -- the RTC chip itself stores a shifted
    /// basis, see `regional::RegionalPreferences::rtc_to_utc`).
    fn reading_stats_now(state: &AppState) -> Option<u64> {
        resolve_unix_timestamp(state.board.rtc.map(|rtc| state.regional.rtc_to_utc(rtc)))
    }

    /// Recompute the Continue Reading "time remaining" snapshot and push it
    /// into `state` immediately, rather than waiting for the next loop tick's
    /// `take_reading_stats_refresh_request()` check. Needed anywhere a screen
    /// showing the Continue Reading card (Home/Reader) is rendered before
    /// that check would otherwise run for the first time -- a real
    /// deep-sleep GPIO wake's very first frame, and the software-only
    /// sleep-image wake's restored frame -- so the card's remaining-time
    /// clause isn't blank until the reader navigates away and back.
    fn refresh_reading_stats_snapshot_now(state: &mut AppState) {
        let snapshot = reading_stats_now(state).map_or_else(ReadingStatsSnapshot::default, |now| {
            compute_snapshot(
                STATS_DIRECTORY,
                now,
                state.reader.continue_reading_progress(),
                state.regional.timezone,
            )
        });
        state.update_reading_stats_snapshot(snapshot);
    }

    /// Feed one Reader page turn (every source funnels through
    /// `AppState::apply`) into the reading-stats session tracker. A no-op when the turn didn't actually
    /// move the position or no reliable clock is available yet.
    fn record_reader_page_turn(state: &mut AppState, tracker: &mut ReadingStatsTracker) {
        let Some(location) = state.take_reader_page_turn_event() else {
            return;
        };
        let Some(now) = reading_stats_now(state) else {
            return;
        };
        let book_id = book_id_for(
            &location.path,
            location.size_bytes,
            location.modified_seconds,
        );
        tracker.note_page_turn(book_id, location.byte_offset, now, STATS_DIRECTORY);
    }

    /// First saved SSID, for log lines; `"--"` when nothing is saved yet.
    fn first_saved_ssid(config: &NetworkConfig) -> &str {
        config
            .networks
            .first()
            .map_or("--", |network| network.ssid.as_str())
    }

    /// Saved-network SSIDs as shown on the phone portal's list (no
    /// passwords), or the empty list before `WIFI.TXT` exists.
    fn saved_ssids(network_config: &Option<NetworkConfig>) -> Vec<String> {
        network_config
            .as_ref()
            .map(|config| {
                config
                    .networks
                    .iter()
                    .map(|network| network.ssid.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Saved-network entries as shown on the on-device read-only "Saved
    /// networks" screen, flagging whichever one matches the live connection.
    fn saved_network_entries(
        network_config: &Option<NetworkConfig>,
        connected_ssid: Option<&str>,
    ) -> Vec<SavedNetworkEntry> {
        network_config
            .as_ref()
            .map(|config| {
                config
                    .networks
                    .iter()
                    .map(|network| SavedNetworkEntry {
                        ssid: network.ssid.clone(),
                        connected: connected_ssid == Some(network.ssid.as_str()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Start or stop the unified portal from the Home "Wi-Fi Transfer" tile
    /// (or the Settings ▸ Network "Configure via phone" shortcut, which
    /// drives the exact same request). Starting picks one of two paths
    /// depending on `state.network`: if Wi-Fi is already joined, the portal
    /// just binds on that existing LAN address, no radio changes; otherwise
    /// it switches the driver into AP+STA (Mixed) mode with a freshly
    /// generated hotspot first, exactly like the old standalone "Configure
    /// via phone" flow, so the same portal -- files and Wi-Fi setup both --
    /// is reachable there instead. Stopping tears down whichever path was
    /// active, reconnecting using the current saved-network list only when
    /// it was the hotspot path.
    #[allow(clippy::too_many_arguments)]
    fn apply_portal_ui_request(
        runtime: &mut NetworkRuntime,
        server: &mut Option<WifiTransferServer>,
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
        via_hotspot: &mut bool,
        lan_recovering: &mut bool,
        storage_browser: &mut StorageBrowser,
    ) {
        let Some(request) = state.take_wifi_transfer_request() else {
            return;
        };
        match request {
            WifiTransferUiRequest::Start => {
                if server.is_some() {
                    return;
                }
                *lan_recovering = false;
                state.update_wifi_transfer_snapshot(WifiTransferSnapshot::starting());
                let code = format!("{:06}", unsafe { sys::esp_random() } % 1_000_000);
                // Free the background-warmed book sessions first, exactly as
                // the OTA install does: they hold ~30 KB of internal RAM
                // (allocations up to SPIRAM_MALLOC_ALWAYSINTERNAL land there),
                // and the hotspot portal's SoftAP, 24 KB httpd stack and DNS
                // task need it. Measured in the field: ~41 KB left before
                // start, then `pthread_mutex_init` failed with ENOMEM right
                // after the hotspot server came up -- a panic and reboot.
                // Books reopen from their SD caches afterwards; background
                // warm-up stays paused while the portal runs.
                log_runtime_memory("before-portal-release-parked-sessions");
                state.reader.release_parked_sessions_for_install();
                log_runtime_memory("before-wifi-transfer-start");
                if let Some(ipv4) = state.network.ipv4_address.clone() {
                    info!("rustmix-wave=wifi-transfer-server status=starting mode=lan ipv4={ipv4} port=80 root={WIFI_TRANSFER_ROOT} stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES}");
                    match WifiTransferServer::start_lan(&ipv4, code) {
                        Ok(active) => {
                            active.set_saved_networks(saved_ssids(network_config));
                            *via_hotspot = false;
                            state.update_wifi_transfer_snapshot(active.snapshot());
                            *server = Some(active);
                            log_runtime_memory("after-wifi-transfer-start");
                        }
                        Err(error) => {
                            warn!(
                                "rustmix-wave=wifi-transfer-server status=start-failed error={error:#}"
                            );
                            state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                                format!("{error:#}"),
                            ));
                        }
                    }
                } else {
                    let ap_info = match runtime.start_provisioning() {
                        Ok(ap_info) => ap_info,
                        Err(error) => {
                            warn!(
                                "rustmix-wave=wifi-transfer-server status=provisioning-failed error={error:#}"
                            );
                            state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                                format!("{error:#}"),
                            ));
                            return;
                        }
                    };
                    info!(
                        "rustmix-wave=wifi-transfer-server status=starting mode=hotspot ap-ssid={} ip={} port=80 root={WIFI_TRANSFER_ROOT} stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES}",
                        ap_info.ap_ssid, ap_info.portal_ip
                    );
                    match WifiTransferServer::start_ap(
                        &ap_info.portal_ip,
                        ap_info.ap_ssid.clone(),
                        ap_info.ap_password.clone(),
                        code,
                    ) {
                        Ok(active) => {
                            active.set_saved_networks(saved_ssids(network_config));
                            *join_pending = None;
                            *via_hotspot = true;
                            state.update_wifi_transfer_snapshot(active.snapshot());
                            *server = Some(active);
                            log_runtime_memory("after-wifi-transfer-start");
                        }
                        Err(error) => {
                            warn!(
                                "rustmix-wave=wifi-transfer-server status=start-failed error={error:#}"
                            );
                            state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                                format!("{error:#}"),
                            ));
                        }
                    }
                }
            }
            WifiTransferUiRequest::Stop => {
                stop_portal_server(
                    runtime,
                    server,
                    network_config,
                    state,
                    join_pending,
                    storage_browser,
                    *via_hotspot,
                    "user-stop",
                );
            }
        }
    }

    /// Tear down the portal (if running). When it was reachable via the
    /// bootstrap hotspot (`via_hotspot`), also drops the hotspot and
    /// reconnects the driver using the current saved-network list, exactly
    /// like the old standalone provisioning flow; when it was on the LAN,
    /// the Wi-Fi connection itself is left untouched.
    #[allow(clippy::too_many_arguments)]
    fn stop_portal_server(
        runtime: &mut NetworkRuntime,
        server: &mut Option<WifiTransferServer>,
        network_config: &Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
        storage_browser: &mut StorageBrowser,
        via_hotspot: bool,
        reason: &str,
    ) {
        if server.take().is_none() {
            return;
        }
        *join_pending = None;
        if via_hotspot {
            if let Err(error) = runtime.stop_provisioning(network_config.as_ref()) {
                warn!(
                    "rustmix-wave=wifi-transfer-server status=stop-reconnect-failed reason={reason} error={error:#}"
                );
            }
            state.update_network_snapshot(runtime.snapshot());
            state.set_saved_networks(saved_network_entries(network_config, None));
        }
        info!("rustmix-wave=wifi-transfer-server status=stopped reason={reason} via-hotspot={via_hotspot}");
        log_runtime_memory("after-wifi-transfer-stop");
        state.update_wifi_transfer_snapshot(WifiTransferSnapshot::default());
        state.reader.refresh_library();
        storage_browser.refresh();
        state.update_storage_snapshot(storage_browser.snapshot());
    }

    /// Drive the portal each main-loop iteration while it is active:
    /// auto-stop on inactivity (or, on the LAN path only, on Wi-Fi loss,
    /// suppressed while a join attempt is in flight); drain a "Forget"
    /// request from the Wi-Fi tab; feed it fresh scan results; and drain a
    /// submitted SSID/password into a real (validate-before-save) connection
    /// attempt, watching that attempt resolve to persist or report failure.
    /// Scan and join work in both modes now -- on the LAN path a candidate
    /// is tried by switching straight to it (no AP to protect), so a failed
    /// attempt is recovered by reconnecting to the saved list and a
    /// successful one restarts the portal clean (its advertised address is
    /// now stale).
    #[allow(clippy::too_many_arguments)]
    fn maintain_portal_server(
        runtime: &mut NetworkRuntime,
        server: &mut Option<WifiTransferServer>,
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
        last_rescan: &mut Instant,
        via_hotspot: bool,
        lan_recovering: &mut bool,
        storage_browser: &mut StorageBrowser,
    ) {
        let Some(active_server) = server.as_ref() else {
            return;
        };
        if *lan_recovering && state.network.wifi_state == WifiConnectionState::Connected {
            *lan_recovering = false;
        }
        let stop_reason = if active_server.is_expired(via_hotspot) {
            Some("inactivity-timeout")
        } else if !via_hotspot
            && join_pending.is_none()
            && !*lan_recovering
            && state.network.wifi_state != WifiConnectionState::Connected
        {
            // Suppressed while a join attempt (or the recovery reconnect
            // after a failed one) is in flight: trying a candidate network
            // from the LAN path (see `NetworkRuntime::try_join_candidate`)
            // disconnects from the current network immediately, which would
            // otherwise look exactly like Wi-Fi loss and tear the portal
            // down mid-attempt, orphaning the STA reconnect no one would
            // then be watching.
            Some("wifi-loss")
        } else {
            None
        };
        if let Some(reason) = stop_reason {
            stop_portal_server(
                runtime,
                server,
                network_config,
                state,
                join_pending,
                storage_browser,
                via_hotspot,
                reason,
            );
            return;
        }
        let Some(active_server) = server.as_ref() else {
            return;
        };

        if let Some(ssid) = active_server.take_pending_delete() {
            if let Some(config) = network_config.as_mut() {
                if config.remove(&ssid) {
                    if let Err(error) = config.save_to_path(WIFI_CONFIG_PATH) {
                        warn!("rustmix-wave=wifi-transfer-forget status=failed error={error:#}");
                    } else {
                        active_server.set_saved_networks(saved_ssids(network_config));
                        state.set_saved_networks(saved_network_entries(
                            network_config,
                            state.network.ssid.as_deref(),
                        ));
                        info!("rustmix-wave=wifi-transfer-forget status=completed ssid={ssid}");
                    }
                }
            }
        }

        // Scan and join work in both modes: while on the bootstrap hotspot
        // this is the original provisioning flow; while already on a LAN,
        // `NetworkRuntime::try_join_candidate` instead switches straight to
        // the candidate (no AP to protect), so a failed attempt is recovered
        // below by reconnecting to the existing saved-network list, and a
        // successful one leaves the device on a (possibly different)
        // network/IP -- the browser session on the old address may simply
        // drop, same as changing Wi-Fi on any router's own admin page.
        if join_pending.is_none() {
            if let Some(request) = active_server.take_pending_join() {
                match runtime.try_join_candidate(request.ssid.clone(), request.password.clone()) {
                    Ok(()) => *join_pending = Some((request.ssid, request.password)),
                    Err(error) => {
                        active_server.record_join_failed(request.ssid, format!("{error:#}"));
                    }
                }
            }
        }

        let mut switched_network = false;
        if let Some((ssid, password)) = join_pending.clone() {
            let snapshot = runtime.snapshot();
            match snapshot.wifi_state {
                WifiConnectionState::Connected => {
                    let base = network_config.clone().unwrap_or_else(|| {
                        NetworkConfig::validated(
                            vec![SavedNetwork {
                                ssid: ssid.clone(),
                                password: password.clone(),
                            }],
                            DEFAULT_TIMEZONE.into(),
                            DEFAULT_NTP_SERVER.into(),
                        )
                        .expect("first saved network is always valid here")
                    });
                    let mut merged = base;
                    let upsert_result = merged.upsert(ssid.clone(), password);
                    let save_result =
                        upsert_result.and_then(|()| merged.save_to_path(WIFI_CONFIG_PATH));
                    match save_result {
                        Ok(()) => {
                            *network_config = Some(merged);
                            active_server.record_join_succeeded(ssid.clone());
                            active_server.set_saved_networks(saved_ssids(network_config));
                            state.set_saved_networks(saved_network_entries(
                                network_config,
                                Some(&ssid),
                            ));
                            info!("rustmix-wave=wifi-transfer-join status=saved ssid={ssid}");
                        }
                        Err(error) => {
                            active_server.record_join_failed(ssid, format!("{error:#}"));
                            warn!("rustmix-wave=wifi-transfer-join status=save-failed error={error:#}");
                        }
                    }
                    *join_pending = None;
                    // The device just switched networks from the LAN path
                    // (not the bootstrap hotspot, which keeps the same
                    // address throughout): the running server's advertised
                    // address is now stale regardless of whether the save
                    // above succeeded, so restart clean below instead of
                    // limping along with it. The user reopens the portal
                    // from Home, which picks up the new address.
                    switched_network = !via_hotspot;
                }
                WifiConnectionState::Failed => {
                    let error = snapshot
                        .error
                        .clone()
                        .unwrap_or_else(|| "connection failed".into());
                    active_server.record_join_failed(ssid, error);
                    *join_pending = None;
                    if !via_hotspot {
                        // Not on the bootstrap hotspot: `try_join_candidate`
                        // already dropped the device's only working
                        // connection to try the candidate, so recover onto
                        // the existing saved-network list instead of leaving
                        // it stranded. Reuses `stop_provisioning`'s
                        // reconnect-with-saved-list logic even though this
                        // device was never provisioning; that logic does not
                        // depend on having been. Marked recovering until the
                        // reconnect actually lands so the wifi-loss check a
                        // few lines up does not race it.
                        *lan_recovering = true;
                        if let Err(error) = runtime.stop_provisioning(network_config.as_ref()) {
                            warn!(
                                "rustmix-wave=wifi-transfer-join status=recover-failed error={error:#}"
                            );
                        }
                        state.update_network_snapshot(runtime.snapshot());
                    }
                }
                _ => {}
            }
        }

        if switched_network {
            stop_portal_server(
                runtime,
                server,
                network_config,
                state,
                join_pending,
                storage_browser,
                via_hotspot,
                "network-switched",
            );
            return;
        }

        // Skip the rescan once a phone has joined the hotspot: it shares the
        // AP's single radio, so an active scan briefly leaves the AP's
        // channel and can reset the phone's in-flight requests, including
        // the captive-portal probe this portal depends on to auto-open (see
        // `NetworkRuntime::provisioning_client_count`, always 0 outside the
        // hotspot path, so harmless to check unconditionally).
        if join_pending.is_none()
            && last_rescan.elapsed() >= Duration::from_secs(NETWORK_PROVISION_RESCAN_SECONDS)
            && runtime.provisioning_client_count() == 0
        {
            *last_rescan = Instant::now();
            let _ = runtime.start_scan();
        }

        let latest = active_server.snapshot();
        if latest != state.wifi_transfer {
            state.update_wifi_transfer_snapshot(latest);
        }
    }

    /// Forget a saved network requested from the on-device "Saved networks"
    /// screen. Works regardless of whether the portal is open or which mode
    /// it is in (a plain `WIFI.TXT` edit); if a portal happens to be active,
    /// its own saved-network list is refreshed too so the Wi-Fi tab reflects
    /// the change immediately instead of waiting for the next poll.
    fn apply_network_saved_ui_request(
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        server: &Option<WifiTransferServer>,
    ) {
        let Some(ssid) = state.take_network_saved_forget_request() else {
            return;
        };
        let Some(config) = network_config.as_mut() else {
            return;
        };
        if !config.remove(&ssid) {
            return;
        }
        if let Err(error) = config.save_to_path(WIFI_CONFIG_PATH) {
            warn!("rustmix-wave=network-saved-forget status=failed error={error:#}");
            return;
        }
        if let Some(server) = server {
            server.set_saved_networks(saved_ssids(network_config));
        }
        state.set_saved_networks(saved_network_entries(
            network_config,
            state.network.ssid.as_deref(),
        ));
        // The Network overview screen's "Saved" count reads this cached
        // field, not the live saved-networks list, so it stays stuck on the
        // pre-forget count (looking like the network is still configured)
        // unless it is refreshed here too.
        state.network.saved_network_count = network_config
            .as_ref()
            .map_or(0, |config| config.networks.len());
        info!("rustmix-wave=network-saved-forget status=completed ssid={ssid}");
    }

    fn suspend_network(
        runtime: &mut NetworkRuntime,
        state: &mut AppState,
        sleep_network: &mut SleepNetworkState,
        last_network_fingerprint: &mut NetworkLogFingerprint,
        last_network_log: &mut Instant,
        reason: &'static str,
    ) -> bool {
        info!("rustmix-wave=network-suspend status=starting reason={reason}");
        match runtime.suspend() {
            Ok(()) => {
                let _ = sleep_network.suspend();
                state.update_network_snapshot(runtime.snapshot());
                *last_network_fingerprint = state.network.log_fingerprint();
                *last_network_log = Instant::now();
                info!("rustmix-wave=sntp-suspend status=stopped reason={reason}");
                info!("rustmix-wave=wifi-suspend status=disconnected reason={reason}");
                info!("rustmix-wave=wifi-suspend status=stopped reason={reason}");
                true
            }
            Err(error) => {
                warn!("rustmix-wave=network-suspend status=failed reason={reason} error={error:#}");
                false
            }
        }
    }

    fn resume_network(
        runtime: &mut NetworkRuntime,
        config: Option<&NetworkConfig>,
        state: &mut AppState,
        sleep_network: &mut SleepNetworkState,
        last_network_fingerprint: &mut NetworkLogFingerprint,
        last_network_log: &mut Instant,
        reason: &'static str,
    ) {
        info!("rustmix-wave=network-resume status=starting reason={reason}");
        if let Some(config) = config {
            // `resume` is non-blocking: it only kicks off the handshake and
            // records the SSID it will try first on `runtime.snapshot()`
            // (logged right below via `log_network_snapshot`), so there is no
            // "connected" outcome to log here -- `tick`/`advance_boot_phase`
            // finish it across later main-loop iterations, the same as the
            // boot-time `connect` path.
            match runtime.resume(config) {
                Ok(()) => info!("rustmix-wave=wifi-resume status=starting reason={reason}"),
                Err(error) => {
                    warn!("rustmix-wave=wifi-resume status=failed reason={reason} error={error:#}");
                    runtime.record_resume_failure(format!("{error:#}"));
                }
            }
        } else {
            runtime.record_configuration_missing();
            info!("rustmix-wave=wifi-resume status=skipped reason={reason} cause=configuration-missing");
        }
        let _ = sleep_network.resume();
        state.update_network_snapshot(runtime.snapshot());
        log_network_snapshot(&state.network);
        *last_network_fingerprint = state.network.log_fingerprint();
        *last_network_log = Instant::now();
    }

    /// Enter real MCU hardware deep sleep: show a sleep-confirmation image,
    /// tear down Wi-Fi transfer/audio/network, cut the panel and audio
    /// PMIC rails, disarm the RTC alarm, and arm
    /// GPIO5 as the wakeup source. Shared by both triggers into this path --
    /// an explicit power-key press and the idle-timeout auto-sleep check --
    /// so the two can never drift into two different sleep-entry sequences.
    #[allow(clippy::too_many_arguments)]
    fn enter_deep_sleep_mode<SPI, DC, RST, CS, BUSY, DELAY, POWER, BoardI2c, PmicI2c>(
        panel: &mut Epaper397<SPI, DC, RST, CS, BUSY, DELAY, POWER>,
        frame: &mut FrameBuffer,
        state: &mut AppState,
        panel_refresh: &mut PanelRefreshCoordinator,
        sleep_images: &mut SleepImageCatalog,
        sleep_mode: &mut SleepModeState,
        sleep_wake_guard: &mut SleepWakeGuard,
        sleep_wake_guard_started_at: &mut Option<Instant>,
        wifi_transfer_server: &mut Option<WifiTransferServer>,
        portal_network_config: &Option<NetworkConfig>,
        portal_join_pending: &mut Option<(String, String)>,
        portal_via_hotspot: bool,
        storage_browser: &mut StorageBrowser,
        audio_engine: Option<&AudioEngine>,
        network_runtime: &mut NetworkRuntime,
        sleep_network: &mut SleepNetworkState,
        last_network_fingerprint: &mut NetworkLogFingerprint,
        last_network_log: &mut Instant,
        wifi_suspended_for_reading: &mut bool,
        reader_route_active_since: &mut Option<Instant>,
        board_services: &mut BoardServices<BoardI2c>,
        misc_power: &mut Axp2101<PmicI2c>,
        audio_suspended_for_reading: &mut bool,
        reading_stats_tracker: &mut ReadingStatsTracker,
    ) -> Result<()>
    where
        SPI: embedded_hal::spi::SpiBus<u8>,
        SPI::Error: core::fmt::Debug,
        DC: embedded_hal::digital::OutputPin,
        DC::Error: core::fmt::Debug,
        RST: embedded_hal::digital::OutputPin,
        RST::Error: core::fmt::Debug,
        CS: embedded_hal::digital::OutputPin,
        CS::Error: core::fmt::Debug,
        BUSY: embedded_hal::digital::InputPin,
        BUSY::Error: core::fmt::Debug,
        DELAY: DelayNs,
        POWER: waveshare_epd397_rust_app::power::PanelPower,
        BoardI2c: embedded_hal::i2c::I2c,
        BoardI2c::Error: core::fmt::Debug,
        PmicI2c: embedded_hal::i2c::I2c,
        PmicI2c::Error: core::fmt::Debug,
    {
        if boot_profile::is_active() {
            finish_boot_profile("sleep-entry", true);
        }
        // Draw the sleep-confirmation image first, before any of the slower
        // teardown below (Wi-Fi transfer server, audio cleanup,
        // network suspend). The user pressed power (or, for the idle-timeout
        // trigger, simply stopped interacting) to get immediate visual
        // confirmation the command was received; making them wait through
        // network suspend first defeats that.
        let restore_route = state.power_key_sleep_restore_route();
        let (sleep_frame, sleep_label) =
            select_sleep_frame(state, sleep_images, restore_route.is_reader_active());
        if !state.panel_awake {
            panel.initialize()?;
            state.panel_awake = true;
        }
        *frame = sleep_frame;
        panel.show_base(frame.as_bytes())?;
        panel_refresh.reset_after_external_global(PanelGlobalReason::SleepImage);
        sync_panel_refresh_diagnostics(state, &*panel_refresh);
        info!(
            "rustmix-wave=panel-refresh plan=global-base reason=sleep-image transport=global-base"
        );
        sleep_mode.enter(restore_route, sleep_label.clone());
        // Real deep sleep is a full reboot, so nothing in RAM survives a
        // SELECT-key wake. Record whether the device was actively reading a
        // book so a real hardware deep-sleep wake (a full reboot) can
        // auto-resume it instead of always landing back on Home.
        let reader_was_active = restore_route.is_reader_active();
        match state
            .reader
            .record_deep_sleep_active_marker(reader_was_active)
        {
            Ok(()) => info!(
                "rustmix-wave=deep-sleep-reader-marker status=recorded active={reader_was_active}"
            ),
            Err(error) => warn!(
                "rustmix-wave=deep-sleep-reader-marker status=failed active={reader_was_active} error={error:#}"
            ),
        }
        // Real deep sleep is a full reboot: anything the reading-stats
        // tracker still has open in RAM must be flushed to SD now or the
        // reading time it represents is lost for good. Same for a page
        // turn's debounced STATE/POSITS/RECENT save (see
        // `ReaderUiState::pending_persist`) -- nothing in RAM survives.
        reading_stats_tracker.close_session(STATS_DIRECTORY);
        state.reader.flush_pending_persist();
        sleep_wake_guard.begin_sleep_entry();
        *sleep_wake_guard_started_at = Some(Instant::now());
        info!(
            "rustmix-wave=sleep-wake-guard status=waiting-for-quiet-window minimum-quiet-ms={POWER_KEY_WAKE_GUARD_QUIET_MS} policy=suppress-stale-power-key"
        );

        stop_portal_server(
            network_runtime,
            wifi_transfer_server,
            portal_network_config,
            state,
            portal_join_pending,
            storage_browser,
            portal_via_hotspot,
            "sleep-entry",
        );
        if let Some(engine) = audio_engine {
            // Pauses a playing book, then the position it reports is saved:
            // sleep ends in a reboot, and the book resumes from here.
            engine.suspend();
            let status = engine.current();
            state.update_audio_snapshot(status.audio);
            state.audiobooks.update_now_playing(status.now_playing);
            save_audiobook_positions(state);
            info!("rustmix-wave=audio-event outcome=playback-stop reason=sleep-mode");
            log_audio_snapshot(&state.audio);
        }
        // Best-effort like the audio-rail/RTC-alarm teardown below: the
        // sleep image is already shown and committed to, so a failed
        // network suspend no longer aborts entering deep sleep, it only
        // skips the Wi-Fi/SNTP pause.
        if !suspend_network(
            network_runtime,
            state,
            sleep_network,
            last_network_fingerprint,
            last_network_log,
            "sleep-image",
        ) {
            warn!("rustmix-wave=network-suspend status=failed-continuing reason=best-effort-deep-sleep");
        }
        *wifi_suspended_for_reading = false;
        *reader_route_active_since = None;
        panel.sleep()?;
        state.panel_awake = false;
        // ALDO2 (Audio_VCC) feeds the codec AVDD pin and the onboard
        // digital microphone; cut it the same way ALDO3 is cut for the
        // e-paper panel. PVDD/DVDD stay powered from the always-on VCC3V3
        // rail regardless.
        match misc_power.disable_audio_rail() {
            Ok(()) => info!("rustmix-wave=pmic-audio-rail status=disabled"),
            Err(error) => {
                warn!("rustmix-wave=pmic-audio-rail status=disable-failed error={error:#}")
            }
        }
        *audio_suspended_for_reading = false;
        info!(
            "rustmix-wave=sleep-mode-enter image={} restore-route={} display=global-refresh panel=deep-sleep aldo3=off aldo2=off wifi=off network-services=paused shutdown=pmic fallback=deep-sleep",
            sleep_label,
            restore_route.marker()
        );
        info!(
            "rustmix-wave=mcu-deep-sleep status=entering wake-gpio={} wake-level=active-low rtc-alarm-wake=disabled reason=gpio45-not-rtc-io-capable shutdown=pmic fallback=deep-sleep",
            mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO
        );
        // Disarm the PCF85063 hardware alarm slot before powering down
        // (it is also disarmed at every boot). It can never wake real MCU
        // deep sleep (GPIO45 is outside the RTC IO range), so an alarm left
        // armed only risks firing while the CPU is off: its interrupt line
        // would then latch low on GPIO45 -- one of the ESP32-S3's boot
        // strapping pins (VDD_SPI voltage select) -- and stay that way until
        // re-sampled at the next reset, which can corrupt the GPIO5 wake boot.
        if let Err(error) = board_services.disable_rtc_alarm() {
            warn!(
                "rustmix-wave=rtc-alarm-disable status=failed reason=pre-deep-sleep error={error:#}"
            );
        }

        // Preferred shutdown path: cut power at the PMIC instead of putting
        // the MCU into deep sleep. Every SD write above (sleep-image marker,
        // Reader deep-sleep marker) already went through `fs::write`'s
        // implicit close, the same durability guarantee the MCU deep-sleep
        // fallback below has always relied on -- PMIC power-off is no more
        // abrupt than that from the filesystem's point of view, so no
        // additional sync is introduced here.
        //
        // The shutdown marker must be written and confirmed before
        // `power_off()` runs: once the PMIC cuts power there is no further
        // chance to record anything, and a shutdown with no marker set would
        // boot back up misclassified as an ordinary power-on, losing the
        // Reader auto-resume. If the marker write itself fails, skip
        // `power_off()` entirely and fall through to the deep-sleep fallback
        // below, which is self-classifying via the ESP32-S3's own wakeup
        // register and does not depend on the PMIC marker at all.
        // Sleep-entry time for the next wake's waveform choice. A missing or
        // unreadable record simply makes that wake use the full waveform.
        let slept_at = board_services
            .read_rtc()
            .ok()
            .and_then(|rtc| resolve_unix_timestamp(Some(state.regional.rtc_to_utc(rtc))));
        let slept_at_result = match slept_at {
            Some(timestamp) => std::fs::write(SLEEP_TIMESTAMP_PATH, timestamp.to_string()),
            None => std::fs::remove_file(SLEEP_TIMESTAMP_PATH),
        };
        if let Err(error) = slept_at_result {
            warn!("rustmix-wave=sleep-timestamp status=write-failed path={SLEEP_TIMESTAMP_PATH} error={error}");
        }
        // SD breadcrumbs for the shutdown itself: a device that "won't wake"
        // leaves no serial log on battery, and without these a PMIC
        // power-off that silently failed (leaving the board in the MCU
        // deep-sleep fallback, which only SELECT wakes -- the Power key does
        // nothing there) is indistinguishable afterwards from one that
        // worked. Each line is closed before the next step, so it is on the
        // card even if the very next call cuts power.
        let uptime_ms = boot_profile::now_us() / 1000;
        // Battery state at shutdown: the AXP2101 refuses a Power-key power-on
        // when the battery sits below its power-on threshold, so a failed wake
        // after a low reading here points at the battery, not the firmware.
        let battery = match misc_power.read_power_snapshot() {
            Ok(snapshot) => format!(
                "battery-mv={} battery-pct={} vbus={} charging={}",
                snapshot.battery_voltage_mv.map_or("none".to_string(), |mv| mv.to_string()),
                snapshot.battery_percent.map_or("none".to_string(), |pct| pct.to_string()),
                snapshot.vbus_present,
                snapshot.charging
            ),
            Err(error) => format!("battery=read-failed error={error:#}"),
        };
        append_boot_timing_log(&format!(
            "rustmix-wave=sleep-entry uptime-ms={uptime_ms} restore-route={} method=pmic-power-off {battery}",
            restore_route.marker()
        ));
        let _ = misc_power.write_lifecycle_stage(power::LifecycleStage::SleepMarkerPending);
        let pmic_shutdown_attempted = match misc_power.write_shutdown_marker() {
            Ok(()) => {
                let _ = misc_power.write_lifecycle_stage(power::LifecycleStage::PowerOffIssued);
                let watchdog_armed = match misc_power.arm_wake_watchdog() {
                    Ok(()) => true,
                    Err(error) => {
                        warn!("rustmix-wave=pmic-wake-watchdog status=arm-failed error={error:#}");
                        false
                    }
                };
                // `power_off()` does not return on real hardware: the rails
                // collapse before this call site can observe anything.
                // Should it somehow return `Ok(())` (the I2C ACK landing a
                // moment before power is actually cut), treat that exactly
                // like an error -- fall through to the deep-sleep fallback
                // rather than assuming the device is already off.
                let outcome = match misc_power.power_off() {
                    Err(error) => {
                        warn!("rustmix-wave=pmic-power-off status=failed error={error:#}");
                        format!("failed error={error:#}")
                    }
                    Ok(()) => {
                        warn!("rustmix-wave=pmic-power-off status=returned-unexpectedly");
                        "returned-unexpectedly".to_string()
                    }
                };
                // With the watchdog armed the PMIC still cuts power 16 s
                // into the fallback, and from then on the Power key wakes
                // the board (the shutdown marker is already written);
                // before that, and without the watchdog, only SELECT does.
                let wake = if watchdog_armed {
                    "select-then-power-key-after-16s"
                } else {
                    "select-only"
                };
                append_boot_timing_log(&format!(
                    "rustmix-wave=pmic-power-off status={outcome} fallback=mcu-deep-sleep wake={wake}"
                ));
                true
            }
            Err(error) => {
                warn!("rustmix-wave=pmic-shutdown-marker status=write-failed error={error:#}");
                append_boot_timing_log(&format!(
                    "rustmix-wave=pmic-shutdown-marker status=write-failed error={error:#} fallback=mcu-deep-sleep wake=select-only"
                ));
                false
            }
        };
        let _ = misc_power.write_lifecycle_stage(power::LifecycleStage::DeepSleepFallback);
        if pmic_shutdown_attempted {
            info!("rustmix-wave=mcu-deep-sleep status=fallback-after-pmic-power-off-attempt");
        }

        // Fallback: real MCU hardware deep sleep. On success this call does
        // not return: the chip powers down and `run()` starts over from the
        // top on the next GPIO5 press. Only a failure to disable automatic
        // light sleep first or to arm the wakeup source returns here, in
        // which case the state above (sleep image shown, ALDO3 off, Wi-Fi
        // suspended) is left in place and the event loop keeps running as a
        // software-only fallback so the board is never stranded asleep with
        // no way to wake it.
        if let Err(error) = mcu_deep_sleep::espidf::enter() {
            warn!("rustmix-wave=mcu-deep-sleep status=fallback-software-only error={error:#}");
            // The event loop keeps running: don't let the PMIC watchdog armed
            // above cut power 16 s from now.
            let _ = misc_power.disarm_wake_watchdog();
            append_boot_timing_log(&format!(
                "rustmix-wave=mcu-deep-sleep status=fallback-software-only error={error:#}"
            ));
            // `enter` only ever turns light sleep off, so whether it failed
            // before or after doing so, the running loop below needs it
            // back.
            if let Err(error) = mcu_deep_sleep::espidf::set_light_sleep_enabled(true) {
                warn!(
                    "rustmix-wave=power-management status=light-sleep-restore-failed error={error:#}"
                );
            }
        }
        Ok(())
    }

    /// Cut power to the ES8311 codec + onboard mic (AXP2101 ALDO2) while
    /// Reader is the active screen and nothing is using audio. The engine
    /// first silences everything and closes the I2S channel -- which also
    /// releases its power-management lock, so light sleep can engage --
    /// and only then does the codec chip lose power, the same rail the
    /// sleep-image path disables before deep sleep.
    fn suspend_audio_for_reading<I2C>(audio_engine: Option<&AudioEngine>, misc_power: &mut Axp2101<I2C>)
    where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        if let Some(engine) = audio_engine {
            engine.suspend();
            info!("rustmix-wave=reader-power-save status=audio-engine-suspended");
        }
        match misc_power.disable_audio_rail() {
            Ok(()) => info!("rustmix-wave=reader-power-save status=audio-rail-disabled"),
            Err(error) => warn!(
                "rustmix-wave=reader-power-save status=audio-rail-disable-failed error={error:#}"
            ),
        }
    }

    /// Restore the ES8311 codec after [`suspend_audio_for_reading`]. The
    /// power cycle resets every ES8311 register to its power-on default, so
    /// the codec is reprogrammed from scratch before use resumes.
    fn resume_audio_after_reading<I2C>(
        audio_engine: Option<&AudioEngine>,
        misc_power: &mut Axp2101<I2C>,
        state: &mut AppState,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        if let Err(error) = misc_power.enable_audio_rail() {
            warn!("rustmix-wave=reader-power-save status=audio-rail-enable-failed error={error:#}");
            return;
        }
        info!("rustmix-wave=reader-power-save status=audio-rail-enabled");
        if let Some(engine) = audio_engine {
            match engine.restore() {
                Ok(()) => info!("rustmix-wave=reader-power-save status=audio-codec-reinitialized"),
                Err(error) => warn!(
                    "rustmix-wave=reader-power-save status=audio-codec-reinit-failed error={error}"
                ),
            }
            state.update_audio_snapshot(engine.current().audio);
        }
    }

    /// Write every audiobook's listening position to the SD card.
    fn save_audiobook_positions(state: &AppState) {
        if let Err(error) = state
            .audiobooks
            .positions
            .save(std::path::Path::new(AUDIOBOOK_POSITIONS_PATH))
        {
            warn!("rustmix-wave=audiobook-positions status=save-failed error={error}");
        }
    }

    /// Carry out a request from the audiobook screens: bring the audio
    /// engine up if needed, then hand it the matching command.
    fn apply_player_request(
        audio_engine: Option<&AudioEngine>,
        state: &mut AppState,
        request: PlayerRequest,
    ) {
        let Some(engine) = audio_engine else {
            state.audiobooks.now_playing.state = PlayerState::Error;
            state.audiobooks.now_playing.error = Some(
                state
                    .audio
                    .error
                    .clone()
                    .unwrap_or_else(|| "audio unavailable".into()),
            );
            return;
        };
        let command = match request {
            PlayerRequest::Open { key } => {
                let Some(book) = state.audiobooks.book(&key).cloned() else {
                    warn!("rustmix-wave=audiobook status=open-failed reason=unknown-title key={key:?}");
                    return;
                };
                let position = state.audiobooks.positions.get(&key).unwrap_or_default();
                AudioCommand::Player(PlayerCommand::Play { book, position })
            }
            PlayerRequest::TogglePause => AudioCommand::Player(PlayerCommand::TogglePause),
            PlayerRequest::SeekBy { seconds } => {
                AudioCommand::Player(PlayerCommand::SeekBy { seconds })
            }
            PlayerRequest::SkipTrack { forward } => {
                AudioCommand::Player(PlayerCommand::SkipTrack { forward })
            }
            PlayerRequest::Stop => AudioCommand::Player(PlayerCommand::Stop),
            PlayerRequest::Volume { up: true } => AudioCommand::Ui(AudioUiRequest::VolumeUp),
            PlayerRequest::Volume { up: false } => AudioCommand::Ui(AudioUiRequest::VolumeDown),
        };
        if !engine.send(command) {
            warn!("rustmix-wave=audio-engine status=gone");
        }
    }

    /// Persist a manually edited local wall-clock value committed from the
    /// Clock screen's Set Date & Time editor and refresh the board snapshot.
    fn apply_clock_set_time_ui_request<I2C>(
        board_services: &mut BoardServices<I2C>,
        state: &mut AppState,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        let Some(stored) = state.take_clock_set_time_request() else {
            return;
        };
        match board_services.write_rtc_datetime(stored) {
            Ok(()) => info!(
                "rustmix-wave=rtc-manual-set status=updated stored={}",
                stored.date_time()
            ),
            Err(error) => warn!("rustmix-wave=rtc-manual-set status=failed error={error:#}"),
        }
        state.update_board_snapshot(board_services.read_light_snapshot());
        log_board_snapshot(state.board, state.regional);
    }

    /// Sync a timezone committed from the Clock screen's Set Date & Time
    /// editor into `WIFI.TXT` when it already exists, preserving the
    /// SSID/password/NTP fields the editor never touches, purely so the two
    /// files do not disagree. `state.regional` already reflects the new
    /// timezone live; persisting it to `CLOCK.TXT` (alongside `locale`) is
    /// handled by the regional-preferences diff-and-save block around the
    /// caller, not here.
    fn apply_clock_set_timezone_ui_request(
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
    ) {
        let Some(timezone) = state.take_clock_set_timezone_request() else {
            return;
        };
        let Some(existing) = network_config.as_ref() else {
            return;
        };
        let updated = match NetworkConfig::validated(
            existing.networks.clone(),
            timezone.name().to_string(),
            existing.ntp_server.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                warn!("rustmix-wave=clock-timezone-set status=wifi-sync-validate-failed error={error:#}");
                return;
            }
        };
        match updated.save_to_path(WIFI_CONFIG_PATH) {
            Ok(()) => *network_config = Some(updated),
            Err(error) => warn!(
                "rustmix-wave=clock-timezone-set status=wifi-sync-write-failed error={error:#}"
            ),
        }
    }

    /// Hand a Settings > Audio request to the engine; the new state comes
    /// back through `AudioEngine::poll` in the main loop.
    fn apply_audio_request(
        audio_engine: Option<&AudioEngine>,
        state: &mut AppState,
        request: AudioUiRequest,
    ) {
        let Some(engine) = audio_engine else {
            warn!("rustmix-wave=audio-event outcome=unavailable request={request:?}");
            return;
        };
        if !engine.send(AudioCommand::Ui(request)) {
            warn!("rustmix-wave=audio-engine status=gone");
            state.update_audio_snapshot(AudioSnapshot::unavailable("audio engine stopped"));
        }
    }

    /// Rescan `RUSTMIX/AUDIO`, creating it the first time so whoever puts
    /// the card in a PC finds where audiobooks go.
    fn refresh_audiobook_library(state: &mut AppState) {
        let root = std::path::Path::new(AUDIOBOOK_ROOT);
        if let Err(error) = std::fs::create_dir_all(root) {
            warn!("rustmix-wave=audiobook-library status=mkdir-failed error={error}");
        }
        let scan = scan_audiobooks(root).map_err(|error| error.to_string());
        match &scan {
            Ok(books) => info!("rustmix-wave=audiobook-library status=scanned titles={}", books.len()),
            Err(error) => warn!("rustmix-wave=audiobook-library status=scan-failed error={error}"),
        }
        state.audiobooks.set_library(scan);
    }

    fn apply_storage_event(browser: &mut StorageBrowser, state: &mut AppState, event: ButtonEvent) {
        if event == ButtonEvent::Select {
            state.note_select_press();
        }
        let outcome = browser.apply_button(event);
        if outcome == StorageUiOutcome::ReturnHome {
            state.router.back();
        }
        state.update_storage_snapshot(browser.snapshot());
        info!(
            "rustmix-wave=storage-browser-event outcome={outcome:?} path={} entries={} retained-entries={} raw-entries={} selected={} preview={}",
            state.storage.current_path,
            state.storage.entries.len(),
            state.storage.scan.retained_entries,
            state.storage.scan.raw_entries,
            state.storage.selected,
            state.storage.preview.is_some()
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RefreshRequest {
        Normal,
        ForceGlobalAfterWake,
        ForceGlobalManual,
        #[allow(dead_code)]
        ForceGlobalSafetyFallback,
    }

    /// Upper bound on finishing a warm-cache book open before the first
    /// refresh of the Reader route. Measured opens take ~100 ms.
    const FAST_READER_OPEN_BUDGET: Duration = Duration::from_millis(400);

    /// While a book plays its position reaches the SD card this often, as
    /// well as at every pause, stop and track change: a power cut costs at
    /// most this much listening.
    const AUDIOBOOK_POSITION_SAVE_INTERVAL: Duration = Duration::from_secs(30);

    /// The SDMMC pins the card is mounted with (see the mount at boot),
    /// for "Connect to PC", which re-initializes the bare card in C.
    const USB_DISK_SD_PINS: usb_disk::espidf::SdPins = usb_disk::espidf::SdPins {
        clk: 16,
        cmd: 17,
        d0: 15,
        d1: 7,
        d2: 8,
        d3: 18,
    };

    /// Disk mode's only loop: nothing else runs while the PC owns the card.
    /// Any key but BOOT restarts the device -- GPIO0 is a strapping pin, and
    /// a restart with it held down would enter download mode. Presses queued
    /// before disk mode began are dropped, so only a fresh one ends it.
    fn wait_for_usb_disk_exit(input_queue: &InputEventQueue) -> ! {
        while input_queue.pop().is_some() {}
        loop {
            input_queue.wait_timeout(Duration::from_millis(500));
            while let Some(event) = input_queue.pop() {
                if !matches!(event, InputEvent::Back) {
                    info!("rustmix-wave=usb-disk status=restarting");
                    usb_disk::espidf::restart();
                }
            }
        }
    }

    /// Opening a book routes to `ReaderLoading` first. When the book's
    /// `.EPX` cache is warm the open itself finishes in ~100 ms, far less
    /// than the ~530 ms partial refresh the loading screen costs, and that
    /// screen was immediately replaced by a second refresh showing the page.
    /// Drive such an open to completion here so the one refresh draws the
    /// page directly. Cold opens (no cache, or TXT) keep the loading screen:
    /// they can take seconds and need the visible feedback. Stops on the
    /// first tick that is not a plain stage change, and between ticks once
    /// the budget is spent; an unfinished open simply carries on behind the
    /// loading screen as before.
    fn settle_fast_reader_open(state: &mut AppState) {
        if state.active_route() != ScreenRoute::ReaderLoading
            || !state.reader.pending_open_has_warm_cache()
        {
            return;
        }
        let mut span = boot_profile::span("reader-fast-open");
        let deadline = Instant::now() + FAST_READER_OPEN_BUDGET;
        let mut outcome = ReaderTickOutcome::None;
        while state.reader.loading.is_some() && Instant::now() < deadline {
            outcome = state.tick_reader();
            if outcome != ReaderTickOutcome::LoadingStageChanged {
                break;
            }
        }
        span.detail(format_args!("{outcome:?}"));
        if outcome == ReaderTickOutcome::FirstPageReady {
            info!(
                "rustmix-wave=reader-first-page-ready route={} policy=fast-open-no-loading-screen",
                state.active_route().marker()
            );
        }
    }

    fn refresh_screen<SPI, DC, RST, CS, BUSY, DELAY, POWER>(
        panel: &mut Epaper397<SPI, DC, RST, CS, BUSY, DELAY, POWER>,
        frame: &mut FrameBuffer,
        state: &mut AppState,
        coordinator: &mut PanelRefreshCoordinator,
        request: RefreshRequest,
    ) -> Result<()>
    where
        SPI: embedded_hal::spi::SpiBus<u8>,
        SPI::Error: core::fmt::Debug,
        DC: embedded_hal::digital::OutputPin,
        DC::Error: core::fmt::Debug,
        RST: embedded_hal::digital::OutputPin,
        RST::Error: core::fmt::Debug,
        CS: embedded_hal::digital::OutputPin,
        CS::Error: core::fmt::Debug,
        BUSY: embedded_hal::digital::InputPin,
        BUSY::Error: core::fmt::Debug,
        DELAY: DelayNs,
        POWER: waveshare_epd397_rust_app::power::PanelPower,
    {
        settle_fast_reader_open(state);
        if state.active_route() == ScreenRoute::AudiobookPlayer {
            state.audiobooks.note_player_drawn();
        }
        // A refresh request while the panel sleeps (rail off after the idle
        // timeout) cannot succeed: the controller is unpowered, BUSY reads
        // high through its pull-up, and `wait_until_idle` times out after
        // 15 s with an error that ends the firmware. Every caller is meant to
        // check `panel_awake` first; wake the panel here instead of trusting
        // that, and treat the frame as the global after-wake refresh a
        // freshly initialized controller needs anyway.
        let request = if state.panel_awake {
            request
        } else {
            let line = format!(
                "rustmix-wave=panel-refresh status=woke-sleeping-panel route={}",
                state.active_route().marker()
            );
            warn!("{line}");
            append_reset_log(&line);
            panel.initialize()?;
            state.panel_awake = true;
            RefreshRequest::ForceGlobalAfterWake
        };
        let coordinator_request = match request {
            RefreshRequest::Normal => PanelRefreshRequest::Normal,
            RefreshRequest::ForceGlobalAfterWake => PanelRefreshRequest::AfterWake,
            RefreshRequest::ForceGlobalManual => PanelRefreshRequest::ManualGhostCleanup,
            RefreshRequest::ForceGlobalSafetyFallback => PanelRefreshRequest::SafetyFallback,
        };
        let full_page_image = state.active_route() == ScreenRoute::ReaderPage
            && state
                .reader
                .session
                .as_ref()
                .is_some_and(|session| session.current_page_is_full_page_image());
        // Must match the inversion `reader::render_page` applies.
        let inverted = state.active_route() == ScreenRoute::ReaderPage
            && state.reader.session.is_some()
            && state.reader.preferences.theme == ReadingTheme::HighContrast;
        let plan = coordinator.plan_for_frame(coordinator_request, full_page_image, inverted);
        sync_panel_refresh_diagnostics(state, coordinator);
        render_current_screen(frame, state)?;

        match plan {
            PanelRefreshPlan::GlobalBase { reason } => {
                if reason.uses_fast_waveform() {
                    panel.show_base_fast(frame.as_bytes())?;
                    info!(
                        "rustmix-wave=panel-refresh plan=global-base reason={} transport=global-base-fast",
                        reason.marker()
                    );
                } else {
                    panel.show_base(frame.as_bytes())?;
                    info!(
                        "rustmix-wave=panel-refresh plan=global-base reason={} transport=global-base",
                        reason.marker()
                    );
                }
                match reason {
                    PanelGlobalReason::AfterWake => info!("rustmix-wave=wake-global-refresh"),
                    PanelGlobalReason::ManualGhostCleanup => {
                        info!("rustmix-wave=reader-clear-ghosting refresh=global-base");
                        info!("rustmix-wave=power-key-clear-ghosting refresh=global-base")
                    }
                    PanelGlobalReason::PeriodicCleanup => {
                        info!("rustmix-wave=global-refresh-after-partials")
                    }
                    PanelGlobalReason::SafetyFallback => {
                        warn!("rustmix-wave=panel-refresh safety-fallback refresh=global-base")
                    }
                    PanelGlobalReason::InitialBoot
                    | PanelGlobalReason::SleepImage
                    | PanelGlobalReason::FullPageImageTransition
                    | PanelGlobalReason::InvertedPageExit => {}
                }
            }
            PanelRefreshPlan::PartialFullscreen { partial_count } => {
                panel.show_partial_fullscreen(frame.as_bytes())?;
                info!(
                    "rustmix-wave=panel-refresh plan=partial-fullscreen reason=normal partial-count={partial_count} partial-limit={PANEL_PARTIAL_REFRESH_LIMIT} transport=existing-fullscreen-partial"
                );
            }
        }
        // A page turn's STATE/POSITS/RECENT save is *not* forced here any
        // more: it's debounced (see `ReaderUiState::pending_persist`) and
        // only actually written once the reader has sat on a page for
        // `READER_PERSIST_DEBOUNCE`, checked from `ReaderUiState::tick` (the
        // reader route's ~250ms poll below) instead of after every redraw.
        // Flushing here unconditionally, right after each page-turn redraw,
        // would write on every single page turn again and defeat that
        // coalescing. Forced flushes for the cases a debounced save must not
        // be left waiting (leaving the Reader route, entering deep sleep)
        // are called explicitly at those sites instead.
        Ok(())
    }

    fn sync_panel_refresh_diagnostics(state: &mut AppState, coordinator: &PanelRefreshCoordinator) {
        state.partial_refreshes = coordinator.partial_count();
    }

    fn log_reader_persistence_event(state: &mut AppState) {
        if let Some(event) = state.reader.take_persistence_event() {
            info!("rustmix-wave=reader-persistence {event}");
        }
    }

    /// Bring the Continue Reading tile's thumbnail into RAM from the SD
    /// cache before a Home paint, so the card is drawn complete in a single
    /// refresh instead of first as an empty frame and then again once the
    /// main loop's Continue Reading block catches up. Only a cache hit is
    /// used (one ~6.5KB file read); a missing thumbnail is still built by
    /// that block, which can take much longer. No-op when the RAM copy
    /// already matches the book.
    fn sync_continue_reading_thumbnail(state: &mut AppState, cover_cache: &CoverCache) {
        let Some(book) = state.reader.continue_reading_book() else {
            return;
        };
        let up_to_date = state
            .reader
            .continue_reading_thumbnail
            .as_ref()
            .is_some_and(|(path, _)| *path == book.path);
        if !up_to_date {
            if let Some(thumbnail) = cover_cache.load_cached_thumbnail(&book) {
                state.reader.continue_reading_thumbnail = Some((book.path, thumbnail));
            }
        }
    }

    /// Pick the deep-sleep frame per the Display setting. Book cover mode
    /// shows the cover only when sleep interrupts reading (`reading`); from
    /// any other screen, or when the cover is unusable, it falls back to the
    /// in-order SD images. Returns the frame and a label for sleep-mode
    /// diagnostics.
    fn select_sleep_frame(
        state: &AppState,
        sleep_images: &mut SleepImageCatalog,
        reading: bool,
    ) -> (FrameBuffer, String) {
        let mode = state.display.sleep_screen;
        if mode == SleepScreenMode::BookCover && !reading {
            info!("rustmix-wave=sleep-cover status=fallback reason=not-reading");
        } else if mode == SleepScreenMode::BookCover {
            if let Some(book) = state.reader.continue_reading_book() {
                let started = Instant::now();
                let cover = CoverCache::new(state.reader.cache_directory())
                    .load_or_generate_fullscreen_cover(&book, SLEEP_COVER_WIDTH, SLEEP_COVER_HEIGHT);
                if let Some(cover) = cover {
                    let percent = state.reader.continue_reading_percent().unwrap_or(0);
                    let (frame, tab) = compose_cover_sleep_frame(
                        &cover,
                        percent,
                        state.regional.locale,
                    );
                    info!(
                        "rustmix-wave=sleep-cover status=ready path={} percent={percent} tab-left={} tab-top={} tab-bottom={} elapsed-ms={}",
                        book.path,
                        tab.left,
                        tab.top,
                        tab.bottom,
                        started.elapsed().as_millis()
                    );
                    return (frame, format!("cover:{}", book.title));
                }
                info!("rustmix-wave=sleep-cover status=fallback reason=no-usable-cover path={}", book.path);
            } else {
                info!("rustmix-wave=sleep-cover status=fallback reason=no-book");
            }
        }
        let selection = if mode == SleepScreenMode::Random {
            sleep_images.select_random(unsafe { sys::esp_random() })
        } else {
            sleep_images.select_next()
        };
        log_sleep_image_selection(&selection);
        (selection.frame, selection.file_name)
    }

    fn log_sleep_image_selection(selection: &SleepImageSelection) {
        let error = selection.scan_error.as_deref().unwrap_or("none");
        if selection.fallback {
            warn!(
                "rustmix-wave=sleep-image-fallback source=built-in path={SLEEP_IMAGE_DIRECTORY} raw={} candidates={} metadata-fallbacks={} ignored={} valid={} rejected={} error={}",
                selection.raw_entries,
                selection.candidate_entries,
                selection.metadata_fallbacks,
                selection.ignored_entries,
                selection.valid_count,
                selection.rejected_count,
                error
            );
        } else {
            info!(
                "rustmix-wave=sleep-image-scan status=ready path={SLEEP_IMAGE_DIRECTORY} raw={} candidates={} metadata-fallbacks={} ignored={} valid={} rejected={} error={}",
                selection.raw_entries,
                selection.candidate_entries,
                selection.metadata_fallbacks,
                selection.ignored_entries,
                selection.valid_count,
                selection.rejected_count,
                error
            );
            info!(
                "rustmix-wave=sleep-image-selected file={} width=800 height=480 bpp=1 payload-bytes=48000",
                selection.file_name
            );
            if let Some(choice) = selection.choice {
                info!(
                    "rustmix-wave=sleep-image-choice mode=hardware-random candidates={} random-word=0x{:08X} previous-index={} selected-index={} anti-repeat={}",
                    selection.valid_count,
                    choice.random_word,
                    choice
                        .previous_index
                        .map_or_else(|| "none".into(), |index| index.to_string()),
                    choice.selected_index,
                    choice.anti_repeat
                );
            }
        }
    }

    fn log_board_snapshot(snapshot: BoardSnapshot, regional: RegionalPreferences) {
        info!(
            "rustmix-wave=board-snapshot time={} timezone={} battery={}",
            snapshot.time_label(regional),
            regional.timezone_label_for_rtc(snapshot.rtc),
            snapshot.battery_label()
        );
    }

    fn log_storage_snapshot(snapshot: &StorageSnapshot) {
        info!(
            "rustmix-wave=storage-browser-snapshot mounted={} path={} entries={} retained-entries={} raw-entries={} metadata-fallbacks={} ignored-special={} selected={} preview={} error={}",
            snapshot.mounted,
            snapshot.current_path,
            snapshot.entries.len(),
            snapshot.scan.retained_entries,
            snapshot.scan.raw_entries,
            snapshot.scan.metadata_fallbacks,
            snapshot.scan.ignored_special,
            snapshot.selected,
            snapshot.preview.is_some(),
            snapshot.error.as_deref().unwrap_or("none")
        );
    }

    fn log_network_snapshot(snapshot: &NetworkSnapshot) {
        info!(
            "rustmix-wave=network-snapshot wifi={} ntp={} ssid={} ipv4={} rssi={} timezone={} ntp-server={} last-sync={} error={}",
            snapshot.wifi_state.label(),
            snapshot.ntp_state.label(),
            snapshot.ssid_label(),
            snapshot.ipv4_label(),
            snapshot.rssi_label(),
            snapshot.timezone_name,
            snapshot.ntp_server,
            snapshot.last_sync_label(),
            snapshot.error.as_deref().unwrap_or("none")
        );
    }

    fn log_audio_snapshot(snapshot: &AudioSnapshot) {
        info!(
            "rustmix-wave=audio-snapshot available={} codec-address={} codec-ready={} i2s-ready={} amp={} mute={} volume={} state={} error={}",
            snapshot.available,
            snapshot.codec_address_label(),
            snapshot.codec_ready,
            snapshot.i2s_ready,
            snapshot.amplifier_enabled,
            snapshot.muted,
            snapshot.volume_percent,
            snapshot.playback_state.label(),
            snapshot.error.as_deref().unwrap_or("none")
        );
    }

    /// Keep every GPIO in its normal, awake configuration during automatic
    /// light sleep.
    ///
    /// On ESP32-S3, ESP-IDF's `ESP_SLEEP_GPIO_RESET_WORKAROUND` (default on)
    /// forces `PM_SLP_DISABLE_GPIO`, which isolates every pin -- outputs
    /// released, pulls removed -- for each light sleep. That floats the
    /// e-paper RST/DC/CS lines and drops the SD bus pull-ups mid-session,
    /// and the first field test with light sleep actually engaging ended in
    /// a hang. The workaround guards against resets from electrostatic
    /// pulses on *floating* input-only pins; every input here (keys, panel
    /// BUSY, RTC INT) has a pull-up, so keeping the awake configuration does
    /// not reintroduce that case. Costs ~0.2-0.3 mA while asleep (ESP-IDF
    /// docs). Must run after every driver has configured its pins, since
    /// the startup hook enabled the per-pin sleep switch before them.
    fn keep_gpio_state_in_light_sleep() {
        unsafe { sys::esp_sleep_enable_gpio_switch(false) };
        info!("rustmix-wave=light-sleep-gpio status=awake-config-kept");
    }

    /// ESP-IDF `NO_LIGHT_SLEEP` power-management lock, held while the main
    /// loop is in a state that must not be interrupted by automatic light
    /// sleep (see its `needs_fast_tick`). Creation failure only disables the
    /// guard (logged), it never stops the firmware.
    struct LightSleepGuard {
        handle: sys::esp_pm_lock_handle_t,
        held: bool,
    }

    impl LightSleepGuard {
        fn new() -> Self {
            let mut handle: sys::esp_pm_lock_handle_t = core::ptr::null_mut();
            let status = unsafe {
                sys::esp_pm_lock_create(
                    sys::esp_pm_lock_type_t_ESP_PM_NO_LIGHT_SLEEP,
                    0,
                    c"rustmix-busy".as_ptr(),
                    &mut handle,
                )
            };
            if status != sys::ESP_OK {
                warn!("rustmix-wave=light-sleep-guard status=create-failed error-code={status}");
                handle = core::ptr::null_mut();
            }
            Self {
                handle,
                held: false,
            }
        }

        fn set(&mut self, hold: bool) {
            if self.handle.is_null() || hold == self.held {
                return;
            }
            let status = unsafe {
                if hold {
                    sys::esp_pm_lock_acquire(self.handle)
                } else {
                    sys::esp_pm_lock_release(self.handle)
                }
            };
            if status == sys::ESP_OK {
                self.held = hold;
            } else {
                warn!("rustmix-wave=light-sleep-guard status=set-failed hold={hold} error-code={status}");
            }
        }
    }

    /// The panel's `SpiBus`, over a per-transaction `SpiDeviceDriver`.
    ///
    /// esp-idf-hal's `SpiBusDriver` holds the SPI bus for its whole lifetime
    /// (`spi_device_acquire_bus`), and ESP-IDF pins an `APB_FREQ_MAX`
    /// power-management lock for as long as a bus is held ("this keeps the
    /// spi clock at 80MHz even if all tasks are blocked", spi_master.c). The
    /// panel driver lives from boot to shutdown, so that lock was never
    /// released: measured with the PM-profiling build, `spi_master` held it
    /// for 100% of uptime, which keeps the chip at >= 80 MHz and rules out
    /// automatic light sleep entirely. A device driver takes the bus and the
    /// lock only for each transfer. The panel is the only device on this bus
    /// and drives its CS as a plain GPIO (see `Epaper397::new`), so nothing
    /// can interleave between two writes.
    struct PanelSpi<'d>(SpiDeviceDriver<'d, SpiDriver<'d>>);

    impl embedded_hal::spi::ErrorType for PanelSpi<'_> {
        type Error = SpiError;
    }

    impl embedded_hal::spi::SpiBus<u8> for PanelSpi<'_> {
        fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            self.0.read(words).map_err(SpiError::other)
        }

        fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
            self.0.write(words).map_err(SpiError::other)
        }

        fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
            self.0.transfer(read, write).map_err(SpiError::other)
        }

        fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            self.0.transfer_in_place(words).map_err(SpiError::other)
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            // Every call above returns only after its transfer completed.
            Ok(())
        }
    }

    /// Delays of at least one FreeRTOS tick yield the CPU; shorter ones
    /// busy-wait instead. At `CONFIG_FREERTOS_HZ=100` a `vTaskDelay` can't be
    /// shorter than one 10 ms tick, so the sensor/panel sequences' 300 us and
    /// 1-2 ms waits used to cost up to 10 ms each (~80 ms per boot measured:
    /// SHTC3 init and sample, QMI8658 CTRL9 handshakes, e-paper reset pulse).
    #[derive(Clone, Copy, Debug, Default)]
    struct FreeRtosDelay;

    const FREERTOS_TICK_US: u32 = 1_000_000 / sys::configTICK_RATE_HZ;

    impl DelayNs for FreeRtosDelay {
        fn delay_ns(&mut self, nanoseconds: u32) {
            let microseconds = nanoseconds.saturating_add(999) / 1_000;
            if microseconds >= FREERTOS_TICK_US {
                FreeRtos::delay_ms(microseconds.div_ceil(1_000));
            } else if microseconds > 0 {
                Ets::delay_us(microseconds);
            }
        }
    }
}

#[cfg(target_os = "espidf")]
fn main() -> anyhow::Result<()> {
    // `run` only returns on an unrecoverable error (a panel or storage
    // operation failing through `?`). Returning from `main` would end the
    // main task and leave the device frozen on its last frame until the
    // battery is disconnected, so log the cause and restart instead.
    if let Err(error) = firmware::run() {
        let line = format!("rustmix-wave=firmware-fatal error={error:#} action=restart-in-3s");
        log::error!("{line}");
        firmware::append_reset_log(&line);
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(3000);
        unsafe { esp_idf_svc::sys::esp_restart() };
    }
    Ok(())
}

#[cfg(not(target_os = "espidf"))]
fn main() {
    println!("Build this firmware for xtensa-esp32s3-espidf. See README.md.");
}
