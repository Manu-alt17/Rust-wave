//! Reading time by week or by month, with speed, streak and time remaining.
//!
//! Purely a view over `state.reading_stats` (see
//! [`crate::reading_stats::ReadingStatsSnapshot`]) plus the already-in-RAM
//! Reader library state (`state.reader.recent`/`positions`, to resolve a
//! logged book id back to a title) -- the runtime owner in main.rs computes
//! the snapshot from SD when this screen is opened and whenever the period
//! on show changes, so this module never touches storage itself.
//!
//! The screen is one period at a time: SELECT changes between the week and
//! the month, the rocker goes back through the earlier ones. Its chart has
//! a bar per day against a time axis, so a bar says how long as well as
//! which day.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        display::DisplayPreferences,
        i18n::t,
        state::AppState,
        typography::{Text, UiTextRole, UiTextStyle},
        widgets::{
            footer::{back_action, back_only, draw_footer, footer_hints, FooterKey},
            header::draw_header,
            layout::{CONTENT_BOTTOM, CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{
                centered_baseline, draw_list_row, draw_row_frame_at, ROW_HEIGHT, ROW_PAD_X,
                ROW_STEP,
            },
            text::{draw_paragraph, draw_text_centered, draw_text_fit, draw_text_right},
        },
    },
    orientation::OrientedFrameBuffer,
    reading_stats::{
        book_id_for, format_duration_seconds, BookPeriodStats, CalendarDay, PeriodDay, PeriodStats,
        StatsPeriod,
    },
    regional::Locale,
};

/// Height of the Week / Month switch.
const SWITCH_HEIGHT: i32 = 46;
const CARD_GAP: i32 = 16;
const CARD_HEIGHT: i32 = 86;
const CARD_WIDTH: i32 = (CONTENT_WIDTH - CARD_GAP) / 2;
/// Height of the chart's plot, from its top line to its base line.
const CHART_HEIGHT: i32 = 116;
/// Gap between the bars of a week; a month's bars keep [`MONTH_BAR_GAP`].
const WEEK_BAR_GAP: i32 = 10;
const MONTH_BAR_GAP: i32 = 2;
/// Gap between the axis labels and the plot.
const AXIS_GAP: i32 = 8;
/// Height of the mark under today's bar, and its distance from the base.
const TODAY_MARK: i32 = 3;
/// Values the top of the time axis may take, in seconds: the smallest one
/// the longest day fits under is used, so the top line and the one at half
/// height always read as round times.
const AXIS_TOPS: [u32; 12] = [
    600, 1_200, 1_800, 3_600, 7_200, 10_800, 14_400, 21_600, 28_800, 43_200, 57_600, 86_400,
];

