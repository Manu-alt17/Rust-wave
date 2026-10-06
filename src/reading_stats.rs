//! Reading-session tracking, SD persistence and lazily-aggregated statistics.
//!
//! A [`ReadingSession`] is a contiguous stretch of page turns on one book,
//! closed on book close, on leaving the Reader screen, or after
//! [`INACTIVITY_TIMEOUT_SECONDS`] of no further page turns -- see
//! [`ReadingStatsTracker`]. Closed sessions are appended, one line per
//! session, to a per-month log under [`STATS_DIRECTORY`] and never rewritten
//! in place, so recording one more session costs one small append rather
//! than reprocessing the device's whole reading history. Daily/weekly/
//! monthly totals and the current streak are derived from those logs lazily,
//! on demand (see [`compute_snapshot`]), by reading only the handful of
//! month files a given query actually needs.
//!
//! The event loop keeps running during panel/idle sleep (a known power
//! issue, see `docs/KNOWN_ISSUES.md`), so a session's boundaries are defined
//! purely by page-turn activity and an explicit inactivity timeout -- never
//! by whether the screen happens to be lit.
//!
//! Days, weeks and months are the reader's own, in the timezone chosen on the
//! device. Counted in UTC, reading in Rome between midnight and 1 or 2 a.m.
//! went to the day before. The per-month log files stay keyed by UTC month,
//! a storage detail the aggregation reads around.

use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use log::warn;

use crate::{date_math, ntp, regional::TimeZoneProfile, rtc::RtcDateTime};

/// SD directory holding one append-only log file per calendar month, plus
/// nothing else -- there is no cross-month summary cache; see this module's
/// header comment for why that stays a lazy, bounded re-read instead.
pub const STATS_DIRECTORY: &str = "/sdcard/RUSTMIX/STATS";

/// Default gap, in seconds, after the last page turn before an open session
/// is considered abandoned ("book left open on the nightstand") rather than
/// still being read. Configurable per [`ReadingStatsTracker::with_inactivity_timeout`].
pub const INACTIVITY_TIMEOUT_SECONDS: u64 = 5 * 60;
/// Sessions shorter than this are almost always an accidental tap, not
/// reading, and are discarded rather than persisted.
pub const MIN_SESSION_SECONDS: u64 = 10;
/// Sessions faster than this are almost always a rapid skim/search rather
/// than real reading, and are discarded rather than persisted.
pub const MAX_PLAUSIBLE_CHARS_PER_SECOND: f64 = 50.0;
/// Preferred number of most-recent sessions folded into the weighted
/// reading-speed average.
pub const SPEED_SESSION_WINDOW: usize = 10;
/// Trailing window, in days, considered for the reading-speed average when
/// it covers more sessions than [`SPEED_SESSION_WINDOW`].
pub const SPEED_DAYS_WINDOW: u64 = 7;
/// Per-session-back exponential decay applied to older sessions in the
/// weighted reading-speed average.
pub const SPEED_DECAY: f64 = 0.9;
/// How many additional months of logs are consulted, beyond the current
/// month, when computing the reading streak. Bounds the lazy re-read to a
/// handful of files instead of the device's entire reading history.
const STREAK_LOOKBACK_MONTHS: u32 = 3;
/// Hard cap on how many days a streak walk-back will ever report, purely as
/// a runaway-loop guard against pathological log content.
const STREAK_SAFETY_CAP_DAYS: u32 = 3_650;

/// One contiguous stretch of reading on a single book, closed on book close,
/// on leaving the Reader screen, or after an inactivity timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadingSession {
    /// Stable per-book identifier, see [`book_id_for`].
    pub book_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    /// Character offset into the book's flattened text at session start.
    /// Deliberately not a "page" number: page boundaries move with font
    /// family/size, but a character offset does not.
    pub start_position: u64,
    pub end_position: u64,
    /// Diagnostic/UX count only -- never used in a duration or speed
    /// calculation, both of which are derived from timestamps and
    /// positions instead.
    pub pages_turned: u16,
}

impl ReadingSession {
    #[must_use]
    pub fn duration_seconds(&self) -> u64 {
        self.end_ts.saturating_sub(self.start_ts)
    }

    #[must_use]
    pub fn chars_read(&self) -> u64 {
        self.end_position.saturating_sub(self.start_position)
    }

    #[must_use]
    pub fn chars_per_second(&self) -> f64 {
        let seconds = self.duration_seconds();
        if seconds == 0 {
            return 0.0;
        }
        self.chars_read() as f64 / seconds as f64
    }

    /// Whether this session looks like real reading rather than an
    /// accidental tap (too short) or a rapid skim/search (too fast). Applied
    /// once, before a session is persisted, so every downstream consumer of
    /// the log -- daily totals, streaks, the speed average -- already sees
    /// only plausible reading time.
    #[must_use]
    fn is_plausible(&self) -> bool {
        self.end_position >= self.start_position
            && self.duration_seconds() >= MIN_SESSION_SECONDS
            && self.chars_per_second() <= MAX_PLAUSIBLE_CHARS_PER_SECOND
    }

    fn to_log_line(self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.book_id,
            self.start_ts,
            self.end_ts,
            self.start_position,
            self.end_position,
            self.pages_turned
        )
    }

    fn parse_log_line(line: &str) -> Option<Self> {
        let mut fields = line.trim().split('|');
        let session = Self {
            book_id: fields.next()?.parse().ok()?,
            start_ts: fields.next()?.parse().ok()?,
            end_ts: fields.next()?.parse().ok()?,
            start_position: fields.next()?.parse().ok()?,
            end_position: fields.next()?.parse().ok()?,
            pages_turned: fields.next()?.parse().ok()?,
        };
        if fields.next().is_some() {
            return None;
        }
        Some(session)
    }
}

/// One calendar day's reading totals, incremented from closed sessions --
/// never rebuilt from a per-tap counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DailyStats {
    /// `YYYYMMDD`.
    pub date: u32,
    pub total_seconds: u32,
    pub sessions: u16,
    pub chars_read: u32,
}

/// Stable per-book identifier derived from the same `(path, size_bytes,
/// modified_seconds)` triple the Reader library already uses to recognize a
/// book (see `ReaderBook`/`ReaderLocation::matches_book`), so no separate
/// book registry is needed. FNV-1a over the three fields, folded to 32 bits.
#[must_use]
pub fn book_id_for(path: &str, size_bytes: u64, modified_seconds: u64) -> u32 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in path
        .as_bytes()
        .iter()
        .chain(size_bytes.to_le_bytes().iter())
        .chain(modified_seconds.to_le_bytes().iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    ((hash >> 32) ^ (hash & 0xFFFF_FFFF)) as u32
}

