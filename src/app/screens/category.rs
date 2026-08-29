//! Generic category list screen with bounded paging, plus the Reader
//! category's dedicated icon-tile grid (same visual style as Home).

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::{
        menu::{category_entries, CATEGORY_PAGE_SIZE},
        router::ScreenRoute,
        state::AppState,
        widgets::{
            footer::draw_footer,
            header::draw_header,
            home_tile::{draw_home_tile, TILE_GAP_X, TILE_GAP_Y, TILE_SIZE},
            menu_row::draw_menu_row,
        },
    },
    orientation::OrientedFrameBuffer,
};

/// Left margin shared with the Home grid.
const READER_GRID_LEFT: i32 = 22;
/// First row's top, directly below the shared product header.
const READER_GRID_TOP: i32 = 96;
/// Tiles per row.
const READER_COLUMNS: usize = 2;

pub fn render_category(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let route = state.active_route();
    if route == ScreenRoute::Reader {
        return render_reader_grid(display, state, route);
    }

    let entries = category_entries(route);
    let selected = state.category_selection(route);
    let page_start = (selected / CATEGORY_PAGE_SIZE) * CATEGORY_PAGE_SIZE;
    let title = route.label().to_ascii_uppercase();

    draw_header(display, state, &title)?;

    for (visible_index, entry) in entries
        .iter()
        .copied()
        .skip(page_start)
        .take(CATEGORY_PAGE_SIZE)
        .enumerate()
    {
        draw_menu_row(
            display,
            168 + visible_index as i32 * 86,
            entry,
            page_start + visible_index == selected,
            state.display,
        )?;
    }

    draw_footer(
        display,
        state.display,
        "UP/DOWN MOVE  SELECT OPEN  BOOT BACK",
    )?;
    Ok(())
}

/// Reader category grid: Continue Reading / Library / Bookmarks as icon
/// tiles, matching the Home dashboard's layout and border-only selection.
fn render_reader_grid(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    route: ScreenRoute,
) -> Result<(), Infallible> {
    let entries = category_entries(route);
    let selected = state.category_selection(route);
    let title = route.label().to_ascii_uppercase();

    draw_header(display, state, &title)?;

    for (index, entry) in entries.iter().copied().enumerate() {
        let column = index % READER_COLUMNS;
        let row = index / READER_COLUMNS;
        let x = READER_GRID_LEFT + column as i32 * (TILE_SIZE.width as i32 + TILE_GAP_X);
        let y = READER_GRID_TOP + row as i32 * (TILE_SIZE.height as i32 + TILE_GAP_Y);
        draw_home_tile(
            display,
            Point::new(x, y),
            entry,
            selected == index,
            state.display,
        )?;
    }

    draw_footer(
        display,
        state.display,
        "UP/DOWN MOVE  SELECT OPEN  BOOT BACK",
    )
}