pub fn render_reading_stats(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let stats = &state.reading_stats;
    let period = &stats.period;
    let body = preferences.body_style();
    let heading = preferences.heading_style();
    let body_line = i32::from(body.line_height());
    let heading_line = i32::from(heading.line_height());
    draw_header(display, state, t(locale, "STATISTICS", "STATISTICHE"))?;

    if !stats.available {
        draw_paragraph(
            display,
            t(
                locale,
                "The clock is not set yet: reading time cannot be counted. Set it in Settings, Clock.",
                "L'orologio non \u{00E8} ancora impostato: il tempo di lettura non si pu\u{00F2} contare. Impostalo in Opzioni, Orologio.",
            ),
            CONTENT_LEFT,
            FIRST_BASELINE,
            body,
            CONTENT_WIDTH,
            5,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }

    draw_period_switch(display, FIRST_ROW_TOP, period.view.period, locale, heading)?;

    // Everything below is stacked by line height, so the three text sizes
    // never push one line into the next.
    let mut baseline = FIRST_ROW_TOP + SWITCH_HEIGHT + 12 + heading_line;
    let (title, detail) = period_title(locale, period);
    let detail_width = if detail.is_empty() {
        0
    } else {
        draw_text_right(
            display,
            &detail,
            CONTENT_LEFT + CONTENT_WIDTH,
            baseline,
            body,
            CONTENT_WIDTH / 2,
        )?;
        body.text_width(&detail).min(CONTENT_WIDTH / 2) + 12
    };
    draw_text_fit(
        display,
        &title,
        Point::new(CONTENT_LEFT, baseline),
        heading,
        CONTENT_WIDTH - detail_width,
    )?;

    let cards_top = baseline + 12;
    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT, cards_top),
        t(locale, "Total", "Totale"),
        &format_duration_seconds(u64::from(period.total_seconds)),
        preferences,
    )?;
    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT + CARD_WIDTH + CARD_GAP, cards_top),
        t(locale, "Daily average", "Media al giorno"),
        &format_duration_seconds(u64::from(period.average_seconds_per_day())),
        preferences,
    )?;

    // Half a line above the plot for the top axis label, which is centered
    // on its line.
    let chart_top = cards_top + CARD_HEIGHT + 8 + body_line;
    draw_period_chart(display, chart_top, period, locale, body)?;

    baseline = chart_top + CHART_HEIGHT + 2 * TODAY_MARK + 4 + body_line + 10 + body_line;
    let today_streak_line = format!(
        "{}: {} \u{00B7} {}: {}",
        t(locale, "Today", "Oggi"),
        format_duration_seconds(u64::from(stats.today_seconds)),
        t(locale, "Streak", "Serie"),
        streak_value_label(locale, stats.streak_days),
    );
    draw_text_fit(
        display,
        &today_streak_line,
        Point::new(CONTENT_LEFT, baseline),
        body,
        CONTENT_WIDTH,
    )?;
    baseline += body_line + 4;
    draw_text_fit(
        display,
        &speed_label(locale, stats),
        Point::new(CONTENT_LEFT, baseline),
        body,
        CONTENT_WIDTH,
    )?;
    if let Some(seconds) = stats.remaining_book_seconds {
        baseline += body_line + 4;
        let remaining = format!(
            "{} {} {}",
            t(locale, "About", "Circa"),
            format_duration_seconds(seconds),
            t(locale, "left in this book", "rimanenti in questo libro")
        );
        draw_text_fit(
            display,
            &remaining,
            Point::new(CONTENT_LEFT, baseline),
            body,
            CONTENT_WIDTH,
        )?;
    }

    // The books of the period take the room that is left: as many rows as
    // fit above the footer, none when not even one does.
    let books_baseline = baseline + 10 + heading_line;
    let rows_top = books_baseline + 10;
    let room = (CONTENT_BOTTOM - rows_top + (ROW_STEP - ROW_HEIGHT)) / ROW_STEP;
    if period.books.is_empty() {
        if books_baseline + 10 + body_line <= CONTENT_BOTTOM {
            draw_text_fit(
                display,
                t(locale, "Books read", "Libri letti"),
                Point::new(CONTENT_LEFT, books_baseline),
                heading,
                CONTENT_WIDTH,
            )?;
            draw_text_fit(
                display,
                t(
                    locale,
                    "No reading in this period.",
                    "Nessuna lettura in questo periodo.",
                ),
                Point::new(CONTENT_LEFT, books_baseline + 10 + body_line),
                body,
                CONTENT_WIDTH,
            )?;
        }
    } else if room > 0 {
        draw_text_fit(
            display,
            t(locale, "Books read", "Libri letti"),
            Point::new(CONTENT_LEFT, books_baseline),
            heading,
            CONTENT_WIDTH,
        )?;
        for (index, book) in period.books.iter().take(room as usize).enumerate() {
            draw_book_row(display, rows_top + index as i32 * ROW_STEP, book, state)?;
        }
    }

    draw_footer(display, state, &footer_hint(locale, state))
}

/// `SELECT MESE · SU/GIÙ PERIODO · BOOT INDIETRO`: the rocker is named only
/// while there is a period to go to.
fn footer_hint(locale: Locale, state: &AppState) -> String {
    let period = &state.reading_stats.period;
    let other = match period.view.period {
        StatsPeriod::Week => t(locale, "MONTH", "MESE"),
        StatsPeriod::Month => t(locale, "WEEK", "SETTIMANA"),
    };
    let mut parts = vec![(FooterKey::Select, other)];
    if period.has_older || period.view.back > 0 {
        parts.push((FooterKey::UpDown, t(locale, "PERIOD", "PERIODO")));
    }
    parts.push((FooterKey::Boot, back_action(locale)));
    footer_hints(locale, &parts)
}