/// Best-effort current unix timestamp: SNTP-synced [`SystemTime`] when
/// plausible, otherwise the hardware RTC (already converted to true UTC by
/// the caller -- see `regional::RegionalPreferences::rtc_to_utc`). `None`
/// only when neither source is available, in which case the caller should
/// skip recording rather than log a session with a meaningless timestamp.
#[must_use]
pub fn resolve_unix_timestamp(utc_rtc_fallback: Option<RtcDateTime>) -> Option<u64> {
    if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
        let seconds = duration.as_secs();
        if seconds >= ntp::MIN_VALID_SNTP_UNIX_SECONDS {
            return Some(seconds);
        }
    }
    utc_rtc_fallback.map(unix_seconds_from_utc)
}

/// Inverse of [`ntp::utc_from_unix_seconds`] (Howard Hinnant's
/// `days_from_civil`), needed only for the hardware-RTC timestamp fallback.
fn unix_seconds_from_utc(utc: RtcDateTime) -> u64 {
    let year = i64::from(utc.year);
    let month = i64::from(utc.month);
    let day = i64::from(utc.day);
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    let seconds_of_day =
        u64::from(utc.hour) * 3_600 + u64::from(utc.minute) * 60 + u64::from(utc.second);
    (days_since_epoch.max(0) as u64) * 86_400 + seconds_of_day
}

/// Local wall-clock fields of the UTC instant `seconds` in `zone`.
fn local_date(seconds: u64, zone: TimeZoneProfile) -> RtcDateTime {
    zone.localize_utc(ntp::utc_from_unix_seconds(seconds))
}

/// `YYYYMMDD` of a date: ordered like the dates themselves.
fn yyyymmdd(date: RtcDateTime) -> u32 {
    u32::from(date.year) * 10_000 + u32::from(date.month) * 100 + u32::from(date.day)
}

/// The calendar date `days` days before `date`. Plain calendar arithmetic:
/// stepping back 86 400 seconds instead skips or repeats a local date
/// across a daylight saving change.
fn days_before(date: RtcDateTime, days: u32) -> RtcDateTime {
    date.shift_minutes(-(days as i32) * 24 * 60)
}

fn month_log_path(stats_root: &str, year: u16, month: u8) -> PathBuf {
    Path::new(stats_root).join(format!("{year:04}{month:02}.LOG"))
}

fn append_session(stats_root: &str, session: ReadingSession) -> Result<(), String> {
    fs::create_dir_all(stats_root).map_err(|error| format!("create {stats_root}: {error}"))?;
    let utc = ntp::utc_from_unix_seconds(session.start_ts);
    let path = month_log_path(stats_root, utc.year, utc.month);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    writeln!(file, "{}", session.to_log_line())
        .map_err(|error| format!("append {}: {error}", path.display()))
}

fn read_month_log(stats_root: &str, year: u16, month: u8) -> Vec<ReadingSession> {
    let mut span = crate::boot_profile::span("reading-stats-read-month");
    if crate::boot_profile::is_active() {
        span.detail(format_args!("{year:04}-{month:02}"));
    }
    let path = month_log_path(stats_root, year, month);
    let Ok(file) = File::open(&path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| ReadingSession::parse_log_line(&line))
        .collect()
}

/// One month's log with the UTC month it is filed under.
type MonthLog = ((u16, u8), Vec<ReadingSession>);

/// The month before `(year, month)`.
const fn previous_month(year: u16, month: u8) -> (u16, u8) {
    if month <= 1 {
        (year.saturating_sub(1), 12)
    } else {
        (year, month - 1)
    }
}

/// The month after `(year, month)`.
const fn next_month(year: u16, month: u8) -> (u16, u8) {
    if month >= 12 {
        (year.saturating_add(1), 1)
    } else {
        (year, month + 1)
    }
}

/// Read the current month's log plus `months_back` preceding months' logs,
/// one entry per month, newest first. Bounded and exact: it walks calendar
/// months by field arithmetic rather than jumping fixed day counts, so it
/// never skips a short month.
fn read_recent_month_logs(stats_root: &str, months_back: u32, now: u64) -> Vec<MonthLog> {
    let start = ntp::utc_from_unix_seconds(now);
    let mut key = (start.year, start.month);
    let mut months = Vec::new();
    for _ in 0..=months_back {
        months.push((key, read_month_log(stats_root, key.0, key.1)));
        key = previous_month(key.0, key.1);
    }
    months
}

/// The oldest month a log exists for, from the names in the folder alone.
fn earliest_logged_month(stats_root: &str) -> Option<(u16, u8)> {
    fs::read_dir(stats_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let stem = name
                .strip_suffix(".LOG")
                .or_else(|| name.strip_suffix(".log"))?;
            if stem.len() != 6 || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let year = stem[..4].parse().ok()?;
            let month: u8 = stem[4..].parse().ok()?;
            (1..=12).contains(&month).then_some((year, month))
        })
        .min()
}

fn aggregate_daily(sessions: &[ReadingSession], zone: TimeZoneProfile) -> Vec<DailyStats> {
    let mut days: Vec<DailyStats> = Vec::new();
    for session in sessions {
        // Attribute the session to the local day it ended on -- a session
        // that happens to straddle midnight is rare enough on a single
        // sitting that splitting it isn't worth the complexity.
        let date = yyyymmdd(local_date(session.end_ts, zone));
        let seconds = session.duration_seconds().min(u64::from(u32::MAX)) as u32;
        let chars = session.chars_read().min(u64::from(u32::MAX)) as u32;
        if let Some(day) = days.iter_mut().find(|day| day.date == date) {
            day.total_seconds = day.total_seconds.saturating_add(seconds);
            day.sessions = day.sessions.saturating_add(1);
            day.chars_read = day.chars_read.saturating_add(chars);
        } else {
            days.push(DailyStats {
                date,
                total_seconds: seconds,
                sessions: 1,
                chars_read: chars,
            });
        }
    }
    days
}

/// Consecutive local days, counted back from `now`, with `total_seconds >
/// 0`. Stops at the first gap -- if today has no reading logged yet, the
/// streak is reported as 0 until today's first session closes.
#[must_use]
pub fn current_streak_days(daily: &[DailyStats], now: u64, zone: TimeZoneProfile) -> u32 {
    let today = local_date(now, zone);
    let mut streak = 0;
    for days in 0..STREAK_SAFETY_CAP_DAYS {
        let date = yyyymmdd(days_before(today, days));
        let has_reading = daily
            .iter()
            .any(|day| day.date == date && day.total_seconds > 0);
        if !has_reading {
            break;
        }
        streak += 1;
    }
    streak
}

