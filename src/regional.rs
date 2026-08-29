//! Regional presentation policy for RTC and sensor values.
//!
//! The PCF85063 stores wall-clock fields without a timezone identifier. The
//! uploaded Waveshare sample uses UTC+08:00 as its RTC basis. Keep that basis
//! explicit and apply a timezone profile only at the presentation boundary.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::{calendar::days_in_month, rtc::RtcDateTime};

/// On-device timezone selection, persisted independently of Wi-Fi
/// provisioning so choosing a timezone from the Clock screen's "Set date &
/// time" editor survives a reboot (including a real deep-sleep wake, which is
/// a full reboot) even when the device has no `WIFI.TXT` on the SD card.
pub const CLOCK_CONFIG_PATH: &str = "/sdcard/RUSTMIX/CLOCK.TXT";

/// RTC wall-clock basis inherited from the uploaded sample application.
pub const SAMPLE_RTC_STORAGE_UTC_OFFSET_MINUTES: i16 = 8 * 60;
/// Daylight offset retained as the fallback when a date is unavailable.
pub const DEFAULT_DISPLAY_UTC_OFFSET_MINUTES: i16 = -4 * 60;
/// Product-facing default timezone profile.
pub const DEFAULT_TIMEZONE_NAME: &str = "America/New_York";
/// Daylight abbreviation retained as the fallback when a date is unavailable.
pub const DEFAULT_TIMEZONE_ABBREVIATION: &str = "EDT";

/// Temperature unit used by product-facing screens.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TemperatureUnit {
    Celsius,
    #[default]
    Fahrenheit,
}

impl TemperatureUnit {
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Celsius => " C",
            Self::Fahrenheit => " F",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }
}

/// Supported timezone profiles for the first Wi-Fi/NTP milestone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeZoneProfile {
    #[default]
    AmericaNewYork,
    Utc,
    EuropeRome,
}

impl TimeZoneProfile {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "America/New_York" => Ok(Self::AmericaNewYork),
            "UTC" => Ok(Self::Utc),
            "Europe/Rome" => Ok(Self::EuropeRome),
            _ => bail!("unsupported timezone profile {value:?}"),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AmericaNewYork => "America/New_York",
            Self::Utc => "UTC",
            Self::EuropeRome => "Europe/Rome",
        }
    }

    #[must_use]
    pub fn offset_minutes_for_utc(self, utc: RtcDateTime) -> i16 {
        match self {
            Self::AmericaNewYork if is_new_york_dst(utc) => -4 * 60,
            Self::AmericaNewYork => -5 * 60,
            Self::Utc => 0,
            Self::EuropeRome if is_eu_dst(utc) => 2 * 60,
            Self::EuropeRome => 60,
        }
    }

    #[must_use]
    pub fn abbreviation_for_utc(self, utc: RtcDateTime) -> &'static str {
        match self {
            Self::AmericaNewYork if is_new_york_dst(utc) => "EDT",
            Self::AmericaNewYork => "EST",
            Self::Utc => "UTC",
            Self::EuropeRome if is_eu_dst(utc) => "CEST",
            Self::EuropeRome => "CET",
        }
    }

    /// Convert a UTC instant into this timezone's local wall-clock fields.
    #[must_use]
    pub fn localize_utc(self, utc: RtcDateTime) -> RtcDateTime {
        utc.shift_minutes(i32::from(self.offset_minutes_for_utc(utc)))
    }

    /// Resolve one local wall-clock reading into UTC under this timezone.
    /// For the repeated DST fall-back hour, prefer the earlier valid UTC
    /// candidate so the result stays deterministic without persisting a
    /// separate DST flag in removable storage.
    #[must_use]
    pub fn local_to_utc(self, local: RtcDateTime) -> RtcDateTime {
        match self {
            Self::Utc => local,
            Self::AmericaNewYork => {
                let daylight = local.shift_minutes(4 * 60);
                let standard = local.shift_minutes(5 * 60);
                let daylight_valid = self.offset_minutes_for_utc(daylight) == -4 * 60;
                let standard_valid = self.offset_minutes_for_utc(standard) == -5 * 60;
                match (daylight_valid, standard_valid) {
                    (true, _) => daylight,
                    (false, true) => standard,
                    (false, false) => standard,
                }
            }
            Self::EuropeRome => {
                let daylight = local.shift_minutes(-2 * 60);
                let standard = local.shift_minutes(-60);
                let daylight_valid = self.offset_minutes_for_utc(daylight) == 2 * 60;
                let standard_valid = self.offset_minutes_for_utc(standard) == 60;
                match (daylight_valid, standard_valid) {
                    (true, _) => daylight,
                    (false, true) => standard,
                    (false, false) => standard,
                }
            }
        }
    }
}

