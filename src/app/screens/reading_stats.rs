//! Reading time, speed, streak and time-remaining overview.
//!
//! Purely a view over `state.reading_stats` (see
//! [`crate::reading_stats::ReadingStatsSnapshot`]) plus the already-in-RAM
//! Reader library state (`state.reader.recent`/`positions`, to resolve a
//! logged book id back to a title) -- the runtime owner in main.rs computes
//! the snapshot from SD when this screen is opened, so this module never
//! touches storage itself.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        display::DisplayPreferences,
        i18n::t,
        state::AppState,
        typography::{Text, UiTextRole},
        widgets::{
            footer::{back_only, draw_footer},
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{draw_list_row, draw_row_frame_at, ROW_PAD_X, ROW_STEP},
            text::{draw_paragraph, draw_text_fit},
        },
    },
    orientation::OrientedFrameBuffer,
    reading_stats::{book_id_for, format_duration_seconds, BookMonthStats, DayBar},
    regional::Locale,
};

const CARD_GAP: i32 = 16;
const CARD_HEIGHT: i32 = 90;
const CARD_WIDTH: i32 = (CONTENT_WIDTH - CARD_GAP) / 2;
const CHART_HEIGHT: i32 = 84;
const CHART_BAR_GAP: i32 = 8;
/// Only this many of `books_this_month` fit above the footer; the backend
/// keeps up to `BOOKS_THIS_MONTH_LIMIT` (5) but the screen shows only as
/// many as there is room for.
const MAX_VISIBLE_BOOKS: usize = 4;

