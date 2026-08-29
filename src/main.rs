#[cfg(target_os = "espidf")]
mod firmware {
    use std::{
        ffi::CString,
        time::{Duration, Instant},
    };

    use anyhow::Result;
    use embedded_hal::delay::DelayNs;
    #[cfg(feature = "rustmix-remote-ble")]
    use esp_idf_svc::{
        bt::{Ble, BtDriver},
        nvs::EspDefaultNvsPartition,
    };
    use esp_idf_svc::{
        fs::fatfs::Fatfs,
        hal::{
            delay::FreeRtos,
            gpio::{AnyIOPin, PinDriver, Pull},
            i2c::{I2cConfig, I2cDriver},
            i2s::{
                config::{
                    ClockSource, Config as I2sChannelConfig, DataBitWidth, MclkMultiple, SlotMode,
                    StdClkConfig, StdConfig, StdGpioConfig, StdSlotConfig,
                },
                I2sBiDir, I2sDriver,
            },
            peripherals::Peripherals,
            sd::{
                mmc::{SdMmcHostConfiguration, SdMmcHostDriver},
                SdCardConfiguration, SdCardDriver,
            },
            spi::{config::Config as SpiConfig, Dma, SpiBusDriver, SpiDriver, SpiDriverConfig},
            units::*,
        },
        io::vfs::MountedFatfs,
        log::EspLogger,
        sys,
    };
    use log::{info, warn};
    #[cfg(feature = "rustmix-remote-ble")]
    use waveshare_epd397_rust_app::rustmix_remote::{
        ble_gatt::RustmixRemoteBleGattService, RemoteEvent, RemoteEventQueue,
    };
    use waveshare_epd397_rust_app::{
        alarm::{AlarmEngine, AlarmSnapshot, AlarmUiOutcome, ALARMS_CONFIG_PATH},
        app::{
            display::{DisplayPreferences, DISPLAY_CONFIG_PATH},
            render_current_screen,
            screens::reader::library_visible_books,
            AppState, ScreenRoute, ALARM_POLL_SECONDS, IMU_EVENT_SCREEN_REFRESH_SECONDS,
            LIBRARY_THUMBNAIL_REFRESH_SECONDS, MOTION_LIVE_REFRESH_SECONDS,
            NETWORK_LIVE_REFRESH_SECONDS, NETWORK_LOG_HEARTBEAT_SECONDS, PANEL_IDLE_SLEEP_SECONDS,
            SAMPLE_LIVE_REFRESH_SECONDS, VOICE_RECORD_SCREEN_REFRESH_SECONDS,
        },
        audio::{
            espidf::AudioRuntime, AudioSnapshot, AudioUiRequest, AUDIO_MCLK_HZ,
            AUDIO_SAMPLE_RATE_HZ, DEFAULT_AUDIO_VOLUME_PERCENT,
        },
        board_services::{BoardServices, BoardSnapshot},
        build_info::{FIRMWARE_VERSION, PRODUCT_SLUG, UI_SHELL_MILESTONE},
        buttons::{
            BootBackButton, ButtonEvent, Buttons, SelectHoldButton, SelectPressEvent,
            SELECT_LONG_PRESS_MS,
        },
        calendar::{
            create_personal_event, delete_personal_event, update_personal_event, CalendarUiRequest,
            CALENDAR_EVENTS_FILE, CALENDAR_ROOT, CALENDAR_US_EVENTS_FILE,
        },
        cover_cache::CoverCache,
        dictionary::{DICTIONARY_ROOT, DICTIONARY_SHARD_MAX_BYTES},
        epaper::Epaper397,
        framebuffer::FrameBuffer,
        games::dirty_regions::MAX_DIRTY_REGIONS,
        imu_events::IMU_EVENT_SAMPLE_INTERVAL_MS,
        input_events::{InputEvent, InputEventQueue},
        lua_runtime::{catalog::LUA_APPS_DIRECTORY, loader::LUA_LOADER_WORKER_STACK_BYTES},
        mcu_deep_sleep,
        network::{
            espidf::NetworkRuntime, NetworkLogFingerprint, NetworkSnapshot, WifiConnectionState,
        },
        network_config::{
            NetworkConfig, SavedNetwork, DEFAULT_NTP_SERVER, DEFAULT_TIMEZONE, WIFI_CONFIG_PATH,
        },
        network_provision::{
            espidf::NetworkProvisionServer, NetworkProvisionSnapshot, NetworkProvisionUiRequest,
            NETWORK_PROVISION_RESCAN_SECONDS,
        },
        network_saved::SavedNetworkEntry,
        panel_refresh::{
            PanelGlobalReason, PanelRefreshCoordinator, PanelRefreshPlan, PanelRefreshRequest,
            PANEL_PARTIAL_REFRESH_LIMIT,
        },
        power::Axp2101,
        power_key::{
            PowerKeyEvent, SleepWakeGuard, SleepWakeGuardDecision, POWER_KEY_POLL_MS,
            POWER_KEY_WAKE_GUARD_QUIET_MS,
        },
        reader::{ReaderDictionaryMode, ReaderTickOutcome},
        regional::{self, RegionalPreferences, CLOCK_CONFIG_PATH},
        rtc::RtcDateTime,
        rtc_alarm_interrupt::{espidf::RtcAlarmInterruptMonitor, RTC_ALARM_INTERRUPT_GPIO},
        runtime_memory::log_runtime_memory,
        shared_i2c::SharedI2cBus,
        sleep_images::{SleepImageCatalog, SleepImageSelection, SLEEP_IMAGE_DIRECTORY},
        sleep_mode::{SleepModeState, SleepWakeCause},
        sleep_network::SleepNetworkState,
        sleep_wake_overlay,
        storage::{
            StorageBrowser, StorageSnapshot, StorageUiOutcome, SDMMC_COMMAND_TIMEOUT_MS,
            SDMMC_STABLE_SPEED_KHZ, SD_MOUNT_POINT, STORAGE_IO_RETRY_ATTEMPTS,
        },
        voice_note_metadata::{
            load_voice_notes_preferences, save_voice_notes_preferences, VoiceNotesPreferences,
            VOICE_UNKNOWN_RECORDED_AT,
        },
        voice_notes::{
            cleanup_stale_voice_tmp, delete_voice_note, save_voice_note_title, VoiceNotesUiRequest,
            VoicePlaybackSession, VoiceRecordingSession, VOICE_NOTES_ROOT,
            VOICE_PCM_MONO_CHUNK_BYTES, VOICE_PCM_STEREO_CAPTURE_BYTES,
        },
        weather::{
            espidf::fetch_open_meteo_on_worker, WeatherFetchError, WeatherSnapshot,
            WEATHER_RETRY_DELAYS_SECONDS, WEATHER_RETRY_LIMIT,
        },
        weather_config::{WeatherConfig, WEATHER_CONFIG_PATH},
        wifi_transfer::{
            espidf::WifiTransferServer, WifiTransferSnapshot, WifiTransferUiRequest,
            WIFI_TRANSFER_INACTIVITY_SECONDS, WIFI_TRANSFER_ROOT, WIFI_TRANSFER_SERVER_STACK_BYTES,
        },
    };

