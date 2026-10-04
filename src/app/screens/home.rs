//! Product-facing Home dashboard for the RustMix Wave shell.
//!
//! Top to bottom: the shared header, the full-size Continue Reading card
//! (see `screens::category::draw_continue_reading_tile` — the one place a
//! book's "what's in progress" state shows at a glance), an Oggi/Streak
//! summary row, then a 3-column grid of compact icon + title tiles
//! (Library/Statistics/Upload in the first row, Files/Settings in the
//! second). The grid uses [`home_tile::draw_home_tile_compact`] here instead
//! of the full-size tile the Settings grid still uses, since the Continue
//! Reading card and summary row above it already claim most of the screen's
//! height.

use core::convert::Infallible;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::Point;
use embedded_iconoir::{
    icons::size24px::{activities::FireFlame, other::Clock as ClockIconSmall},
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        i18n::t,
        menu::home_entries,
        state::AppState,
        typography::Text,
        widgets::{
            header::draw_header,
            home_tile::{
                draw_home_tile_compact, draw_iconoir_icon, COMPACT_TILE_SIZE, TILE_GAP_X,
                TILE_GAP_Y,
            },
        },
    },
    orientation::OrientedFrameBuffer,
    reading_stats::format_duration_seconds,
};

use super::{
    category::{draw_continue_reading_tile, CONTINUE_TILE_HEIGHT},
    reading_stats::streak_value_label,
};

/// Left margin shared with the rest of the product shell.
const GRID_LEFT: i32 = 22;
/// Full content width of the Continue Reading card
/// (`home_tile::TILE_SIZE.width * 2 + TILE_GAP_X`).
const CONTENT_WIDTH: i32 = 436;
/// Tiles per row.
const COLUMNS: usize = 3;

/// Top of the Continue Reading card, directly below the shared header.
const CARD_TOP: i32 = 54;

/// Gap between the Continue Reading card and the Oggi/Streak row.
const STATS_ROW_GAP: i32 = 14;
/// Native square size of the Oggi/Streak row's `size24px` glyphs.
const STATS_ICON_SIZE: i32 = 24;
/// Gap between an Oggi/Streak icon and its text.
const STATS_ICON_TEXT_GAP: i32 = 6;
/// Gap between the "Oggi" and "Streak" segments.
const STATS_SEGMENT_GAP: i32 = 28;
/// Fixed height reserved for the Oggi/Streak row.
const STATS_ROW_HEIGHT: i32 = 24;

/// Gap between the Oggi/Streak row and the tile grid.
const GRID_TOP_GAP: i32 = 14;

pub fn render_home(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_header(display, state, "HOME")?;

    // `home_entries()` keeps Continue Reading last (see `menu::HOME_ENTRIES`)
    // precisely so its index doubles as the cycled SELECT target here: the
    // card is drawn selected exactly when Up/Down navigation lands on it.
    let continue_reading_index = home_entries().len() - 1;
    draw_continue_reading_tile(
        display,
        state,
        Point::new(GRID_LEFT, CARD_TOP),
        CONTENT_WIDTH,
        state.home_selected == continue_reading_index,
    )?;

    let stats_row_top = CARD_TOP + CONTINUE_TILE_HEIGHT + STATS_ROW_GAP;
    draw_today_streak_row(display, state, Point::new(GRID_LEFT, stats_row_top))?;

    let grid_top = stats_row_top + STATS_ROW_HEIGHT + GRID_TOP_GAP;
    for (index, entry) in home_entries()
        .iter()
        .copied()
        .take(continue_reading_index)
        .enumerate()
    {
        let column = index % COLUMNS;
        let row = index / COLUMNS;
        let x = GRID_LEFT + column as i32 * (COMPACT_TILE_SIZE.width as i32 + TILE_GAP_X);
        let y = grid_top + row as i32 * (COMPACT_TILE_SIZE.height as i32 + TILE_GAP_Y);
        draw_home_tile_compact(
            display,
            Point::new(x, y),
            entry,
            state.home_selected == index,
            state.display,
            locale,
        )?;
    }

    Ok(())
}

/// "Oggi: 23m - Streak: 5 giorni" summary row: a small clock glyph and the
/// today total, then a small flame glyph and the current streak, matching
/// the reference mock's compact stat row above the tile grid.
fn draw_today_streak_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top_left: Point,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let stats = &state.reading_stats;
    let body = state.display.body_style();

    let today_icon_top = top_left.y;
    draw_iconoir_icon(
        display,
        Point::new(top_left.x, today_icon_top),
        &ClockIconSmall::new(BinaryColor::On),
    )?;
    let today_text_left = top_left.x + STATS_ICON_SIZE + STATS_ICON_TEXT_GAP;
    let today_label = format!(
        "{}: {}",
        t(locale, "Today", "Oggi"),
        format_duration_seconds(u64::from(stats.today_seconds)),
    );
    let today_baseline = top_left.y + (STATS_ICON_SIZE + i32::from(body.line_height())) / 2 - 4;
    Text::new(
        &today_label,
        Point::new(today_text_left, today_baseline),
        body,
    )
    .draw(display)?;

    let streak_left = today_text_left + body.text_width(&today_label) + STATS_SEGMENT_GAP;
    draw_iconoir_icon(
        display,
        Point::new(streak_left, today_icon_top),
        &FireFlame::new(BinaryColor::On),
    )?;
    let streak_text_left = streak_left + STATS_ICON_SIZE + STATS_ICON_TEXT_GAP;
    let streak_label = format!(
        "{}: {}",
        t(locale, "Streak", "Serie"),
        streak_value_label(locale, stats.streak_days),
    );
    Text::new(
        &streak_label,
        Point::new(streak_text_left, today_baseline),
        body,
    )
    .draw(display)?;

    Ok(())
}
