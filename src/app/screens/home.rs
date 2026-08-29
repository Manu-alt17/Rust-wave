//! Product-facing Home dashboard for the RustMix Wave shell.
//!
//! A 2-column grid of icon + title tiles replaces the old full-width list:
//! Reader/Productivity in the first row, Games/Upload in the second, and
//! Tools/Settings in the third.

use crate::{
    app::{
        menu::home_entries,
        state::AppState,
        widgets::{
            footer::draw_footer,
            header::draw_header,
            home_tile::{draw_home_tile, TILE_GAP_X, TILE_GAP_Y, TILE_SIZE},
        },
    },
    orientation::OrientedFrameBuffer,
};
use core::convert::Infallible;
use embedded_graphics::prelude::Point;

/// Left margin shared with the rest of the product shell.
const GRID_LEFT: i32 = 22;
/// First row's top, directly below the shared product header.
const GRID_TOP: i32 = 96;
/// Tiles per row.
const COLUMNS: usize = 2;

pub fn render_home(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    draw_header(display, state, "HOME")?;

    for (index, entry) in home_entries().iter().copied().enumerate() {
        let column = index % COLUMNS;
        let row = index / COLUMNS;
        let x = GRID_LEFT + column as i32 * (TILE_SIZE.width as i32 + TILE_GAP_X);
        let y = GRID_TOP + row as i32 * (TILE_SIZE.height as i32 + TILE_GAP_Y);
        draw_home_tile(
            display,
            Point::new(x, y),
            entry,
            state.home_selected == index,
            state.display,
        )?;
    }

    draw_footer(display, state.display, "UP/DOWN MOVE  SELECT OPEN")
}
