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
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextRole, UiTextStyle},
        widgets::header::draw_header,
    },
    orientation::OrientedFrameBuffer,
    reading_stats::{book_id_for, format_duration_seconds, BookMonthStats, DayBar},
    regional::Locale,
};

const CONTENT_LEFT: i32 = 22;
const CONTENT_WIDTH: i32 = 436;
const CARD_GAP: i32 = 16;
const CARD_HEIGHT: i32 = 90;
const CARD_WIDTH: i32 = (CONTENT_WIDTH - CARD_GAP) / 2;
const CARD_TOP: i32 = 84;
const CARD_CORNER: Size = Size::new(16, 16);

const CHART_HEIGHT: i32 = 100;
const CHART_BAR_GAP: i32 = 8;

const BOOK_ROW_HEIGHT: i32 = 64;
const BOOK_ROW_GAP: i32 = 12;
const BOOK_COVER_SIZE: Size = Size::new(40, 54);
/// Only this many of `books_this_month` fit above the footer at this row
/// height; the backend keeps up to `BOOKS_THIS_MONTH_LIMIT` (5) but the
/// screen shows only as many as there is room for.
const MAX_VISIBLE_BOOKS: usize = 4;

pub fn render_reading_stats(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let stats = &state.reading_stats;

    draw_header(display, state, t(locale, "STATISTICS", "STATISTICHE"))?;
    Text::new(
        t(locale, "< Home", "< Home"),
        Point::new(CONTENT_LEFT, 64),
        state.display.detail_style(),
    )
    .draw(display)?;

    if !stats.available {
        Text::new(
            t(
                locale,
                "Clock not set yet -- open Clock in Settings.",
                "Orologio non impostato -- apri Orologio nelle Impostazioni.",
            ),
            Point::new(CONTENT_LEFT, 120),
            state.display.body_style(),
        )
        .draw(display)?;
        return Ok(());
    }

    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT, CARD_TOP),
        t(locale, "This week", "Questa settimana"),
        &format_duration_seconds(u64::from(stats.week_seconds)),
        state.display,
    )?;
    draw_stat_card(
        display,
        Point::new(CONTENT_LEFT + CARD_WIDTH + CARD_GAP, CARD_TOP),
        t(locale, "Streak", "Streak"),
        &streak_value_label(locale, stats.streak_days),
        state.display,
    )?;

    let body = state.display.body_style();
    let today_month_line = format!(
        "{}: {} - {}: {}",
        t(locale, "Today", "Oggi"),
        format_duration_seconds(u64::from(stats.today_seconds)),
        t(locale, "This month", "Questo mese"),
        format_duration_seconds(u64::from(stats.month_seconds)),
    );
    Text::new(&today_month_line, Point::new(CONTENT_LEFT, 192), body).draw(display)?;

    let speed_line = speed_and_remaining_label(locale, stats);
    Text::new(&speed_line, Point::new(CONTENT_LEFT, 216), body).draw(display)?;

    Text::new(
        t(locale, "Last 7 days", "Ultimi 7 giorni"),
        Point::new(CONTENT_LEFT, 240),
        state.display.heading_style(),
    )
    .draw(display)?;
    draw_week_bar_chart(
        display,
        Point::new(CONTENT_LEFT, 252),
        &stats.last_7_days,
        locale,
        state.display,
    )?;

    Text::new(
        t(locale, "Books read this month", "Libri letti questo mese"),
        Point::new(CONTENT_LEFT, 412),
        state.display.heading_style(),
    )
    .draw(display)?;

    let mut row_top = 424;
    if stats.books_this_month.is_empty() {
        Text::new(
            t(
                locale,
                "No books finished yet this month.",
                "Nessun libro letto questo mese.",
            ),
            Point::new(CONTENT_LEFT, row_top + 26),
            state.display.body_style(),
        )
        .draw(display)?;
    } else {
        for book in stats.books_this_month.iter().take(MAX_VISIBLE_BOOKS) {
            draw_book_row(display, row_top, book, state)?;
            row_top += BOOK_ROW_HEIGHT + BOOK_ROW_GAP;
        }
    }

    Ok(())
}