    pub fn run() -> Result<()> {
        sys::link_patches();
        EspLogger::initialize_default();
        info!("rustmix-wave=epd397-rust-app-start");

        // Battery optimization: let the CPU drop to XTAL frequency (40 MHz)
        // and, whenever every FreeRTOS task is blocked/suspended for long
        // enough, into automatic light sleep, instead of always running at
        // the configured 160 MHz ceiling. Enabled here for the ordinary
        // active-use main loop; `mcu_deep_sleep::espidf::enter` disables it
        // again immediately before arming GPIO5's real deep-sleep EXT1
        // wakeup (the two were observed to conflict when both were active
        // at once -- see that function's docs) and restores it if the
        // real-deep-sleep attempt fails and the software-only fallback loop
        // keeps running instead. Wi-Fi's own modem sleep already defaults to
        // WIFI_PS_MIN_MODEM and is unaffected by this call.
        let pm_config = sys::esp_pm_config_t {
            max_freq_mhz: mcu_deep_sleep::CPU_MAX_FREQ_MHZ,
            min_freq_mhz: mcu_deep_sleep::CPU_MIN_FREQ_MHZ,
            light_sleep_enable: true,
        };
        match unsafe { sys::esp_pm_configure((&raw const pm_config).cast::<core::ffi::c_void>()) } {
            sys::ESP_OK => info!(
                "rustmix-wave=power-management status=enabled max-mhz={} min-mhz={} light-sleep={}",
                pm_config.max_freq_mhz, pm_config.min_freq_mhz, pm_config.light_sleep_enable
            ),
            error => warn!("rustmix-wave=power-management status=failed error-code={error}"),
        }
        info!(
            "rustmix-wave=product-ui-shell-start product={PRODUCT_SLUG} version={FIRMWARE_VERSION} milestone={UI_SHELL_MILESTONE}"
        );
        let boot_cause = mcu_deep_sleep::espidf::boot_cause();
        info!(
            "rustmix-wave=boot-cause status=classified cause={} wake-gpio={}",
            boot_cause.marker(),
            mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO
        );

        let peripherals = Peripherals::take()?;

        #[cfg(feature = "rustmix-remote-ble")]
        let rustmix_remote_queue = RemoteEventQueue::default();

        // The uploaded Waveshare sample uses SDMMC in 4-bit mode:
        // CMD GPIO17, CLK GPIO16, D0 GPIO15, D1 GPIO7, D2 GPIO8, D3 GPIO18.
        // Mount failure is non-fatal so the verified product shell still boots
        // when no card is inserted.
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
                info!(
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
        let mut storage_browser = StorageBrowser::new(SD_MOUNT_POINT, mounted_sd.is_some());
        let _mounted_sd = mounted_sd;

        // The e-paper panel and the I2C-driven PMIC rail that powers it are
        // brought up here, ahead of the display/network/weather/alarm config
        // loads and sensor bring-up below, so a real hardware deep-sleep wake
        // (a full reboot; see mcu_deep_sleep) can show the "RIATTIVAZIONE"
        // wake overlay as early as possible instead of leaving the retained
        // sleep image on screen with no feedback through the rest of boot.
        //
        // PMIC (power key), RTC (alarms) and IMU all share this bus and are
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
        let panel_power = Axp2101::new(shared_i2c.clone());

        let spi_driver_config = SpiDriverConfig::new().dma(Dma::Auto(4096));
        let spi_driver = SpiDriver::new(
            peripherals.spi3,
            peripherals.pins.gpio11,
            peripherals.pins.gpio12,
            None::<AnyIOPin>,
            &spi_driver_config,
        )?;
        let spi_config = SpiConfig::new().baudrate(20.MHz().into()).write_only(true);
        let spi = SpiBusDriver::new(spi_driver, &spi_config)?;

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

        // Real deep sleep is a full reboot: nothing in RAM survived, but the
        // e-paper image itself needs no redraw to "stay" since it is still
        // physically on the glass with no power applied. Only the overlay
        // box is new work here; the rest of boot (SD-backed config loads,
        // sensor bring-up) proceeds exactly as on any other boot afterward,
        // ending in the same single clean global refresh to Home. Best
        // effort: a failure here just means the ordinary panel-initialize
        // path below runs instead, same as a normal boot.
        let mut panel_initialized = false;
        if boot_cause == mcu_deep_sleep::BootCause::DeepSleepGpioWake {
            match draw_deep_sleep_wake_overlay(&mut panel) {
                Ok(()) => {
                    panel_initialized = true;
                    info!(
                        "rustmix-wave=wake-overlay status=shown label=RIATTIVAZIONE cause=deep-sleep-gpio-wake"
                    );
                }
                Err(error) => warn!(
                    "rustmix-wave=wake-overlay status=failed cause=deep-sleep-gpio-wake error={error:#}"
                ),
            }
        }

        let display_preferences = match DisplayPreferences::load_from_path(DISPLAY_CONFIG_PATH) {
            Ok(preferences) => {
                info!(
                    "rustmix-wave=display-config status=ready path={DISPLAY_CONFIG_PATH} font-family={} font-size={}",
                    preferences.font_family.marker(),
                    preferences.font_size.marker()
                );
                preferences
            }
            Err(error) => {
                let preferences = DisplayPreferences::default();
                warn!(
                    "rustmix-wave=display-config status=default path={DISPLAY_CONFIG_PATH} font-family={} font-size={} error={error:#}",
                    preferences.font_family.marker(),
                    preferences.font_size.marker()
                );
                preferences
            }
        };

        // Credentials are read from removable storage. Never log the password.
        // Mutable so a save from the on-device Wi-Fi setup screen keeps this
        // cache in sync for later sleep/wake reconnects.
        let mut network_config = match NetworkConfig::load_from_path(WIFI_CONFIG_PATH) {
            Ok(config) => {
                info!(
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

        let weather_config = match WeatherConfig::load_from_path(WEATHER_CONFIG_PATH) {
            Ok(config) => {
                info!(
                    "rustmix-wave=weather-config status=ready path={WEATHER_CONFIG_PATH} provider={} location={} latitude={:.4} longitude={:.4} timezone={} refresh-minutes={}",
                    config.provider,
                    config.location,
                    config.latitude,
                    config.longitude,
                    config.timezone,
                    config.refresh_minutes
                );
                Some(config)
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=weather-config status=unavailable path={WEATHER_CONFIG_PATH} error={error:#}"
                );
                None
            }
        };

        let mut alarm_engine = match AlarmEngine::load_from_path(ALARMS_CONFIG_PATH) {
            Ok(engine) => {
                let snapshot = engine.snapshot();
                info!(
                    "rustmix-wave=alarm-config status=ready path={ALARMS_CONFIG_PATH} schedules={} snooze-minutes={}",
                    snapshot.alarms.len(), snapshot.snooze_minutes
                );
                engine
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=alarm-config status=unavailable path={ALARMS_CONFIG_PATH} error={error:#}"
                );
                AlarmEngine::unavailable(format!("{error:#}"))
            }
        };

        let mut board_services = BoardServices::new(shared_i2c.clone());

        // Schematic trace confirmed ALDO1, ALDO4, BLDO1, BLDO2, CPUSLDO,
        // DLDO1, DLDO2, and DCDC2-4 have no downstream load on this board
        // revision (unmounted resistor, no net, or no inductor on LX), so
        // disable them once at boot regardless of the PMIC's power-on
        // default. DCDC1 (system VCC3V3) and DCDC5 are never touched.
        let mut misc_power = Axp2101::new(shared_i2c.clone());
        match misc_power.disable_unused_pmic_rails() {
            Ok(()) => info!("rustmix-wave=pmic-unused-rails-disable status=done rails=aldo1,aldo4,bldo1,bldo2,cpusldo,dldo1,dldo2,dcdc2,dcdc3,dcdc4"),
            Err(error) => {
                warn!("rustmix-wave=pmic-unused-rails-disable status=failed error={error:#}")
            }
        }

        let buttons = Buttons::new(
            PinDriver::input(peripherals.pins.gpio4, Pull::Up)?,
            PinDriver::input(peripherals.pins.gpio6, Pull::Up)?,
        );
        let select_button =
            SelectHoldButton::new(PinDriver::input(peripherals.pins.gpio5, Pull::Up)?);
        let back_button = BootBackButton::new(PinDriver::input(peripherals.pins.gpio0, Pull::Up)?);
        info!(
            "rustmix-wave=boot-button-back status=ready gpio=0 active-low=true press=short action=back"
        );
        info!(
            "rustmix-wave=select-button-hold status=ready gpio=5 active-low=true short-press=confirm hold-ms={SELECT_LONG_PRESS_MS} long-press=contextual-navigation"
        );
        // The uploaded BSP routes the PCF85063 active-low alarm output to
        // GPIO45. Validate that board-level line before introducing MCU
        // deep-sleep entry in the following isolated power milestone.
        let mut rtc_alarm_interrupt =
            RtcAlarmInterruptMonitor::new(PinDriver::input(peripherals.pins.gpio45, Pull::Up)?);
        info!(
            "rustmix-wave=rtc-alarm-int status=ready gpio={RTC_ALARM_INTERRUPT_GPIO} active-low=true wake-policy=active-loop-readiness"
        );

        // Button GPIOs are polled on a dedicated background thread so a
        // press is never dropped while the main loop is stuck busy-waiting
        // inside a slow e-paper panel refresh (`Epaper397::wait_until_idle`
        // can block for hundreds of ms up to ~1.5s). The thread only detects
        // and enqueues events (same boundary contract as the BLE remote
        // queue below); the main loop remains the sole owner of `AppState`
        // and all rendering, draining one event per tick in FIFO order.
        const INPUT_POLL_STACK_BYTES: usize = 8 * 1024;
        const INPUT_POLL_IDLE_SLEEP_MS: u64 = 10;
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
                                input_queue.push(InputEvent::SelectLongPress);
                                activity = true;
                            }
                            Ok(Some(SelectPressEvent::ShortPress)) => {
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
        info!(
            "rustmix-wave=input-poll-thread status=ready stack-bytes={INPUT_POLL_STACK_BYTES} idle-sleep-ms={INPUT_POLL_IDLE_SLEEP_MS}"
        );

        let mut service_delay = FreeRtosDelay;
        let mut frame = FrameBuffer::new_white();
        // Keep the growing product UI state off the firmware main-task stack.
        // HTTPS weather retrieval and display refreshes still execute from the
        // same orchestrator, but their stack budget is no longer reduced by a
        // long-lived inline AppState allocation.
        let mut state = Box::new(AppState::default());
        let mut panel_refresh = PanelRefreshCoordinator::default();
        sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
        state.display = display_preferences;
        // Reader/voice-notes/Lua SD catalog scans, and the audio codec
        // bring-up right after them, are deferred until after the first
        // e-paper frame is visible (see below the panel draw). The Home
        // screen's menu tiles are a static const list and never read these,
        // so nothing before the first frame needs them.
        let mut sleep_images = SleepImageCatalog::default();
        let mut sleep_mode = SleepModeState::default();
        let mut sleep_wake_guard = SleepWakeGuard::default();
        let mut sleep_wake_guard_started_at: Option<Instant> = None;
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
        match regional::load_timezone_name(CLOCK_CONFIG_PATH) {
            Ok(timezone) => {
                state.regional = state.regional.with_timezone_name(&timezone)?;
                info!(
                    "rustmix-wave=clock-config status=ready path={CLOCK_CONFIG_PATH} timezone={timezone}"
                );
            }
            Err(error) => {
                info!(
                    "rustmix-wave=clock-config status=unavailable path={CLOCK_CONFIG_PATH} error={error:#}"
                );
            }
        }
        if let Some(config) = weather_config.as_ref() {
            state.update_weather_snapshot(WeatherSnapshot::provisioned(config));
        }
        state.update_alarm_snapshot(alarm_engine.snapshot());
        state.update_storage_snapshot(storage_browser.snapshot());
        log_storage_snapshot(&state.storage);
        info!(
            "rustmix-wave=regional-profile timezone={} display-offset={} rtc-storage-offset={} temperature-unit={}",
            state.regional.timezone_name(),
            state.regional.timezone_label_for_rtc(state.board.rtc),
            state.regional.rtc_storage_label(),
            state.regional.temperature_unit.marker()
        );

        let init = board_services.initialize(&mut service_delay);
        info!(
            "rustmix-wave=sample-board-services-init rtc={} environment={} power={} imu={} rtc-integrity-lost={} shtc3-id={} qmi8658-address={} qmi8658-revision={}",
            init.rtc_available,
            init.environment_available,
            init.power_monitoring_available,
            init.imu_available,
            init.rtc_clock_integrity_was_lost,
            init.environment_sensor_id
                .map_or_else(|| "unavailable".into(), |id| format!("0x{id:04X}")),
            init.imu_address
                .map_or_else(|| "unavailable".into(), |value| format!("0x{value:02X}")),
            init.imu_revision
                .map_or_else(|| "unavailable".into(), |value| format!("0x{value:02X}"))
        );
        let mut power_key_available = match board_services.initialize_power_key_events() {
            Ok(()) => {
                info!("rustmix-wave=power-key status=ready source=axp2101-pek events=short-menu,long-sleep poll-ms={POWER_KEY_POLL_MS}");
                true
            }
            Err(error) => {
                warn!(
                    "rustmix-wave=power-key status=unavailable source=axp2101-pek error={error:#}"
                );
                false
            }
        };
        state.update_board_snapshot(board_services.read_snapshot(&mut service_delay));
        log_board_snapshot(state.board, state.regional);
        if let Some(rtc) = state.board.rtc {
            alarm_engine.recompute_next(state.regional.localize_rtc(rtc));
        }
        sync_alarm_hardware(&mut alarm_engine, &mut board_services, state.regional);
        state.update_alarm_snapshot(alarm_engine.snapshot());
        log_alarm_snapshot(&state.alarms);

        // A real hardware deep-sleep wake is a full reboot: nothing in RAM,
        // including the router's route, survived. Reader persistence is
        // normally deferred until after the first frame (see below) to keep
        // every other boot fast, but when the durable marker recorded at the
        // last deep-sleep entry (see the long-press handler below) says the
        // Reader was active, load it now so the very first frame can route
        // straight into resuming the last book instead of Home.
        let mut reader_persistence_preloaded = None;
        if boot_cause == mcu_deep_sleep::BootCause::DeepSleepGpioWake
            && _mounted_sd.is_some()
            && state.reader.deep_sleep_marker_indicates_active()
        {
            let report = state.reader.load_persistent_state();
            if state.reader.request_continue() {
                state.router.navigate_to(ScreenRoute::ReaderLoading);
                info!("rustmix-wave=deep-sleep-restore status=reader-resume-requested");
            } else {
                info!("rustmix-wave=deep-sleep-restore status=no-resumable-book");
            }
            reader_persistence_preloaded = Some(report);
        }

        if !panel_initialized {
            panel.initialize()?;
        }
        // On a real hardware deep-sleep wake, the "RIATTIVAZIONE" overlay
        // drawn above is still on the glass and must stay there until the
        // device can actually respond to input: the button-polling loop
        // below does not start until every subsystem in between (reader
        // persistence, audio codec, networking, SD-backed catalogs) has
        // finished. Removing the overlay/sleep image here, before any of
        // that has run, would show what looks like a ready screen while
        // button presses still go nowhere. Every other boot keeps the prior
        // fast-first-frame behavior: there is no overlay promise to keep.
        if boot_cause != mcu_deep_sleep::BootCause::DeepSleepGpioWake {
            render_current_screen(&mut frame, &state)?;
            panel.show_base(frame.as_bytes())?;
            panel_refresh.reset_after_external_global(PanelGlobalReason::InitialBoot);
            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
            info!(
                "rustmix-wave=panel-refresh plan=global-base reason=initial-boot transport=global-base"
            );
        }
        info!("rustmix-wave=epd397-rust-display-ready");

        // Reader/voice-notes/Lua SD catalog scans and the audio codec
        // bring-up happen only after the first e-paper frame is visible.
        // None of them are needed to draw the Home screen (its menu tiles
        // are a static const list), and on a real deep-sleep wake this is a
        // full reboot, so keeping them off the path to the first frame and
        // the button-polling main loop matters every time the device wakes.
        // Skipped here when the block above already loaded it to decide
        // whether to auto-resume the Reader before that first frame.
        let reader_persistence = match reader_persistence_preloaded {
            Some(report) => report,
            None => state.reader.load_persistent_state(),
        };
        state.reader.refresh_library();
        if _mounted_sd.is_some() {
            match cleanup_stale_voice_tmp(std::path::Path::new(VOICE_NOTES_ROOT)) {
                Ok(removed) => info!(
                    "rustmix-wave=voice-note-stale-tmp-cleanup status=completed removed={removed} root={VOICE_NOTES_ROOT}"
                ),
                Err(error) => warn!(
                    "rustmix-wave=voice-note-stale-tmp-cleanup status=failed root={VOICE_NOTES_ROOT} error={error:#}"
                ),
            }
        }
        if _mounted_sd.is_some() {
            match load_voice_notes_preferences(std::path::Path::new(VOICE_NOTES_ROOT)) {
                Ok(preferences) => {
                    state.voice_notes.mic_gain = preferences.mic_gain;
                    info!(
                        "rustmix-wave=voice-note-settings-load status=completed mic-gain={} path={VOICE_NOTES_ROOT}/SETTINGS.TXT",
                        preferences.mic_gain.marker()
                    );
                }
                Err(error) => warn!(
                    "rustmix-wave=voice-note-settings-load status=failed path={VOICE_NOTES_ROOT}/SETTINGS.TXT error={error:#}"
                ),
            }
        }
        refresh_voice_note_storage_available(&mut state, _mounted_sd.is_some());
        state.refresh_voice_notes_catalog();
        state.refresh_lua_app_catalog(_mounted_sd.is_some());
        log_lua_runtime_events(&mut state);
        info!(
            "rustmix-wave=reader-persistence-load state-loaded={} preferences-loaded={} positions={} recent={} bookmarks={} warning={}",
            reader_persistence.state_loaded,
            reader_persistence.preferences_loaded,
            reader_persistence.position_count,
            reader_persistence.recent_count,
            reader_persistence.bookmark_count,
            reader_persistence.warning.as_deref().unwrap_or("none")
        );

        // Bidirectional ES8311 Voice Notes milestone. The uploaded BSP uses I2S0 with
        // MCLK GPIO13, BCLK GPIO14, WS GPIO47, ESP-to-codec DOUT GPIO48,
        // codec-to-ESP DIN GPIO21 and amplifier GPIO39. Start muted with the
        // amplifier disabled; audio failure remains non-fatal.
        //
        // ALDO2 (Audio_VCC) feeds the codec AVDD pin and the onboard digital
        // microphone and must be enabled before the codec is probed over I2C.
        // PVDD/DVDD stay powered from the always-on VCC3V3 rail regardless.
        if let Err(error) = misc_power.enable_audio_rail() {
            warn!("rustmix-wave=pmic-audio-rail status=enable-failed error={error:#}");
        }
        info!("rustmix-wave=audio-init status=starting codec=es8311 address=0x18 wire-write=0x30");
        let audio_attempt = (|| -> Result<_> {
            let i2s_config = StdConfig::new(
                I2sChannelConfig::new().auto_clear(true),
                StdClkConfig::new(
                    AUDIO_SAMPLE_RATE_HZ,
                    ClockSource::default(),
                    MclkMultiple::M384,
                ),
                StdSlotConfig::philips_slot_default(DataBitWidth::Bits16, SlotMode::Stereo),
                StdGpioConfig::default(),
            );
            let mut i2s = I2sDriver::<I2sBiDir>::new_std_bidir(
                peripherals.i2s0,
                &i2s_config,
                peripherals.pins.gpio14,
                peripherals.pins.gpio21,
                peripherals.pins.gpio48,
                Some(peripherals.pins.gpio13),
                peripherals.pins.gpio47,
            )?;
            i2s.tx_enable()?;
            i2s.rx_enable()?;
            let amplifier = PinDriver::output(peripherals.pins.gpio39)?;
            AudioRuntime::initialize(shared_i2c.clone(), i2s, amplifier, &mut FreeRtosDelay)
        })();
        let (mut audio_runtime, initial_audio_snapshot) = match audio_attempt {
            Ok(runtime) => {
                let snapshot = runtime.snapshot();
                info!(
                    "rustmix-wave=audio-codec status=ready codec=es8311 address={} wire-write={} mclk-hz={AUDIO_MCLK_HZ}",
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
                info!("rustmix-wave=audio-i2s status=ready direction=bidir sample-rate={AUDIO_SAMPLE_RATE_HZ} bits=16 tx-channels=2 rx-channels=2 voice-wav-channels=1 mclk-gpio=13 bclk-gpio=14 ws-gpio=47 dout-gpio=48 din-gpio=21");
                info!("rustmix-wave=audio-amp status=ready gpio=39 default=off");
                info!("rustmix-wave=audio-subsystem-ready mute=true volume={DEFAULT_AUDIO_VOLUME_PERCENT}");
                (Some(runtime), snapshot)
            }
            Err(error) => {
                warn!("rustmix-wave=audio-init status=unavailable codec=es8311 error={error:#}");
                (None, AudioSnapshot::unavailable(format!("{error:#}")))
            }
        };
        state.update_audio_snapshot(initial_audio_snapshot);
        log_audio_snapshot(&state.audio);

        // Start optional networking only after the first e-paper frame is
        // visible. A missing config or failed association never blocks shell
        // startup. Keep the runtime alive so Wi-Fi and SNTP remain active.
        //
        // Rustmix Remote BLE r1 is deliberately feature-gated and default-off.
        // In the r1 feature build, BLE owns the ESP32-S3 modem so Wi-Fi startup
        // is skipped. This keeps the accepted default firmware path unchanged
        // while validating the BLE GATT command path first.
        #[cfg(feature = "rustmix-remote-ble")]
        let (mut network_runtime, _rustmix_remote_ble) = {
            let ble_service = (|| -> Result<RustmixRemoteBleGattService> {
                let nvs = EspDefaultNvsPartition::take()?;
                let bt = std::sync::Arc::new(BtDriver::new(peripherals.modem, Some(nvs.clone()))?);
                RustmixRemoteBleGattService::start(bt, rustmix_remote_queue.clone())
            })();
            let ble_service = match ble_service {
                Ok(service) => {
                    info!(
                        "rustmix-wave=rustmix-remote-ble status=ready feature=rustmix-remote-ble"
                    );
                    Some(service)
                }
                Err(error) => {
                    warn!("rustmix-wave=rustmix-remote-ble status=unavailable feature=rustmix-remote-ble error={error:#}");
                    None
                }
            };
            let runtime = if let Some(config) = network_config.as_ref() {
                warn!(
                    "rustmix-wave=wifi-connect status=skipped ssid={} reason=rustmix-remote-ble-r1-owns-modem",
                    first_saved_ssid(config)
                );
                NetworkRuntime::failed(
                    config,
                    "Rustmix Remote BLE r1 owns the modem; Wi-Fi disabled in this feature build",
                )
            } else {
                NetworkRuntime::configuration_missing()
            };
            (runtime, ble_service)
        };
        #[cfg(not(feature = "rustmix-remote-ble"))]
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
        let mut last_network_log = Instant::now();
        let mut last_network_fingerprint = state.network.log_fingerprint();
        // Explicitly activated only.  Normal boot never starts the portal.
        let mut wifi_transfer_server: Option<WifiTransferServer> = None;
        state.update_wifi_transfer_snapshot(WifiTransferSnapshot::default());
        // Explicitly activated only, from Network > Configure via phone.
        let mut network_provision_server: Option<NetworkProvisionServer> = None;
        state.update_network_provision_snapshot(NetworkProvisionSnapshot::default());
        let mut network_provision_join_pending: Option<(String, String)> = None;
        let mut network_provision_last_rescan = Instant::now();
        state.set_saved_networks(saved_network_entries(&network_config, None));
        let mut voice_recording: Option<VoiceRecordingSession> = None;
        let mut voice_playback: Option<VoicePlaybackSession> = None;
        let mut voice_stereo_buffer = vec![0_u8; VOICE_PCM_STEREO_CAPTURE_BYTES];
        let mut voice_mono_buffer = vec![0_u8; VOICE_PCM_MONO_CHUNK_BYTES];
        info!("rustmix-wave=open-meteo-weather-forecast-ready");
        info!("rustmix-wave=open-meteo-fixed-point-json-parser-ready");
        info!("rustmix-wave=open-meteo-whitespace-parser-repair-ready");
        info!("rustmix-wave=rtc-alarm-scheduling-ui-ready");
        info!("rustmix-wave=es8311-audio-diagnostics-audible-alarm-ready");
        info!("rustmix-wave=es8311-pindriver-output-mode-repair-ready");
        info!("rustmix-wave=es8311-i2s-data-route-repair-ready");
        info!("rustmix-wave=es8311-waveshare-codec-profile-repair-ready");
        info!(
            "rustmix-wave=wifi-monitor-log-quieting-ready policy=state-change-or-heartbeat heartbeat-seconds={NETWORK_LOG_HEARTBEAT_SECONDS} rssi-immediate=false"
        );
        info!("rustmix-wave=rtc-alarm-int-readiness-ready gpio={RTC_ALARM_INTERRUPT_GPIO} active-low=true");
        info!("rustmix-wave=power-key-sleep-image-mode-ready path={SLEEP_IMAGE_DIRECTORY} format=native-800x480-1bpp-bmp mcu-sleep=true wake-gpio={} rtc-alarm-wake=disabled", mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO);
        info!("rustmix-wave=power-key-short-menu-long-sleep-ready short-press=display-maintenance-menu long-press=sleep-image wake=power-key menu-action=manual-global-refresh");
        info!("rustmix-wave=release-flash-workflow-safety-ready docs=consolidated workflow=ci release-artifact=elf supported-flash=espflash-flash factory-image=deferred");
        info!("rustmix-wave=text-editor-layout-alignment-ready voice-title-editor=shared-grid-keyboard calendar-editor-status=compact-date keyboard=select-hold-hv-axis footer=width-safe");
        info!("rustmix-wave=sleep-image-directory-classification-fix-ready policy=fat-metadata-fallback");
        info!("rustmix-wave=network-suspended-sleep-image-mode-ready wifi=stop-on-sleep sntp=paused weather=paused mcu-sleep=true");
        info!("rustmix-wave=random-sleep-image-selection-ready source=esp-random policy=avoid-immediate-repeat-when-multiple");
        info!("rustmix-wave=main-category-navigation-ready categories=5");
        info!("rustmix-wave=reader-category-ready entries=3");
        info!("rustmix-wave=productivity-category-ready entries=2");
        info!("rustmix-wave=games-category-ready entries=1 status=sd-lua-catalog");
        info!("rustmix-wave=tools-category-ready entries=3");
        info!("rustmix-wave=settings-category-ready entries=9 display=true");
        info!("rustmix-wave=display-settings-ready default-family=inter alternate-family=atkinson-hyperlegible default-size=standard profiles=compact,standard,large persistence={DISPLAY_CONFIG_PATH} scope=all-user-facing-screens");
        info!("rustmix-wave=global-ui-typography-ready default-family=inter alternate-family=atkinson-hyperlegible default-size=standard profiles=compact,standard,large persistence={DISPLAY_CONFIG_PATH} scope=all-user-facing-screens");
        info!("rustmix-wave=boot-button-hierarchical-back-ready gpio=0 active-low=true press=short policy=short-press-back");
        info!("rustmix-wave=category-back-row-removal-ready policy=boot-press");
        info!("rustmix-wave=global-typography-scale-increase-ready shift=two-raster-steps settings-page-size=6 display-copy=compact default-family=inter default-size=standard");
        info!("rustmix-wave=secondary-screen-readability-reflow-ready detail-role=technical-tokens-only pagination=device-info-3-pages details=weather,audio,rtc,environment,motion,network synthetic-back-rows=removed");
        info!("rustmix-wave=weather-fetch-resilience-ready retries=3 backoff-seconds=2,5,15 cache=last-known-good-in-memory retryable=tls-eof,http-connect,timeout,http-429,http-500,http-502,http-503,http-504");
        info!("rustmix-wave=home-dashboard-redesign-ready header=simplified-dark date-time-row=true summary-strip=weather,battery,wifi cards=high-contrast footer=fixed categories=5 developer-notes=removed");
        info!("rustmix-wave=calendar-foundation-ready mode=read-only monthly-view=true selected-day-summary=true range=2000-2099");
        info!(
            "rustmix-wave=calendar-local-date-ready timezone=regional-profile source=rtc-localized"
        );
        info!("rustmix-wave=calendar-navigation-ready modes=day,month select=toggle-mode select-hold=agenda back=boot-press");
        info!("rustmix-wave=calendar-us-events-daily-agenda-ready root={CALENDAR_ROOT} personal={CALENDAR_EVENTS_FILE} us={CALENDAR_US_EVENTS_FILE} hindu=excluded markers=month-grid agenda=scrollable details=personal-editor missing-files=safe alarms=separate");
        info!("rustmix-wave=calendar-personal-event-editor-ready writable=EVENTS.TXT temp=EVENTS.TMP backup=EVENTS.BAK operations=create,edit,delete us-holidays=read-only keyboard=select-hold-hv-axis alarms=separate");
        info!("rustmix-wave=power-key-sleep-entry-wake-guard-ready source=axp2101-pek minimum-quiet-ms={POWER_KEY_WAKE_GUARD_QUIET_MS} policy=suppress-stale-until-quiet-window");
        info!("rustmix-wave=unit-converter-foundation-ready categories=length,mass,temperature,volume mode=offline fixed-point=true precision=thousandths");
        info!("rustmix-wave=unit-converter-navigation-ready fields=category,from-unit,value,to-unit,step-size back=boot-press");
        info!("rustmix-wave=unit-converter-host-tests-ready coverage=length,mass,temperature,volume,bounds");
        info!("rustmix-wave=reader-library-txt-foundation-ready path=/sdcard/RUSTMIX/BOOKS formats=txt,epub encoding=utf8,bom,windows-1252 opening=staged-first-page-first cache=ram-nearby-pages");
        info!("rustmix-wave=reader-state-persistence-ready path=/sdcard/RUSTMIX/READER files=STATE.TXT,POSITS.TXT,RECENT.TXT,MARKS.TXT cache=CACHE atomic-replace=tmp-primary-backup fallback=corrupt-record-safe");
        info!("rustmix-wave=reader-bookmarks-ready add-remove=true list=true recent=true continue-reading=true cache-fingerprint=path,size,modified,format,layout");
        info!("rustmix-wave=reader-loading-ui-ready stages=open,encoding,resume,first-page,cache cancel=boot-press refresh=coarse-stage-boundaries");
        info!("rustmix-wave=reader-options-shell-ready toc=none-for-txt,list-for-epub bookmarks=persistent clear-ghosting=manual-global-refresh");
        info!("rustmix-wave=reader-ux-repair-ready menu=continue,library,bookmarks-ready normalization=utf8-punctuation,latin1,underscore-emphasis byte-offsets=preserved");
        info!("rustmix-wave=reader-preferences-ready path=/sdcard/RUSTMIX/READER/PREFS.TXT theme=classic,high-contrast orientation=portrait,landscape font-size=small,medium,large,xlarge book-font=inter,atkinson-hyperlegible,serif,literata paragraph-alignment=justified,left,center,right show-progress=on,off atomic-replace=tmp-primary-backup");
        info!("rustmix-wave=reader-high-contrast-layout-ready viewport=shared border=outside-text top-padding=true clip=right,bottom theme-change=redraw-only ghost-refresh=global-base");
        info!("rustmix-wave=reader-txt-emphasis-cleanup-ready multiline-gutenberg=true word-internal-underscores=preserved repeated-separators=preserved byte-offsets=preserved");
        info!("rustmix-wave=reader-per-book-resume-ready path=/sdcard/RUSTMIX/READER/POSITS.TXT records=64 fingerprint=path,size,modified,format atomic-replace=tmp-primary-backup routes=continue,books,files,bookmark");
        info!("rustmix-wave=reader-controls-alignment-ready navigation=up-down-move-select-activate preferences=up-down-move-select-change back=boot-press");
        info!("rustmix-wave=reader-options-split-ready actions=bookmark,toc,preferences,clear-ghosting,library,home editor=theme,orientation,font-size,font,paragraph-alignment,show-progress");
        info!("rustmix-wave=reader-preferences-settings-navigation-ready move=up-down change=select back=boot-press persistence=immediate rows=theme,orientation,font-size,font,paragraph-alignment,show-progress");
        info!("rustmix-wave=reader-fat83-persistence-ready positions=POSITS.TXT legacy-read=POSITIONS.TXT cache-basename=8hex extensions=CCH,TMP,BAK atomic-replace=true");
        info!("rustmix-wave=reader-fat83-runtime-ready positions-write=POSITS.TXT legacy-read=POSITIONS.TXT cache-write=8hex-no-prefix extensions=CCH,TMP,BAK duplicate-degraded-log=suppressed");
        info!("rustmix-wave=reader-bookmark-page-labels-ready anchor=byte-offset display=page-number layout-aware=true fallback=stored-page");
        info!("rustmix-wave=library-bookmark-tab-rendering-ready status=saved-marks source=MARKS.TXT rows=title,page-number anchors=byte-offset page-label=layout-aware-fallback-stored books-files=txt-open preserved=true");
        info!("rustmix-wave=reader-epub-reflowable-foundation-ready archive=zip-central-directory compression=stored,deflate package=container-xml,opf spine=xhtml reflow=bounded-utf8 cache=ram-nearby-pages");
        info!("rustmix-wave=reader-epub-toc-ready sources=epub3-nav,epub2-ncx,fallback-spine route=reader-toc selection=byte-offset");
        info!("rustmix-wave=reader-epub-parser-stack-isolation-ready worker=epub-parser stack-bytes=65536 main-task-stack-bytes=16384 policy=short-lived-worker-join");
        info!("rustmix-wave=reader-epub-chapter-aware-presentation-ready page-label=chapter,page-of-total bookmarks=chapter,page-of-total library-title=opf-metadata fallback=fat-filename txt-path=preserved");
        info!("rustmix-wave=reader-epub-watchdog-memory-pressure-repair-ready index-yield-every-pages=4 index-yield-ms=1 session-release=before-book-open layout-rebuild=move-document toc-jump=no-document-clone parser-worker-stack-bytes=65536 title-worker-stack-bytes=32768");
        info!("rustmix-wave=reader-eink-font-pack-ready fonts=inter,atkinson-hyperlegible,serif,literata atkinson-source=atkinson-hyperlegible-next-medium literata-source=literata-medium glyphs=printable-ascii persisted-keys=serif,atkinson-hyperlegible cache-fingerprint=book-font epub-repagination=layout-rebuild bookmarks=byte-offset txt-epub-aligned=true");
        info!("rustmix-wave=lua-runtime-foundation-ready mode=bootstrap-static,event-bridge root={LUA_APPS_DIRECTORY} manifest=APP.TOM entry=MAIN.LUA script-max-bytes=65536 vm-callbacks=sudoku,minesweeper,tilt-maze,motion-2048,sokoban-tilt-bounded-native");
        info!("rustmix-wave=lua-native-dirty-region-canvas-ready commands=256 text-bytes=160 dirty-regions={MAX_DIRTY_REGIONS} partial-limit={PANEL_PARTIAL_REFRESH_LIMIT} transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=panel-refresh-coordinator-ready partial-limit={PANEL_PARTIAL_REFRESH_LIMIT} transport=existing-fullscreen-partial state=main-loop-owned lua-route-global-refresh=false");
        info!("rustmix-wave=runtime-worker-boundary-ready workers=weather-fetch,lua-loader policy=short-lived-named-stack panel-spi=main-task-only");
        info!("rustmix-wave=lua-loader-stack-isolation-ready worker=lua-loader stack-bytes={LUA_LOADER_WORKER_STACK_BYTES} main-task-stack-bytes=16384 policy=short-lived-worker-join");
        info!("rustmix-wave=lua-sudoku-event-bridge-ready sample=SUDOKU input=up,down,select,select-hold-context board=native dirty=old-cell,new-cell,status refresh=shared-panel-coordinator transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=lua-sudoku-boot-axis-navigation-ready long-press=select nav=axis-toggle edit=cancel default-axis=horizontal back=boot-press dirty=status-or-cell refresh=shared-panel-coordinator");
        info!("rustmix-wave=lua-sudoku-boot-mode-ux-repair-ready nav=select-hold-axis-toggle edit=select-hold-cancel back=boot-press dirty=axis-status-or-edit-cell-status refresh=shared-panel-coordinator");
        info!("rustmix-wave=lua-minesweeper-event-bridge-ready sample=MINES board=beginner-9x9 mines=10 first-reveal=safe input=up,down,select,select-hold-context action=reveal,flag dirty=old-cell,new-cell,status-or-board refresh=shared-panel-coordinator transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=imu-event-bridge-ready events=tilt,shake,rotate,level sampling=motion-events-or-motion-game sample-ms={IMU_EVENT_SAMPLE_INTERVAL_MS} diagnostics=thresholds,debounce,counters redraw=event-or-{IMU_EVENT_SCREEN_REFRESH_SECONDS}s-heartbeat raw-i2c=rust-owned lua-api=none");
        info!("rustmix-wave=imu-event-thresholds tilt-mg={} shake-delta-mg={} rotate-dps={} level-tolerance-mg={} debounce-ms={}", state.imu_events.thresholds.tilt_enter_mg, state.imu_events.thresholds.shake_delta_mg, state.imu_events.thresholds.rotate_dps, state.imu_events.thresholds.level_tolerance_mg, state.imu_events.thresholds.debounce_ms);
        info!("rustmix-wave=imu-event-discrete-latching-ready tilt=release-to-neutral rotate=release-to-neutral level=edge-only shake=cooldown raw-i2c=rust-owned");
        info!("rustmix-wave=lua-tilt-maze-event-bridge-ready sample=TILTMAZE board=9x9 motion=debounced-tilt-only dirty=old-cell,new-cell,status-or-board refresh=shared-panel-coordinator transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=lua-tilt-maze-portrait-axis-repair-ready logical=portrait mapping=raw:+x->down,-x->up,+y->left,-y->right diagnostics=logical-direction,raw-axis");
        info!("rustmix-wave=lua-motion-2048-event-bridge-ready sample=M2048 board=4x4 motion=debounced-tilt-swipe dirty=board,status refresh=shared-panel-coordinator transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=lua-sokoban-tilt-event-bridge-ready sample=SOKOBAN board=9x9 motion=debounced-tilt-only dirty=old-cell,new-cell,status-or-board refresh=shared-panel-coordinator transport=existing-fullscreen-partial panel-api=rust-owned");
        info!("rustmix-wave=weather-fetch-stack-isolation-ready worker=weather-fetch stack-bytes=65536 main-task-stack-bytes=16384 response-max-bytes=8192 state=heap-boxed policy=short-lived-worker-join");
        info!("rustmix-wave=wifi-transfer-web-portal-ready activation=settings-network-explicit-toggle auto-start=false root={WIFI_TRANSFER_ROOT} transport=http-lan-only token=required server-stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES} main-task-stack-bytes=16384 upload=streamed-atomic-tmp fat83=true protected-config=true inactivity-seconds={WIFI_TRANSFER_INACTIVITY_SECONDS}");
        log_runtime_memory("boot-complete");
        info!("rustmix-wave=hierarchical-router-ready policy=category-subcategory-feature-details");
        info!("rustmix-wave=wifi-transfer-lifecycle-ready state=off-until-settings-network-toggle server=temporary-http-task sd-root=/sdcard/RUSTMIX stop=switch-off,back,sleep,wifi-loss,inactivity");
        info!("rustmix-wave=wifi-transfer-immediate-start-redraw-repair-ready dispatch=ordinary-button-event-before-refresh snapshot=ready-url-code refresh=single-normal-partial");
        info!("rustmix-wave=voice-notes-foundation-ready root={VOICE_NOTES_ROOT} format=wav-pcm16-mono-16khz storage=streamed-tmp-rename capture=cooperative-bounded-i2s-rx chunk-bytes={VOICE_PCM_MONO_CHUNK_BYTES} main-task-stack-bytes=16384 audio-owner=native");
        info!("rustmix-wave=voice-notes-microphone-gain-ready profiles=low,normal,high,boost default=high multipliers=1x,2x,3x,4x clipping=per-recording-saturated-sample-count wav-format=unchanged");
        info!("rustmix-wave=voice-notes-fat-metadata-catalog-repair-ready policy=stat-metadata-final-classification overwrite=refuse-existing-target");
        info!("rustmix-wave=voice-notes-catalog-scrolling-saved-wav-playback-ready visible-rows=6 format=wav-pcm16-mono-16khz playback=bounded-sd-stream mono-to-stereo=true volume=existing-codec-setting audio-owner=native stale-tmp-cleanup=boot alarms=interrupt");
        info!("rustmix-wave=voice-notes-organizer-controls-export-ready gain-persistence=SETTINGS.TXT metadata=META.TXT titles=friendly-sidecar filenames=fat83-wav recording-date-time=rtc-local storage=esp-vfs-fat-info delete-confirmation=true pause-resume=rx-discard export=wifi-transfer-shortcut");
        info!("rustmix-wave=offline-dictionary-x4-pack-native-foundation-ready root={DICTIONARY_ROOT} index=INDEX.TXT shards=DATA/*.JSN shard-max-bytes={DICTIONARY_SHARD_MAX_BYTES} lookup=exact-prefix-fallback wildcard=true ui=native-rust");
        info!("rustmix-wave=dictionary-keyboard-boot-axis-navigation-ready short-press=boot toggle=horizontal,vertical default-axis=horizontal selected-key=preserved long-press=hierarchical-back helper=keyboard-grid-navigation");
        info!(
            "rustmix-wave=voice-notes-catalog status=completed notes={} root={VOICE_NOTES_ROOT}",
            state.voice_notes.notes.len()
        );

        // Everything the button-polling loop below depends on is now up:
        // this is the single global refresh that replaces the retained
        // sleep image and "RIATTIVAZIONE" overlay with the real screen (see
        // the deferral above). Nothing in RAM survived the reboot, so this
        // is also the first render of the actual route for this boot.
        if boot_cause == mcu_deep_sleep::BootCause::DeepSleepGpioWake {
            render_current_screen(&mut frame, &state)?;
            panel.show_base(frame.as_bytes())?;
            panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
            info!(
                "rustmix-wave=panel-refresh plan=global-base reason=after-wake transport=global-base"
            );
            info!("rustmix-wave=wake-global-refresh reason=deep-sleep-gpio-wake-boot-complete");
        }

        let mut last_activity = Instant::now();
        let mut last_status_refresh = Instant::now();
        let mut last_alarm_poll = Instant::now();
        let mut last_power_key_poll = Instant::now();
        let mut last_weather_attempt: Option<Instant> = None;
        let mut last_reader_tick = Instant::now();
        let imu_event_started_at = Instant::now();
        let mut last_imu_event_sample = Instant::now();
        let mut last_imu_event_screen_refresh = Instant::now();
        let mut weather_retry = WeatherRetryState::default();
        let mut last_voice_record_refresh = Instant::now();
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
        loop {
            maintain_wifi_transfer_server(
                &mut wifi_transfer_server,
                &mut state,
                &mut storage_browser,
                _mounted_sd.is_some(),
            );
            if state.panel_awake
                && last_activity.elapsed() >= Duration::from_secs(PANEL_IDLE_SLEEP_SECONDS)
            {
                panel.sleep()?;
                state.panel_awake = false;
                info!("rustmix-wave=epd397-panel-sleep");
            }

            let mut voice_capture_failure = None;
            if let Some(session) = voice_recording.as_mut() {
                if state.voice_notes.recording_paused {
                    let discard = audio_runtime
                        .as_mut()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "audio runtime unavailable during paused voice recording"
                            )
                        })
                        .and_then(|runtime| runtime.discard_voice_pcm(&mut voice_stereo_buffer));
                    if let Err(error) = discard {
                        voice_capture_failure = Some(format!("{error:#}"));
                    }
                } else {
                    let capture = audio_runtime
                        .as_mut()
                        .ok_or_else(|| {
                            anyhow::anyhow!("audio runtime unavailable during voice recording")
                        })
                        .and_then(|runtime| {
                            runtime.read_voice_pcm_mono(
                                &mut voice_stereo_buffer,
                                &mut voice_mono_buffer,
                                state.voice_notes.mic_gain,
                            )
                        });
                    match capture {
                        Ok(metrics) if metrics.bytes > 0 => {
                            session.add_clipped_samples(metrics.clipped_samples);
                            if let Err(error) =
                                session.append_pcm16_mono(&voice_mono_buffer[..metrics.bytes])
                            {
                                voice_capture_failure = Some(format!("{error:#}"));
                            } else {
                                state.voice_notes.update_recording_progress(
                                    session.pcm_bytes(),
                                    session.peak(),
                                    session.clipped_samples(),
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(error) => voice_capture_failure = Some(format!("{error:#}")),
                    }
                }
                if voice_capture_failure.is_none()
                    && state.panel_awake
                    && state.active_route() == ScreenRoute::VoiceNoteRecording
                    && last_voice_record_refresh.elapsed()
                        >= Duration::from_secs(VOICE_RECORD_SCREEN_REFRESH_SECONDS)
                {
                    if state.voice_notes.recording_paused {
                        info!("rustmix-wave=voice-record status=paused file={} elapsed-seconds={} pcm-bytes={} peak={} clipped-samples={} mic-gain={}", session.file_name(), state.voice_notes.elapsed_seconds, session.pcm_bytes(), session.peak(), session.clipped_samples(), state.voice_notes.mic_gain.marker());
                    } else {
                        info!("rustmix-wave=voice-record status=active file={} elapsed-seconds={} pcm-bytes={} peak={} clipped-samples={} mic-gain={}", session.file_name(), state.voice_notes.elapsed_seconds, session.pcm_bytes(), session.peak(), session.clipped_samples(), state.voice_notes.mic_gain.marker());
                    }
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                    last_voice_record_refresh = Instant::now();
                }
            }
            if let Some(error) = voice_capture_failure {
                warn!("rustmix-wave=voice-record status=failed stage=capture error={error}");
                if let Some(active) = voice_recording.take() {
                    let _ = active.cancel();
                }
                if let Some(runtime) = audio_runtime.as_mut() {
                    let _ = runtime.finish_voice_recording();
                    state.update_audio_snapshot(runtime.snapshot());
                }
                state.voice_notes.fail(error);
                log_runtime_memory("after-voice-record-stop");
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
                // stack — see `CoverCache::generate_thumbnail`).
                if let Some((book, thumbnail)) = cover_cache.pump_pending(&visible_books) {
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

            let mut voice_playback_finished = None;
            let mut voice_playback_failure = None;
            if voice_recording.is_none() {
                if let Some(session) = voice_playback.as_mut() {
                    match session.read_pcm16_mono(&mut voice_mono_buffer) {
                        Ok(0) => voice_playback_finished = Some(session.file_name().to_string()),
                        Ok(bytes) => {
                            let output = audio_runtime
                                .as_mut()
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "audio runtime unavailable during voice-note playback"
                                    )
                                })
                                .and_then(|runtime| {
                                    runtime.write_voice_pcm16_mono(
                                        &voice_mono_buffer[..bytes],
                                        &mut voice_stereo_buffer,
                                    )
                                });
                            match output {
                                Ok(()) => {
                                    state.voice_notes.update_playback_progress(
                                        session.played_pcm_bytes(),
                                        session.total_pcm_bytes(),
                                    );
                                    if session.is_complete() {
                                        voice_playback_finished =
                                            Some(session.file_name().to_string());
                                    }
                                }
                                Err(error) => {
                                    voice_playback_failure = Some(format!("{error:#}"));
                                }
                            }
                        }
                        Err(error) => voice_playback_failure = Some(format!("{error:#}")),
                    }
                }
            }
            if let Some(file_name) = voice_playback_finished {
                stop_voice_note_playback(
                    &mut voice_playback,
                    &mut audio_runtime,
                    &mut state,
                    "completed",
                );
                info!("rustmix-wave=voice-note-playback status=completed file={file_name}");
                if state.panel_awake && state.active_route() == ScreenRoute::VoiceNoteDetails {
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                }
            }
            if let Some(error) = voice_playback_failure {
                warn!("rustmix-wave=voice-note-playback status=failed error={error}");
                stop_voice_note_playback(
                    &mut voice_playback,
                    &mut audio_runtime,
                    &mut state,
                    "stream-error",
                );
                state.voice_notes.fail(format!("Playback failed: {error}"));
                if state.panel_awake && state.active_route() == ScreenRoute::VoiceNoteDetails {
                    refresh_screen(
                        &mut panel,
                        &mut frame,
                        &mut state,
                        &mut panel_refresh,
                        RefreshRequest::Normal,
                    )?;
                }
            }

            if voice_recording.is_none() && voice_playback.is_none() {
                if let Some(runtime) = audio_runtime.as_mut() {
                    match runtime.tick() {
                        Ok(changed) => {
                            let latest = runtime.snapshot();
                            if latest != state.audio {
                                state.update_audio_snapshot(latest);
                                log_audio_snapshot(&state.audio);
                                if changed
                                    && state.panel_awake
                                    && matches!(
                                        state.active_route(),
                                        ScreenRoute::Audio
                                            | ScreenRoute::AudioDetails
                                            | ScreenRoute::Alarms
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
                        Err(error) => {
                            warn!(
                                "rustmix-wave=audio-event outcome=playback-error error={error:#}"
                            );
                            runtime.record_failure(format!("{error:#}"));
                            state.update_audio_snapshot(runtime.snapshot());
                            log_audio_snapshot(&state.audio);
                        }
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
                    state.update_board_snapshot(board_services.read_snapshot(&mut service_delay));
                    log_board_snapshot(state.board, state.regional);
                    if let Some(rtc) = state.board.rtc {
                        alarm_engine.recompute_next(state.regional.localize_rtc(rtc));
                        sync_alarm_hardware(&mut alarm_engine, &mut board_services, state.regional);
                        state.update_alarm_snapshot(alarm_engine.snapshot());
                        log_alarm_snapshot(&state.alarms);
                    }
                }
                let latest_network = network_runtime.snapshot();
                if latest_network != state.network {
                    state.update_network_snapshot(latest_network);
                }
                if let Some(scan_result) = network_runtime.poll_scan() {
                    if let Some(server) = network_provision_server.as_ref() {
                        server.set_scan_results(scan_result.unwrap_or_default());
                    }
                }
                let provision_snapshot_before = state.network_provision.clone();
                maintain_network_provision(
                    &mut network_runtime,
                    &mut network_provision_server,
                    &mut network_config,
                    &mut state,
                    &mut network_provision_join_pending,
                    &mut network_provision_last_rescan,
                );
                if state.panel_awake
                    && state.network_provision != provision_snapshot_before
                    && matches!(
                        state.active_route(),
                        ScreenRoute::NetworkProvision | ScreenRoute::Network
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
                let latest_fingerprint = state.network.log_fingerprint();
                if latest_fingerprint != last_network_fingerprint
                    || last_network_log.elapsed()
                        >= Duration::from_secs(NETWORK_LOG_HEARTBEAT_SECONDS)
                {
                    log_network_snapshot(&state.network);
                    last_network_fingerprint = latest_fingerprint;
                    last_network_log = Instant::now();
                }
            }

            if alarm_engine.should_poll()
                && last_alarm_poll.elapsed() >= Duration::from_secs(ALARM_POLL_SECONDS)
            {
                match board_services.read_rtc() {
                    Ok(rtc) => {
                        let local = state.regional.localize_rtc(rtc);
                        let interrupt_sample = rtc_alarm_interrupt.sample();
                        if interrupt_sample.changed {
                            info!(
                                "rustmix-wave=rtc-alarm-int status={} gpio={RTC_ALARM_INTERRUPT_GPIO} level={}",
                                interrupt_sample.level.marker(),
                                interrupt_sample.level.raw_level_marker()
                            );
                        }
                        let hardware_flag = match board_services.take_rtc_alarm_flag() {
                            Ok(flag) => flag,
                            Err(error) => {
                                warn!("rustmix-wave=rtc-alarm-flag status=unavailable error={error:#}");
                                false
                            }
                        };
                        let outcome =
                            alarm_engine.poll(local, hardware_flag || interrupt_sample.asserted());
                        if outcome.schedule_changed {
                            sync_alarm_hardware(
                                &mut alarm_engine,
                                &mut board_services,
                                state.regional,
                            );
                        }
                        state.update_alarm_snapshot(alarm_engine.snapshot());
                        if outcome.triggered {
                            if let Some(active) = voice_recording.take() {
                                let _ = active.cancel();
                                if let Some(runtime) = audio_runtime.as_mut() {
                                    let _ = runtime.finish_voice_recording();
                                    state.update_audio_snapshot(runtime.snapshot());
                                }
                                state.voice_notes.cancel_recording();
                                info!("rustmix-wave=voice-record status=cancelled reason=alarm-trigger");
                            }
                            if voice_playback.is_some() {
                                stop_voice_note_playback(
                                    &mut voice_playback,
                                    &mut audio_runtime,
                                    &mut state,
                                    "alarm-trigger",
                                );
                            }
                            info!(
                                "rustmix-wave=alarm-triggered active={} local={} hardware-flag={hardware_flag} interrupt-low={}",
                                state.alarms.active.as_ref().map_or("alarm", |active| active.name.as_str()),
                                local.date_time(),
                                interrupt_sample.asserted()
                            );
                            if let Some(runtime) = audio_runtime.as_mut() {
                                match runtime.start_alarm_chime() {
                                    Ok(()) => {
                                        info!("rustmix-wave=audio-event outcome=alarm-chime-start")
                                    }
                                    Err(error) => {
                                        warn!("rustmix-wave=audio-event outcome=alarm-chime-failed error={error:#}");
                                        runtime.record_failure(format!("{error:#}"));
                                    }
                                }
                                state.update_audio_snapshot(runtime.snapshot());
                                log_audio_snapshot(&state.audio);
                            } else {
                                warn!("rustmix-wave=audio-event outcome=alarm-chime-unavailable fallback=visual-only");
                            }
                            let woke_from_sleep = !state.panel_awake;
                            if sleep_mode.is_sleeping() {
                                let _ = sleep_mode.exit(SleepWakeCause::RtcAlarm);
                                sleep_wake_guard.reset_after_wake();
                                sleep_wake_guard_started_at = None;
                                info!("rustmix-wave=sleep-mode-exit cause=rtc-alarm restore-route=alarms");
                            }
                            if woke_from_sleep {
                                panel.initialize()?;
                                state.panel_awake = true;
                                panel_refresh
                                    .reset_after_external_global(PanelGlobalReason::AfterWake);
                                sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                                if let Err(error) = board_services.wake_imu() {
                                    warn!(
                                        "rustmix-wave=imu-suspend status=resume-failed error={error:#}"
                                    );
                                }
                            }
                            state.router.navigate_to(ScreenRoute::Alarms);
                            info!("rustmix-wave=screen-route route=alarms cause=alarm-trigger");
                            if woke_from_sleep {
                                info!(
                                    "rustmix-wave=wake-global-refresh reason=rtc-alarm-sleep-image"
                                );
                            }
                            refresh_screen(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                if woke_from_sleep {
                                    RefreshRequest::ForceGlobalAfterWake
                                } else {
                                    RefreshRequest::Normal
                                },
                            )?;
                            if sleep_network.is_suspended() {
                                resume_network_after_sleep(
                                    &mut network_runtime,
                                    network_config.as_ref(),
                                    &mut state,
                                    &mut sleep_network,
                                    &mut last_network_fingerprint,
                                    &mut last_network_log,
                                    &mut last_weather_attempt,
                                    &mut weather_retry,
                                );
                            }
                            last_activity = Instant::now();
                            last_status_refresh = Instant::now();
                        }
                    }
                    Err(error) => {
                        warn!("rustmix-wave=rtc-alarm-poll status=unavailable error={error:#}")
                    }
                }
                last_alarm_poll = Instant::now();
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
                            if let Err(error) = board_services.wake_imu() {
                                warn!(
                                    "rustmix-wave=imu-suspend status=resume-failed error={error:#}"
                                );
                            }
                            state.router.navigate_to(restore_route);
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
                                resume_network_after_sleep(
                                    &mut network_runtime,
                                    network_config.as_ref(),
                                    &mut state,
                                    &mut sleep_network,
                                    &mut last_network_fingerprint,
                                    &mut last_network_log,
                                    &mut last_weather_attempt,
                                    &mut weather_retry,
                                );
                            }
                            last_activity = Instant::now();
                            last_status_refresh = Instant::now();
                        } else if event == PowerKeyEvent::ShortPress {
                            if state.alarms.active.is_some() {
                                warn!(
                                    "rustmix-wave=power-key-menu outcome=rejected reason=active-alarm"
                                );
                            } else {
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
                            }
                        } else if state.alarms.active.is_some() {
                            warn!(
                                "rustmix-wave=sleep-mode-enter status=rejected reason=active-alarm"
                            );
                        } else {
                            // Draw the sleep-confirmation image first, before any
                            // of the slower teardown below (Wi-Fi transfer
                            // server, voice/audio cleanup, network suspend). The
                            // user long-pressed power to get immediate visual
                            // confirmation the command was received; making them
                            // wait through network suspend first defeats that.
                            let selection =
                                sleep_images.select_random(unsafe { sys::esp_random() });
                            log_sleep_image_selection(&selection);
                            if !state.panel_awake {
                                panel.initialize()?;
                                state.panel_awake = true;
                            }
                            let restore_route = state.power_key_sleep_restore_route();
                            frame = selection.frame;
                            panel.show_base(frame.as_bytes())?;
                            panel_refresh
                                .reset_after_external_global(PanelGlobalReason::SleepImage);
                            sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                            info!("rustmix-wave=panel-refresh plan=global-base reason=sleep-image transport=global-base");
                            sleep_mode.enter(restore_route, selection.file_name.clone());
                            // Real deep sleep is a full reboot, so nothing in
                            // RAM survives a SELECT-key wake. This marker
                            // lets the fresh boot reload the same frame and
                            // draw the "RIATTIVAZIONE" wake overlay on top of
                            // it. Best-effort, like the teardown steps below:
                            // a failed write only means the wake overlay
                            // falls back to the built-in sleep frame.
                            match sleep_images.record_wake_marker(&selection.file_name) {
                                Ok(()) => info!(
                                    "rustmix-wave=sleep-wake-marker status=recorded file={}",
                                    selection.file_name
                                ),
                                Err(error) => warn!(
                                    "rustmix-wave=sleep-wake-marker status=failed file={} error={error:#}",
                                    selection.file_name
                                ),
                            }
                            // Same rationale as the sleep-image wake marker
                            // above: record whether the device was actively
                            // reading a book so a real hardware deep-sleep
                            // wake (a full reboot) can auto-resume it instead
                            // of always landing back on Home.
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
                            sleep_wake_guard.begin_sleep_entry();
                            sleep_wake_guard_started_at = Some(Instant::now());
                            info!(
                                "rustmix-wave=sleep-wake-guard status=waiting-for-quiet-window minimum-quiet-ms={POWER_KEY_WAKE_GUARD_QUIET_MS} policy=suppress-stale-power-key"
                            );

                            stop_wifi_transfer_server(
                                &mut wifi_transfer_server,
                                &mut state,
                                &mut storage_browser,
                                _mounted_sd.is_some(),
                                "sleep-entry",
                            );
                            if let Some(active) = voice_recording.take() {
                                let _ = active.cancel();
                                if let Some(runtime) = audio_runtime.as_mut() {
                                    let _ = runtime.finish_voice_recording();
                                    state.update_audio_snapshot(runtime.snapshot());
                                }
                                state.voice_notes.cancel_recording();
                                info!(
                                    "rustmix-wave=voice-record status=cancelled reason=sleep-entry"
                                );
                            }
                            if voice_playback.is_some() {
                                stop_voice_note_playback(
                                    &mut voice_playback,
                                    &mut audio_runtime,
                                    &mut state,
                                    "sleep-entry",
                                );
                            }
                            if let Some(runtime) = audio_runtime.as_mut() {
                                match runtime.stop_playback() {
                                    Ok(()) => info!("rustmix-wave=audio-event outcome=playback-stop reason=sleep-mode"),
                                    Err(error) => {
                                        warn!("rustmix-wave=audio-event outcome=playback-stop-failed reason=sleep-mode error={error:#}");
                                        runtime.record_failure(format!("{error:#}"));
                                    }
                                }
                                state.update_audio_snapshot(runtime.snapshot());
                                log_audio_snapshot(&state.audio);
                            }
                            // Best-effort like the IMU/audio-rail/RTC-alarm
                            // teardown below: the sleep image is already shown
                            // and committed to, so a failed network suspend no
                            // longer aborts entering deep sleep, it only skips
                            // the Wi-Fi/SNTP/weather pause.
                            if !suspend_network_for_sleep(
                                &mut network_runtime,
                                &mut state,
                                &mut sleep_network,
                                &mut last_network_fingerprint,
                                &mut last_network_log,
                            ) {
                                warn!("rustmix-wave=sleep-network-suspend status=failed-continuing reason=best-effort-deep-sleep");
                            }
                            panel.sleep()?;
                            state.panel_awake = false;
                            // QMI8658 sits on the always-on VCC3V3 rail, so it
                            // cannot be power-gated by the AXP2101 the way the
                            // e-paper panel's ALDO3 rail is. Disabling its
                            // accelerometer/gyroscope over I2C is the only
                            // available lever to cut its current draw while
                            // the board is otherwise asleep.
                            match board_services.sleep_imu() {
                                Ok(()) => info!("rustmix-wave=imu-suspend status=low-power"),
                                Err(error) => {
                                    warn!("rustmix-wave=imu-suspend status=failed error={error:#}")
                                }
                            }
                            // ALDO2 (Audio_VCC) feeds the codec AVDD pin and the
                            // onboard digital microphone; cut it the same way
                            // ALDO3 is cut for the e-paper panel. PVDD/DVDD stay
                            // powered from the always-on VCC3V3 rail regardless.
                            match misc_power.disable_audio_rail() {
                                Ok(()) => info!("rustmix-wave=pmic-audio-rail status=disabled"),
                                Err(error) => warn!(
                                    "rustmix-wave=pmic-audio-rail status=disable-failed error={error:#}"
                                ),
                            }
                            info!(
                                "rustmix-wave=sleep-mode-enter image={} restore-route={} display=global-refresh panel=deep-sleep aldo3=off aldo2=off imu=low-power wifi=off network-services=paused mcu-sleep=pending",
                                selection.file_name,
                                restore_route.marker()
                            );
                            info!(
                                "rustmix-wave=mcu-deep-sleep status=entering wake-gpio={} wake-level=active-low rtc-alarm-wake=disabled reason=gpio45-not-rtc-io-capable",
                                mcu_deep_sleep::DEEP_SLEEP_WAKE_GPIO
                            );
                            // Disarm the PCF85063 hardware alarm slot before
                            // powering down. It can never wake real MCU deep
                            // sleep (GPIO45 is outside the RTC IO range), so
                            // leaving it armed only risks the alarm firing
                            // while the CPU is off: its interrupt line would
                            // then latch low on GPIO45 — one of the ESP32-S3's
                            // boot strapping pins (VDD_SPI voltage select) —
                            // and stay that way until re-sampled at the next
                            // reset, which can corrupt the GPIO5 wake boot.
                            // The next boot's `sync_alarm_hardware` call
                            // re-arms it for software polling, which is the
                            // only path that ever actually rings the alarm.
                            if let Err(error) = board_services.disable_rtc_alarm() {
                                warn!(
                                    "rustmix-wave=rtc-alarm-disable status=failed reason=pre-deep-sleep error={error:#}"
                                );
                            }
                            // On success this call does not return: the chip
                            // powers down and `run()` starts over from the
                            // top on the next GPIO5 press. Only a failure to
                            // disable automatic light sleep first or to arm
                            // the wakeup source returns here, in which case
                            // the state above (sleep image shown, ALDO3 off,
                            // Wi-Fi suspended) is left in place and the event
                            // loop keeps running as a software-only fallback
                            // so the board is never stranded asleep with no
                            // way to wake it.
                            if let Err(error) = mcu_deep_sleep::espidf::enter() {
                                warn!(
                                    "rustmix-wave=mcu-deep-sleep status=fallback-software-only error={error:#}"
                                );
                                // `enter` only ever turns light sleep off, so
                                // whether it failed before or after doing so,
                                // the running loop below needs it back.
                                if let Err(error) =
                                    mcu_deep_sleep::espidf::set_light_sleep_enabled(true)
                                {
                                    warn!(
                                        "rustmix-wave=power-management status=light-sleep-restore-failed error={error:#}"
                                    );
                                }
                            }
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

            let manual_weather_refresh = state.take_weather_refresh_request();
            if !sleep_network.is_suspended() {
                if let Some(config) = weather_config.as_ref() {
                    if manual_weather_refresh {
                        weather_retry.clear();
                    }
                    let wifi_connected = state.network.wifi_state == WifiConnectionState::Connected;
                    let interval_due = last_weather_attempt.map_or(true, |last| {
                        last.elapsed()
                            >= Duration::from_secs(config.refresh_minutes.saturating_mul(60))
                    });
                    let scheduled_retry = if wifi_connected {
                        weather_retry.take_due()
                    } else {
                        None
                    };
                    let attempt = if manual_weather_refresh {
                        Some(WeatherFetchAttempt::initial("manual"))
                    } else if let Some(retry) = scheduled_retry {
                        Some(retry)
                    } else if interval_due && !weather_retry.is_pending() {
                        Some(WeatherFetchAttempt::initial(
                            if last_weather_attempt.is_none() {
                                "network-ready"
                            } else {
                                "periodic"
                            },
                        ))
                    } else {
                        None
                    };

                    if let Some(attempt) = attempt {
                        if wifi_connected {
                            run_weather_fetch_attempt(
                                config,
                                attempt,
                                &mut weather_retry,
                                &mut state,
                            );
                            last_weather_attempt = Some(Instant::now());
                            if state.panel_awake
                                && matches!(
                                    state.active_route(),
                                    ScreenRoute::Home
                                        | ScreenRoute::Weather
                                        | ScreenRoute::WeatherDetails
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
                        } else if manual_weather_refresh {
                            state.weather.record_failure("Wi-Fi is not connected");
                            warn!("rustmix-wave=weather-fetch status=deferred cause=manual error=wifi-not-connected");
                            if state.panel_awake
                                && matches!(
                                    state.active_route(),
                                    ScreenRoute::Weather | ScreenRoute::WeatherDetails
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
                } else if manual_weather_refresh {
                    state
                        .weather
                        .record_failure("weather configuration is missing");
                    warn!("rustmix-wave=weather-fetch status=deferred cause=manual error=weather-config-missing");
                    if state.panel_awake
                        && matches!(
                            state.active_route(),
                            ScreenRoute::Weather | ScreenRoute::WeatherDetails
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
                apply_wifi_transfer_ui_request(
                    &mut wifi_transfer_server,
                    &mut state,
                    &mut storage_browser,
                    _mounted_sd.is_some(),
                    voice_recording.is_some(),
                    voice_playback.is_some(),
                );
                apply_network_provision_ui_request(
                    &mut network_runtime,
                    &mut network_provision_server,
                    &mut network_config,
                    &mut state,
                    &mut network_provision_join_pending,
                );
                apply_network_saved_ui_request(
                    &mut network_config,
                    &mut state,
                    &network_provision_server,
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

            if !sleep_mode.is_sleeping()
                && state.panel_awake
                && (state.active_route() == ScreenRoute::MotionEvents
                    || state.lua_game_needs_imu_events())
                && last_imu_event_sample.elapsed()
                    >= Duration::from_millis(IMU_EVENT_SAMPLE_INTERVAL_MS)
            {
                match board_services.read_imu_motion() {
                    Ok(reading) => {
                        let now_ms = imu_event_started_at.elapsed().as_millis() as u64;
                        let event = state.update_imu_event_sample(reading, now_ms);
                        if let Some(event) = event {
                            info!("rustmix-wave=imu-event type={} detail={} at-ms={} samples={} counts=tilt:{},shake:{},rotate:{},level:{} thresholds=tilt:{}mg,shake:{}mg,rotate:{}dps,level:{}mg,debounce:{}ms", event.kind.marker(), event.kind.detail_marker(), event.at_ms, state.imu_events.samples, state.imu_events.counters.tilt, state.imu_events.counters.shake, state.imu_events.counters.rotate, state.imu_events.counters.level, state.imu_events.thresholds.tilt_enter_mg, state.imu_events.thresholds.shake_delta_mg, state.imu_events.thresholds.rotate_dps, state.imu_events.thresholds.level_tolerance_mg, state.imu_events.thresholds.debounce_ms);
                        }
                        let game_motion_changed =
                            event.is_some_and(|event| state.apply_lua_game_motion_event(event));
                        if game_motion_changed {
                            log_lua_runtime_events(&mut state);
                        }
                        let diagnostic_refresh = state.active_route() == ScreenRoute::MotionEvents
                            && (event.is_some()
                                || last_imu_event_screen_refresh.elapsed()
                                    >= Duration::from_secs(IMU_EVENT_SCREEN_REFRESH_SECONDS));
                        if game_motion_changed || diagnostic_refresh {
                            refresh_screen(
                                &mut panel,
                                &mut frame,
                                &mut state,
                                &mut panel_refresh,
                                RefreshRequest::Normal,
                            )?;
                            last_imu_event_screen_refresh = Instant::now();
                        }
                    }
                    Err(error) => {
                        warn!("rustmix-wave=imu-event-sample status=unavailable error={error:#}")
                    }
                }
                last_imu_event_sample = Instant::now();
            }

            let live_refresh_seconds = match state.active_route() {
                ScreenRoute::Motion | ScreenRoute::MotionDetails => MOTION_LIVE_REFRESH_SECONDS,
                ScreenRoute::Network | ScreenRoute::NetworkDetails => NETWORK_LIVE_REFRESH_SECONDS,
                _ => SAMPLE_LIVE_REFRESH_SECONDS,
            };
            if state.panel_awake
                && state.active_route().uses_live_status()
                && last_status_refresh.elapsed() >= Duration::from_secs(live_refresh_seconds)
            {
                state.update_board_snapshot(board_services.read_snapshot(&mut service_delay));
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

            if let Some(input_event) = input_queue.pop() {
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
                        state.update_board_snapshot(
                            board_services.read_snapshot(&mut service_delay),
                        );
                        log_board_snapshot(state.board, state.regional);
                        let previous_route = state.active_route();
                        if previous_route == ScreenRoute::Home {
                            info!("rustmix-wave=hierarchical-back outcome=ignored route=home");
                        } else {
                            state.back();
                            apply_voice_notes_ui_request(
                                &mut voice_recording,
                                &mut voice_playback,
                                &mut audio_runtime,
                                &mut state,
                                _mounted_sd.is_some(),
                            );
                            apply_wifi_transfer_ui_request(
                                &mut wifi_transfer_server,
                                &mut state,
                                &mut storage_browser,
                                _mounted_sd.is_some(),
                                voice_recording.is_some(),
                                voice_playback.is_some(),
                            );
                            apply_network_provision_ui_request(
                                &mut network_runtime,
                                &mut network_provision_server,
                                &mut network_config,
                                &mut state,
                                &mut network_provision_join_pending,
                            );
                            log_lua_runtime_events(&mut state);
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
                        if woke_from_sleep || state.active_route() != previous_route {
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
                        let calendar_agenda_context = state.apply_calendar_select_long_press();
                        let keyboard_context = if calendar_agenda_context {
                            false
                        } else {
                            state.apply_keyboard_select_long_press()
                        };
                        let lua_game_context = if calendar_agenda_context || keyboard_context {
                            false
                        } else {
                            state.apply_lua_game_select_long_press()
                        };
                        let reader_dictionary_context =
                            if calendar_agenda_context || keyboard_context || lua_game_context {
                                false
                            } else {
                                state.apply_reader_dictionary_select_long_press()
                            };
                        let network_saved_context = if calendar_agenda_context
                            || keyboard_context
                            || lua_game_context
                            || reader_dictionary_context
                        {
                            false
                        } else {
                            state.apply_network_saved_select_long_press()
                        };
                        if calendar_agenda_context
                            || keyboard_context
                            || lua_game_context
                            || reader_dictionary_context
                            || network_saved_context
                        {
                            if calendar_agenda_context {
                                info!("rustmix-wave=calendar-agenda route=selected-day outcome=opened");
                            }
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
                            if keyboard_context {
                                if state.active_route() == ScreenRoute::CalendarEventEditor {
                                    if let Some(editor) = state.calendar.editor.as_ref() {
                                        info!(
                                        "rustmix-wave=calendar-editor-keyboard-nav axis={} outcome=toggled",
                                        editor.navigation_mode_label()
                                    );
                                    }
                                } else if state.active_route() == ScreenRoute::VoiceNoteDetails
                                    && state.voice_notes.title_editing
                                {
                                    info!(
                                    "rustmix-wave=voice-note-title-keyboard-nav axis={} outcome=toggled",
                                    state.voice_notes.title_editor_navigation_mode_label()
                                );
                                } else {
                                    info!(
                                    "rustmix-wave=dictionary-keyboard-nav axis={} outcome=toggled",
                                    state.dictionary.navigation_mode_label()
                                );
                                }
                            }
                            let woke_from_sleep = !state.panel_awake;
                            if woke_from_sleep {
                                panel.initialize()?;
                                state.panel_awake = true;
                                panel_refresh
                                    .reset_after_external_global(PanelGlobalReason::AfterWake);
                                sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                            }
                            state.update_board_snapshot(
                                board_services.read_snapshot(&mut service_delay),
                            );
                            log_board_snapshot(state.board, state.regional);
                            log_lua_runtime_events(&mut state);
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

                        state.update_board_snapshot(
                            board_services.read_snapshot(&mut service_delay),
                        );
                        log_board_snapshot(state.board, state.regional);
                        let previous_route = state.active_route();
                        let previous_display = state.display;
                        if previous_route == ScreenRoute::Files {
                            apply_storage_event(&mut storage_browser, &mut state, event);
                        } else if previous_route == ScreenRoute::Alarms {
                            let local = state.board.rtc.map_or_else(fallback_local_time, |rtc| {
                                state.regional.localize_rtc(rtc)
                            });
                            let outcome =
                                apply_alarm_event(&mut alarm_engine, &mut state, event, local);
                            if matches!(
                                outcome,
                                AlarmUiOutcome::Saved
                                    | AlarmUiOutcome::Snoozed
                                    | AlarmUiOutcome::Dismissed
                            ) {
                                sync_alarm_hardware(
                                    &mut alarm_engine,
                                    &mut board_services,
                                    state.regional,
                                );
                                state.update_alarm_snapshot(alarm_engine.snapshot());
                                log_alarm_snapshot(&state.alarms);
                            }
                            if matches!(
                                outcome,
                                AlarmUiOutcome::Snoozed | AlarmUiOutcome::Dismissed
                            ) {
                                if let Some(runtime) = audio_runtime.as_mut() {
                                    match runtime.stop_playback() {
                                Ok(()) => info!("rustmix-wave=audio-event outcome=alarm-chime-stop reason={outcome:?}"),
                                Err(error) => {
                                    warn!("rustmix-wave=audio-event outcome=alarm-chime-stop-failed error={error:#}");
                                    runtime.record_failure(format!("{error:#}"));
                                }
                            }
                                    state.update_audio_snapshot(runtime.snapshot());
                                    log_audio_snapshot(&state.audio);
                                }
                            }
                        } else if previous_route == ScreenRoute::Audio {
                            if let Some(request) = state.apply_audio_button(event) {
                                apply_audio_request(&mut audio_runtime, &mut state, request);
                            }
                        } else {
                            state.apply(event);
                            log_lua_runtime_events(&mut state);
                            if state.active_route() == ScreenRoute::Files {
                                storage_browser.refresh();
                                state.update_storage_snapshot(storage_browser.snapshot());
                                log_storage_snapshot(&state.storage);
                            }
                        }
                        // Consume Settings > Network transfer start/stop intents before
                        // rendering the next frame.  This guarantees that the transfer
                        // route shows READY plus its LAN URL and code on the same normal
                        // partial refresh that follows the SELECT event.
                        apply_calendar_ui_request(&mut state, _mounted_sd.is_some());
                        apply_network_provision_ui_request(
                            &mut network_runtime,
                            &mut network_provision_server,
                            &mut network_config,
                            &mut state,
                            &mut network_provision_join_pending,
                        );
                        apply_network_saved_ui_request(
                            &mut network_config,
                            &mut state,
                            &network_provision_server,
                        );
                        apply_voice_notes_ui_request(
                            &mut voice_recording,
                            &mut voice_playback,
                            &mut audio_runtime,
                            &mut state,
                            _mounted_sd.is_some(),
                        );
                        apply_wifi_transfer_ui_request(
                            &mut wifi_transfer_server,
                            &mut state,
                            &mut storage_browser,
                            _mounted_sd.is_some(),
                            voice_recording.is_some(),
                            voice_playback.is_some(),
                        );
                        apply_clock_set_time_ui_request(
                            &mut board_services,
                            &mut service_delay,
                            &mut alarm_engine,
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
                        "rustmix-wave=display-settings-updated font-family={} font-size={} persistence=sd-file path={DISPLAY_CONFIG_PATH}",
                        state.display.font_family.marker(),
                        state.display.font_size.marker()
                    );
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
                        let reader_clear_ghost = state.take_reader_clear_ghost_request();
                        let power_key_clear_ghost = state.take_power_key_manual_refresh_request();
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

            #[cfg(feature = "rustmix-remote-ble")]
            while let Some(remote_event) = rustmix_remote_queue.pop() {
                info!(
                    "rustmix-wave=rustmix-remote-event event={} route={}",
                    remote_event.marker(),
                    state.active_route().marker()
                );
                if sleep_mode.is_sleeping() {
                    info!(
                        "rustmix-wave=rustmix-remote-event action=suppressed reason=sleeping event={}",
                        remote_event.marker()
                    );
                    continue;
                }
                let Some(event) = (match remote_event {
                    RemoteEvent::PageNext => Some(ButtonEvent::Down),
                    RemoteEvent::PagePrevious => Some(ButtonEvent::Up),
                    _ => None,
                }) else {
                    info!(
                        "rustmix-wave=rustmix-remote-event action=ignored reason=unsupported-r1 event={}",
                        remote_event.marker()
                    );
                    continue;
                };
                if state.active_route() != ScreenRoute::ReaderPage {
                    info!(
                        "rustmix-wave=rustmix-remote-event action=ignored reason=route-not-reader-page event={} route={}",
                        remote_event.marker(),
                        state.active_route().marker()
                    );
                    continue;
                }

                let woke_from_sleep = !state.panel_awake;
                if woke_from_sleep {
                    panel.initialize()?;
                    state.panel_awake = true;
                    panel_refresh.reset_after_external_global(PanelGlobalReason::AfterWake);
                    sync_panel_refresh_diagnostics(&mut state, &panel_refresh);
                }

                let previous_route = state.active_route();
                state.apply(event);
                log_reader_persistence_event(&mut state);
                if state.active_route() != previous_route {
                    info!(
                        "rustmix-wave=screen-route route={} cause=rustmix-remote",
                        state.active_route().marker()
                    );
                }
                let reader_clear_ghost = state.take_reader_clear_ghost_request();
                let request = if woke_from_sleep {
                    RefreshRequest::ForceGlobalAfterWake
                } else if reader_clear_ghost {
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

            FreeRtos::delay_ms(20);
        }
    }

    fn apply_calendar_ui_request(state: &mut AppState, mounted: bool) {
        let Some(request) = state.take_calendar_request() else {
            return;
        };
        if !mounted {
            state.calendar.fail("SD card unavailable");
            warn!(
                "rustmix-wave=calendar-personal-event-write status=rejected reason=sd-unavailable"
            );
            return;
        }
        let root = std::path::Path::new(CALENDAR_ROOT);
        let outcome = match request {
            CalendarUiRequest::CreatePersonal { date, title, detail } => {
                create_personal_event(root, date, &title, &detail).map(|()| {
                    info!("rustmix-wave=calendar-personal-event-write status=completed operation=create title={title}");
                    "Personal event created"
                })
            }
            CalendarUiRequest::UpdatePersonal {
                source_row,
                title,
                detail,
            } => update_personal_event(root, source_row, &title, &detail).map(|()| {
                info!("rustmix-wave=calendar-personal-event-write status=completed operation=edit source-row={source_row} title={title}");
                "Personal event updated"
            }),
            CalendarUiRequest::DeletePersonal { source_row } => {
                delete_personal_event(root, source_row).map(|()| {
                    info!("rustmix-wave=calendar-personal-event-write status=completed operation=delete source-row={source_row}");
                    "Personal event deleted"
                })
            }
        };
        match outcome {
            Ok(message) => {
                state.calendar.refresh_events();
                state.calendar.mark_persistence_completed(message);
                state.router.navigate_to(ScreenRoute::CalendarAgenda);
            }
            Err(error) => {
                state.calendar.fail(format!("{error:#}"));
                warn!("rustmix-wave=calendar-personal-event-write status=failed error={error:#}");
            }
        }
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

    /// Start or stop the phone Wi-Fi provisioning portal from the Network ▸
    /// Configure via phone action. Starting switches the driver into AP+STA
    /// (Mixed) mode with a freshly generated hotspot and brings up the small
    /// HTTP portal used to add, update or forget saved networks; stopping
    /// tears both down and reconnects using the current saved-network list.
    /// This replaces the old on-device rotary-keyboard credential editor.
    fn apply_network_provision_ui_request(
        runtime: &mut NetworkRuntime,
        server: &mut Option<NetworkProvisionServer>,
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
    ) {
        let Some(request) = state.take_network_provision_request() else {
            return;
        };
        match request {
            NetworkProvisionUiRequest::Start => {
                if server.is_some() {
                    return;
                }
                state.update_network_provision_snapshot(NetworkProvisionSnapshot::starting());
                let ap_info = match runtime.start_provisioning() {
                    Ok(ap_info) => ap_info,
                    Err(error) => {
                        warn!("rustmix-wave=network-provision status=failed error={error:#}");
                        state.update_network_provision_snapshot(NetworkProvisionSnapshot::failed(
                            format!("{error:#}"),
                        ));
                        return;
                    }
                };
                match NetworkProvisionServer::start(&ap_info.portal_ip) {
                    Ok(mut new_server) => {
                        new_server.set_ap_credentials(
                            ap_info.ap_ssid.clone(),
                            ap_info.ap_password.clone(),
                        );
                        new_server.set_saved_networks(saved_ssids(network_config));
                        *join_pending = None;
                        let saved_count = network_config.as_ref().map_or(0, |c| c.networks.len());
                        state.update_network_provision_snapshot(new_server.snapshot(saved_count));
                        info!(
                            "rustmix-wave=network-provision status=ready ap-ssid={} ip={}",
                            ap_info.ap_ssid, ap_info.portal_ip
                        );
                        *server = Some(new_server);
                    }
                    Err(error) => {
                        warn!(
                            "rustmix-wave=network-provision-server status=failed error={error:#}"
                        );
                        state.update_network_provision_snapshot(NetworkProvisionSnapshot::failed(
                            format!("{error:#}"),
                        ));
                    }
                }
            }
            NetworkProvisionUiRequest::Stop => {
                stop_network_provision(
                    runtime,
                    server,
                    network_config,
                    state,
                    join_pending,
                    "user-stop",
                );
            }
        }
    }

    /// Tear down the provisioning hotspot/portal (if running) and reconnect
    /// the driver using the current saved-network list.
    fn stop_network_provision(
        runtime: &mut NetworkRuntime,
        server: &mut Option<NetworkProvisionServer>,
        network_config: &Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
        reason: &str,
    ) {
        if server.take().is_none() {
            return;
        }
        *join_pending = None;
        if let Err(error) = runtime.stop_provisioning(network_config.as_ref()) {
            warn!(
                "rustmix-wave=network-provision status=stop-reconnect-failed reason={reason} error={error:#}"
            );
        }
        state.update_network_snapshot(runtime.snapshot());
        state.update_network_provision_snapshot(NetworkProvisionSnapshot::default());
        state.set_saved_networks(saved_network_entries(network_config, None));
        info!("rustmix-wave=network-provision-server status=stopped reason={reason}");
    }

    /// Drive the provisioning portal each main-loop iteration while it is
    /// active: feed it fresh scan results, drain a phone-submitted
    /// SSID/password into a real (validate-before-save) connection attempt,
    /// watch that attempt resolve to persist or report failure, drain a
    /// "Forget" request from the phone, and auto-stop on inactivity.
    #[allow(clippy::too_many_arguments)]
    fn maintain_network_provision(
        runtime: &mut NetworkRuntime,
        server: &mut Option<NetworkProvisionServer>,
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        join_pending: &mut Option<(String, String)>,
        last_rescan: &mut Instant,
    ) {
        let Some(active_server) = server.as_ref() else {
            return;
        };
        if active_server.is_expired() {
            stop_network_provision(
                runtime,
                server,
                network_config,
                state,
                join_pending,
                "inactivity-timeout",
            );
            return;
        }
        let Some(active_server) = server.as_ref() else {
            return;
        };

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
                            info!("rustmix-wave=network-provision-join status=saved ssid={ssid}");
                        }
                        Err(error) => {
                            active_server.record_join_failed(ssid, format!("{error:#}"));
                            warn!("rustmix-wave=network-provision-join status=save-failed error={error:#}");
                        }
                    }
                    *join_pending = None;
                }
                WifiConnectionState::Failed => {
                    let error = snapshot
                        .error
                        .clone()
                        .unwrap_or_else(|| "connection failed".into());
                    active_server.record_join_failed(ssid, error);
                    *join_pending = None;
                }
                _ => {}
            }
        }

        if let Some(ssid) = active_server.take_pending_delete() {
            if let Some(config) = network_config.as_mut() {
                if config.remove(&ssid) {
                    if let Err(error) = config.save_to_path(WIFI_CONFIG_PATH) {
                        warn!(
                            "rustmix-wave=network-provision-forget status=failed error={error:#}"
                        );
                    } else {
                        active_server.set_saved_networks(saved_ssids(network_config));
                        state.set_saved_networks(saved_network_entries(network_config, None));
                        info!("rustmix-wave=network-provision-forget status=completed ssid={ssid}");
                    }
                }
            }
        }

        // Skip the rescan once a phone has joined the hotspot: it shares the
        // AP's single radio, so an active scan briefly leaves the AP's
        // channel and can reset the phone's in-flight requests, including
        // the captive-portal probe this portal depends on to auto-open (see
        // `NetworkRuntime::provisioning_client_count`).
        if join_pending.is_none()
            && last_rescan.elapsed() >= Duration::from_secs(NETWORK_PROVISION_RESCAN_SECONDS)
            && runtime.provisioning_client_count() == 0
        {
            *last_rescan = Instant::now();
            let _ = runtime.start_scan();
        }

        let saved_count = network_config
            .as_ref()
            .map_or(0, |config| config.networks.len());
        let latest = active_server.snapshot(saved_count);
        if latest != state.network_provision {
            state.update_network_provision_snapshot(latest);
        }
    }

    /// Forget a saved network requested from the on-device "Saved networks"
    /// screen. If it was the network currently connected, provisioning stays
    /// off (the list is simply shorter); the next boot or provisioning-stop
    /// reconnect uses the updated list.
    fn apply_network_saved_ui_request(
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
        server: &Option<NetworkProvisionServer>,
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

    fn maintain_wifi_transfer_server(
        server: &mut Option<WifiTransferServer>,
        state: &mut AppState,
        storage_browser: &mut StorageBrowser,
        mounted: bool,
    ) {
        let stop_reason = server.as_ref().and_then(|active| {
            if state.network.wifi_state != WifiConnectionState::Connected {
                Some("wifi-loss")
            } else if active.is_expired() {
                Some("inactivity-timeout")
            } else {
                None
            }
        });
        if let Some(reason) = stop_reason {
            stop_wifi_transfer_server(server, state, storage_browser, mounted, reason);
        } else if let Some(active) = server.as_ref() {
            let snapshot = active.snapshot();
            if snapshot != state.wifi_transfer {
                state.update_wifi_transfer_snapshot(snapshot);
            }
        }
    }

    fn apply_wifi_transfer_ui_request(
        server: &mut Option<WifiTransferServer>,
        state: &mut AppState,
        storage_browser: &mut StorageBrowser,
        mounted: bool,
        voice_recording_active: bool,
        voice_playback_active: bool,
    ) {
        let Some(request) = state.take_wifi_transfer_request() else {
            return;
        };
        match request {
            WifiTransferUiRequest::Start => {
                info!(
                    "rustmix-wave=wifi-transfer-ui-request request=start dispatch=before-refresh"
                );
                if voice_recording_active {
                    state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                        "Voice recording is active; stop recording before Wi-Fi transfer",
                    ));
                    warn!("rustmix-wave=wifi-transfer-server status=rejected reason=voice-recording-active");
                    return;
                }
                if voice_playback_active {
                    state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                        "Voice-note playback is active; stop playback before Wi-Fi transfer",
                    ));
                    warn!("rustmix-wave=wifi-transfer-server status=rejected reason=voice-note-playback-active");
                    return;
                }
                if server.is_some() {
                    return;
                }
                state.update_wifi_transfer_snapshot(WifiTransferSnapshot::starting());
                let Some(ipv4) = state.network.ipv4_address.as_deref() else {
                    state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(
                        "Connect Wi-Fi before starting transfer",
                    ));
                    warn!("rustmix-wave=wifi-transfer-server status=start-rejected reason=wifi-not-connected");
                    return;
                };
                let code = format!("{:06}", unsafe { sys::esp_random() } % 1_000_000);
                info!("rustmix-wave=wifi-transfer-server status=starting ipv4={ipv4} port=80 root={WIFI_TRANSFER_ROOT} stack-bytes={WIFI_TRANSFER_SERVER_STACK_BYTES}");
                log_runtime_memory("before-wifi-transfer-start");
                match WifiTransferServer::start(ipv4, code) {
                    Ok(active) => {
                        state.update_wifi_transfer_snapshot(active.snapshot());
                        *server = Some(active);
                        log_runtime_memory("after-wifi-transfer-start");
                    }
                    Err(error) => {
                        warn!(
                            "rustmix-wave=wifi-transfer-server status=start-failed error={error:#}"
                        );
                        state.update_wifi_transfer_snapshot(WifiTransferSnapshot::failed(format!(
                            "{error:#}"
                        )));
                    }
                }
            }
            WifiTransferUiRequest::Stop => {
                info!("rustmix-wave=wifi-transfer-ui-request request=stop dispatch=before-refresh");
                stop_wifi_transfer_server(
                    server,
                    state,
                    storage_browser,
                    mounted,
                    "settings-toggle",
                );
            }
        }
    }

    fn stop_wifi_transfer_server(
        server: &mut Option<WifiTransferServer>,
        state: &mut AppState,
        storage_browser: &mut StorageBrowser,
        mounted: bool,
        reason: &'static str,
    ) {
        if server.take().is_some() {
            info!("rustmix-wave=wifi-transfer-server status=stopped reason={reason}");
            log_runtime_memory("after-wifi-transfer-stop");
        }
        state.update_wifi_transfer_snapshot(WifiTransferSnapshot::default());
        state.refresh_lua_app_catalog(mounted);
        state.reader.refresh_library();
        state.calendar.refresh_events();
        storage_browser.refresh();
        state.update_storage_snapshot(storage_browser.snapshot());
    }

    #[derive(Clone, Copy, Debug)]
    struct WeatherFetchAttempt {
        cause: &'static str,
        retry_attempt: usize,
    }

    impl WeatherFetchAttempt {
        const fn initial(cause: &'static str) -> Self {
            Self {
                cause,
                retry_attempt: 0,
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct PendingWeatherRetry {
        due: Instant,
        attempt: WeatherFetchAttempt,
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct WeatherRetryState {
        pending: Option<PendingWeatherRetry>,
    }

    impl WeatherRetryState {
        fn clear(&mut self) {
            self.pending = None;
        }

        #[must_use]
        const fn is_pending(&self) -> bool {
            self.pending.is_some()
        }

        fn take_due(&mut self) -> Option<WeatherFetchAttempt> {
            if self
                .pending
                .as_ref()
                .is_some_and(|pending| Instant::now() >= pending.due)
            {
                self.pending.take().map(|pending| pending.attempt)
            } else {
                None
            }
        }

        fn schedule_next(&mut self, failed_attempt: WeatherFetchAttempt) -> Option<(usize, u64)> {
            let retry_attempt = failed_attempt.retry_attempt.saturating_add(1);
            let delay_seconds = *WEATHER_RETRY_DELAYS_SECONDS.get(retry_attempt - 1)?;
            self.pending = Some(PendingWeatherRetry {
                due: Instant::now() + Duration::from_secs(delay_seconds),
                attempt: WeatherFetchAttempt {
                    cause: failed_attempt.cause,
                    retry_attempt,
                },
            });
            Some((retry_attempt, delay_seconds))
        }
    }

    fn run_weather_fetch_attempt(
        config: &WeatherConfig,
        attempt: WeatherFetchAttempt,
        retry: &mut WeatherRetryState,
        state: &mut AppState,
    ) {
        let attempt_label = if attempt.retry_attempt == 0 {
            "initial".into()
        } else {
            format!("{}/{}", attempt.retry_attempt, WEATHER_RETRY_LIMIT)
        };
        info!(
            "rustmix-wave=weather-fetch status=starting cause={} attempt={} provider={} location={}",
            attempt.cause, attempt_label, config.provider, config.location
        );
        state.weather.mark_fetching();
        match fetch_open_meteo_on_worker(config) {
            Ok(data) => {
                retry.clear();
                state.weather.record_success(data);
                log_weather_snapshot(&state.weather);
                info!(
                    "rustmix-wave=weather-fetch status=completed cause={} attempt={} forecast-days={}",
                    attempt.cause,
                    attempt_label,
                    state.weather.forecast.len()
                );
            }
            Err(error) => {
                handle_weather_fetch_failure(attempt, error, retry, state);
                log_weather_snapshot(&state.weather);
            }
        }
    }

    fn handle_weather_fetch_failure(
        attempt: WeatherFetchAttempt,
        error: WeatherFetchError,
        retry: &mut WeatherRetryState,
        state: &mut AppState,
    ) {
        let message = error.to_string();
        if error.is_retryable() {
            if let Some((retry_attempt, delay_seconds)) = retry.schedule_next(attempt) {
                state.weather.mark_retrying(message.clone());
                warn!(
                    "rustmix-wave=weather-fetch-retry-scheduled cause={} attempt={}/{} delay-seconds={} classification={} error={}",
                    attempt.cause,
                    retry_attempt,
                    WEATHER_RETRY_LIMIT,
                    delay_seconds,
                    error.category(),
                    message
                );
                if state.weather.current.is_some() {
                    info!(
                        "rustmix-wave=weather-fetch outcome=stale-cache-retained state=retrying last-success={} error={}",
                        state.weather.last_success_label(),
                        message
                    );
                }
                return;
            }
        }

        retry.clear();
        state.weather.record_failure(message.clone());
        warn!(
            "rustmix-wave=weather-fetch status=failed cause={} retryable={} retries-exhausted={} classification={} error={}",
            attempt.cause,
            error.is_retryable(),
            error.is_retryable(),
            error.category(),
            message
        );
        if state.weather.current.is_some() {
            info!(
                "rustmix-wave=weather-fetch outcome=stale-cache-retained state=stale last-success={} error={}",
                state.weather.last_success_label(),
                message
            );
        }
    }

    fn suspend_network_for_sleep(
        runtime: &mut NetworkRuntime,
        state: &mut AppState,
        sleep_network: &mut SleepNetworkState,
        last_network_fingerprint: &mut NetworkLogFingerprint,
        last_network_log: &mut Instant,
    ) -> bool {
        info!("rustmix-wave=sleep-network-suspend status=starting");
        match runtime.suspend() {
            Ok(()) => {
                let _ = sleep_network.suspend();
                state.update_network_snapshot(runtime.snapshot());
                *last_network_fingerprint = state.network.log_fingerprint();
                *last_network_log = Instant::now();
                info!("rustmix-wave=sntp-suspend status=stopped");
                info!("rustmix-wave=wifi-suspend status=disconnected");
                info!("rustmix-wave=wifi-suspend status=stopped");
                info!("rustmix-wave=weather-suspend status=paused");
                true
            }
            Err(error) => {
                warn!("rustmix-wave=sleep-network-suspend status=failed error={error:#}");
                false
            }
        }
    }

    fn resume_network_after_sleep(
        runtime: &mut NetworkRuntime,
        config: Option<&NetworkConfig>,
        state: &mut AppState,
        sleep_network: &mut SleepNetworkState,
        last_network_fingerprint: &mut NetworkLogFingerprint,
        last_network_log: &mut Instant,
        last_weather_attempt: &mut Option<Instant>,
        weather_retry: &mut WeatherRetryState,
    ) {
        info!("rustmix-wave=sleep-network-resume status=starting");
        if let Some(config) = config {
            info!(
                "rustmix-wave=wifi-resume status=starting ssid={}",
                first_saved_ssid(config)
            );
            match runtime.resume(config) {
                Ok(()) => {
                    info!(
                        "rustmix-wave=wifi-resume status=connected ssid={}",
                        first_saved_ssid(config)
                    );
                    info!("rustmix-wave=sntp-resume status=started");
                    info!("rustmix-wave=weather-resume status=pending-network-ready");
                }
                Err(error) => {
                    warn!(
                        "rustmix-wave=wifi-resume status=failed ssid={} error={error:#}",
                        first_saved_ssid(config)
                    );
                    runtime.record_resume_failure(format!("{error:#}"));
                }
            }
        } else {
            runtime.record_configuration_missing();
            info!("rustmix-wave=wifi-resume status=skipped reason=configuration-missing");
        }
        let _ = sleep_network.resume();
        state.update_network_snapshot(runtime.snapshot());
        log_network_snapshot(&state.network);
        *last_network_fingerprint = state.network.log_fingerprint();
        *last_network_log = Instant::now();
        *last_weather_attempt = None;
        weather_retry.clear();
    }

    fn apply_alarm_event(
        engine: &mut AlarmEngine,
        state: &mut AppState,
        event: ButtonEvent,
        now_local: RtcDateTime,
    ) -> AlarmUiOutcome {
        if event == ButtonEvent::Select {
            state.note_select_press();
        }
        let outcome = engine.apply_button(event, now_local);
        if outcome == AlarmUiOutcome::ReturnHome {
            state.router.back();
        }
        state.update_alarm_snapshot(engine.snapshot());
        info!(
            "rustmix-wave=alarm-ui-event outcome={outcome:?} active={} schedules={} selected={} next={} hardware-programmed={}",
            state.alarms.active.is_some(),
            state.alarms.alarms.len(),
            state.alarms.selected,
            state.alarms.next_label(),
            state.alarms.hardware_programmed
        );
        outcome
    }

    /// Persist a manually edited local wall-clock value committed from the
    /// Clock screen's Set Date & Time editor and refresh dependent state:
    /// the board snapshot and, since alarm scheduling depends on the wall
    /// clock, the alarm engine's next occurrence and hardware programming.
    fn apply_clock_set_time_ui_request<I2C, D>(
        board_services: &mut BoardServices<I2C>,
        service_delay: &mut D,
        alarm_engine: &mut AlarmEngine,
        state: &mut AppState,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
        D: embedded_hal::delay::DelayNs,
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
        state.update_board_snapshot(board_services.read_snapshot(service_delay));
        log_board_snapshot(state.board, state.regional);
        if let Some(rtc) = state.board.rtc {
            alarm_engine.recompute_next(state.regional.localize_rtc(rtc));
            sync_alarm_hardware(alarm_engine, board_services, state.regional);
            state.update_alarm_snapshot(alarm_engine.snapshot());
            log_alarm_snapshot(&state.alarms);
        }
    }

    /// Persist a timezone committed from the Clock screen's Set Date & Time
    /// editor into its own `CLOCK.TXT` file, independent of whether Wi-Fi has
    /// ever been configured. `state.regional` already reflects the new
    /// timezone live regardless of whether persistence below succeeds, so a
    /// write failure only costs the selection surviving the next reboot, not
    /// the current session. When `WIFI.TXT` already exists, its `timezone=`
    /// line is kept in sync too, preserving the SSID/password/NTP fields the
    /// editor never touches, purely so the two files do not disagree.
    fn apply_clock_set_timezone_ui_request(
        network_config: &mut Option<NetworkConfig>,
        state: &mut AppState,
    ) {
        let Some(timezone) = state.take_clock_set_timezone_request() else {
            return;
        };
        match regional::save_timezone_name(CLOCK_CONFIG_PATH, timezone.name()) {
            Ok(()) => info!(
                "rustmix-wave=clock-timezone-set status=persisted path={CLOCK_CONFIG_PATH} timezone={}",
                timezone.name()
            ),
            Err(error) => warn!(
                "rustmix-wave=clock-timezone-set status=write-failed path={CLOCK_CONFIG_PATH} error={error:#}"
            ),
        }

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

    fn apply_audio_request<'d, I2C>(
        runtime: &mut Option<AudioRuntime<'d, I2C>>,
        state: &mut AppState,
        request: AudioUiRequest,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        let Some(runtime) = runtime.as_mut() else {
            warn!("rustmix-wave=audio-event outcome=unavailable request={request:?}");
            return;
        };
        match runtime.apply_request(request) {
            Ok(outcome) => info!("rustmix-wave=audio-event outcome={outcome}"),
            Err(error) => {
                warn!("rustmix-wave=audio-event outcome=request-failed request={request:?} error={error:#}");
                runtime.record_failure(format!("{error:#}"));
            }
        }
        state.update_audio_snapshot(runtime.snapshot());
        log_audio_snapshot(&state.audio);
    }

    fn sd_available_bytes(path: &str) -> Option<u64> {
        let path = CString::new(path).ok()?;
        let mut total_bytes = 0_u64;
        let mut free_bytes = 0_u64;
        if unsafe { sys::esp_vfs_fat_info(path.as_ptr(), &mut total_bytes, &mut free_bytes) }
            != sys::ESP_OK
        {
            return None;
        }
        Some(free_bytes)
    }

    fn refresh_voice_note_storage_available(state: &mut AppState, mounted: bool) {
        let available = mounted
            .then(|| sd_available_bytes(SD_MOUNT_POINT))
            .flatten();
        state.voice_notes.set_available_storage_bytes(available);
    }

    fn stop_voice_note_playback<'d, I2C>(
        session: &mut Option<VoicePlaybackSession>,
        audio_runtime: &mut Option<AudioRuntime<'d, I2C>>,
        state: &mut AppState,
        reason: &str,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        let Some(active) = session.take() else {
            return;
        };
        let file_name = active.file_name().to_string();
        if let Some(runtime) = audio_runtime.as_mut() {
            if let Err(error) = runtime.finish_voice_note_playback() {
                warn!("rustmix-wave=voice-note-playback status=stop-failed file={file_name} reason={reason} error={error:#}");
                runtime.record_failure(format!("{error:#}"));
            }
            state.update_audio_snapshot(runtime.snapshot());
            log_audio_snapshot(&state.audio);
        }
        state.voice_notes.stop_playback();
        info!("rustmix-wave=voice-note-playback status=stopped file={file_name} reason={reason}");
    }

    fn apply_voice_notes_ui_request<'d, I2C>(
        session: &mut Option<VoiceRecordingSession>,
        playback: &mut Option<VoicePlaybackSession>,
        audio_runtime: &mut Option<AudioRuntime<'d, I2C>>,
        state: &mut AppState,
        mounted: bool,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        let Some(request) = state.take_voice_notes_request() else {
            return;
        };
        match request {
            VoiceNotesUiRequest::StartRecording => {
                if !mounted {
                    state.voice_notes.fail("SD card unavailable");
                    warn!("rustmix-wave=voice-record status=rejected reason=sd-unavailable");
                    return;
                }
                if state.wifi_transfer.is_active() {
                    state
                        .voice_notes
                        .fail("Stop Wi-Fi Transfer before recording");
                    warn!("rustmix-wave=voice-record status=rejected reason=wifi-transfer-active");
                    return;
                }
                if state.alarms.active.is_some() {
                    state.voice_notes.fail("Alarm active");
                    warn!("rustmix-wave=voice-record status=rejected reason=active-alarm");
                    return;
                }
                if playback.is_some() {
                    stop_voice_note_playback(playback, audio_runtime, state, "recording-start");
                }
                let Some(runtime) = audio_runtime.as_mut() else {
                    state.voice_notes.fail("Microphone unavailable");
                    warn!("rustmix-wave=voice-record status=rejected reason=audio-unavailable");
                    return;
                };
                if session.is_some() {
                    return;
                }
                let recorded_at = state
                    .board
                    .rtc
                    .map(|rtc| state.regional.localize_rtc(rtc).date_time())
                    .unwrap_or_else(|| VOICE_UNKNOWN_RECORDED_AT.into());
                log_runtime_memory("before-voice-record");
                match VoiceRecordingSession::start_with_recorded_at(
                    std::path::Path::new(VOICE_NOTES_ROOT),
                    recorded_at.clone(),
                ) {
                    Ok(created) => {
                        if let Err(error) = runtime.begin_voice_recording() {
                            let _ = created.cancel();
                            state.voice_notes.fail(format!("{error:#}"));
                            warn!("rustmix-wave=voice-record status=failed stage=audio-start error={error:#}");
                            return;
                        }
                        let file_name = created.file_name().to_string();
                        state
                            .voice_notes
                            .begin_recording(file_name.clone(), recorded_at.clone());
                        *session = Some(created);
                        log_runtime_memory("after-voice-record-start");
                        info!("rustmix-wave=voice-record status=starting file={} recorded-at={} sample-rate=16000 bits=16 channels=1 chunk-bytes={} capture=cooperative-bounded-i2s-rx mic-gain={}", file_name, recorded_at, VOICE_PCM_MONO_CHUNK_BYTES, state.voice_notes.mic_gain.marker());
                    }
                    Err(error) => {
                        state.voice_notes.fail(format!("{error:#}"));
                        warn!("rustmix-wave=voice-record status=failed stage=storage-start error={error:#}");
                    }
                }
            }
            VoiceNotesUiRequest::StopRecording => {
                let Some(active) = session.take() else {
                    return;
                };
                match active.finalize() {
                    Ok(entry) => {
                        if let Some(runtime) = audio_runtime.as_mut() {
                            let _ = runtime.finish_voice_recording();
                            state.update_audio_snapshot(runtime.snapshot());
                        }
                        info!("rustmix-wave=voice-record status=completed file={} recorded-at={} duration-seconds={} pcm-bytes={} wav-bytes={}", entry.file_name, entry.recorded_at, entry.duration_seconds, entry.pcm_bytes, entry.wav_bytes);
                        state.voice_notes.complete_recording(entry);
                        state.refresh_voice_notes_catalog();
                        refresh_voice_note_storage_available(state, mounted);
                        log_runtime_memory("after-voice-record-stop");
                    }
                    Err(error) => {
                        if let Some(runtime) = audio_runtime.as_mut() {
                            let _ = runtime.finish_voice_recording();
                            state.update_audio_snapshot(runtime.snapshot());
                        }
                        state.voice_notes.fail(format!("{error:#}"));
                        warn!("rustmix-wave=voice-record status=failed stage=finalize error={error:#}");
                        log_runtime_memory("after-voice-record-stop");
                    }
                }
            }
            VoiceNotesUiRequest::PauseRecording => {
                if session.is_some() {
                    state.voice_notes.pause_recording();
                    info!("rustmix-wave=voice-record status=paused");
                }
            }
            VoiceNotesUiRequest::ResumeRecording => {
                if session.is_some() {
                    state.voice_notes.resume_recording();
                    info!("rustmix-wave=voice-record status=resumed");
                }
            }
            VoiceNotesUiRequest::CancelRecording => {
                if let Some(active) = session.take() {
                    let _ = active.cancel();
                }
                if let Some(runtime) = audio_runtime.as_mut() {
                    let _ = runtime.finish_voice_recording();
                    state.update_audio_snapshot(runtime.snapshot());
                }
                state.voice_notes.cancel_recording();
                refresh_voice_note_storage_available(state, mounted);
                info!("rustmix-wave=voice-record status=cancelled");
            }
            VoiceNotesUiRequest::StartPlayback => {
                if !mounted {
                    state.voice_notes.fail("SD card unavailable");
                    warn!("rustmix-wave=voice-note-playback status=rejected reason=sd-unavailable");
                    return;
                }
                if session.is_some() {
                    state.voice_notes.fail("Stop recording before playback");
                    warn!("rustmix-wave=voice-note-playback status=rejected reason=voice-recording-active");
                    return;
                }
                if state.wifi_transfer.is_active() {
                    state
                        .voice_notes
                        .fail("Stop Wi-Fi Transfer before playback");
                    warn!("rustmix-wave=voice-note-playback status=rejected reason=wifi-transfer-active");
                    return;
                }
                if state.alarms.active.is_some() {
                    state.voice_notes.fail("Alarm active");
                    warn!("rustmix-wave=voice-note-playback status=rejected reason=active-alarm");
                    return;
                }
                let Some(file_name) = state
                    .voice_notes
                    .selected_note()
                    .map(|note| note.file_name.clone())
                else {
                    state.voice_notes.fail("No voice note selected");
                    warn!("rustmix-wave=voice-note-playback status=rejected reason=no-selection");
                    return;
                };
                if audio_runtime.is_none() {
                    state.voice_notes.fail("Speaker unavailable");
                    warn!(
                        "rustmix-wave=voice-note-playback status=rejected reason=audio-unavailable"
                    );
                    return;
                }
                if playback.is_some() {
                    stop_voice_note_playback(playback, audio_runtime, state, "replace-selection");
                }
                match VoicePlaybackSession::open(std::path::Path::new(VOICE_NOTES_ROOT), &file_name)
                {
                    Ok(created) => {
                        let total_pcm_bytes = created.total_pcm_bytes();
                        let runtime = audio_runtime
                            .as_mut()
                            .expect("audio runtime checked before playback start");
                        if let Err(error) = runtime.begin_voice_note_playback() {
                            runtime.record_failure(format!(
                                "Voice-note playback start failed: {error:#}"
                            ));
                            state.update_audio_snapshot(runtime.snapshot());
                            state.voice_notes.fail(format!("{error:#}"));
                            warn!("rustmix-wave=voice-note-playback status=failed stage=audio-start file={file_name} error={error:#}");
                            return;
                        }
                        state
                            .voice_notes
                            .begin_playback(file_name.clone(), total_pcm_bytes);
                        *playback = Some(created);
                        state.update_audio_snapshot(runtime.snapshot());
                        log_audio_snapshot(&state.audio);
                        info!("rustmix-wave=voice-note-playback status=starting file={file_name} pcm-bytes={total_pcm_bytes} sample-rate=16000 bits=16 source-channels=1 output-channels=2 chunk-bytes={VOICE_PCM_MONO_CHUNK_BYTES} volume={}", state.audio.volume_percent);
                    }
                    Err(error) => {
                        state.voice_notes.fail(format!("{error:#}"));
                        warn!("rustmix-wave=voice-note-playback status=failed stage=storage-open file={file_name} error={error:#}");
                    }
                }
            }
            VoiceNotesUiRequest::StopPlayback => {
                stop_voice_note_playback(playback, audio_runtime, state, "ui-stop");
            }
            VoiceNotesUiRequest::PersistMicGain(mic_gain) => {
                let preferences = VoiceNotesPreferences { mic_gain };
                match save_voice_notes_preferences(std::path::Path::new(VOICE_NOTES_ROOT), preferences) {
                    Ok(()) => info!("rustmix-wave=voice-note-settings-write status=completed mic-gain={} path={VOICE_NOTES_ROOT}/SETTINGS.TXT", mic_gain.marker()),
                    Err(error) => {
                        state.voice_notes.fail(format!("{error:#}"));
                        warn!("rustmix-wave=voice-note-settings-write status=failed mic-gain={} error={error:#}", mic_gain.marker());
                    }
                }
            }
            VoiceNotesUiRequest::SaveEditedTitle { file_name, title } => {
                match save_voice_note_title(
                    std::path::Path::new(VOICE_NOTES_ROOT),
                    &file_name,
                    &title,
                ) {
                    Ok(()) => {
                        state.refresh_voice_notes_catalog();
                        info!("rustmix-wave=voice-note-title-write status=completed file={file_name} title={title}");
                    }
                    Err(error) => {
                        state.voice_notes.fail(format!("{error:#}"));
                        warn!("rustmix-wave=voice-note-title-write status=failed file={file_name} error={error:#}");
                    }
                }
            }
            VoiceNotesUiRequest::ExportSelected => {
                if !mounted {
                    state.voice_notes.fail("SD card unavailable");
                    warn!("rustmix-wave=voice-note-export status=rejected reason=sd-unavailable");
                    return;
                }
                if session.is_some() {
                    state.voice_notes.fail("Stop recording before export");
                    warn!("rustmix-wave=voice-note-export status=rejected reason=voice-recording-active");
                    return;
                }
                if playback.is_some() {
                    stop_voice_note_playback(playback, audio_runtime, state, "export-note");
                }
                let Some(file_name) = state
                    .voice_notes
                    .selected_note()
                    .map(|note| note.file_name.clone())
                else {
                    state.voice_notes.fail("No voice note selected");
                    return;
                };
                state.voice_notes.mark_export_requested(file_name.clone());
                state.request_wifi_transfer_start();
                info!("rustmix-wave=voice-note-export status=requested file={file_name} portal-path=VOICE/{file_name}");
            }
            VoiceNotesUiRequest::DeleteSelected => {
                if playback.is_some() {
                    stop_voice_note_playback(playback, audio_runtime, state, "delete-note");
                }
                let selected = state
                    .voice_notes
                    .selected_note()
                    .map(|note| note.file_name.clone());
                if let Some(file_name) = selected {
                    match delete_voice_note(std::path::Path::new(VOICE_NOTES_ROOT), &file_name) {
                        Ok(()) => {
                            state.voice_notes.remove_selected_note();
                            state.refresh_voice_notes_catalog();
                            refresh_voice_note_storage_available(state, mounted);
                            state.router.navigate_to(ScreenRoute::VoiceNotes);
                            info!(
                                "rustmix-wave=voice-note-delete status=completed file={file_name} confirmation=accepted"
                            );
                        }
                        Err(error) => {
                            state.voice_notes.fail(format!("{error:#}"));
                            warn!("rustmix-wave=voice-note-delete status=failed file={file_name} error={error:#}");
                        }
                    }
                }
            }
            VoiceNotesUiRequest::RefreshCatalog => {
                state.refresh_voice_notes_catalog();
                refresh_voice_note_storage_available(state, mounted);
            }
        }
    }

    fn sync_alarm_hardware<I2C>(
        engine: &mut AlarmEngine,
        board_services: &mut BoardServices<I2C>,
        regional: RegionalPreferences,
    ) where
        I2C: embedded_hal::i2c::I2c,
        I2C::Error: core::fmt::Debug,
    {
        if let Some(next) = engine.next_occurrence() {
            let stored = regional.local_to_rtc(next.local);
            match board_services.program_rtc_alarm(stored) {
                Ok(()) => {
                    engine.set_hardware_programmed(true);
                    info!(
                        "rustmix-wave=rtc-alarm-program status=armed local={} stored={} snooze={}",
                        next.local.date_time(),
                        stored.date_time(),
                        next.snooze
                    );
                }
                Err(error) => {
                    engine.set_hardware_programmed(false);
                    warn!("rustmix-wave=rtc-alarm-program status=failed error={error:#}");
                }
            }
        } else {
            match board_services.disable_rtc_alarm() {
                Ok(()) => {
                    engine.set_hardware_programmed(false);
                    info!("rustmix-wave=rtc-alarm-program status=idle");
                }
                Err(error) => warn!("rustmix-wave=rtc-alarm-disable status=failed error={error:#}"),
            }
        }
    }

    fn fallback_local_time() -> RtcDateTime {
        RtcDateTime {
            year: 2000,
            month: 1,
            day: 1,
            weekday: 6,
            hour: 0,
            minute: 0,
            second: 0,
        }
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

    /// Best-effort redraw of the retained sleep image plus a "RIATTIVAZIONE"
    /// box, called as early as possible after a real hardware deep-sleep
    /// reboot (SELECT/GPIO5). The e-paper image itself needs no redraw to
    /// "stay" visible — it is still physically on the glass with no power
    /// applied — this only reloads the same frame from the wake marker and
    /// adds the wake-in-progress box on top of it before the rest of boot
    /// (SD/network/sensor init) has run.
    fn draw_deep_sleep_wake_overlay<SPI, DC, RST, CS, BUSY, DELAY, POWER>(
        panel: &mut Epaper397<SPI, DC, RST, CS, BUSY, DELAY, POWER>,
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
        panel.initialize()?;
        let catalog = SleepImageCatalog::new(SLEEP_IMAGE_DIRECTORY);
        let mut frame = catalog.load_wake_marker_frame();
        sleep_wake_overlay::draw_wake_overlay(&mut frame)?;
        panel.show_partial_fullscreen(frame.as_bytes())
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
        let coordinator_request = match request {
            RefreshRequest::Normal => PanelRefreshRequest::Normal,
            RefreshRequest::ForceGlobalAfterWake => PanelRefreshRequest::AfterWake,
            RefreshRequest::ForceGlobalManual => PanelRefreshRequest::ManualGhostCleanup,
            RefreshRequest::ForceGlobalSafetyFallback => PanelRefreshRequest::SafetyFallback,
        };
        let plan = coordinator.plan(coordinator_request);
        sync_panel_refresh_diagnostics(state, coordinator);
        render_current_screen(frame, state)?;

        match plan {
            PanelRefreshPlan::GlobalBase { reason } => {
                panel.show_base(frame.as_bytes())?;
                info!(
                    "rustmix-wave=panel-refresh plan=global-base reason={} transport=global-base",
                    reason.marker()
                );
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
                    PanelGlobalReason::InitialBoot | PanelGlobalReason::SleepImage => {}
                }
            }
            PanelRefreshPlan::PartialFullscreen { partial_count } => {
                panel.show_partial_fullscreen(frame.as_bytes())?;
                info!(
                    "rustmix-wave=panel-refresh plan=partial-fullscreen reason=normal partial-count={partial_count} partial-limit={PANEL_PARTIAL_REFRESH_LIMIT} transport=existing-fullscreen-partial"
                );
            }
        }
        Ok(())
    }

    fn sync_panel_refresh_diagnostics(state: &mut AppState, coordinator: &PanelRefreshCoordinator) {
        state.partial_refreshes = coordinator.partial_count();
    }

    fn log_lua_runtime_events(state: &mut AppState) {
        for line in state.take_lua_runtime_diagnostics() {
            info!("{line}");
        }
    }

    fn log_reader_persistence_event(state: &mut AppState) {
        if let Some(event) = state.reader.take_persistence_event() {
            info!("rustmix-wave=reader-persistence {event}");
        }
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
        let imu = snapshot.imu.map_or_else(
            || "unavailable".into(),
            |reading| {
                format!(
                    "motion={}mg axis={} acc=[{}] gyro=[{}]",
                    reading.motion_magnitude_mg,
                    reading.dominant_axis.label(),
                    reading.acceleration_mg_tenths.compact_label(),
                    reading.gyroscope_dps_tenths.compact_label()
                )
            },
        );
        info!(
            "rustmix-wave=sample-board-snapshot time={} timezone={} battery={} temperature={} humidity={} imu={}",
            snapshot.time_label(regional),
            regional.timezone_label_for_rtc(snapshot.rtc),
            snapshot.battery_label(),
            snapshot.temperature_label(regional.temperature_unit),
            snapshot.humidity_label(),
            imu
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

    fn log_alarm_snapshot(snapshot: &AlarmSnapshot) {
        info!(
            "rustmix-wave=alarm-snapshot schedules={} active={} selected={} next={} snooze-minutes={} hardware-programmed={} error={}",
            snapshot.alarms.len(),
            snapshot.active.as_ref().map_or("none", |active| active.name.as_str()),
            snapshot.selected,
            snapshot.next_label(),
            snapshot.snooze_minutes,
            snapshot.hardware_programmed,
            snapshot.error.as_deref().unwrap_or("none")
        );
    }

    fn log_weather_snapshot(snapshot: &WeatherSnapshot) {
        info!(
            "rustmix-wave=weather-snapshot state={} provider={} location={} timezone={} current={} forecast-days={} last-success={} error={}",
            snapshot.state.label(),
            snapshot.provider,
            snapshot.location,
            snapshot.provider_timezone,
            snapshot.current_summary(),
            snapshot.forecast.len(),
            snapshot.last_success_label(),
            snapshot.error.as_deref().unwrap_or("none")
        );
    }

    /// FreeRTOS-backed delays are sufficient for the millisecond timings used
    /// by the panel and sample-board reference sequences. Round sub-millisecond
    /// requests up so short sensor waits remain conservative.
    #[derive(Clone, Copy, Debug, Default)]
    struct FreeRtosDelay;

    impl DelayNs for FreeRtosDelay {
        fn delay_ns(&mut self, nanoseconds: u32) {
            let milliseconds = nanoseconds.saturating_add(999_999) / 1_000_000;
            if milliseconds > 0 {
                FreeRtos::delay_ms(milliseconds);
            }
        }
    }
}

#[cfg(target_os = "espidf")]
fn main() -> anyhow::Result<()> {
    firmware::run()
}

#[cfg(not(target_os = "espidf"))]
fn main() {
    println!("Build this firmware for xtensa-esp32s3-espidf. See README.md.");
}