/// Regional presentation settings owned by UI state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionalPreferences {
    pub rtc_storage_utc_offset_minutes: i16,
    pub timezone: TimeZoneProfile,
    pub temperature_unit: TemperatureUnit,
}

impl Default for RegionalPreferences {
    fn default() -> Self {
        Self {
            rtc_storage_utc_offset_minutes: SAMPLE_RTC_STORAGE_UTC_OFFSET_MINUTES,
            timezone: TimeZoneProfile::default(),
            temperature_unit: TemperatureUnit::default(),
        }
    }
}

impl RegionalPreferences {
    /// Apply a validated boot-time timezone profile from removable storage.
    pub fn with_timezone_name(mut self, timezone: &str) -> Result<Self> {
        self.timezone = TimeZoneProfile::parse(timezone)?;
        Ok(self)
    }

    #[must_use]
    pub const fn timezone_name(self) -> &'static str {
        self.timezone.name()
    }

    /// Convert the stored RTC wall clock into UTC.
    #[must_use]
    pub fn rtc_to_utc(self, rtc: RtcDateTime) -> RtcDateTime {
        rtc.shift_minutes(-i32::from(self.rtc_storage_utc_offset_minutes))
    }

    /// Convert stored RTC fields into the selected timezone with automatic DST.
    #[must_use]
    pub fn localize_rtc(self, rtc: RtcDateTime) -> RtcDateTime {
        self.timezone.localize_utc(self.rtc_to_utc(rtc))
    }

    /// Convert a local schedule value into the retained RTC storage basis
    /// under the currently selected timezone.
    #[must_use]
    pub fn local_to_rtc(self, local: RtcDateTime) -> RtcDateTime {
        self.local_to_rtc_for_zone(self.timezone, local)
    }

    /// Convert a local schedule value into the retained RTC storage basis
    /// under an explicitly chosen timezone, independent of the currently
    /// selected one. Used by the Clock screen's "Set date & time" editor so
    /// picking a new timezone there can be resolved before it is committed
    /// to `self.timezone`.
    #[must_use]
    pub fn local_to_rtc_for_zone(self, zone: TimeZoneProfile, local: RtcDateTime) -> RtcDateTime {
        zone.local_to_utc(local)
            .shift_minutes(i32::from(self.rtc_storage_utc_offset_minutes))
    }

    /// Render the selected timezone using the RTC date when available.
    #[must_use]
    pub fn timezone_label_for_rtc(self, rtc: Option<RtcDateTime>) -> String {
        if let Some(rtc) = rtc {
            let utc = self.rtc_to_utc(rtc);
            return format!(
                "{} {}",
                self.timezone.abbreviation_for_utc(utc),
                format_utc_offset(self.timezone.offset_minutes_for_utc(utc))
            );
        }
        match self.timezone {
            TimeZoneProfile::AmericaNewYork => format!(
                "{} {}",
                DEFAULT_TIMEZONE_ABBREVIATION,
                format_utc_offset(DEFAULT_DISPLAY_UTC_OFFSET_MINUTES)
            ),
            TimeZoneProfile::Utc => "UTC UTC+00:00".into(),
            TimeZoneProfile::EuropeRome => "CET UTC+01:00".into(),
        }
    }

    /// Compatibility label for startup logs before an RTC snapshot exists.
    #[must_use]
    pub fn timezone_label(self) -> String {
        self.timezone_label_for_rtc(None)
    }

    #[must_use]
    pub fn rtc_storage_label(self) -> String {
        format_utc_offset(self.rtc_storage_utc_offset_minutes)
    }
}

/// Load a timezone selection previously saved by [`save_timezone_name`].
/// Independent of `WIFI.TXT`/`NetworkConfig`: this is the durable home for a
/// timezone chosen on-device, regardless of whether Wi-Fi has ever been
/// configured.
pub fn load_timezone_name(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("timezone="))
        .ok_or_else(|| anyhow::anyhow!("missing timezone key in {}", path.display()))?
        .trim();
    TimeZoneProfile::parse(value)
        .with_context(|| format!("invalid timezone in {}", path.display()))?;
    Ok(value.to_string())
}

/// Persist a timezone selection to `path`, creating the parent directory if
/// needed. Called every time the Clock screen's "Set date & time" editor
/// commits a timezone, so the choice survives the next boot on its own,
/// without requiring `WIFI.TXT` to already exist.
pub fn save_timezone_name(path: impl AsRef<Path>, timezone: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, format!("timezone={timezone}\n"))
        .with_context(|| format!("write {}", path.display()))
}

