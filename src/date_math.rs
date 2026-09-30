//! Gregorian date arithmetic shared by the clock editor and the regional
//! timezone rules.

/// Gregorian leap-year rule.
#[must_use]
pub const fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Number of days in one month. Invalid months safely return zero.
#[must_use]
pub const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Sunday-zero weekday for one valid Gregorian date.
#[must_use]
pub fn weekday(year: u16, month: u8, day: u8) -> u8 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut adjusted_year = i32::from(year);
    if month < 3 {
        adjusted_year -= 1;
    }
    let index = usize::from(month.saturating_sub(1).min(11));
    (adjusted_year + adjusted_year / 4 - adjusted_year / 100
        + adjusted_year / 400
        + OFFSETS[index]
        + i32::from(day))
    .rem_euclid(7) as u8
}

#[cfg(test)]
mod tests {
    use super::{days_in_month, is_leap_year, weekday};

    #[test]
    fn handles_rtc_range_leap_years() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2025));
        assert!(!is_leap_year(2100));
        assert!(is_leap_year(2000));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2025, 13), 0);
    }

    #[test]
    fn weekday_matches_known_dates() {
        assert_eq!(weekday(2026, 6, 4), 4); // Thursday
        assert_eq!(weekday(2000, 1, 1), 6); // Saturday
        assert_eq!(weekday(2026, 9, 30), 3); // Wednesday
    }
}