/// Exponentially-decayed weighted average reading speed, in characters per
/// second, over whichever covers more sessions: the most recent
/// [`SPEED_SESSION_WINDOW`] sessions, or every session in the trailing
/// [`SPEED_DAYS_WINDOW`] days. `None` when there is no plausible session to
/// average at all.
#[must_use]
pub fn weighted_chars_per_second(sessions: &[ReadingSession], now: u64) -> Option<f64> {
    let mut ordered: Vec<&ReadingSession> = sessions.iter().collect();
    ordered.sort_by(|left, right| right.end_ts.cmp(&left.end_ts));

    let cutoff = now.saturating_sub(SPEED_DAYS_WINDOW * 86_400);
    let within_window = ordered
        .iter()
        .take_while(|session| session.end_ts >= cutoff)
        .count();
    let chosen_count = within_window.max(SPEED_SESSION_WINDOW).min(ordered.len());

    if chosen_count == 0 {
        return None;
    }
    let mut weight = 1.0_f64;
    let mut weighted_sum = 0.0_f64;
    let mut weight_total = 0.0_f64;
    for session in &ordered[..chosen_count] {
        weighted_sum += session.chars_per_second() * weight;
        weight_total += weight;
        weight *= SPEED_DECAY;
    }
    Some(weighted_sum / weight_total)
}

/// Estimated seconds remaining to reach `end_position` from
/// `current_position` at `chars_per_second`. `None` when the speed is
/// non-positive (nothing recorded yet) or the position is already past the
/// end.
#[must_use]
pub fn estimated_seconds_remaining(
    current_position: u64,
    end_position: u64,
    chars_per_second: f64,
) -> Option<u64> {
    if chars_per_second <= 0.0 {
        return None;
    }
    let remaining_chars = end_position.checked_sub(current_position)?;
    Some((remaining_chars as f64 / chars_per_second).round() as u64)
}

/// Compact "Xh YYm" / "Ym" duration label shared by the Continue Reading
/// tile and the Reading Stats screen.
#[must_use]
pub fn format_duration_seconds(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// Current book's chapter/book end offsets, when known, for the "time
/// remaining" estimate. Left `None` when the Reader hasn't indexed that far
/// yet (chapter end) or for formats without chapters (TXT).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CurrentBookProgress {
    pub current_position: u64,
    pub chapter_end_position: Option<u64>,
    pub book_end_position: Option<u64>,
}

/// What span of time the statistics screen charts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatsPeriod {
    /// A calendar week, Monday to Sunday.
    #[default]
    Week,
    /// A calendar month.
    Month,
}

impl StatsPeriod {
    /// The other one: the screen has two.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Week => Self::Month,
            Self::Month => Self::Week,
        }
    }

    /// How many periods back from the current one the screen goes: a year.
    #[must_use]
    pub const fn max_back(self) -> u16 {
        match self {
            Self::Week => 52,
            Self::Month => 12,
        }
    }
}

/// The period on show: a week or a month, and how many of them before the
/// current one (`0` is the week or month of today).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatsView {
    pub period: StatsPeriod,
    pub back: u16,
}

/// A calendar date, local to the timezone chosen on the device.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct CalendarDay {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl CalendarDay {
    fn of(date: RtcDateTime) -> Self {
        Self {
            year: date.year,
            month: date.month,
            day: date.day,
        }
    }
}

/// One day of the charted period: a bar of the chart. Carries the weekday
/// (`0` = Sunday .. `6` = Saturday, matching
/// [`ntp::utc_from_unix_seconds`]) and the day of the month, so the screen
/// labels it without wall-clock access of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PeriodDay {
    pub day: u8,
    pub weekday: u8,
    pub total_seconds: u32,
    /// Today.
    pub today: bool,
    /// Still to come: it has no bar, where a day without reading has an
    /// empty one.
    pub future: bool,
}

/// Most books a period lists: all a screen ever lays out at once.
pub const PERIOD_BOOKS_LIMIT: usize = 5;

/// One book's total reading time within the charted period. Carries only
/// [`Self::book_id`] -- the screen resolves title/cover/percent from
/// `AppState`'s already-in-RAM Reader library state, which `reading_stats`
/// deliberately does not depend on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BookPeriodStats {
    pub book_id: u32,
    pub total_seconds: u32,
}

/// The reading of one week or one month, day by day.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PeriodStats {
    pub view: StatsView,
    pub first: CalendarDay,
    pub last: CalendarDay,
    /// One entry per day from `first` to `last`: 7, or 28 to 31.
    pub days: Vec<PeriodDay>,
    pub total_seconds: u32,
    /// Days of the period up to today, all of them for a past one: what the
    /// daily average divides by.
    pub elapsed_days: u16,
    /// Sorted by total time descending, bounded to [`PERIOD_BOOKS_LIMIT`].
    pub books: Vec<BookPeriodStats>,
    /// Whether an earlier period may hold reading: there is a log older
    /// than this one's first day, and the limit of a year is not reached.
    pub has_older: bool,
}

#[cfg(test)]
impl PeriodStats {
    /// A period for the screen tests: `seconds` of reading per day from
    /// `first` on, the day at index `today` being today and those after it
    /// still to come.
    pub(crate) fn sample(
        view: StatsView,
        first: CalendarDay,
        seconds: &[u32],
        today: Option<usize>,
        books: Vec<BookPeriodStats>,
        has_older: bool,
    ) -> Self {
        let start = calendar_date(first.year, first.month, first.day);
        let days: Vec<PeriodDay> = seconds
            .iter()
            .enumerate()
            .map(|(index, total_seconds)| {
                let date = days_after(start, index as u32);
                PeriodDay {
                    day: date.day,
                    weekday: date.weekday,
                    total_seconds: *total_seconds,
                    today: today == Some(index),
                    future: today.is_some_and(|today| index > today),
                }
            })
            .collect();
        let last = days_after(start, seconds.len().saturating_sub(1) as u32);
        Self {
            view,
            first,
            last: CalendarDay::of(last),
            total_seconds: days.iter().map(|day| day.total_seconds).sum(),
            elapsed_days: today.map_or(days.len(), |today| today + 1) as u16,
            days,
            books,
            has_older,
        }
    }
}

impl PeriodStats {
    /// Average reading time per day elapsed, in seconds.
    #[must_use]
    pub fn average_seconds_per_day(&self) -> u32 {
        self.total_seconds / u32::from(self.elapsed_days.max(1))
    }

    /// The longest day, in seconds.
    #[must_use]
    pub fn longest_day_seconds(&self) -> u32 {
        self.days
            .iter()
            .map(|day| day.total_seconds)
            .max()
            .unwrap_or(0)
    }
}

/// Pure, display-ready aggregate consumed by the Reading Stats screen.
/// Produced by [`compute_snapshot`] and pushed into `AppState` by the
/// runtime owner in main.rs -- screens never touch SD directly.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReadingStatsSnapshot {
    /// `false` only when no reliable clock (SNTP or RTC) was available to
    /// compute the snapshot at all.
    pub available: bool,
    pub today_seconds: u32,
    pub sessions_today: u16,
    pub streak_days: u32,
    pub chars_per_minute: Option<u32>,
    pub remaining_chapter_seconds: Option<u64>,
    pub remaining_book_seconds: Option<u64>,
    /// The week or month asked for, day by day.
    pub period: PeriodStats,
}