/// Render an offset using the user-facing `UTC+HH:MM` form.
#[must_use]
pub fn format_utc_offset(minutes: i16) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let magnitude = i32::from(minutes).abs();
    format!("UTC{sign}{:02}:{:02}", magnitude / 60, magnitude % 60)
}

fn is_new_york_dst(utc: RtcDateTime) -> bool {
    let start_day = nth_sunday_of_month(utc.year, 3, 2);
    let end_day = nth_sunday_of_month(utc.year, 11, 1);
    let start = date_time_key(utc.year, 3, start_day, 7, 0, 0);
    let end = date_time_key(utc.year, 11, end_day, 6, 0, 0);
    let current = date_time_key(
        utc.year, utc.month, utc.day, utc.hour, utc.minute, utc.second,
    );
    current >= start && current < end
}

/// EU-wide clocks change at 01:00 UTC on the last Sunday of March (forward)
/// and the last Sunday of October (back), the same moment across every EU
/// member state regardless of local time.
fn is_eu_dst(utc: RtcDateTime) -> bool {
    let start_day = last_sunday_of_month(utc.year, 3);
    let end_day = last_sunday_of_month(utc.year, 10);
    let start = date_time_key(utc.year, 3, start_day, 1, 0, 0);
    let end = date_time_key(utc.year, 10, end_day, 1, 0, 0);
    let current = date_time_key(
        utc.year, utc.month, utc.day, utc.hour, utc.minute, utc.second,
    );
    current >= start && current < end
}

fn last_sunday_of_month(year: u16, month: u8) -> u8 {
    let last_day = days_in_month(year, month);
    let last_weekday = weekday_for_date(year, month, last_day);
    last_day - last_weekday
}

fn nth_sunday_of_month(year: u16, month: u8, nth: u8) -> u8 {
    let first_weekday = weekday_for_date(year, month, 1);
    let first_sunday = if first_weekday == 0 {
        1
    } else {
        8 - first_weekday
    };
    first_sunday + 7 * (nth - 1)
}

