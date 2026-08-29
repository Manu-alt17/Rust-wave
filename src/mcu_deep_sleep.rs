//! Real ESP32-S3 hardware deep sleep entry and GPIO wakeup.
//!
//! GPIO45, the PCF85063 RTC alarm interrupt line documented in
//! [`crate::rtc_alarm_interrupt`], sits outside the ESP32-S3's RTC IO range
//! (GPIO0-21) and cannot be configured as an `ext1` deep-sleep wakeup source.
//! Real hardware deep sleep therefore wakes on the rotary SELECT key (GPIO5)
//! only; a PCF85063 alarm cannot wake the board once it is in hardware deep
//! sleep. This is an accepted product trade-off, not an oversight: the
//! long-press sleep path always arms real deep sleep, even while an alarm is
//! enabled.
//!
//! Deep sleep powers down the ESP32-S3 digital domain and its RAM. Waking up
//! from it is a full reboot through the bootloader, not a resume:
//! `firmware::run` starts over from the top and the product naturally lands
//! back on the Home route, the same as any other cold boot.

/// Rotary SELECT key GPIO used as the sole deep sleep wakeup source. RTC IO
/// capable (within the ESP32-S3's GPIO0-21 range), active low with an
/// internal pull-up, matching [`crate::buttons::ButtonEvent::Select`].
pub const DEEP_SLEEP_WAKE_GPIO: u8 = 5;

/// Dynamic-frequency-scaling ceiling/floor shared by every `esp_pm_configure`
/// call in this firmware (the startup call in `firmware::run` and the
/// light-sleep toggle in [`espidf::set_light_sleep_enabled`]), so the two
/// call sites can never drift apart on frequency while only differing on
/// `light_sleep_enable`.
pub const CPU_MAX_FREQ_MHZ: core::ffi::c_int = 160;
/// See [`CPU_MAX_FREQ_MHZ`].
pub const CPU_MIN_FREQ_MHZ: core::ffi::c_int = 40;

/// Product-facing classification of why the current boot happened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootCause {
    /// Cold boot, external reset, brown-out, or any cause other than the
    /// tracked deep-sleep wakeup pin.
    PowerOnOrReset,
    /// The rotary SELECT key (GPIO5) woke the board from hardware deep sleep.
    DeepSleepGpioWake,
}