/// Lazily aggregate everything the Reading Stats screen shows by reading
/// only the current month's log plus, for the streak, up to
/// [`STREAK_LOOKBACK_MONTHS`] preceding months, and the one to three logs
/// the period on show (`view`) lies in when it is older than those -- never
/// the device's entire reading history. Days, weeks and months are local to
/// `zone`.
#[must_use]
pub fn compute_snapshot(
    stats_root: &str,
    now: u64,
    book_progress: Option<CurrentBookProgress>,
    zone: TimeZoneProfile,
    view: StatsView,
) -> ReadingStatsSnapshot {
    let _span = crate::boot_profile::span("reading-stats-compute");
    let local_now = local_date(now, zone);
    let today = yyyymmdd(local_now);

    // Every log is read once: the current month, the recent window and the
    // streak window below are all prefixes of this newest-first list.
    let months = read_recent_month_logs(stats_root, STREAK_LOOKBACK_MONTHS, now);

    // Current + previous log always covers today and the 10-session speed
    // window: the logs are split by UTC month, which is never more than a
    // day away from the local one.
    let recent_sessions: Vec<ReadingSession> = months
        .iter()
        .take(2)
        .flat_map(|(_, sessions)| sessions)
        .copied()
        .collect();
    let recent_daily = aggregate_daily(&recent_sessions, zone);
    let today_stats = recent_daily.iter().find(|day| day.date == today).copied();

    let streak_sessions: Vec<ReadingSession> = months
        .iter()
        .flat_map(|(_, sessions)| sessions)
        .copied()
        .collect();
    let streak_daily = aggregate_daily(&streak_sessions, zone);
    let streak_days = current_streak_days(&streak_daily, now, zone);

    let chars_per_second = weighted_chars_per_second(&recent_sessions, now);
    let chars_per_minute = chars_per_second.map(|value| (value * 60.0).round() as u32);

    let mut remaining_chapter_seconds = None;
    let mut remaining_book_seconds = None;
    if let (Some(progress), Some(speed)) = (book_progress, chars_per_second) {
        if let Some(chapter_end) = progress.chapter_end_position {
            remaining_chapter_seconds =
                estimated_seconds_remaining(progress.current_position, chapter_end, speed);
        }
        if let Some(book_end) = progress.book_end_position {
            remaining_book_seconds =
                estimated_seconds_remaining(progress.current_position, book_end, speed);
        }
    }

    ReadingStatsSnapshot {
        available: true,
        today_seconds: today_stats.map_or(0, |day| day.total_seconds),
        sessions_today: today_stats.map_or(0, |day| day.sessions),
        streak_days,
        chars_per_minute,
        remaining_chapter_seconds,
        remaining_book_seconds,
        period: period_stats(stats_root, &months, local_now, zone, view),
    }
}

/// A date with its weekday, at midnight: what the calendar steps below
/// start from.
fn calendar_date(year: u16, month: u8, day: u8) -> RtcDateTime {
    RtcDateTime {
        year,
        month,
        day,
        weekday: date_math::weekday(year, month, day),
        hour: 0,
        minute: 0,
        second: 0,
    }
}

/// The calendar date `days` days after `date`.
fn days_after(date: RtcDateTime, days: u32) -> RtcDateTime {
    date.shift_minutes(days as i32 * 24 * 60)
}

/// First and last day of the week or month `view` asks for, counted back
/// from the one `today` is in. Weeks run Monday to Sunday.
fn period_bounds(today: RtcDateTime, view: StatsView) -> (RtcDateTime, RtcDateTime) {
    let back = view.back.min(view.period.max_back());
    let today = calendar_date(today.year, today.month, today.day);
    match view.period {
        StatsPeriod::Week => {
            let since_monday = (u32::from(today.weekday) + 6) % 7;
            let first = days_before(today, since_monday + 7 * u32::from(back));
            (first, days_after(first, 6))
        }
        StatsPeriod::Month => {
            let mut key = (today.year, today.month);
            for _ in 0..back {
                key = previous_month(key.0, key.1);
            }
            (
                calendar_date(key.0, key.1, 1),
                calendar_date(key.0, key.1, date_math::days_in_month(key.0, key.1)),
            )
        }
    }
}

/// The reading of the week or month `view` asks for. `loaded` are the logs
/// [`compute_snapshot`] already read; a period older than those reads its
/// own, at most three: the logs are filed by UTC month, a day at most away
/// from the local one at either end.
fn period_stats(
    stats_root: &str,
    loaded: &[MonthLog],
    local_now: RtcDateTime,
    zone: TimeZoneProfile,
    view: StatsView,
) -> PeriodStats {
    let view = StatsView {
        period: view.period,
        back: view.back.min(view.period.max_back()),
    };
    let (first, last) = period_bounds(local_now, view);
    let (first_date, last_date) = (yyyymmdd(first), yyyymmdd(last));
    let today = yyyymmdd(local_now);

    let before = days_before(first, 1);
    let after = days_after(last, 1);
    let mut key = (before.year, before.month);
    let end = (after.year, after.month);
    let mut sessions: Vec<ReadingSession> = Vec::new();
    loop {
        let in_period = |session: &&ReadingSession| {
            (first_date..=last_date).contains(&yyyymmdd(local_date(session.end_ts, zone)))
        };
        match loaded.iter().find(|(month, _)| *month == key) {
            Some((_, log)) => sessions.extend(log.iter().filter(in_period)),
            // Nothing is logged in a month still to come.
            None if key > (local_now.year, local_now.month) => {}
            None => sessions.extend(
                read_month_log(stats_root, key.0, key.1)
                    .iter()
                    .filter(in_period),
            ),
        }
        if key >= end {
            break;
        }
        key = next_month(key.0, key.1);
    }

    let daily = aggregate_daily(&sessions, zone);
    let mut days = Vec::new();
    let mut total_seconds = 0_u32;
    let mut elapsed_days = 0_u16;
    let mut date = first;
    loop {
        let stamp = yyyymmdd(date);
        let seconds = daily
            .iter()
            .find(|day| day.date == stamp)
            .map_or(0, |day| day.total_seconds);
        total_seconds = total_seconds.saturating_add(seconds);
        if stamp <= today {
            elapsed_days += 1;
        }
        days.push(PeriodDay {
            day: date.day,
            weekday: date.weekday,
            total_seconds: seconds,
            today: stamp == today,
            future: stamp > today,
        });
        if stamp >= last_date {
            break;
        }
        date = days_after(date, 1);
    }

    let has_older = view.back < view.period.max_back()
        && earliest_logged_month(stats_root).is_some_and(|(year, month)| {
            // The UTC month a log is filed under starts at most a day off
            // the local one: close enough to say whether to offer a step.
            yyyymmdd(calendar_date(year, month, 1)) < first_date
        });

    PeriodStats {
        view,
        first: CalendarDay::of(first),
        last: CalendarDay::of(last),
        days,
        total_seconds,
        elapsed_days,
        books: top_books(&sessions),
        has_older,
    }
}