/// Sunday is zero, matching the RTC weekday convention used by this firmware.
fn weekday_for_date(year: u16, month: u8, day: u8) -> u8 {
    let table = [0_i32, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut year = i32::from(year);
    if month < 3 {
        year -= 1;
    }
    (year + year / 4 - year / 100 + year / 400 + table[usize::from(month - 1)] + i32::from(day))
        .rem_euclid(7) as u8
}

fn date_time_key(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> u64 {
    u64::from(year) * 10_000_000_000
        + u64::from(month) * 100_000_000
        + u64::from(day) * 1_000_000
        + u64::from(hour) * 10_000
        + u64::from(minute) * 100
        + u64::from(second)
}

#[cfg(test)]
mod tests {
    use super::{
        format_utc_offset, RegionalPreferences, TemperatureUnit, TimeZoneProfile,
        DEFAULT_DISPLAY_UTC_OFFSET_MINUTES, SAMPLE_RTC_STORAGE_UTC_OFFSET_MINUTES,
    };
    use crate::rtc::RtcDateTime;

    fn utc(month: u8, day: u8, hour: u8) -> RtcDateTime {
        RtcDateTime {
            year: 2026,
            month,
            day,
            weekday: 0,
            hour,
            minute: 0,
            second: 0,
        }
    }

    #[test]
    fn defaults_to_new_york_and_fahrenheit() {
        let preferences = RegionalPreferences::default();
        assert_eq!(preferences.timezone_name(), "America/New_York");
        assert_eq!(preferences.temperature_unit, TemperatureUnit::Fahrenheit);
        assert_eq!(preferences.timezone_label(), "EDT UTC-04:00");
    }

    #[test]
    fn converts_local_alarm_schedule_back_into_rtc_storage_basis() {
        let preferences = RegionalPreferences::default();
        let local = RtcDateTime {
            year: 2026,
            month: 6,
            day: 3,
            weekday: 3,
            hour: 7,
            minute: 30,
            second: 0,
        };
        let stored = preferences.local_to_rtc(local);
        assert_eq!(stored.date_time(), "2026-06-03  19:30:00");
        assert_eq!(preferences.localize_rtc(stored), local);
    }

    #[test]
    fn records_uploaded_sample_rtc_storage_basis_explicitly() {
        assert_eq!(SAMPLE_RTC_STORAGE_UTC_OFFSET_MINUTES, 480);
        assert_eq!(DEFAULT_DISPLAY_UTC_OFFSET_MINUTES, -240);
    }

    #[test]
    fn new_york_profile_applies_automatic_dst_transitions() {
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(1, 4, 12)),
            -300
        );
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(6, 4, 12)),
            -240
        );
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(3, 8, 6)),
            -300
        );
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(3, 8, 7)),
            -240
        );
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(11, 1, 5)),
            -240
        );
        assert_eq!(
            TimeZoneProfile::AmericaNewYork.offset_minutes_for_utc(utc(11, 1, 6)),
            -300
        );
    }

    #[test]
    fn europe_rome_profile_applies_automatic_eu_wide_dst_transitions() {
        // 2026 EU DST: starts 2026-03-29 01:00 UTC, ends 2026-10-25 01:00 UTC.
        let before_spring = RtcDateTime {
            year: 2026,
            month: 3,
            day: 29,
            weekday: 0,
            hour: 0,
            minute: 59,
            second: 0,
        };
        let after_spring = RtcDateTime {
            hour: 1,
            minute: 0,
            ..before_spring
        };
        let before_fall = RtcDateTime {
            year: 2026,
            month: 10,
            day: 25,
            weekday: 0,
            hour: 0,
            minute: 59,
            second: 0,
        };
        let after_fall = RtcDateTime {
            hour: 1,
            minute: 0,
            ..before_fall
        };
        assert_eq!(
            TimeZoneProfile::EuropeRome.offset_minutes_for_utc(before_spring),
            60
        );
        assert_eq!(
            TimeZoneProfile::EuropeRome.offset_minutes_for_utc(after_spring),
            120
        );
        assert_eq!(
            TimeZoneProfile::EuropeRome.offset_minutes_for_utc(before_fall),
            120
        );
        assert_eq!(
            TimeZoneProfile::EuropeRome.offset_minutes_for_utc(after_fall),
            60
        );
        assert_eq!(
            TimeZoneProfile::EuropeRome.abbreviation_for_utc(after_spring),
            "CEST"
        );
        assert_eq!(
            TimeZoneProfile::EuropeRome.abbreviation_for_utc(after_fall),
            "CET"
        );
    }

    #[test]
    fn localizes_and_round_trips_europe_rome_local_time() {
        let preferences = RegionalPreferences::default()
            .with_timezone_name("Europe/Rome")
            .unwrap();
        let local = RtcDateTime {
            year: 2026,
            month: 8,
            day: 18,
            weekday: 2,
            hour: 15,
            minute: 0,
            second: 0,
        };
        let stored = preferences.local_to_rtc(local);
        assert_eq!(preferences.localize_rtc(stored), local);
        // Mid-August is CEST (UTC+2): 15:00 local is 13:00 UTC.
        assert_eq!(preferences.rtc_to_utc(stored).hour, 13);
    }

    #[test]
    fn localizes_sample_storage_clock_into_new_york_daylight_time() {
        let sample_wall_clock = RtcDateTime {
            year: 2026,
            month: 6,
            day: 4,
            weekday: 4,
            hour: 1,
            minute: 25,
            second: 30,
        };
        assert_eq!(
            RegionalPreferences::default()
                .localize_rtc(sample_wall_clock)
                .date_time(),
            "2026-06-03  13:25:30"
        );
    }

    #[test]
    fn accepts_utc_profile_and_formats_offsets() {
        let preferences = RegionalPreferences::default()
            .with_timezone_name("UTC")
            .unwrap();
        assert_eq!(preferences.timezone_name(), "UTC");
        assert_eq!(format_utc_offset(480), "UTC+08:00");
        assert_eq!(format_utc_offset(-240), "UTC-04:00");
        assert_eq!(format_utc_offset(330), "UTC+05:30");
    }

    #[test]
    fn round_trips_timezone_selection_through_its_own_config_file() {
        use super::{load_timezone_name, save_timezone_name};

        let dir = std::env::temp_dir().join(format!(
            "rustmix-wave-clock-config-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("CLOCK.TXT");

        save_timezone_name(&path, "Europe/Rome").unwrap();
        assert_eq!(load_timezone_name(&path).unwrap(), "Europe/Rome");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unsupported_timezone_in_clock_config_file() {
        use super::load_timezone_name;

        let dir = std::env::temp_dir().join(format!(
            "rustmix-wave-clock-config-invalid-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("CLOCK.TXT");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "timezone=Mars/Olympus\n").unwrap();

        assert!(load_timezone_name(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