pub fn render_reading_stats(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let stats = &state.reading_stats;
    let body = preferences.body_style();
    let body_line = i32::from(body.line_height());
    let heading_line = i32::from(preferences.heading_style().line_height());
    draw_header(display, state, t(locale, "STATISTICS", "STATISTICHE"))?;

    if !stats.available {
        draw_paragraph(
            display,
            t(
                locale,
                "The clock is not set yet: reading time cannot be counted. Set it in Settings, Clock.",
                "L'orologio non \u{00E8} ancora impostato: il tempo di lettura non si pu\u{00F2} contare. Impostalo in Impostazioni, Orologio.",
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

    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT, FIRST_ROW_TOP),
        t(locale, "This week", "Questa settimana"),
        &format_duration_seconds(u64::from(stats.week_seconds)),
        preferences,
    )?;
    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT + CARD_WIDTH + CARD_GAP, FIRST_ROW_TOP),
        t(locale, "Streak", "Serie"),
        &streak_value_label(locale, stats.streak_days),
        preferences,
    )?;

    // Everything below is stacked by line height, so the three text sizes
    // never push one line into the next.
    let mut baseline = FIRST_ROW_TOP + CARD_HEIGHT + 10 + body_line;
    let today_month_line = format!(
        "{}: {} \u{00B7} {}: {}",
        t(locale, "Today", "Oggi"),
        format_duration_seconds(u64::from(stats.today_seconds)),
        t(locale, "This month", "Questo mese"),
        format_duration_seconds(u64::from(stats.month_seconds)),
    );
    draw_text_fit(
        display,
        &today_month_line,
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

    baseline += heading_line + 10;
    draw_text_fit(
        display,
        t(locale, "Last 7 days", "Ultimi 7 giorni"),
        Point::new(CONTENT_LEFT, baseline),
        preferences.heading_style(),
        CONTENT_WIDTH,
    )?;
    let chart_top = baseline + 10;
    draw_week_bar_chart(
        display,
        Point::new(CONTENT_LEFT, chart_top),
        &stats.last_7_days,
        locale,
        preferences,
    )?;

    baseline = chart_top + CHART_HEIGHT + 24 + heading_line + 8;
    draw_text_fit(
        display,
        t(locale, "Books this month", "Libri di questo mese"),
        Point::new(CONTENT_LEFT, baseline),
        preferences.heading_style(),
        CONTENT_WIDTH,
    )?;
    if stats.books_this_month.is_empty() {
        draw_paragraph(
            display,
            t(
                locale,
                "No reading recorded this month.",
                "Nessuna lettura registrata questo mese.",
            ),
            CONTENT_LEFT,
            baseline + body_line + 12,
            body,
            CONTENT_WIDTH,
            2,
            6,
        )?;
    } else {
        let rows_top = baseline + 12;
        for (index, book) in stats
            .books_this_month
            .iter()
            .take(MAX_VISIBLE_BOOKS)
            .enumerate()
        {
            draw_book_row(display, rows_top + index as i32 * ROW_STEP, book, state)?;
        }
    }
    draw_footer(display, state, &back_only(locale))
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
        Point::new(top_left.x + ROW_PAD_X, top_left.y + 70),
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

/// Bars for the last 7 calendar days ending today, tallest scaled to
/// [`CHART_HEIGHT`]; a day with zero reading time draws as a thin outline
/// instead of a filled bar so it stays visible rather than disappearing.
fn draw_week_bar_chart(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    bars: &[DayBar; 7],
    locale: Locale,
    preferences: DisplayPreferences,
) -> Result<(), Infallible> {
    let bar_width = (CONTENT_WIDTH - CHART_BAR_GAP * 6) / 7;
    let max_seconds = bars
        .iter()
        .map(|bar| bar.total_seconds)
        .max()
        .unwrap_or(0)
        .max(1);
    let detail = preferences.detail_style();
    for (index, bar) in bars.iter().enumerate() {
        let x = top_left.x + index as i32 * (bar_width + CHART_BAR_GAP);
        if bar.total_seconds == 0 {
            Rectangle::new(
                Point::new(x, top_left.y + CHART_HEIGHT - 4),
                Size::new(bar_width as u32, 4),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
        } else {
            let bar_height = (i64::from(CHART_HEIGHT) * i64::from(bar.total_seconds)
                / i64::from(max_seconds))
            .max(4) as i32;
            Rectangle::new(
                Point::new(x, top_left.y + CHART_HEIGHT - bar_height),
                Size::new(bar_width as u32, bar_height as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }
        let label = weekday_initial(locale, bar.weekday);
        let label_width = detail.text_width(label);
        Text::new(
            label,
            Point::new(
                x + (bar_width - label_width) / 2,
                top_left.y + CHART_HEIGHT + 20,
            ),
            detail,
        )
        .draw(display)?;
    }
    Ok(())
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

/// One book read this month: its title, and the time spent on it with the
/// reading percentage. Not selectable, so never drawn selected.
fn draw_book_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    book: &BookMonthStats,
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
    use super::render_reading_stats;
    use crate::{
        app::AppState,
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        reading_stats::{BookMonthStats, DayBar, ReadingStatsSnapshot},
    };

    #[test]
    fn renders_without_a_computed_snapshot() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_reading_stats(&mut display, &state).unwrap();
    }

    #[test]
    fn renders_with_a_full_snapshot() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        state.update_reading_stats_snapshot(ReadingStatsSnapshot {
            available: true,
            today_seconds: 1_800,
            sessions_today: 3,
            week_seconds: 7_200,
            month_seconds: 20_000,
            streak_days: 5,
            chars_per_minute: Some(180),
            remaining_chapter_seconds: Some(600),
            remaining_book_seconds: Some(9_000),
            last_7_days: [
                DayBar {
                    weekday: 1,
                    total_seconds: 600,
                },
                DayBar {
                    weekday: 2,
                    total_seconds: 1_200,
                },
                DayBar {
                    weekday: 3,
                    total_seconds: 0,
                },
                DayBar {
                    weekday: 4,
                    total_seconds: 1_800,
                },
                DayBar {
                    weekday: 5,
                    total_seconds: 300,
                },
                DayBar {
                    weekday: 6,
                    total_seconds: 900,
                },
                DayBar {
                    weekday: 0,
                    total_seconds: 1_500,
                },
            ],
            books_this_month: vec![
                BookMonthStats {
                    book_id: 1,
                    total_seconds: 8_280,
                },
                BookMonthStats {
                    book_id: 2,
                    total_seconds: 5_040,
                },
            ],
        });
        render_reading_stats(&mut display, &state).unwrap();
    }

    #[test]
    fn renders_with_more_books_than_fit_on_screen() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        let mut snapshot = ReadingStatsSnapshot {
            available: true,
            ..Default::default()
        };
        snapshot.books_this_month = (0..5)
            .map(|book_id| BookMonthStats {
                book_id,
                total_seconds: 600,
            })
            .collect();
        state.update_reading_stats_snapshot(snapshot);
        render_reading_stats(&mut display, &state).unwrap();
    }
}