/// The Week / Month switch: both names in one frame, the one on show in a
/// filled half.
fn draw_period_switch(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    shown: StatsPeriod,
    locale: Locale,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    draw_row_frame_at(
        display,
        CONTENT_LEFT,
        top,
        CONTENT_WIDTH,
        SWITCH_HEIGHT,
        false,
    )?;
    let half = CONTENT_WIDTH / 2;
    let baseline = centered_baseline(style, top, SWITCH_HEIGHT);
    for (index, (period, label)) in [
        (StatsPeriod::Week, t(locale, "Week", "Settimana")),
        (StatsPeriod::Month, t(locale, "Month", "Mese")),
    ]
    .into_iter()
    .enumerate()
    {
        let left = CONTENT_LEFT + index as i32 * half;
        let selected = period == shown;
        if selected {
            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(left + 4, top + 4),
                    Size::new((half - 8) as u32, (SWITCH_HEIGHT - 8) as u32),
                ),
                CornerRadii::new(Size::new(9, 9)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
        let color = if selected {
            BinaryColor::Off
        } else {
            BinaryColor::On
        };
        draw_text_centered(
            display,
            label,
            left + 8,
            half - 16,
            baseline,
            style.with_color(color),
        )?;
    }
    Ok(())
}

const MONTHS_SHORT_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_SHORT_IT: [&str; 12] = [
    "gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_IT: [&str; 12] = [
    "Gennaio",
    "Febbraio",
    "Marzo",
    "Aprile",
    "Maggio",
    "Giugno",
    "Luglio",
    "Agosto",
    "Settembre",
    "Ottobre",
    "Novembre",
    "Dicembre",
];

fn month_index(month: u8) -> usize {
    usize::from(month.clamp(1, 12)) - 1
}

/// "29 set – 5 ott", "5 – 11 ott"; "Sep 29 – Oct 5", "Oct 5 – 11".
fn day_range(locale: Locale, first: CalendarDay, last: CalendarDay) -> String {
    let same_month = (first.year, first.month) == (last.year, last.month);
    match locale {
        Locale::Italian => {
            let month = |day: CalendarDay| MONTHS_SHORT_IT[month_index(day.month)];
            if same_month {
                format!("{} \u{2013} {} {}", first.day, last.day, month(last))
            } else {
                format!(
                    "{} {} \u{2013} {} {}",
                    first.day,
                    month(first),
                    last.day,
                    month(last)
                )
            }
        }
        Locale::English => {
            let month = |day: CalendarDay| MONTHS_SHORT_EN[month_index(day.month)];
            if same_month {
                format!("{} {} \u{2013} {}", month(first), first.day, last.day)
            } else {
                format!(
                    "{} {} \u{2013} {} {}",
                    month(first),
                    first.day,
                    month(last),
                    last.day
                )
            }
        }
    }
}

/// "Ottobre 2026".
fn month_and_year(locale: Locale, day: CalendarDay) -> String {
    let name = match locale {
        Locale::Italian => MONTHS_IT[month_index(day.month)],
        Locale::English => MONTHS_EN[month_index(day.month)],
    };
    format!("{name} {}", day.year)
}

/// The period's name on the left and, for the current and the last one,
/// its dates on the right: "Questa settimana" with "5 – 11 ott". An
/// earlier period is named by its dates alone.
fn period_title(locale: Locale, period: &PeriodStats) -> (String, String) {
    let dates = match period.view.period {
        StatsPeriod::Week => day_range(locale, period.first, period.last),
        StatsPeriod::Month => month_and_year(locale, period.first),
    };
    let name = match (period.view.period, period.view.back) {
        (StatsPeriod::Week, 0) => t(locale, "This week", "Questa settimana"),
        (StatsPeriod::Week, 1) => t(locale, "Last week", "Settimana scorsa"),
        (StatsPeriod::Month, 0) => t(locale, "This month", "Questo mese"),
        (StatsPeriod::Month, 1) => t(locale, "Last month", "Mese scorso"),
        _ => return (dates, String::new()),
    };
    (name.to_string(), dates)
}

fn draw_stat_card(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    label: &str,
    value: &str,
    preferences: DisplayPreferences,
) -> Result<(), Infallible> {
    draw_row_frame_at(
        display,
        top_left.x,
        top_left.y,
        CARD_WIDTH,
        CARD_HEIGHT,
        false,
    )?;
    let inner_width = CARD_WIDTH - 2 * ROW_PAD_X;
    draw_text_fit(
        display,
        label,
        Point::new(top_left.x + ROW_PAD_X, top_left.y + 28),
        preferences.body_style(),
        inner_width,
    )?;
    draw_text_fit(
        display,
        value,
        Point::new(top_left.x + ROW_PAD_X, top_left.y + 68),
        preferences.text_style(UiTextRole::Large, BinaryColor::On),
        inner_width,
    )
}

/// Shared with the Home dashboard's today/streak summary row (see
/// `screens::home::draw_today_streak_row`).
pub(crate) fn streak_value_label(locale: Locale, days: u32) -> String {
    match days {
        0 => t(locale, "0 days", "0 giorni").to_string(),
        1 => t(locale, "1 day", "1 giorno").to_string(),
        _ => format!("{days} {}", t(locale, "days", "giorni")),
    }
}

/// "Speed: N chars/min", or that there is not enough reading yet to tell.
fn speed_label(locale: Locale, stats: &crate::reading_stats::ReadingStatsSnapshot) -> String {
    let speed = stats.chars_per_minute.map_or_else(
        || t(locale, "not enough data yet", "dati insufficienti").to_string(),
        |value| format!("{value} {}", t(locale, "chars/min", "caratteri/min")),
    );
    format!("{}: {speed}", t(locale, "Speed", "Velocit\u{00E0}"))
}

/// Top of the time axis for a period whose longest day is `longest`
/// seconds: the first of [`AXIS_TOPS`] it fits under. A period without
/// reading still gets an axis, of one hour.
fn axis_top_seconds(longest: u32) -> u32 {
    if longest == 0 {
        return 3_600;
    }
    AXIS_TOPS
        .into_iter()
        .find(|top| *top >= longest)
        .unwrap_or(AXIS_TOPS[AXIS_TOPS.len() - 1])
}

/// A time on the axis, as short as it reads: "30m", "2h", "1h30".
fn axis_label(seconds: u32) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    match (hours, minutes) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h{minutes:02}"),
    }
}