/// Sum the sessions per book, sorted by total time descending and bounded
/// to [`PERIOD_BOOKS_LIMIT`] entries.
fn top_books(sessions: &[ReadingSession]) -> Vec<BookPeriodStats> {
    let mut totals: Vec<BookPeriodStats> = Vec::new();
    for session in sessions {
        let seconds = session.duration_seconds().min(u64::from(u32::MAX)) as u32;
        if let Some(entry) = totals
            .iter_mut()
            .find(|entry| entry.book_id == session.book_id)
        {
            entry.total_seconds = entry.total_seconds.saturating_add(seconds);
        } else {
            totals.push(BookPeriodStats {
                book_id: session.book_id,
                total_seconds: seconds,
            });
        }
    }
    totals.sort_by(|left, right| right.total_seconds.cmp(&left.total_seconds));
    totals.truncate(PERIOD_BOOKS_LIMIT);
    totals
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct OpenSession {
    book_id: u32,
    start_ts: u64,
    start_position: u64,
    last_ts: u64,
    last_position: u64,
    pages_turned: u16,
}

/// Owns the currently-open [`ReadingSession`], if any, and the logic that
/// opens, extends and closes it. Lives in main.rs alongside the other
/// hardware-adjacent trackers (`tap_diagnostics`, `storage_browser`, ...) --
/// not in `AppState`, which stays hardware-independent -- and is fed one
/// page-turn or inactivity check at a time from the event loop.
#[derive(Debug, Default)]
pub struct ReadingStatsTracker {
    open: Option<OpenSession>,
    inactivity_timeout_seconds: u64,
}

impl ReadingStatsTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            open: None,
            inactivity_timeout_seconds: INACTIVITY_TIMEOUT_SECONDS,
        }
    }

    #[must_use]
    pub fn with_inactivity_timeout(inactivity_timeout_seconds: u64) -> Self {
        Self {
            open: None,
            inactivity_timeout_seconds,
        }
    }

    /// Record one page turn landing on `position` in `book_id` at `now`.
    /// Starts a new session if none is open (or the previous one just went
    /// stale/changed book -- flushed first, see [`Self::close_session`]).
    pub fn note_page_turn(&mut self, book_id: u32, position: u64, now: u64, stats_root: &str) {
        self.close_if_stale(now, stats_root);
        match &mut self.open {
            Some(open) if open.book_id == book_id => {
                open.last_ts = now;
                open.last_position = position;
                open.pages_turned = open.pages_turned.saturating_add(1);
            }
            _ => {
                self.close_session(stats_root);
                self.open = Some(OpenSession {
                    book_id,
                    start_ts: now,
                    start_position: position,
                    last_ts: now,
                    last_position: position,
                    pages_turned: 1,
                });
            }
        }
    }

    /// Call periodically (e.g. every event-loop tick) so a reader who stops
    /// tapping without ever navigating away from the book still gets a
    /// closed, persisted session once the inactivity timeout elapses --
    /// rather than only on the next tap or an unrelated navigation.
    pub fn poll_inactivity(&mut self, now: u64, stats_root: &str) {
        self.close_if_stale(now, stats_root);
    }

    fn close_if_stale(&mut self, now: u64, stats_root: &str) {
        let is_stale = self
            .open
            .as_ref()
            .is_some_and(|open| now.saturating_sub(open.last_ts) > self.inactivity_timeout_seconds);
        if is_stale {
            self.close_session(stats_root);
        }
    }

    /// Close and persist the open session, if any -- using the last known
    /// good page-turn timestamp as `end_ts`, never the moment the timeout
    /// was noticed, so idle time is not counted as reading. Call this when
    /// the book is closed, when the Reader screen is left, and before a real
    /// MCU deep sleep (a full reboot, so anything left open in RAM would
    /// otherwise be silently lost). A no-op when nothing is open.
    pub fn close_session(&mut self, stats_root: &str) {
        let Some(open) = self.open.take() else {
            return;
        };
        let session = ReadingSession {
            book_id: open.book_id,
            start_ts: open.start_ts,
            end_ts: open.last_ts,
            start_position: open.start_position,
            end_position: open.last_position,
            pages_turned: open.pages_turned,
        };
        if !session.is_plausible() {
            return;
        }
        if let Err(error) = append_session(stats_root, session) {
            warn!("rustmix-wave=reading-stats-append status=failed error={error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_daily, append_session, book_id_for, compute_snapshot, current_streak_days,
        estimated_seconds_remaining, format_duration_seconds, local_date, period_bounds,
        read_month_log, resolve_unix_timestamp, top_books, unix_seconds_from_utc,
        weighted_chars_per_second, CalendarDay, CurrentBookProgress, DailyStats, ReadingSession,
        ReadingStatsTracker, StatsPeriod, StatsView,
    };
    use crate::{ntp::utc_from_unix_seconds, regional::TimeZoneProfile, rtc::RtcDateTime};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    const UTC: TimeZoneProfile = TimeZoneProfile::Utc;
    const ROME: TimeZoneProfile = TimeZoneProfile::EuropeRome;
    const WEEK: StatsView = StatsView {
        period: StatsPeriod::Week,
        back: 0,
    };
    const MONTH: StatsView = StatsView {
        period: StatsPeriod::Month,
        back: 0,
    };

    fn weeks_back(back: u16) -> StatsView {
        StatsView {
            period: StatsPeriod::Week,
            back,
        }
    }

    fn months_back(back: u16) -> StatsView {
        StatsView {
            period: StatsPeriod::Month,
            back,
        }
    }

    fn day(year: u16, month: u8, day: u8) -> CalendarDay {
        CalendarDay { year, month, day }
    }

    fn utc_day(seconds: u64) -> u32 {
        super::yyyymmdd(utc_from_unix_seconds(seconds))
    }

    fn unix(month: u8, day: u8, hour: u8, minute: u8) -> u64 {
        unix_in(2026, month, day, hour, minute)
    }

    fn unix_in(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> u64 {
        unix_seconds_from_utc(RtcDateTime {
            year,
            month,
            day,
            weekday: 0,
            hour,
            minute,
            second: 0,
        })
    }

    fn reading_day(date: u32) -> DailyStats {
        DailyStats {
            date,
            total_seconds: 600,
            sessions: 1,
            chars_read: 6_000,
        }
    }

    #[test]
    fn days_are_local_to_the_chosen_timezone() {
        // 23:30 UTC on 3 June is 01:30 on 4 June in Rome (CEST).
        let late = session(1, unix(6, 3, 23, 20), unix(6, 3, 23, 30), 0, 6_000);
        assert_eq!(aggregate_daily(&[late], ROME)[0].date, 20260604);
        assert_eq!(aggregate_daily(&[late], UTC)[0].date, 20260603);

        let root = fixture_root("local-days");
        append_session(&root, late).unwrap();
        let now = unix(6, 4, 8, 0);
        let rome = compute_snapshot(&root, now, None, ROME, WEEK);
        assert_eq!(rome.today_seconds, 600);
        assert_eq!(rome.streak_days, 1);
        // 4 June 2026 is a Thursday: the fourth day of a week from Monday.
        assert_eq!(rome.period.days[3].total_seconds, 600);
        assert!(rome.period.days[3].today);
        let utc = compute_snapshot(&root, now, None, UTC, WEEK);
        assert_eq!(utc.today_seconds, 0);
        assert_eq!(utc.period.days[2].total_seconds, 600);
        assert!(utc.period.days[3].today);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_month_is_the_local_one() {
        // 22:30 UTC on 30 June, logged in June's file, is 00:30 on 1 July in
        // Rome: July's reading there, June's in UTC.
        let root = fixture_root("local-month");
        append_session(
            &root,
            session(1, unix(6, 30, 22, 20), unix(6, 30, 22, 30), 0, 6_000),
        )
        .unwrap();
        let now = unix(7, 1, 8, 0);
        let july = |zone| compute_snapshot(&root, now, None, zone, MONTH).period;
        let june = |zone| compute_snapshot(&root, now, None, zone, months_back(1)).period;
        assert_eq!(july(ROME).total_seconds, 600);
        assert_eq!(july(ROME).books.len(), 1);
        assert_eq!(july(ROME).days[0].total_seconds, 600);
        assert_eq!(june(ROME).total_seconds, 0);
        assert_eq!(july(UTC).total_seconds, 0);
        assert_eq!(june(UTC).total_seconds, 600);
        assert_eq!(june(UTC).days[29].total_seconds, 600);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn streak_steps_through_a_daylight_saving_change() {
        // 29 March 2026 is 23 hours long in Rome. At 00:30 on 30 March, 24
        // hours earlier is already 28 March: stepping by seconds skipped the
        // 29th.
        let now = unix(3, 29, 22, 30);
        let daily = [
            reading_day(20260330),
            reading_day(20260329),
            reading_day(20260328),
        ];
        assert_eq!(current_streak_days(&daily, now, ROME), 3);
    }

    #[test]
    fn a_week_runs_monday_to_sunday_in_the_local_calendar() {
        // 22:30 UTC on Sunday 29 March 2026 is 00:30 on Monday 30 in Rome,
        // the night the clocks went forward: a new week there, the last day
        // of the old one in UTC.
        let root = fixture_root("weeks");
        for (day, hour) in [(28, 12), (29, 12), (29, 22)] {
            append_session(
                &root,
                session(1, unix(3, day, hour, 20), unix(3, day, hour, 30), 0, 6_000),
            )
            .unwrap();
        }
        let now = unix(3, 29, 22, 30);
        let seconds = |zone, view| {
            compute_snapshot(&root, now, None, zone, view)
                .period
                .days
                .iter()
                .map(|day| day.total_seconds)
                .collect::<Vec<_>>()
        };

        let this_week = compute_snapshot(&root, now, None, ROME, WEEK).period;
        assert_eq!(this_week.first, day(2026, 3, 30));
        assert_eq!(this_week.last, day(2026, 4, 5));
        assert_eq!(seconds(ROME, WEEK), [600, 0, 0, 0, 0, 0, 0]);
        assert!(this_week.days[0].today && !this_week.days[0].future);
        assert!(this_week.days[1..].iter().all(|day| day.future));
        assert_eq!(this_week.days[0].weekday, 1);
        assert_eq!(this_week.elapsed_days, 1);
        assert_eq!(this_week.average_seconds_per_day(), 600);

        let last_week = compute_snapshot(&root, now, None, ROME, weeks_back(1)).period;
        assert_eq!(last_week.first, day(2026, 3, 23));
        assert_eq!(last_week.last, day(2026, 3, 29));
        assert_eq!(seconds(ROME, weeks_back(1)), [0, 0, 0, 0, 0, 600, 600]);
        assert!(last_week.days.iter().all(|day| !day.future && !day.today));
        assert_eq!(last_week.total_seconds, 1_200);
        assert_eq!(last_week.elapsed_days, 7);
        assert_eq!(last_week.longest_day_seconds(), 600);

        assert_eq!(seconds(UTC, WEEK), [0, 0, 0, 0, 0, 600, 1_200]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_month_has_a_bar_for_each_of_its_days_and_steps_back_across_years() {
        let bounds = |year, month, day, view| {
            let today = local_date(unix_in(year, month, day, 12, 0), UTC);
            let (first, last) = period_bounds(today, view);
            (
                (first.year, first.month, first.day),
                (last.year, last.month, last.day),
            )
        };
        assert_eq!(bounds(2026, 1, 15, MONTH), ((2026, 1, 1), (2026, 1, 31)));
        assert_eq!(
            bounds(2026, 1, 15, months_back(1)),
            ((2025, 12, 1), (2025, 12, 31))
        );
        assert_eq!(
            bounds(2028, 3, 31, months_back(1)),
            ((2028, 2, 1), (2028, 2, 29))
        );
        // No further back than a year, whatever is asked.
        assert_eq!(
            bounds(2026, 1, 15, months_back(40)),
            bounds(2026, 1, 15, months_back(12))
        );
        assert_eq!(
            bounds(2026, 1, 15, months_back(12)),
            ((2025, 1, 1), (2025, 1, 31))
        );
        // Thursday 1 January 2026: its week starts in the year before.
        assert_eq!(bounds(2026, 1, 1, WEEK), ((2025, 12, 29), (2026, 1, 4)));

        let root = fixture_root("month-days");
        let february = compute_snapshot(&root, unix_in(2028, 2, 10, 12, 0), None, UTC, MONTH);
        assert_eq!(february.period.days.len(), 29);
        assert_eq!(february.period.days[9].day, 10);
        assert!(february.period.days[9].today);
        assert_eq!(february.period.elapsed_days, 10);
        assert_eq!(february.period.view, MONTH);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_old_period_reads_its_own_log_and_the_steps_back_stop_at_the_first_one() {
        // One sitting in November 2025, looked at in June 2026: its log is
        // none of those the streak reads.
        let root = fixture_root("old-period");
        append_session(
            &root,
            session(
                7,
                unix_in(2025, 11, 10, 20, 0),
                unix_in(2025, 11, 10, 20, 10),
                0,
                6_000,
            ),
        )
        .unwrap();
        let now = unix(6, 4, 8, 0);
        let period = |view| compute_snapshot(&root, now, None, UTC, view).period;

        let november = period(months_back(7));
        assert_eq!(november.first, day(2025, 11, 1));
        assert_eq!(november.total_seconds, 600);
        assert_eq!(november.days[9].total_seconds, 600);
        assert_eq!(november.books[0].book_id, 7);
        assert_eq!(november.elapsed_days, 30);
        assert!(!november.has_older, "nothing is logged before November");

        assert!(period(months_back(6)).has_older);
        assert_eq!(period(months_back(6)).total_seconds, 0);
        assert!(period(MONTH).has_older);
        assert!(period(WEEK).has_older);
        // Monday 10 November 2025 opens its week; the week before holds the
        // first days of the month the log is named after.
        let week = (0..=52)
            .map(|back| period(weeks_back(back)))
            .find(|week| week.total_seconds > 0)
            .unwrap();
        assert_eq!(week.first, day(2025, 11, 10));
        assert_eq!(week.days[0].total_seconds, 600);
        assert!(week.has_older);
        assert!(!period(weeks_back(week.view.back + 2)).has_older);

        // A year back is as far as it goes.
        assert!(!period(months_back(12)).has_older);
        assert!(!period(weeks_back(52)).has_older);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_any_log_there_is_no_older_period() {
        let root = fixture_root("no-logs");
        let now = unix(6, 4, 8, 0);
        for view in [WEEK, MONTH] {
            let period = compute_snapshot(&root, now, None, UTC, view).period;
            assert!(!period.has_older);
            assert_eq!(period.total_seconds, 0);
            assert!(period.books.is_empty());
        }
        fs::remove_dir_all(&root).unwrap();
    }

    fn fixture_root(label: &str) -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root: PathBuf = std::env::temp_dir().join(format!("reading-stats-{label}-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        root.to_string_lossy().into_owned()
    }

    fn session(
        book_id: u32,
        start_ts: u64,
        end_ts: u64,
        start_position: u64,
        end_position: u64,
    ) -> ReadingSession {
        ReadingSession {
            book_id,
            start_ts,
            end_ts,
            start_position,
            end_position,
            pages_turned: 1,
        }
    }

    #[test]
    fn book_id_is_stable_and_distinguishes_books() {
        let a = book_id_for("/sdcard/RUSTMIX/BOOKS/A.TXT", 1_000, 500);
        let b = book_id_for("/sdcard/RUSTMIX/BOOKS/A.TXT", 1_000, 500);
        let c = book_id_for("/sdcard/RUSTMIX/BOOKS/B.TXT", 1_000, 500);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn log_line_round_trips_through_parse() {
        let original = session(42, 1_780_000_000, 1_780_000_600, 100, 900);
        let line = original.to_log_line();
        assert_eq!(ReadingSession::parse_log_line(&line), Some(original));
    }

    #[test]
    fn parse_log_line_rejects_malformed_input() {
        assert_eq!(ReadingSession::parse_log_line(""), None);
        assert_eq!(ReadingSession::parse_log_line("1|2|3"), None);
        assert_eq!(ReadingSession::parse_log_line("1|2|3|4|5|6|7"), None);
        assert_eq!(ReadingSession::parse_log_line("a|b|c|d|e|f"), None);
    }

    #[test]
    fn plausibility_filters_accidental_taps_and_rapid_skims() {
        // Real reading: 400 chars in 200s = 2 char/s.
        assert!(session(1, 0, 200, 0, 400).is_plausible());
        // Accidental tap: under the minimum duration.
        assert!(!session(1, 0, 5, 0, 400).is_plausible());
        // Rapid skim/search: over the plausible speed ceiling.
        assert!(!session(1, 0, 20, 0, 2_000).is_plausible());
    }

    #[test]
    fn tracker_closes_stale_session_using_last_tap_timestamp_not_the_timeout_moment() {
        let root = fixture_root("close-stale");
        let mut tracker = ReadingStatsTracker::with_inactivity_timeout(300);
        tracker.note_page_turn(7, 0, 1_000, &root);
        tracker.note_page_turn(7, 400, 1_060, &root);
        // A much-later tap on the same book, after the 300s timeout elapsed.
        tracker.note_page_turn(7, 5_000, 2_000, &root);

        let sessions = read_month_log(
            &root,
            utc_from_unix_seconds(1_000).year,
            utc_from_unix_seconds(1_000).month,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].start_ts, 1_000);
        // Closed at the last valid tap (1_060), not at the 2_000 timestamp
        // that revealed the gap.
        assert_eq!(sessions[0].end_ts, 1_060);
        assert_eq!(sessions[0].end_position, 400);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tracker_flushes_previous_book_before_opening_a_new_one() {
        let root = fixture_root("switch-book");
        let mut tracker = ReadingStatsTracker::new();
        tracker.note_page_turn(1, 0, 10_000, &root);
        tracker.note_page_turn(1, 300, 10_100, &root);
        tracker.note_page_turn(2, 0, 10_200, &root);
        tracker.note_page_turn(2, 300, 10_300, &root);
        tracker.close_session(&root);

        let sessions = read_month_log(
            &root,
            utc_from_unix_seconds(10_000).year,
            utc_from_unix_seconds(10_000).month,
        );
        assert_eq!(sessions.iter().filter(|s| s.book_id == 1).count(), 1);
        assert!(sessions.iter().any(|s| s.book_id == 2));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn poll_inactivity_closes_without_a_further_page_turn() {
        let root = fixture_root("poll-inactivity");
        let mut tracker = ReadingStatsTracker::with_inactivity_timeout(60);
        tracker.note_page_turn(1, 0, 0, &root);
        tracker.note_page_turn(1, 200, 30, &root);
        tracker.poll_inactivity(200, &root); // 200 - 30 > 60s timeout
        let sessions = read_month_log(
            &root,
            utc_from_unix_seconds(0).year,
            utc_from_unix_seconds(0).month,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].end_ts, 30);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn implausible_session_is_discarded_not_persisted() {
        let root = fixture_root("discard-implausible");
        let mut tracker = ReadingStatsTracker::new();
        tracker.note_page_turn(1, 0, 100, &root);
        tracker.close_session(&root); // duration 0s, below MIN_SESSION_SECONDS
        let sessions = read_month_log(
            &root,
            utc_from_unix_seconds(100).year,
            utc_from_unix_seconds(100).month,
        );
        assert!(sessions.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn append_writes_to_the_session_start_months_log_file() {
        let root = fixture_root("append-month");
        let start = 1_780_000_000_u64; // 2026-06-03 in this project's reference instant.
        append_session(&root, session(1, start, start + 600, 0, 500)).unwrap();
        let utc = utc_from_unix_seconds(start);
        let expected_path =
            PathBuf::from(&root).join(format!("{:04}{:02}.LOG", utc.year, utc.month));
        assert!(expected_path.exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn aggregate_daily_sums_multiple_sessions_on_the_same_day() {
        let day_seconds = 1_780_000_000_u64 - (1_780_000_000_u64 % 86_400);
        let sessions = vec![
            session(1, day_seconds + 100, day_seconds + 700, 0, 600),
            session(1, day_seconds + 800, day_seconds + 1_100, 600, 900),
        ];
        let daily = aggregate_daily(&sessions, UTC);
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].total_seconds, 600 + 300);
        assert_eq!(daily[0].sessions, 2);
        assert_eq!(daily[0].chars_read, 900);
    }

    #[test]
    fn streak_counts_back_from_today_and_stops_at_a_gap() {
        let now = 10 * 86_400 + 3_600; // some time on "day 10".
        let today = utc_day(now);
        let yesterday = utc_day(now - 86_400);
        let two_days_ago = utc_day(now - 2 * 86_400);
        let daily = vec![
            DailyStats {
                date: today,
                total_seconds: 60,
                sessions: 1,
                chars_read: 10,
            },
            DailyStats {
                date: yesterday,
                total_seconds: 60,
                sessions: 1,
                chars_read: 10,
            },
            DailyStats {
                date: two_days_ago,
                total_seconds: 0,
                sessions: 0,
                chars_read: 0,
            },
        ];
        assert_eq!(current_streak_days(&daily, now, UTC), 2);
    }

    #[test]
    fn streak_is_zero_when_today_has_no_reading_yet() {
        let now = 10 * 86_400 + 3_600;
        let yesterday = utc_day(now - 86_400);
        let daily = vec![DailyStats {
            date: yesterday,
            total_seconds: 60,
            sessions: 1,
            chars_read: 10,
        }];
        assert_eq!(current_streak_days(&daily, now, UTC), 0);
    }

    #[test]
    fn weighted_speed_favors_recent_faster_sessions_via_decay() {
        let now = 100_000;
        // Older, slower sessions further back in time; a faster one most recent.
        let sessions = vec![
            session(1, now - 500, now - 400, 0, 100), // 1 char/s, oldest
            session(1, now - 300, now - 200, 0, 100), // 1 char/s
            session(1, now - 100, now - 50, 0, 100),  // 2 char/s, most recent
        ];
        let speed = weighted_chars_per_second(&sessions, now).unwrap();
        // Weighted toward the faster, most-recent session, so above the
        // unweighted mean of ~1.33 char/s.
        assert!(
            speed > 1.33,
            "expected decay-weighted speed above simple mean, got {speed}"
        );
    }

    #[test]
    fn weighted_speed_is_none_without_any_sessions() {
        assert_eq!(weighted_chars_per_second(&[], 1_000), None);
    }

    #[test]
    fn estimated_remaining_time_scales_with_speed() {
        assert_eq!(estimated_seconds_remaining(0, 200, 2.0), Some(100));
        assert_eq!(estimated_seconds_remaining(150, 200, 2.0), Some(25));
        assert_eq!(estimated_seconds_remaining(250, 200, 2.0), None);
        assert_eq!(estimated_seconds_remaining(0, 200, 0.0), None);
    }

    #[test]
    fn unix_seconds_from_utc_round_trips_with_utc_from_unix_seconds() {
        let reference = 1_780_488_000_u64;
        let utc = utc_from_unix_seconds(reference);
        assert_eq!(unix_seconds_from_utc(utc), reference);
    }

    #[test]
    fn resolve_unix_timestamp_falls_back_to_rtc_when_system_clock_is_unsynced() {
        // The host test clock is always SNTP-plausible, so this only checks
        // the fallback's own conversion math via a direct call.
        let rtc = RtcDateTime {
            year: 2026,
            month: 6,
            day: 3,
            weekday: 3,
            hour: 12,
            minute: 0,
            second: 0,
        };
        assert_eq!(unix_seconds_from_utc(rtc), 1_780_488_000);
        assert!(resolve_unix_timestamp(Some(rtc)).is_some());
    }

    #[test]
    fn compute_snapshot_reports_unavailable_speed_without_history() {
        let root = fixture_root("empty-snapshot");
        let snapshot = compute_snapshot(&root, 1_780_488_000, None, UTC, WEEK);
        assert!(snapshot.available);
        assert_eq!(snapshot.today_seconds, 0);
        assert_eq!(snapshot.streak_days, 0);
        assert_eq!(snapshot.chars_per_minute, None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compute_snapshot_estimates_remaining_time_from_recorded_speed() {
        let root = fixture_root("remaining-time");
        let now = 1_780_488_000_u64;
        // 2 char/s over a plausible 100s session ending now.
        append_session(&root, session(1, now - 100, now, 0, 200)).unwrap();
        let progress = CurrentBookProgress {
            current_position: 200,
            chapter_end_position: Some(400),
            book_end_position: Some(1_000),
        };
        let snapshot = compute_snapshot(&root, now, Some(progress), UTC, WEEK);
        assert_eq!(snapshot.chars_per_minute, Some(120));
        assert_eq!(snapshot.remaining_chapter_seconds, Some(100));
        assert_eq!(snapshot.remaining_book_seconds, Some(400));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn top_books_sums_per_book_and_orders_by_time_descending() {
        let sessions = vec![
            session(1, 0, 100, 0, 50),
            session(2, 200, 500, 0, 50),
            session(1, 600, 750, 50, 100),
        ];
        let top = top_books(&sessions);
        assert_eq!(top.len(), 2);
        assert_eq!(
            top[0].book_id, 2,
            "book 2's single 300s session outranks book 1's summed 250s"
        );
        assert_eq!(top[0].total_seconds, 300);
        assert_eq!(top[1].book_id, 1);
        assert_eq!(top[1].total_seconds, 100 + 150);
    }

    #[test]
    fn top_books_is_bounded_to_the_display_limit() {
        let sessions: Vec<ReadingSession> = (0..10)
            .map(|book_id| session(book_id, 0, 100, 0, 10))
            .collect();
        assert_eq!(top_books(&sessions).len(), super::PERIOD_BOOKS_LIMIT);
    }

    #[test]
    fn formats_compact_duration_labels() {
        assert_eq!(format_duration_seconds(0), "0m");
        assert_eq!(format_duration_seconds(59), "0m");
        assert_eq!(format_duration_seconds(60), "1m");
        assert_eq!(format_duration_seconds(3_600), "1h 00m");
        assert_eq!(format_duration_seconds(12_020), "3h 20m");
    }
}
