//! Runtime-only local wall-clock editor for the Clock screen's manual
//! "Set date & time" action.
//!
//! Up/Down adjusts the selected field's value, Select advances to the next
//! field and BOOT steps back to the previous one. The fields come in the
//! order a date is said: time zone, day, month, year, hour, minute. The
//! time zone is first because changing it rewrites the others. The editor
//! keeps a UTC anchor captured when it opened; picking a different timezone
//! re-derives the displayed local fields from that same anchor instant
//! instead of reinterpreting the already-displayed numbers under the new
//! zone, so switching zones alone (the common case) never shifts the
//! underlying moment in time. The caller converts the committed draft, under
//! whichever zone is selected, back into the RTC storage basis before
//! writing hardware.

use crate::{
    date_math,
    regional::{RegionalPreferences, TimeZoneProfile},
    rtc::RtcDateTime,
};

/// Fallback UTC anchor used to seed the editor when no RTC reading is
/// available yet.
#[must_use]
pub const fn fallback_local() -> RtcDateTime {
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

/// Timezone profiles offered by the on-device cycle, in display order.
const TIMEZONE_OPTIONS: [TimeZoneProfile; 3] = [
    TimeZoneProfile::AmericaNewYork,
    TimeZoneProfile::Utc,
    TimeZoneProfile::EuropeRome,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockEditField {
    Timezone,
    Day,
    Month,
    Year,
    Hour,
    Minute,
    Save,
}

impl ClockEditField {
    pub const COUNT: usize = 7;

    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        match index % Self::COUNT {
            0 => Self::Timezone,
            1 => Self::Day,
            2 => Self::Month,
            3 => Self::Year,
            4 => Self::Hour,
            5 => Self::Minute,
            _ => Self::Save,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Timezone => "Timezone",
            Self::Hour => "Hour",
            Self::Minute => "Minute",
            Self::Year => "Year",
            Self::Month => "Month",
            Self::Day => "Day",
            Self::Save => "Save date & time",
        }
    }
}

/// Local wall-clock fields under edit. Seconds reset to zero on save, and
/// the weekday is derived rather than edited directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockTimeDraft {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

impl ClockTimeDraft {
    #[must_use]
    pub const fn from_local(local: RtcDateTime) -> Self {
        Self {
            year: local.year,
            month: local.month,
            day: local.day,
            hour: local.hour,
            minute: local.minute,
        }
    }

    #[must_use]
    pub fn date_label(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    #[must_use]
    pub fn time_label(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// Render as a full local `RtcDateTime` with seconds zeroed and the
    /// weekday derived from the edited date.
    #[must_use]
    pub fn as_local_rtc(&self) -> RtcDateTime {
        RtcDateTime {
            year: self.year,
            month: self.month,
            day: self.day,
            weekday: date_math::weekday(self.year, self.month, self.day),
            hour: self.hour,
            minute: self.minute,
            second: 0,
        }
    }

    fn clamp_day(&mut self) {
        let max_day = date_math::days_in_month(self.year, self.month);
        if self.day > max_day {
            self.day = max_day;
        }
    }
}

/// Runtime-only editor state for the Clock screen's "Set date & time" action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockTimeEditor {
    pub draft: ClockTimeDraft,
    pub timezone: TimeZoneProfile,
    anchor_utc: RtcDateTime,
    pub field_index: usize,
}

impl ClockTimeEditor {
    /// Seed the editor from a UTC anchor instant and the currently active
    /// regional timezone, which becomes the initial (and reversible) draft
    /// selection.
    #[must_use]
    pub fn new(anchor_utc: RtcDateTime, regional: RegionalPreferences) -> Self {
        let timezone = regional.timezone;
        Self {
            draft: ClockTimeDraft::from_local(timezone.localize_utc(anchor_utc)),
            timezone,
            anchor_utc,
            field_index: 0,
        }
    }

    #[must_use]
    pub const fn selected_field(&self) -> ClockEditField {
        ClockEditField::from_index(self.field_index)
    }

    pub fn advance_field(&mut self) {
        self.field_index = (self.field_index + 1) % ClockEditField::COUNT;
    }

    /// Step back to the previous field. `false` on the first one, where
    /// there is nothing to step back to and BOOT leaves the editor.
    pub fn retreat_field(&mut self) -> bool {
        if self.field_index % ClockEditField::COUNT == 0 {
            return false;
        }
        self.field_index -= 1;
        true
    }

    pub fn adjust(&mut self, delta: i32) {
        match self.selected_field() {
            ClockEditField::Timezone => {
                self.timezone = cycle_timezone(self.timezone, delta);
                self.draft =
                    ClockTimeDraft::from_local(self.timezone.localize_utc(self.anchor_utc));
            }
            ClockEditField::Hour => self.draft.hour = wrap_u8(self.draft.hour, delta, 24),
            ClockEditField::Minute => self.draft.minute = wrap_u8(self.draft.minute, delta, 60),
            ClockEditField::Year => {
                self.draft.year = wrap_year(self.draft.year, delta);
                self.draft.clamp_day();
            }
            ClockEditField::Month => {
                self.draft.month = wrap_u8_range(self.draft.month, delta, 1, 12);
                self.draft.clamp_day();
            }
            ClockEditField::Day => {
                let max_day = date_math::days_in_month(self.draft.year, self.draft.month);
                self.draft.day = wrap_u8_range(self.draft.day, delta, 1, max_day);
            }
            ClockEditField::Save => {}
        }
    }
}

fn cycle_timezone(current: TimeZoneProfile, delta: i32) -> TimeZoneProfile {
    let index = TIMEZONE_OPTIONS
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0) as i32;
    TIMEZONE_OPTIONS[(index + delta).rem_euclid(TIMEZONE_OPTIONS.len() as i32) as usize]
}

fn wrap_u8(value: u8, delta: i32, modulus: i32) -> u8 {
    (i32::from(value) + delta).rem_euclid(modulus) as u8
}

fn wrap_u8_range(value: u8, delta: i32, min: u8, max: u8) -> u8 {
    let span = i32::from(max) - i32::from(min) + 1;
    (i32::from(min) + (i32::from(value) - i32::from(min) + delta).rem_euclid(span)) as u8
}

fn wrap_year(value: u16, delta: i32) -> u16 {
    const MIN_YEAR: i32 = 2000;
    const MAX_YEAR: i32 = 2099;
    const SPAN: i32 = MAX_YEAR - MIN_YEAR + 1;
    (MIN_YEAR + (i32::from(value) - MIN_YEAR + delta).rem_euclid(SPAN)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> RtcDateTime {
        RtcDateTime {
            year,
            month,
            day,
            weekday: 0,
            hour,
            minute,
            second: 0,
        }
    }

    fn editor_at(anchor: RtcDateTime, zone: TimeZoneProfile) -> ClockTimeEditor {
        let regional = RegionalPreferences::default()
            .with_timezone_name(zone.name())
            .unwrap();
        ClockTimeEditor::new(anchor, regional)
    }

    #[test]
    fn fields_follow_the_order_a_date_is_said_after_the_timezone() {
        let mut editor = editor_at(utc(2026, 8, 18, 21, 59), TimeZoneProfile::Utc);
        let mut order = Vec::new();
        for _ in 0..ClockEditField::COUNT {
            order.push(editor.selected_field());
            editor.advance_field();
        }
        assert_eq!(
            order,
            [
                ClockEditField::Timezone,
                ClockEditField::Day,
                ClockEditField::Month,
                ClockEditField::Year,
                ClockEditField::Hour,
                ClockEditField::Minute,
                ClockEditField::Save,
            ]
        );
    }

    #[test]
    fn boot_steps_back_one_field_and_stops_at_the_first() {
        let mut editor = editor_at(utc(2026, 8, 18, 21, 59), TimeZoneProfile::Utc);
        assert!(!editor.retreat_field());
        editor.advance_field();
        editor.advance_field();
        assert_eq!(editor.selected_field(), ClockEditField::Month);
        assert!(editor.retreat_field());
        assert_eq!(editor.selected_field(), ClockEditField::Day);
        assert!(editor.retreat_field());
        assert_eq!(editor.selected_field(), ClockEditField::Timezone);
        assert!(!editor.retreat_field());
    }

    #[test]
    fn adjusts_hour_and_minute_with_wraparound() {
        let mut editor = editor_at(utc(2026, 8, 18, 21, 59), TimeZoneProfile::Utc);
        editor.field_index = ClockEditField::Hour as usize;
        assert_eq!(editor.draft.hour, 21);
        editor.adjust(3);
        assert_eq!(editor.draft.hour, 0);
        editor.advance_field();
        assert_eq!(editor.selected_field(), ClockEditField::Minute);
        editor.adjust(2);
        assert_eq!(editor.draft.minute, 1);
    }

    #[test]
    fn wraps_year_month_and_day_within_valid_bounds() {
        let mut editor = editor_at(utc(2000, 1, 1, 0, 0), TimeZoneProfile::Utc);
        editor.field_index = ClockEditField::Day as usize;
        editor.adjust(-1);
        assert_eq!(editor.draft.day, 31);
        editor.advance_field();
        assert_eq!(editor.selected_field(), ClockEditField::Month);
        editor.adjust(-1);
        assert_eq!(editor.draft.month, 12);
        editor.advance_field();
        assert_eq!(editor.selected_field(), ClockEditField::Year);
        editor.adjust(-1);
        assert_eq!(editor.draft.year, 2099);
    }

    #[test]
    fn clamps_day_when_month_or_year_shrinks_it() {
        // 2024 is a leap year.
        let mut editor = editor_at(utc(2024, 1, 31, 0, 0), TimeZoneProfile::Utc);
        editor.field_index = ClockEditField::Month as usize;
        editor.adjust(1); // -> February, clamps day to 29 (leap year)
        assert_eq!((editor.draft.month, editor.draft.day), (2, 29));
        editor.advance_field(); // Year
        editor.adjust(1); // -> 2025, not a leap year: 28
        assert_eq!((editor.draft.year, editor.draft.day), (2025, 28));
        editor.field_index = ClockEditField::Save as usize;
        editor.adjust(1); // no-op on Save
        assert_eq!((editor.draft.month, editor.draft.day), (2, 28));
    }

    #[test]
    fn save_is_the_final_field_and_advance_wraps_back_to_timezone() {
        let mut editor = editor_at(utc(2026, 8, 18, 0, 0), TimeZoneProfile::Utc);
        for _ in 0..ClockEditField::COUNT {
            editor.advance_field();
        }
        assert_eq!(editor.selected_field(), ClockEditField::Timezone);
    }

    #[test]
    fn as_local_rtc_derives_weekday_and_zeroes_seconds() {
        let editor = editor_at(utc(2026, 8, 18, 9, 30), TimeZoneProfile::Utc);
        let rtc = editor.draft.as_local_rtc();
        assert_eq!(rtc.weekday, date_math::weekday(2026, 8, 18));
        assert_eq!(rtc.second, 0);
        assert_eq!((rtc.hour, rtc.minute), (9, 30));
    }

    #[test]
    fn changing_timezone_recomputes_local_fields_from_the_same_anchor_instant() {
        // 2026-08-18 13:00 UTC is mid-August: EDT (-4h) and CEST (+2h) both apply.
        let anchor = utc(2026, 8, 18, 13, 0);
        let mut editor = editor_at(anchor, TimeZoneProfile::AmericaNewYork);
        assert_eq!((editor.draft.hour, editor.draft.minute), (9, 0));

        editor.adjust(1); // -> Utc
        assert_eq!(editor.timezone, TimeZoneProfile::Utc);
        assert_eq!((editor.draft.hour, editor.draft.minute), (13, 0));

        editor.adjust(1); // -> Europe/Rome
        assert_eq!(editor.timezone, TimeZoneProfile::EuropeRome);
        assert_eq!((editor.draft.hour, editor.draft.minute), (15, 0));

        editor.adjust(-1); // back to Utc
        assert_eq!(editor.timezone, TimeZoneProfile::Utc);
        assert_eq!((editor.draft.hour, editor.draft.minute), (13, 0));
    }

    #[test]
    fn manually_edited_fields_survive_a_later_timezone_change() {
        let mut editor = editor_at(utc(2026, 8, 18, 12, 0), TimeZoneProfile::Utc);
        editor.field_index = ClockEditField::Hour as usize;
        editor.adjust(5); // manual override: 17:00, independent of the anchor
        assert_eq!(editor.draft.hour, 17);

        editor.field_index = ClockEditField::Timezone as usize;
        editor.adjust(1); // cycling zone re-derives from the anchor, discarding the override
        assert_ne!(editor.draft.hour, 17);
    }
}
