//! Board-service facade for the RTC and battery status, plus the boot-time
//! power-down of the two sensors nothing reads (SHTC3, QMI8658).

use core::fmt::Debug;

use embedded_hal::{delay::DelayNs, i2c::I2c};
use log::warn;

use crate::{
    environment::Shtc3,
    imu::Qmi8658,
    ntp::rtc_storage_wall_clock_from_utc,
    power::{Axp2101, BatteryPercentFilter, PowerSnapshot},
    power_key::PowerKeyEvent,
    regional::RegionalPreferences,
    rtc::{Pcf85063, RtcDateTime},
    shared_i2c::SharedI2cBus,
};

/// Hardware-independent snapshot consumed by product screens.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BoardSnapshot {
    pub rtc: Option<RtcDateTime>,
    pub power: Option<PowerSnapshot>,
    pub rtc_clock_integrity_was_lost: bool,
}

impl BoardSnapshot {
    #[must_use]
    pub fn time_label(self, regional: RegionalPreferences) -> String {
        self.rtc.map_or_else(
            || "--:--".into(),
            |rtc| regional.localize_rtc(rtc).time_hm(),
        )
    }

    #[must_use]
    pub fn date_time_label(self, regional: RegionalPreferences) -> String {
        self.rtc.map_or_else(
            || "RTC unavailable".into(),
            |rtc| regional.localize_rtc(rtc).date_time(),
        )
    }

    #[must_use]
    pub fn battery_label(self) -> String {
        self.power
            .and_then(|snapshot| snapshot.battery_percent)
            .map_or_else(|| "BAT --".into(), |percent| format!("BAT {percent}%"))
    }
}

/// Startup availability report. Missing optional services do not block display.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BoardInitReport {
    pub rtc_available: bool,
    pub power_monitoring_available: bool,
    pub rtc_clock_integrity_was_lost: bool,
    /// SHTC3 ID read while putting the sensor to sleep, when it answered.
    pub environment_sensor_id: Option<u16>,
    /// QMI8658 address and revision found while powering it down.
    pub imu_address: Option<u8>,
    pub imu_revision: Option<u8>,
}

/// Owns independent protocol drivers backed by one cloneable I2C adapter.
pub struct BoardServices<I2C> {
    rtc: Pcf85063<SharedI2cBus<I2C>>,
    environment: Shtc3<SharedI2cBus<I2C>>,
    power: Axp2101<SharedI2cBus<I2C>>,
    imu: Qmi8658<SharedI2cBus<I2C>>,
    init_report: BoardInitReport,
    /// Smooths out lone AXP2101 fuel-gauge glitches (see
    /// [`BatteryPercentFilter`]) across every [`Self::sample_power`] call,
    /// including the frequent per-button-press [`Self::read_light_snapshot`].
    battery_percent_filter: BatteryPercentFilter,
}