impl BootCause {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::PowerOnOrReset => "power-on-or-reset",
            Self::DeepSleepGpioWake => "deep-sleep-gpio-wake",
        }
    }

    /// Classify a raw `esp_sleep_wakeup_cause_t` value. Only
    /// `ESP_SLEEP_WAKEUP_EXT1` (value 3) is the accepted deep-sleep wake
    /// path armed by [`espidf::enter`]; every other cause, including
    /// `ESP_SLEEP_WAKEUP_UNDEFINED` (a normal power-on or external reset),
    /// resolves to [`BootCause::PowerOnOrReset`].
    #[must_use]
    pub const fn from_raw_wakeup_cause(cause: u32) -> Self {
        const ESP_SLEEP_WAKEUP_EXT1: u32 = 3;
        if cause == ESP_SLEEP_WAKEUP_EXT1 {
            Self::DeepSleepGpioWake
        } else {
            Self::PowerOnOrReset
        }
    }
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use anyhow::{bail, Result};
    use esp_idf_svc::sys::{
        esp_deep_sleep_start, esp_pm_config_t, esp_pm_configure, esp_sleep_enable_ext1_wakeup_io,
        esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW, esp_sleep_get_wakeup_cause,
        rtc_gpio_deinit, rtc_gpio_pulldown_dis, rtc_gpio_pullup_en, ESP_OK,
    };
    use super::{BootCause, CPU_MAX_FREQ_MHZ, CPU_MIN_FREQ_MHZ, DEEP_SLEEP_WAKE_GPIO};

    /// Toggle ESP-IDF's automatic light sleep, keeping the same DFS
    /// frequency ceiling/floor ([`CPU_MAX_FREQ_MHZ`]/[`CPU_MIN_FREQ_MHZ`])
    /// used throughout the app. `firmware::run` enables it once at startup
    /// (`true`) for the battery savings automatic light sleep gives the main
    /// loop's idle gaps during ordinary active use. [`enter`] disables it
    /// (`false`) immediately before arming the `ext1` GPIO5 wakeup and
    /// calling `esp_deep_sleep_start`: automatic light sleep left enabled at
    /// that exact moment was observed to corrupt the following real deep
    /// sleep, waking the board again moments later instead of staying off
    /// until the next GPIO5 press. Callers that see [`enter`] fail (and so
    /// fall back to the software-only sleep-image loop, still running) must
    /// re-enable it with `true` so the running loop keeps its idle savings.
    pub fn set_light_sleep_enabled(enabled: bool) -> Result<()> {
        let pm_config = esp_pm_config_t {
            max_freq_mhz: CPU_MAX_FREQ_MHZ,
            min_freq_mhz: CPU_MIN_FREQ_MHZ,
            light_sleep_enable: enabled,
        };
        let status =
            unsafe { esp_pm_configure((&raw const pm_config).cast::<core::ffi::c_void>()) };
        if status != ESP_OK {
            bail!("esp_pm_configure(light_sleep_enable={enabled}) failed: {status}");
        }
        Ok(())
    }

    /// Read and classify the ESP-IDF wakeup cause for the current boot.
    #[must_use]
    pub fn boot_cause() -> BootCause {
        BootCause::from_raw_wakeup_cause(unsafe { esp_sleep_get_wakeup_cause() })
    }

    /// Release GPIO5 from the RTC IO domain that [`enter`] switches it into
    /// before the previous sleep cycle, so it can be reconfigured as a normal
    /// digital `PinDriver` input again. Safe to call on every boot, including
    /// a cold boot where GPIO5 was never RTC-owned.
    pub fn release_wake_pin() -> Result<()> {
        let status = unsafe { rtc_gpio_deinit(i32::from(DEEP_SLEEP_WAKE_GPIO)) };
        if status != ESP_OK {
            bail!("rtc_gpio_deinit(GPIO{DEEP_SLEEP_WAKE_GPIO}) failed: {status}");
        }
        Ok(())
    }

    /// Arm GPIO5 (SELECT) as an `ext1` wakeup source and enter real hardware
    /// deep sleep. Returns an error without sleeping if automatic light
    /// sleep could not be disabled first (see
    /// [`set_light_sleep_enabled`]) or if the wakeup source could not be
    /// armed, so the caller can fall back to the software-only sleep-image
    /// loop instead of stranding the board with no way to wake it -- and
    /// must then re-enable light sleep itself, since this call only ever
    /// turns it off.
    ///
    /// On success this call does not return: `esp_deep_sleep_start` powers
    /// the chip down immediately and the next code to run is the bootloader.
    pub fn enter() -> Result<()> {
        // Must happen before the GPIO5/ext1 arming below: see
        // `set_light_sleep_enabled`'s docs for why the two conflict.
        set_light_sleep_enabled(false)?;

        let gpio_num = i32::from(DEEP_SLEEP_WAKE_GPIO);

        let pullup_status = unsafe { rtc_gpio_pullup_en(gpio_num) };
        if pullup_status != ESP_OK {
            bail!("rtc_gpio_pullup_en(GPIO{DEEP_SLEEP_WAKE_GPIO}) failed: {pullup_status}");
        }
        let pulldown_status = unsafe { rtc_gpio_pulldown_dis(gpio_num) };
        if pulldown_status != ESP_OK {
            bail!("rtc_gpio_pulldown_dis(GPIO{DEEP_SLEEP_WAKE_GPIO}) failed: {pulldown_status}");
        }

        let wake_mask = 1_u64 << DEEP_SLEEP_WAKE_GPIO;
        let wakeup_status = unsafe {
            esp_sleep_enable_ext1_wakeup_io(
                wake_mask,
                esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW,
            )
        };
        if wakeup_status != ESP_OK {
            bail!(
                "esp_sleep_enable_ext1_wakeup_io(GPIO{DEEP_SLEEP_WAKE_GPIO}) failed: {wakeup_status}"
            );
        }

        unsafe { esp_deep_sleep_start() }
    }
}

#[cfg(test)]
mod tests {
    use super::BootCause;

    #[test]
    fn only_ext1_raw_cause_is_a_deep_sleep_gpio_wake() {
        assert_eq!(
            BootCause::from_raw_wakeup_cause(3),
            BootCause::DeepSleepGpioWake
        );
        assert_eq!(
            BootCause::from_raw_wakeup_cause(0),
            BootCause::PowerOnOrReset
        );
        assert_eq!(
            BootCause::from_raw_wakeup_cause(4),
            BootCause::PowerOnOrReset
        );
    }

    #[test]
    fn markers_are_stable_log_tokens() {
        assert_eq!(BootCause::PowerOnOrReset.marker(), "power-on-or-reset");
        assert_eq!(
            BootCause::DeepSleepGpioWake.marker(),
            "deep-sleep-gpio-wake"
        );
    }
}