fn draw_stat_card(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    label: &str,
    value: &str,
    preferences: crate::app::display::DisplayPreferences,
) -> Result<(), Infallible> {
    RoundedRectangle::new(
        Rectangle::new(top_left, Size::new(CARD_WIDTH as u32, CARD_HEIGHT as u32)),
        CornerRadii::new(CARD_CORNER),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    Text::new(
        label,
        Point::new(top_left.x + 14, top_left.y + 26),
        preferences.body_style(),
    )
    .draw(display)?;
    Text::new(
        value,
        Point::new(top_left.x + 14, top_left.y + 68),
        preferences.text_style(UiTextRole::Large, BinaryColor::On),
    )
    .draw(display)?;
    Ok(())
}

/// Shared with the Home dashboard's Oggi/Streak summary row (see
/// `screens::home::draw_today_streak_row`).
pub(crate) fn streak_value_label(locale: Locale, days: u32) -> String {
    match days {
        0 => t(locale, "0 days", "0 giorni").to_string(),
        1 => t(locale, "1 day", "1 giorno").to_string(),
        _ => format!("{days} {}", t(locale, "days", "giorni")),
    }
}

/// "Speed: N chars/min" line, with a "~Xh YYm left" clause appended once the
/// current book's remaining time is known (see
/// `ReaderUiState::continue_reading_progress`, refreshed alongside this
/// snapshot).
fn speed_and_remaining_label(
    locale: Locale,
    stats: &crate::reading_stats::ReadingStatsSnapshot,
) -> String {
    let speed = stats.chars_per_minute.map_or_else(
        || t(locale, "Not enough data yet", "Dati insufficienti").to_string(),
        |value| format!("{value} {}", t(locale, "chars/min", "car/min")),
    );
    let base = format!("{}: {speed}", t(locale, "Speed", "Velocità"));
    stats.remaining_book_seconds.map_or_else(
        || base.clone(),
        |seconds| {
            format!(
                "{base} - ~{} {}",
                format_duration_seconds(seconds),
                t(locale, "left in this book", "rimanenti in questo libro")
            )
        },
    )
}

/// Bars for the last 7 calendar days ending today, tallest scaled to
/// [`CHART_HEIGHT`]; a day with zero reading time draws as a thin outline
/// instead of a filled bar so it stays visible rather than disappearing.
fn draw_week_bar_chart(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    bars: &[DayBar; 7],
    locale: Locale,
    preferences: crate::app::display::DisplayPreferences,
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
            let bar_height = (CHART_HEIGHT * bar.total_seconds as i32 / max_seconds as i32).max(4);
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

fn draw_book_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    book: &BookMonthStats,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    Rectangle::new(
        Point::new(CONTENT_LEFT, top),
        Size::new(CONTENT_WIDTH as u32, BOOK_ROW_HEIGHT as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    let cover_top_left = Point::new(
        CONTENT_LEFT + 10,
        top + (BOOK_ROW_HEIGHT - BOOK_COVER_SIZE.height as i32) / 2,
    );
    Rectangle::new(cover_top_left, BOOK_COVER_SIZE)
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

    let text_left = cover_top_left.x + BOOK_COVER_SIZE.width as i32 + 14;
    let text_right = CONTENT_LEFT + CONTENT_WIDTH - 30;
    let text_width = (text_right - text_left).max(0);

    let heading = state.display.body_style();
    let detail = state.display.detail_style();

    let (title, percent) = resolve_book_title(state, book.book_id).unwrap_or_else(|| {
        (
            t(locale, "Unknown book", "Libro sconosciuto").to_string(),
            None,
        )
    });
    Text::new(
        &truncate_to_width(heading, &title, text_width),
        Point::new(text_left, top + 26),
        heading,
    )
    .draw(display)?;

    let time_label = format_duration_seconds(u64::from(book.total_seconds));
    let subtitle = percent.map_or(time_label.clone(), |percent| {
        format!("{time_label} - {}%", percent.min(100))
    });
    Text::new(
        &truncate_to_width(detail, &subtitle, text_width),
        Point::new(text_left, top + 48),
        detail,
    )
    .draw(display)?;

    Text::new(">", Point::new(text_right + 6, top + 38), heading).draw(display)?;
    Ok(())
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

/// Trim `text` to fit within `max_width` pixels under `style`, appending an
/// ellipsis. Mirrors `truncate_to_width` in `screens/category.rs`,
/// duplicated rather than shared since the two draw into unrelated bounds.
fn truncate_to_width(style: UiTextStyle, text: &str, max_width: i32) -> String {
    if max_width <= 0 || style.text_width(text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "...";
        if style.text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    "...".into()
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