impl<I2C> BoardServices<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    #[must_use]
    pub fn new(bus: SharedI2cBus<I2C>) -> Self {
        Self {
            rtc: Pcf85063::new(bus.clone()),
            environment: Shtc3::new(bus.clone()),
            power: Axp2101::new(bus.clone()),
            imu: Qmi8658::new(bus),
            init_report: BoardInitReport::default(),
            battery_percent_filter: BatteryPercentFilter::default(),
        }
    }

    /// Initialize the RTC and battery monitoring and power the unused
    /// sensors down, each independently so a missing chip never prevents the
    /// e-paper shell from booting.
    pub fn initialize<D: DelayNs>(&mut self, delay: &mut D) -> BoardInitReport {
        let rtc_span = crate::boot_profile::span("board-rtc-init");
        match self.rtc.initialize() {
            Ok(report) => {
                self.init_report.rtc_available = true;
                self.init_report.rtc_clock_integrity_was_lost = report.clock_integrity_was_lost;
            }
            Err(error) => warn!("board-services: RTC init unavailable: {error:#}"),
        }

        rtc_span.end();
        let environment_span = crate::boot_profile::span("board-shtc3-sleep");
        match self.environment.put_to_sleep(delay) {
            Ok(id) => self.init_report.environment_sensor_id = Some(id),
            Err(error) => warn!("board-services: SHTC3 sleep unavailable: {error:#}"),
        }

        environment_span.end();
        let power_span = crate::boot_profile::span("board-pmic-monitoring-init");
        match self.power.initialize_sample_monitoring() {
            Ok(()) => self.init_report.power_monitoring_available = true,
            Err(error) => warn!("board-services: AXP2101 monitoring unavailable: {error:#}"),
        }

        power_span.end();
        let imu_span = crate::boot_profile::span("board-imu-power-down");
        match self.imu.power_down() {
            Ok(report) => {
                self.init_report.imu_address = Some(report.address);
                self.init_report.imu_revision = Some(report.revision);
            }
            Err(error) => warn!("board-services: QMI8658 power-down unavailable: {error:#}"),
        }
        imu_span.end();

        self.init_report
    }

    /// Persist one validated UTC SNTP result into the PCF85063 wall-clock basis
    /// retained from the uploaded sample app.
    pub fn sync_rtc_from_utc(&mut self, utc: RtcDateTime) -> anyhow::Result<RtcDateTime> {
        let stored = rtc_storage_wall_clock_from_utc(utc);
        self.rtc.write_datetime(stored)?;
        self.init_report.rtc_available = true;
        self.init_report.rtc_clock_integrity_was_lost = false;
        Ok(stored)
    }

    /// Read only the RTC wall clock without generating a full snapshot.
    pub fn read_rtc(&mut self) -> anyhow::Result<RtcDateTime> {
        self.rtc.read_datetime()
    }

    /// Persist a manually edited wall-clock value from the Clock screen's Set
    /// Date & Time editor. The caller has already converted the local edit
    /// into the retained RTC storage basis.
    pub fn write_rtc_datetime(&mut self, stored: RtcDateTime) -> anyhow::Result<()> {
        self.rtc.write_datetime(stored)?;
        self.init_report.rtc_available = true;
        self.init_report.rtc_clock_integrity_was_lost = false;
        Ok(())
    }

    /// Disable the PCF85063 hardware alarm slot and clear its flag, releasing
    /// the interrupt line on GPIO45 (a boot strapping pin).
    pub fn disable_rtc_alarm(&mut self) -> anyhow::Result<()> {
        self.rtc.disable_alarm()
    }

    /// Enable PMIC short-sleep and long-menu Power-key event polling.
    pub fn initialize_power_key_events(&mut self) -> anyhow::Result<()> {
        self.power.initialize_power_key_events()
    }

    /// Return and clear one PMIC short power-key event when present.
    pub fn take_power_key_event(&mut self) -> anyhow::Result<Option<PowerKeyEvent>> {
        self.power.take_power_key_event()
    }

    fn sample_rtc(&mut self) -> Option<RtcDateTime> {
        match self.rtc.read_datetime() {
            Ok(value) => Some(value),
            Err(error) => {
                warn!("board-services: RTC read unavailable: {error:#}");
                None
            }
        }
    }

    fn sample_power(&mut self) -> Option<PowerSnapshot> {
        match self.power.read_power_snapshot() {
            Ok(mut value) => {
                value.battery_percent = value
                    .battery_percent
                    .map(|raw| self.battery_percent_filter.apply(raw));
                Some(value)
            }
            Err(error) => {
                warn!("board-services: AXP2101 status unavailable: {error:#}");
                None
            }
        }
    }

    /// Per-interaction refresh: RTC (header clock, plus the screens that
    /// timestamp "now") and PMIC status (header battery glyph).
    pub fn read_light_snapshot(&mut self) -> BoardSnapshot {
        BoardSnapshot {
            rtc: self.sample_rtc(),
            power: self.sample_power(),
            rtc_clock_integrity_was_lost: self.init_report.rtc_clock_integrity_was_lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BoardSnapshot;
    use crate::{power::PowerSnapshot, regional::RegionalPreferences, rtc::RtcDateTime};

    #[test]
    fn unavailable_snapshot_renders_placeholders() {
        let snapshot = BoardSnapshot::default();
        assert_eq!(snapshot.time_label(RegionalPreferences::default()), "--:--");
        assert_eq!(snapshot.battery_label(), "BAT --");
    }

    #[test]
    fn available_snapshot_renders_sample_status() {
        let snapshot = BoardSnapshot {
            rtc: Some(RtcDateTime {
                year: 2026,
                month: 6,
                day: 3,
                weekday: 3,
                hour: 14,
                minute: 5,
                second: 8,
            }),
            power: Some(PowerSnapshot {
                battery_percent: Some(82),
                battery_voltage_mv: Some(3980),
                vbus_present: true,
                charging: true,
            }),
            ..BoardSnapshot::default()
        };
        assert_eq!(snapshot.time_label(RegionalPreferences::default()), "02:05");
        assert_eq!(snapshot.battery_label(), "BAT 82%");
    }
}
