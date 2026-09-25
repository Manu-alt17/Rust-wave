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

use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use log::warn;

use crate::{ntp, rtc::RtcDateTime};

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

fn yyyymmdd_from_unix_seconds(seconds: u64) -> u32 {
    let utc = ntp::utc_from_unix_seconds(seconds);
    u32::from(utc.year) * 10_000 + u32::from(utc.month) * 100 + u32::from(utc.day)
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

/// Read the current month's log plus `months_back` preceding months' logs.
/// Bounded and exact: it walks calendar months by field arithmetic rather
/// than jumping fixed day counts, so it never skips a short month.
fn read_sessions_recent_months(
    stats_root: &str,
    months_back: u32,
    now: u64,
) -> Vec<ReadingSession> {
    let start = ntp::utc_from_unix_seconds(now);
    let mut year = start.year;
    let mut month = start.month;
    let mut sessions = Vec::new();
    for _ in 0..=months_back {
        sessions.extend(read_month_log(stats_root, year, month));
        if month == 1 {
            month = 12;
            year = year.saturating_sub(1);
        } else {
            month -= 1;
        }
    }
    sessions
}

fn aggregate_daily(sessions: &[ReadingSession]) -> Vec<DailyStats> {
    let mut days: Vec<DailyStats> = Vec::new();
    for session in sessions {
        // Attribute the session to the day it ended on -- a session that
        // happens to straddle midnight is rare enough on a single sitting
        // that splitting it isn't worth the complexity.
        let date = yyyymmdd_from_unix_seconds(session.end_ts);
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

/// Consecutive days, counted back from `now`, with `total_seconds > 0`.
/// Stops at the first gap -- if today has no reading logged yet, the streak
/// is reported as 0 until today's first session closes.
#[must_use]
pub fn current_streak_days(daily: &[DailyStats], now: u64) -> u32 {
    let mut streak = 0;
    let mut cursor = now;
    for _ in 0..STREAK_SAFETY_CAP_DAYS {
        let date = yyyymmdd_from_unix_seconds(cursor);
        let has_reading = daily
            .iter()
            .any(|day| day.date == date && day.total_seconds > 0);
        if !has_reading {
            break;
        }
        streak += 1;
        let Some(previous_day) = cursor.checked_sub(86_400) else {
            break;
        };
        cursor = previous_day;
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

/// One day's reading total for the last-7-days bar chart, carrying the
/// weekday (`0` = Sunday .. `6` = Saturday, matching
/// [`ntp::utc_from_unix_seconds`]) so the screen can render a locale-aware
/// day-initial label without needing wall-clock access of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DayBar {
    pub weekday: u8,
    pub total_seconds: u32,
}

/// Top N books (by [`BOOKS_THIS_MONTH_LIMIT`]) is the number of entries a
/// screen should ever need to lay out at once for the "read this month"
/// list.
pub const BOOKS_THIS_MONTH_LIMIT: usize = 5;

/// One book's total reading time within the current month. Carries only
/// [`Self::book_id`] -- the screen resolves title/cover/percent from
/// `AppState`'s already-in-RAM Reader library state, which `reading_stats`
/// deliberately does not depend on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BookMonthStats {
    pub book_id: u32,
    pub total_seconds: u32,
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
    pub week_seconds: u32,
    pub month_seconds: u32,
    pub streak_days: u32,
    pub chars_per_minute: Option<u32>,
    pub remaining_chapter_seconds: Option<u64>,
    pub remaining_book_seconds: Option<u64>,
    /// Oldest to newest, index 6 is today.
    pub last_7_days: [DayBar; 7],
    /// Sorted by total time descending, bounded to [`BOOKS_THIS_MONTH_LIMIT`].
    pub books_this_month: Vec<BookMonthStats>,
}

/// Lazily aggregate everything the Reading Stats screen shows by reading
/// only the current month's log plus, for the streak, up to
/// [`STREAK_LOOKBACK_MONTHS`] preceding months -- never the device's entire
/// reading history.
#[must_use]
pub fn compute_snapshot(
    stats_root: &str,
    now: u64,
    book_progress: Option<CurrentBookProgress>,
) -> ReadingStatsSnapshot {
    let today = yyyymmdd_from_unix_seconds(now);

    let current_month = ntp::utc_from_unix_seconds(now);
    let month_sessions = read_month_log(stats_root, current_month.year, current_month.month);
    let month_seconds: u32 = month_sessions
        .iter()
        .map(ReadingSession::duration_seconds)
        .sum::<u64>()
        .min(u64::from(u32::MAX)) as u32;

    // Current + previous month always covers any trailing 7-day window and
    // the 10-session speed window, regardless of where in the month `now`
    // falls.
    let recent_sessions = read_sessions_recent_months(stats_root, 1, now);
    let recent_daily = aggregate_daily(&recent_sessions);
    let today_stats = recent_daily.iter().find(|day| day.date == today).copied();
    let week_start = now.saturating_sub(6 * 86_400);
    let week_seconds: u32 = recent_sessions
        .iter()
        .filter(|session| session.end_ts >= week_start && session.end_ts <= now)
        .map(ReadingSession::duration_seconds)
        .sum::<u64>()
        .min(u64::from(u32::MAX)) as u32;

    let streak_sessions = read_sessions_recent_months(stats_root, STREAK_LOOKBACK_MONTHS, now);
    let streak_daily = aggregate_daily(&streak_sessions);
    let streak_days = current_streak_days(&streak_daily, now);

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
        week_seconds,
        month_seconds,
        streak_days,
        chars_per_minute,
        remaining_chapter_seconds,
        remaining_book_seconds,
        last_7_days: last_7_days_bars(&recent_daily, now),
        books_this_month: top_books_this_month(&month_sessions),
    }
}

/// Build the last-7-days bar-chart data (oldest to newest, today last) from
/// an already-aggregated daily list -- `recent_daily` in [`compute_snapshot`]
/// always covers the current + previous month, which is always enough for
/// any trailing 7-day window regardless of where in the month `now` falls.
fn last_7_days_bars(daily: &[DailyStats], now: u64) -> [DayBar; 7] {
    let mut bars = [DayBar::default(); 7];
    for (offset, bar) in bars.iter_mut().enumerate() {
        let days_back = 6 - offset as u64;
        let Some(timestamp) = now.checked_sub(days_back * 86_400) else {
            continue;
        };
        let date = yyyymmdd_from_unix_seconds(timestamp);
        let weekday = ntp::utc_from_unix_seconds(timestamp).weekday;
        let total_seconds = daily
            .iter()
            .find(|day| day.date == date)
            .map_or(0, |day| day.total_seconds);
        *bar = DayBar {
            weekday,
            total_seconds,
        };
    }
    bars
}

/// Sum this month's sessions per book, sorted by total time descending and
/// bounded to [`BOOKS_THIS_MONTH_LIMIT`] entries.
fn top_books_this_month(month_sessions: &[ReadingSession]) -> Vec<BookMonthStats> {
    let mut totals: Vec<BookMonthStats> = Vec::new();
    for session in month_sessions {
        let seconds = session.duration_seconds().min(u64::from(u32::MAX)) as u32;
        if let Some(entry) = totals
            .iter_mut()
            .find(|entry| entry.book_id == session.book_id)
        {
            entry.total_seconds = entry.total_seconds.saturating_add(seconds);
        } else {
            totals.push(BookMonthStats {
                book_id: session.book_id,
                total_seconds: seconds,
            });
        }
    }
    totals.sort_by(|left, right| right.total_seconds.cmp(&left.total_seconds));
    totals.truncate(BOOKS_THIS_MONTH_LIMIT);
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
        estimated_seconds_remaining, format_duration_seconds, last_7_days_bars, read_month_log,
        resolve_unix_timestamp, top_books_this_month, unix_seconds_from_utc,
        weighted_chars_per_second, CurrentBookProgress, DailyStats, ReadingSession,
        ReadingStatsTracker,
    };
    use crate::{ntp::utc_from_unix_seconds, rtc::RtcDateTime};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

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
        let daily = aggregate_daily(&sessions);
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].total_seconds, 600 + 300);
        assert_eq!(daily[0].sessions, 2);
        assert_eq!(daily[0].chars_read, 900);
    }

    #[test]
    fn streak_counts_back_from_today_and_stops_at_a_gap() {
        let now = 10 * 86_400 + 3_600; // some time on "day 10".
        let today = super::yyyymmdd_from_unix_seconds(now);
        let yesterday = super::yyyymmdd_from_unix_seconds(now - 86_400);
        let two_days_ago = super::yyyymmdd_from_unix_seconds(now - 2 * 86_400);
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
        assert_eq!(current_streak_days(&daily, now), 2);
    }

    #[test]
    fn streak_is_zero_when_today_has_no_reading_yet() {
        let now = 10 * 86_400 + 3_600;
        let yesterday = super::yyyymmdd_from_unix_seconds(now - 86_400);
        let daily = vec![DailyStats {
            date: yesterday,
            total_seconds: 60,
            sessions: 1,
            chars_read: 10,
        }];
        assert_eq!(current_streak_days(&daily, now), 0);
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
        let snapshot = compute_snapshot(&root, 1_780_488_000, None);
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
        let snapshot = compute_snapshot(&root, now, Some(progress));
        assert_eq!(snapshot.chars_per_minute, Some(120));
        assert_eq!(snapshot.remaining_chapter_seconds, Some(100));
        assert_eq!(snapshot.remaining_book_seconds, Some(400));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn last_7_days_bars_places_today_last_and_fills_gaps_with_zero() {
        let now = 1_780_488_000_u64; // 2026-06-03, some weekday.
        let today = super::yyyymmdd_from_unix_seconds(now);
        let two_days_ago = super::yyyymmdd_from_unix_seconds(now - 2 * 86_400);
        let daily = vec![
            DailyStats {
                date: today,
                total_seconds: 600,
                sessions: 1,
                chars_read: 100,
            },
            DailyStats {
                date: two_days_ago,
                total_seconds: 300,
                sessions: 1,
                chars_read: 50,
            },
        ];
        let bars = last_7_days_bars(&daily, now);
        assert_eq!(bars[6].total_seconds, 600, "today must be the last entry");
        assert_eq!(
            bars[4].total_seconds, 300,
            "two days ago is index 4 (6 - 2)"
        );
        assert_eq!(
            bars[0].total_seconds, 0,
            "days with no log entry read as zero"
        );
        // Weekday advances by exactly one each entry (no calendar skips).
        for pair in bars.windows(2) {
            assert_eq!((pair[0].weekday + 1) % 7, pair[1].weekday);
        }
    }

    #[test]
    fn top_books_this_month_sums_per_book_and_orders_by_time_descending() {
        let sessions = vec![
            session(1, 0, 100, 0, 50),
            session(2, 200, 500, 0, 50),
            session(1, 600, 750, 50, 100),
        ];
        let top = top_books_this_month(&sessions);
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
    fn top_books_this_month_is_bounded_to_the_display_limit() {
        let sessions: Vec<ReadingSession> = (0..10)
            .map(|book_id| session(book_id, 0, 100, 0, 10))
            .collect();
        assert_eq!(
            top_books_this_month(&sessions).len(),
            super::BOOKS_THIS_MONTH_LIMIT
        );
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
