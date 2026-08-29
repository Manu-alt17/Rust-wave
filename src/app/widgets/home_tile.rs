//! Home dashboard grid tiles: icon + title only, no item counts or list
//! chevrons. Selection is conveyed purely by the border weight, matching the
//! rest of the shell's list rows.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageDrawable},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};
use embedded_iconoir::{
    icons::size96px::{
        activities::{Book, BookStack, OpenBook},
        docs::Folder,
        gaming::Gamepad,
        organization::BookmarkEmpty,
        other::Import,
        system::Calendar,
        system::Settings as SettingsIcon,
    },
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        display::DisplayPreferences,
        menu::MenuEntry,
        router::ScreenRoute,
        typography::{Text, UiTextRole},
    },
    orientation::OrientedFrameBuffer,
};

/// Tile footprint shared by every home grid cell. Tall enough for a 96x96
/// icon plus its title with the same visual breathing room the old 56x56
/// icons had.
pub const TILE_SIZE: Size = Size::new(210, 180);
/// Horizontal gap between the two tiles in a row.
pub const TILE_GAP_X: i32 = 16;
/// Vertical gap between rows.
pub const TILE_GAP_Y: i32 = 20;

/// Native square size of every `embedded-iconoir` glyph on the home grid.
const ICON_SIZE: i32 = 96;
/// Distance from the tile's top edge to the icon box's top edge. Set so the
/// icon+title group sits centered as a whole within `TILE_SIZE`, not pinned
/// to the top.
const ICON_TOP_INSET: i32 = 24;
/// Baseline of the title, measured from the tile's top edge: icon bottom
/// (`ICON_TOP_INSET + ICON_SIZE`) plus a 26px gap — half the old 52px gap.
const LABEL_BASELINE_INSET: i32 = ICON_TOP_INSET + ICON_SIZE + 26;

/// Draw one home-grid tile: a bordered square holding a category glyph above
/// its title. `selected` only changes the border weight (4px vs 1px) — there
/// is no separate arrow glyph to draw or erase.
pub fn draw_home_tile(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    entry: MenuEntry,
    selected: bool,
    preferences: DisplayPreferences,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(top_left, TILE_SIZE)
        .into_styled(border)
        .draw(display)?;

    let center_x = top_left.x + TILE_SIZE.width as i32 / 2;
    let icon_top_left = Point::new(center_x - ICON_SIZE / 2, top_left.y + ICON_TOP_INSET);
    draw_route_icon(display, entry.route, icon_top_left)?;

    let heading = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
    let label_width = heading.text_width(entry.label);
    Text::new(
        entry.label,
        Point::new(
            center_x - label_width / 2,
            top_left.y + LABEL_BASELINE_INSET,
        ),
        heading,
    )
    .draw(display)?;
    Ok(())
}

/// Dispatch to the category glyph. Every tile uses a `size96px`
/// `embedded-iconoir` outline glyph:
/// Book/Calendar/Gamepad/Import/Folder/Settings on the home grid,
/// OpenBook/BookStack/BookmarkEmpty on the Reader grid.
fn draw_route_icon(
    display: &mut OrientedFrameBuffer<'_>,
    route: ScreenRoute,
    top_left: Point,
) -> Result<(), Infallible> {
    match route {
        ScreenRoute::Reader => draw_iconoir_icon(display, top_left, &Book::new(BinaryColor::On)),
        ScreenRoute::Productivity => {
            draw_iconoir_icon(display, top_left, &Calendar::new(BinaryColor::On))
        }
        ScreenRoute::Games => draw_iconoir_icon(display, top_left, &Gamepad::new(BinaryColor::On)),
        ScreenRoute::WifiTransfer => {
            draw_iconoir_icon(display, top_left, &Import::new(BinaryColor::On))
        }
        ScreenRoute::Tools => draw_iconoir_icon(display, top_left, &Folder::new(BinaryColor::On)),
        ScreenRoute::Settings => {
            draw_iconoir_icon(display, top_left, &SettingsIcon::new(BinaryColor::On))
        }
        ScreenRoute::ContinueReading => {
            draw_iconoir_icon(display, top_left, &OpenBook::new(BinaryColor::On))
        }
        ScreenRoute::Library => {
            draw_iconoir_icon(display, top_left, &BookStack::new(BinaryColor::On))
        }
        ScreenRoute::Bookmarks => {
            draw_iconoir_icon(display, top_left, &BookmarkEmpty::new(BinaryColor::On))
        }
        _ => Ok(()),
    }
}

/// Draw an `embedded-iconoir` glyph at `top_left`.
fn draw_iconoir_icon<I>(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    icon: &I,
) -> Result<(), Infallible>
where
    I: ImageDrawable<Color = BinaryColor>,
{
    Image::new(icon, top_left).draw(display)
}