/// Where the chart puts its bars: the plot's left edge, the width of a bar
/// and the step from one bar to the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChartColumns {
    left: i32,
    bar_width: i32,
    step: i32,
}

/// Columns for `count` bars to the right of an axis `axis_width` wide: the
/// bars as wide as the count allows, the row of them centered in the plot.
fn chart_columns(axis_width: i32, count: usize) -> ChartColumns {
    let count = count.max(1) as i32;
    let plot_left = CONTENT_LEFT + axis_width;
    let plot_width = CONTENT_WIDTH - axis_width;
    let gap = if count <= 7 {
        WEEK_BAR_GAP
    } else {
        MONTH_BAR_GAP
    };
    let bar_width = ((plot_width - gap * (count - 1)) / count).max(1);
    let used = bar_width * count + gap * (count - 1);
    ChartColumns {
        left: plot_left + (plot_width - used) / 2,
        bar_width,
        step: bar_width + gap,
    }
}

/// One bar per day against a time axis: a line at the top of the scale and
/// one at half of it, each with its time on the left, so a bar's height
/// reads as a duration. A day without reading has no bar on the base line,
/// a day still to come has none either; today's column has a mark under
/// the base line. A week is labelled with the days' initials, a month with
/// the day of the month every fifth day.
fn draw_period_chart(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    period: &PeriodStats,
    locale: Locale,
    label_style: UiTextStyle,
) -> Result<(), Infallible> {
    let ink = PrimitiveStyle::with_fill(BinaryColor::On);
    let axis_top = axis_top_seconds(period.longest_day_seconds());
    let top_label = axis_label(axis_top);
    let half_label = axis_label(axis_top / 2);
    let axis_width = label_style
        .text_width(&top_label)
        .max(label_style.text_width(&half_label))
        + AXIS_GAP;
    let columns = chart_columns(axis_width, period.days.len());
    let plot_left = CONTENT_LEFT + axis_width;
    let plot_right = CONTENT_LEFT + CONTENT_WIDTH;
    let bottom = top + CHART_HEIGHT;

    // The two lines of the scale, dotted so the bars stay the darkest
    // thing in the chart, and their times centered on them.
    let (cap_top, _) = label_style.text_ink_bounds("0");
    for (label, y) in [(&top_label, top), (&half_label, top + CHART_HEIGHT / 2)] {
        let mut x = plot_left;
        while x < plot_right {
            Rectangle::new(Point::new(x, y), Size::new(2, 1))
                .into_styled(ink)
                .draw(display)?;
            x += 6;
        }
        draw_text_right(
            display,
            label,
            plot_left - AXIS_GAP,
            y - cap_top / 2,
            label_style,
            axis_width,
        )?;
    }
    Rectangle::new(
        Point::new(plot_left, bottom),
        Size::new((plot_right - plot_left) as u32, 2),
    )
    .into_styled(ink)
    .draw(display)?;

    let label_baseline = bottom + 2 * TODAY_MARK + 4 + i32::from(label_style.line_height());
    for (index, day) in period.days.iter().enumerate() {
        let x = columns.left + index as i32 * columns.step;
        if day.total_seconds > 0 {
            let height = (i64::from(CHART_HEIGHT) * i64::from(day.total_seconds.min(axis_top))
                / i64::from(axis_top))
            .max(3) as i32;
            Rectangle::new(
                Point::new(x, bottom - height),
                Size::new(columns.bar_width as u32, height as u32),
            )
            .into_styled(ink)
            .draw(display)?;
        }
        if day.today {
            Rectangle::new(
                Point::new(x, bottom + 2 + TODAY_MARK),
                Size::new(columns.bar_width as u32, TODAY_MARK as u32),
            )
            .into_styled(ink)
            .draw(display)?;
        }
        if let Some(label) = day_label(locale, period.view.period, day) {
            let width = label_style.text_width(&label);
            Text::new(
                &label,
                Point::new(x + (columns.bar_width - width) / 2, label_baseline),
                label_style,
            )
            .draw(display)?;
        }
    }
    Ok(())
}

