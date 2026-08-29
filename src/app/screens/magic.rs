//! Magic: The Gathering token library and full/side-by-side view screens.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageRaw},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    magic_tokens::{MagicTile, MagicToken, MAGIC_CATALOG_PAGE_SIZE},
    orientation::{DisplayOrientation, OrientedFrameBuffer},
};

pub fn render_magic_library(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let magic = &state.magic;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    draw_header(display, state, "MAGIC TOKENS")?;

    if magic.entries.is_empty() {
        Text::new("No tokens downloaded yet.", Point::new(22, 130), heading).draw(display)?;
        Text::new(
            "Pick tokens from your phone to add them.",
            Point::new(22, 164),
            body,
        )
        .draw(display)?;
        if let Some(warning) = magic.warning.as_deref() {
            Text::new(warning, Point::new(22, 198), detail).draw(display)?;
        }
        draw_action(
            display,
            240,
            "Configure from phone",
            true,
            body,
        )?;
    } else {
        let page_size = MAGIC_CATALOG_PAGE_SIZE;
        let last_entry = magic.entries.len() - 1;
        let page_start = (magic.selected.min(last_entry) / page_size) * page_size;
        let row_height = 62;
        let list_top = 130;
        for (visible_index, entry) in magic
            .entries
            .iter()
            .skip(page_start)
            .take(page_size)
            .enumerate()
        {
            let top = list_top + visible_index as i32 * row_height;
            let is_selected = page_start + visible_index == magic.selected;
            let is_active = magic.active.iter().any(|id| id == &entry.id);
            draw_token_row(display, top, entry, is_selected, is_active, body)?;
        }

        let mut next_top = list_top + page_size as i32 * row_height + 20;
        if let Some(view_index) = magic.view_row_index() {
            draw_action(
                display,
                next_top,
                "Show on screen",
                magic.selected == view_index,
                body,
            )?;
            next_top += 60;
        }
        draw_action(
            display,
            next_top,
            "Configure from phone",
            magic.selected == magic.configure_row_index(),
            body,
        )?;
    }

    draw_footer(display, state.display, "MOVE  SELECT TOGGLE/OPEN  BOOT BACK")?;
    Ok(())
}

pub fn render_magic_view(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    match state.magic.view_tiles.as_slice() {
        [] => {
            draw_header(display, state, "MAGIC TOKENS")?;
            Text::new(
                "No token art loaded.",
                Point::new(22, 300),
                state.display.heading_style(),
            )
            .draw(display)?;
            draw_footer(display, state.display, "BOOT BACK")?;
        }
        [only] => {
            draw_header(display, state, "MAGIC TOKENS")?;
            let top_left = Point::new((480 - i32::from(only.width)) / 2, 100);
            draw_tile(display, top_left, only)?;
            draw_footer(display, state.display, "BOOT BACK")?;
        }
        [first, second, ..] => {
            // Two tokens rotate the whole screen to Landscape (see
            // `AppState::sync_orientation_for_active_route`) so each card
            // stays upright once the device itself is turned 90 degrees.
            // Each tile is generated at exactly half the Landscape canvas
            // (`magic_tokens::HALF_TILE_WIDTH/HEIGHT`), so placing them
            // flush at the left and right edges fills the screen with no
            // gap, margin or header/footer chrome.
            draw_tile(display, Point::new(0, 0), first)?;
            draw_tile(display, Point::new(i32::from(first.width), 0), second)?;
        }
    }
    Ok(())
}

fn draw_tile(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile: &MagicTile,
) -> Result<(), Infallible> {
    if display.orientation() == DisplayOrientation::Portrait {
        // Fast path: only the single-token view and the library list run in
        // Portrait; the two-token pair view rotates to Landscape (see
        // `AppState::sync_orientation_for_active_route`) and falls through
        // to the generic `Image` draw below.
        display.blit_packed_bitmap_portrait(top_left, tile.width, tile.height, &tile.bits);
    } else {
        let raw = ImageRaw::<BinaryColor>::new(&tile.bits, u32::from(tile.width));
        Image::new(&raw, top_left).draw(display)?;
    }
    Ok(())
}

fn draw_token_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    entry: &MagicToken,
    selected: bool,
    active: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 54))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(34, top + 34),
        style,
    )
    .draw(display)?;
    let label = if entry.power_toughness.is_empty() {
        truncate(&entry.name, 20)
    } else {
        truncate(&format!("{} {}", entry.name, entry.power_toughness), 20)
    };
    Text::new(&label, Point::new(58, top + 34), style).draw(display)?;
    if active {
        Text::new("ON", Point::new(400, top + 34), style).draw(display)?;
    }
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 48))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 32),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 32), style).draw(display)?;
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.into();
    }
    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}