/// What is written under a day's bar: its initial in a week, the day of the
/// month every fifth day (and the first) in a month.
fn day_label(locale: Locale, period: StatsPeriod, day: &PeriodDay) -> Option<String> {
    match period {
        StatsPeriod::Week => Some(weekday_initial(locale, day.weekday).to_string()),
        StatsPeriod::Month => (day.day == 1 || day.day % 5 == 0).then(|| day.day.to_string()),
    }
}

/// Single-letter weekday initial for the bar chart. `weekday` is `0` =
/// Sunday .. `6` = Saturday, matching [`crate::ntp::utc_from_unix_seconds`].
fn weekday_initial(locale: Locale, weekday: u8) -> &'static str {
    match locale {
        Locale::English => match weekday {
            0 => "S",
            1 => "M",
            2 => "T",
            3 => "W",
            4 => "T",
            5 => "F",
            6 => "S",
            _ => "?",
        },
        Locale::Italian => match weekday {
            0 => "D",
            1 => "L",
            2 => "M",
            3 => "M",
            4 => "G",
            5 => "V",
            6 => "S",
            _ => "?",
        },
    }
}

/// One book read in the period: its title, and the time spent on it with the
/// reading percentage. Not selectable, so never drawn selected.
fn draw_book_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    book: &BookPeriodStats,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let (title, percent) = resolve_book_title(state, book.book_id).unwrap_or_else(|| {
        (
            t(locale, "Unknown book", "Libro sconosciuto").to_string(),
            None,
        )
    });
    let time_label = format_duration_seconds(u64::from(book.total_seconds));
    let value = percent.map_or(time_label.clone(), |percent| {
        format!("{time_label} \u{00B7} {}%", percent.min(100))
    });
    draw_list_row(display, state.display, top, &title, &value, false)
}

/// Resolve a logged `book_id` back to a title and last-known reading
/// percentage by re-hashing every book the Reader library already has in
/// RAM (`recent` first, then the full `positions` list) until one matches --
/// the same `(path, size_bytes, modified_seconds)` identity
/// [`book_id_for`] was computed from when the session was recorded.
fn resolve_book_title(state: &AppState, book_id: u32) -> Option<(String, Option<u8>)> {
    state
        .reader
        .recent
        .iter()
        .chain(state.reader.positions.iter())
        .find(|location| {
            book_id_for(
                &location.path,
                location.size_bytes,
                location.modified_seconds,
            ) == book_id
        })
        .map(|location| (location.title.clone(), location.reading_percent))
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::Point;

    use super::{
        axis_label, axis_top_seconds, chart_columns, day_range, period_title, render_reading_stats,
        CHART_HEIGHT, MONTH_BAR_GAP, WEEK_BAR_GAP,
    };
    use crate::{
        app::{
            display::UiFontSize,
            typography::audit,
            widgets::layout::{CONTENT_LEFT, CONTENT_WIDTH},
            AppState,
        },
        buttons::ButtonEvent,
        framebuffer::FrameBuffer,
        orientation::{DisplayOrientation, OrientedFrameBuffer},
        reading_stats::{
            BookPeriodStats, CalendarDay, PeriodStats, ReadingStatsSnapshot, StatsPeriod, StatsView,
        },
        regional::Locale,
    };

    const WEEK: StatsView = StatsView {
        period: StatsPeriod::Week,
        back: 0,
    };
    const MONTH: StatsView = StatsView {
        period: StatsPeriod::Month,
        back: 0,
    };

    fn day(year: u16, month: u8, day: u8) -> CalendarDay {
        CalendarDay { year, month, day }
    }

    /// The week of Monday 5 October 2026, looked at on its Wednesday.
    fn week() -> PeriodStats {
        PeriodStats::sample(
            WEEK,
            day(2026, 10, 5),
            &[1_800, 5_400, 2_700, 0, 0, 0, 0],
            Some(2),
            vec![BookPeriodStats {
                book_id: 1,
                total_seconds: 9_900,
            }],
            true,
        )
    }

    /// October 2026 on its seventh day.
    fn month() -> PeriodStats {
        let mut seconds = [0_u32; 31];
        seconds[..7].copy_from_slice(&[600, 0, 3_000, 4_200, 1_800, 5_400, 2_700]);
        PeriodStats::sample(MONTH, day(2026, 10, 1), &seconds, Some(6), Vec::new(), true)
    }

    fn state_with(period: PeriodStats) -> AppState {
        let mut state = AppState::default();
        state.regional.locale = Locale::Italian;
        state.stats_view = period.view;
        state.update_reading_stats_snapshot(ReadingStatsSnapshot {
            available: true,
            today_seconds: 2_700,
            sessions_today: 2,
            streak_days: 3,
            chars_per_minute: Some(1_180),
            remaining_chapter_seconds: None,
            remaining_book_seconds: Some(9_000),
            period,
        });
        state
    }

    /// The frame and every string drawn on it.
    fn rendered(state: &AppState) -> (FrameBuffer, Vec<audit::Rec>) {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
        audit::start();
        render_reading_stats(&mut display, state).unwrap();
        (frame, audit::take())
    }

    fn texts(records: &[audit::Rec]) -> Vec<String> {
        records.iter().map(|rec| rec.text.clone()).collect()
    }

    fn black(frame: &FrameBuffer, x: i32, y: i32) -> bool {
        DisplayOrientation::Portrait
            .map_logical_to_native(Point::new(x, y))
            .and_then(|native| frame.is_black(native))
            .unwrap_or(false)
    }

    #[test]
    fn renders_without_a_computed_snapshot() {
        let (_, records) = rendered(&AppState::default());
        // No clock, no figures: nothing to switch or to chart.
        assert!(!texts(&records).iter().any(|text| text == "Week"));
    }

    #[test]
    fn the_axis_tops_out_at_a_round_time_the_longest_day_fits_under() {
        assert_eq!(axis_top_seconds(0), 3_600);
        assert_eq!(axis_top_seconds(1), 600);
        assert_eq!(axis_top_seconds(600), 600);
        assert_eq!(axis_top_seconds(601), 1_200);
        assert_eq!(axis_top_seconds(3_000), 3_600);
        assert_eq!(axis_top_seconds(5_400), 7_200);
        assert_eq!(axis_top_seconds(7_201), 10_800);
        assert_eq!(axis_top_seconds(40_000), 43_200);
        // More than a day cannot be read in a day; the axis stops there.
        assert_eq!(axis_top_seconds(200_000), 86_400);

        assert_eq!(axis_label(600), "10m");
        assert_eq!(axis_label(300), "5m");
        assert_eq!(axis_label(3_600), "1h");
        assert_eq!(axis_label(5_400), "1h30");
        assert_eq!(axis_label(43_200), "12h");
    }

    #[test]
    fn the_chart_says_how_long_a_bar_is() {
        let (_, records) = rendered(&state_with(week()));
        let shown = texts(&records);
        // Longest day 1h 30m: the axis tops out at 2h, with 1h half way up.
        assert!(shown.contains(&"2h".to_string()), "{shown:?}");
        assert!(shown.contains(&"1h".to_string()), "{shown:?}");
        // The two times sit one above the other, half the plot apart.
        let baseline = |text: &str| {
            records
                .iter()
                .find(|rec| rec.text == text)
                .unwrap()
                .baseline
        };
        assert_eq!(baseline("1h") - baseline("2h"), CHART_HEIGHT / 2);
    }

    #[test]
    fn a_bar_is_as_tall_as_its_share_of_the_axis() {
        let state = state_with(week());
        let (frame, records) = rendered(&state);
        let top_label = records.iter().find(|rec| rec.text == "2h").unwrap();
        let axis_width = top_label.x + top_label.width + super::AXIS_GAP - CONTENT_LEFT;
        let columns = chart_columns(axis_width, 7);
        let column = |index: i32| columns.left + index * columns.step + columns.bar_width / 2;
        // The base line is where the days' initials hang from.
        let initial = records.iter().find(|rec| rec.text == "L").unwrap();
        let bottom = initial.baseline
            - i32::from(state.display.body_style().line_height())
            - 2 * super::TODAY_MARK
            - 4;
        assert!(black(&frame, column(0), bottom) && black(&frame, column(5), bottom));
        let height = |index: i32| {
            (1..=CHART_HEIGHT + 4)
                .take_while(|up| black(&frame, column(index), bottom - *up))
                .count() as i32
        };
        // 30m, 1h 30m and 45m against a 2h axis.
        assert_eq!(height(0), CHART_HEIGHT / 4);
        assert_eq!(height(1), CHART_HEIGHT * 3 / 4);
        assert_eq!(height(2), CHART_HEIGHT * 3 / 8);
        // Days to come have no bar, and only today's column is marked.
        assert_eq!(height(5), 0);
        let marked = |index: i32| black(&frame, column(index), bottom + 2 + super::TODAY_MARK);
        assert!(marked(2));
        assert!(!marked(1) && !marked(3));
    }

    #[test]
    fn a_week_shows_seven_initials_and_a_month_its_days() {
        let (_, records) = rendered(&state_with(week()));
        let shown = texts(&records);
        let initials: Vec<&String> = shown
            .iter()
            .filter(|text| ["L", "M", "G", "V", "S", "D"].contains(&text.as_str()))
            .collect();
        assert_eq!(initials.len(), 7, "{shown:?}");
        assert!(shown.contains(&"Questa settimana".to_string()));
        assert!(
            shown.contains(&"5 \u{2013} 11 ott".to_string()),
            "{shown:?}"
        );

        let (_, records) = rendered(&state_with(month()));
        let shown = texts(&records);
        for label in ["1", "5", "10", "15", "20", "25", "30"] {
            assert!(shown.contains(&label.to_string()), "{label}: {shown:?}");
        }
        assert!(!shown.contains(&"31".to_string()));
        assert!(shown.contains(&"Questo mese".to_string()));
        assert!(shown.contains(&"Ottobre 2026".to_string()), "{shown:?}");
    }

    #[test]
    fn thirty_one_bars_fit_the_plot_at_every_text_size() {
        for size in [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large] {
            let mut state = state_with(month());
            state.display.font_size = size;
            let (_, records) = rendered(&state);
            let top_label = records.iter().find(|rec| rec.text == "2h").unwrap();
            let axis_width = top_label.x + top_label.width + super::AXIS_GAP - CONTENT_LEFT;
            let columns = chart_columns(axis_width, 31);
            assert!(columns.bar_width >= 6, "{size:?}: {columns:?}");
            assert_eq!(columns.step, columns.bar_width + MONTH_BAR_GAP);
            let right = columns.left + 30 * columns.step + columns.bar_width;
            assert!(right <= CONTENT_LEFT + CONTENT_WIDTH, "{size:?}: {right}");
            assert!(columns.left >= CONTENT_LEFT + axis_width, "{size:?}");
        }
        assert_eq!(
            chart_columns(40, 7).step - chart_columns(40, 7).bar_width,
            WEEK_BAR_GAP
        );
    }

    #[test]
    fn periods_are_named_by_how_far_back_they_are() {
        let named = |view: StatsView, first, last| {
            let period = PeriodStats {
                view,
                first,
                last,
                ..Default::default()
            };
            period_title(Locale::Italian, &period)
        };
        let weeks = |back| StatsView {
            period: StatsPeriod::Week,
            back,
        };
        let months = |back| StatsView {
            period: StatsPeriod::Month,
            back,
        };
        assert_eq!(
            named(weeks(0), day(2026, 9, 28), day(2026, 10, 4)),
            ("Questa settimana".into(), "28 set \u{2013} 4 ott".into())
        );
        assert_eq!(
            named(weeks(1), day(2026, 9, 21), day(2026, 9, 27)).0,
            "Settimana scorsa"
        );
        assert_eq!(
            named(weeks(2), day(2026, 9, 14), day(2026, 9, 20)),
            ("14 \u{2013} 20 set".into(), String::new())
        );
        assert_eq!(
            named(months(0), day(2026, 10, 1), day(2026, 10, 31)),
            ("Questo mese".into(), "Ottobre 2026".into())
        );
        assert_eq!(
            named(months(1), day(2026, 9, 1), day(2026, 9, 30)).0,
            "Mese scorso"
        );
        assert_eq!(
            named(months(3), day(2026, 7, 1), day(2026, 7, 31)),
            ("Luglio 2026".into(), String::new())
        );
        assert_eq!(
            day_range(Locale::English, day(2026, 9, 28), day(2026, 10, 4)),
            "Sep 28 \u{2013} Oct 4"
        );
        assert_eq!(
            day_range(Locale::English, day(2026, 10, 5), day(2026, 10, 11)),
            "Oct 5 \u{2013} 11"
        );
    }

    #[test]
    fn the_footer_names_the_other_view_and_the_rocker_only_with_somewhere_to_go() {
        let footer = |state: &AppState| {
            let (_, records) = rendered(state);
            records
                .iter()
                .find(|rec| rec.text.contains("SELECT"))
                .unwrap()
                .text
                .clone()
        };
        let mut state = state_with(week());
        assert!(footer(&state).starts_with("SELECT MESE \u{00B7} SU/GI\u{00D9} PERIODO"));
        state.reading_stats.period.has_older = false;
        assert!(footer(&state).starts_with("SELECT MESE \u{00B7} BOOT"));
        let state = state_with(month());
        assert!(footer(&state).starts_with("SELECT SETTIMANA"));
    }

    #[test]
    fn select_changes_the_view_and_the_rocker_the_period() {
        let mut state = state_with(week());
        state
            .router
            .navigate_to(crate::app::ScreenRoute::ReadingStats);
        assert!(!state.take_reading_stats_refresh_request());

        // SELECT: the month, the current one, and its figures are asked for.
        state.apply(ButtonEvent::Select);
        assert_eq!(state.stats_view, MONTH);
        assert!(state.take_reading_stats_refresh_request());

        // Until they arrive the screen still shows the week, and a step
        // back from a period not on show yet is not taken.
        state.apply(ButtonEvent::Up);
        assert_eq!(state.stats_view, MONTH);
        assert!(!state.take_reading_stats_refresh_request());

        state.update_reading_stats_snapshot(ReadingStatsSnapshot {
            available: true,
            period: month(),
            ..Default::default()
        });
        state.apply(ButtonEvent::Up);
        assert_eq!(state.stats_view.back, 1);
        assert!(state.take_reading_stats_refresh_request());

        // Down comes back towards today and stops there.
        state.apply(ButtonEvent::Down);
        assert_eq!(state.stats_view, MONTH);
        assert!(state.take_reading_stats_refresh_request());
        state.apply(ButtonEvent::Down);
        assert_eq!(state.stats_view, MONTH);
        assert!(!state.take_reading_stats_refresh_request());

        // No earlier period with reading: up stays where it is.
        let mut first = month();
        first.has_older = false;
        state.update_reading_stats_snapshot(ReadingStatsSnapshot {
            available: true,
            period: first,
            ..Default::default()
        });
        state.apply(ButtonEvent::Up);
        assert_eq!(state.stats_view, MONTH);
        assert!(!state.take_reading_stats_refresh_request());
    }

    #[test]
    fn opening_the_screen_starts_from_the_current_period_of_the_last_view() {
        let mut state = AppState::default();
        state.stats_view = StatsView {
            period: StatsPeriod::Month,
            back: 4,
        };
        state.home_selected = crate::app::menu::home_entries()
            .iter()
            .position(|entry| entry.route == crate::app::ScreenRoute::ReadingStats)
            .unwrap();
        state.apply(ButtonEvent::Select);
        assert_eq!(state.active_route(), crate::app::ScreenRoute::ReadingStats);
        assert_eq!(state.stats_view, MONTH);
        assert!(state.take_reading_stats_refresh_request());
    }

    #[test]
    fn without_a_clock_the_keys_do_nothing() {
        let mut state = AppState::default();
        state
            .router
            .navigate_to(crate::app::ScreenRoute::ReadingStats);
        for event in [ButtonEvent::Select, ButtonEvent::Up, ButtonEvent::Down] {
            state.apply(event);
        }
        assert_eq!(state.stats_view, StatsView::default());
        assert!(!state.take_reading_stats_refresh_request());
    }
}
