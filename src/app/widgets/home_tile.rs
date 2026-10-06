//! Home dashboard grid tiles: icon + title only, no item counts or list
//! chevrons. Selection is conveyed purely by the border weight, matching the
//! rest of the shell's list rows.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageDrawable},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        display::DisplayPreferences,
        menu::MenuEntry,
        router::ScreenRoute,
        typography::{Text, UiTextRole},
        widgets::tile_icons::{TileIcon, TILE_ICON_SIZE},
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

/// Horizontal gap between the two tiles in a row.
pub const TILE_GAP_X: i32 = 16;
/// Vertical gap between rows.
pub const TILE_GAP_Y: i32 = 20;

/// Corner radius applied to every tile's border, matching the reference
/// mock's softened card look (the shell otherwise draws square corners).
const TILE_CORNER_RADIUS: Size = Size::new(16, 16);
/// Size of the small filled square drawn in the selected tile's top-right
/// corner, echoing the reference mock's selection indicator.
const SELECTION_DOT_SIZE: Size = Size::new(10, 10);
/// Inset of the selection dot from the tile's top and right edges. Kept
/// clear of `TILE_CORNER_RADIUS` so the dot never overlaps the rounded
/// corner.
const SELECTION_DOT_INSET: i32 = 18;

/// Footprint of a compact Home-grid tile, used once the Home dashboard grew
/// a Continue Reading card and an Oggi/Streak summary row above the grid and
/// switched to a 3-column layout (see `screens::home`). Width is sized so
/// three tiles plus two `TILE_GAP_X` gaps fill the shared 436px content
/// width exactly: `(436 - 2*16) / 3 = 134`.
pub const COMPACT_TILE_SIZE: Size = Size::new(134, 150);
/// Distance from the tile's top edge to the top of the glyph's
/// [`TILE_ICON_SIZE`] box. Set so the icon and title together -- from the
/// glyph's top edge down to the title's descenders (g/p/y and friends), not
/// just its baseline -- sit centered as a whole within `COMPACT_TILE_SIZE`.
const TILE_ICON_TOP_INSET: i32 = 30;
/// Distance from the tile's top edge to the title's baseline: 22 px under
/// the glyph's box.
const TILE_LABEL_BASELINE_INSET: i32 = TILE_ICON_TOP_INSET + TILE_ICON_SIZE as i32 + 22;

/// Compact icon + title tile shared by the Home dashboard's grid (which also
/// carries a Continue Reading card and an Oggi/Streak row above it, hence the
/// smaller footprint) and the Settings grid
/// (`screens::category::render_tile_grid`).
pub fn draw_home_tile_compact(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    entry: MenuEntry,
    selected: bool,
    preferences: DisplayPreferences,
    locale: Locale,
) -> Result<(), Infallible> {
    draw_tile_with(
        display,
        top_left,
        COMPACT_TILE_SIZE,
        entry.label(locale),
        selected,
        preferences,
        route_icon(entry.route),
    )
}

/// Same card, icon and title layout as [`draw_home_tile_compact`], for menus
/// that are not router categories (e.g. Reader Options): any [`TileIcon`],
/// any label, any footprint at least as tall as [`COMPACT_TILE_SIZE`] (icon
/// and title stay centered horizontally and keep their vertical insets from
/// the top edge).
pub fn draw_icon_tile(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile_size: Size,
    label: &str,
    icon: TileIcon,
    selected: bool,
    preferences: DisplayPreferences,
) -> Result<(), Infallible> {
    draw_tile_with(
        display,
        top_left,
        tile_size,
        label,
        selected,
        preferences,
        Some(icon),
    )
}

fn draw_tile_with(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile_size: Size,
    label: &str,
    selected: bool,
    preferences: DisplayPreferences,
    icon: Option<TileIcon>,
) -> Result<(), Infallible> {
    draw_tile_frame(display, top_left, tile_size, selected)?;

    let center_x = top_left.x + tile_size.width as i32 / 2;
    if let Some(icon) = icon {
        draw_tile_icon(
            display,
            Point::new(
                center_x - i32::from(TILE_ICON_SIZE) / 2,
                top_left.y + TILE_ICON_TOP_INSET,
            ),
            icon,
        );
    }

    let heading = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
    let label_width = heading.text_width(label);
    Text::new(
        label,
        Point::new(
            center_x - label_width / 2,
            top_left.y + TILE_LABEL_BASELINE_INSET,
        ),
        heading,
    )
    .draw(display)?;
    Ok(())
}

/// Rounded bordered card shared by [`draw_home_tile`] and
/// [`draw_home_tile_compact`], parameterized on the tile footprint so the two
/// callers can size their grid differently while sharing the same
/// border/selection-dot drawing.
fn draw_tile_frame(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile_size: Size,
    selected: bool,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    RoundedRectangle::new(
        Rectangle::new(top_left, tile_size),
        CornerRadii::new(TILE_CORNER_RADIUS),
    )
    .into_styled(border)
    .draw(display)?;

    if selected {
        let dot_top_left = Point::new(
            top_left.x + tile_size.width as i32
                - SELECTION_DOT_INSET
                - SELECTION_DOT_SIZE.width as i32,
            top_left.y + SELECTION_DOT_INSET,
        );
        Rectangle::new(dot_top_left, SELECTION_DOT_SIZE)
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
    }
    Ok(())
}

/// The glyph of a router category's tile: Library, Audiobooks, Statistics,
/// Upload, Files and Settings on the Home grid (Continue Reading draws its
/// own cover art instead of a glyph -- see
/// `screens::category::draw_continue_reading_tile`; Bookmarks has no tile of
/// its own, reached instead by holding SELECT on a cover in the Library
/// grid), and one glyph per entry on the Settings grid.
fn route_icon(route: ScreenRoute) -> Option<TileIcon> {
    match route {
        ScreenRoute::Library => Some(TileIcon::BookStack),
        ScreenRoute::AudiobookLibrary => Some(TileIcon::Headset),
        ScreenRoute::ReadingStats => Some(TileIcon::StatsReport),
        ScreenRoute::Upload => Some(TileIcon::Import),
        ScreenRoute::Settings => Some(TileIcon::Settings),
        ScreenRoute::Audio => Some(TileIcon::SoundLow),
        ScreenRoute::Clock => Some(TileIcon::Clock),
        ScreenRoute::Display => Some(TileIcon::TextSize),
        ScreenRoute::Language => Some(TileIcon::Language),
        ScreenRoute::DeviceInfo => Some(TileIcon::InfoEmpty),
        ScreenRoute::Network => Some(TileIcon::Wifi),
        ScreenRoute::OtaUpdate => Some(TileIcon::RefreshDouble),
        ScreenRoute::Files => Some(TileIcon::Folder),
        ScreenRoute::Setup => Some(TileIcon::HelpCircle),
        _ => None,
    }
}

/// Draw a tile glyph with its box's top-left corner at `top_left`. The
/// glyphs are stored at the size they are shown (see
/// [`crate::app::widgets::tile_icons`]), so this is a plain copy of their
/// ink: nothing is scaled, and nothing white is painted.
fn draw_tile_icon(display: &mut OrientedFrameBuffer<'_>, top_left: Point, icon: TileIcon) {
    display.blit_packed_bitmap(top_left, TILE_ICON_SIZE, TILE_ICON_SIZE, icon.bits());
}

/// Draw an `embedded-iconoir` glyph at `top_left`, at the size of its own
/// strike (the 24 px and 96 px glyphs outside the tiles).
pub(crate) fn draw_iconoir_icon<I>(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    icon: &I,
) -> Result<(), Infallible>
where
    I: ImageDrawable<Color = BinaryColor>,
{
    Image::new(icon, top_left).draw(display)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::{Point, Size};

    use super::{
        draw_icon_tile, route_icon, COMPACT_TILE_SIZE, TILE_ICON_TOP_INSET,
        TILE_LABEL_BASELINE_INSET,
    };
    use crate::{
        app::{
            display::DisplayPreferences,
            router::ScreenRoute,
            widgets::tile_icons::{TileIcon, TILE_ICON_BYTES, TILE_ICON_SIZE},
        },
        framebuffer::FrameBuffer,
        orientation::{DisplayOrientation, OrientedFrameBuffer},
    };

    fn ink(icon: TileIcon, x: usize, y: usize) -> bool {
        let row_bytes = usize::from(TILE_ICON_SIZE) / 8;
        icon.bits()[y * row_bytes + x / 8] & (0x80 >> (x % 8)) != 0
    }

    #[test]
    fn the_glyph_and_the_title_stay_where_the_tiles_were_laid_out_for() {
        // The 64 px box the upscaled 48 px glyph used to fill, and the
        // title 22 px under it.
        assert_eq!(TILE_ICON_SIZE, 64);
        assert_eq!(TILE_ICON_TOP_INSET, 30);
        assert_eq!(TILE_LABEL_BASELINE_INSET, 116);
    }

    #[test]
    fn every_glyph_is_a_whole_drawing_with_a_margin() {
        let size = usize::from(TILE_ICON_SIZE);
        assert_eq!(TILE_ICON_BYTES, size * size / 8);
        for icon in TileIcon::ALL {
            let count = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .filter(|&(x, y)| ink(icon, x, y))
                .count();
            // An outline drawing: neither empty nor a filled block.
            assert!((250..2000).contains(&count), "{icon:?}: {count} ink pixels");
            // Nothing on the edge of the box, so nothing was clipped.
            for i in 0..size {
                assert!(!ink(icon, i, 0) && !ink(icon, i, size - 1), "{icon:?}");
                assert!(!ink(icon, 0, i) && !ink(icon, size - 1, i), "{icon:?}");
            }
        }
    }

    #[test]
    fn horizontal_and_vertical_strokes_are_five_pixels_thick() {
        // Straight strokes are the long runs of equal columns (or rows):
        // where a column of ink repeats unchanged for 12 px or more, it is
        // a horizontal stroke seen edge on, and its thickness is the run.
        // The 4/3 upscaling this replaced gave 5 and 6 px side by side.
        let size = usize::from(TILE_ICON_SIZE);
        let mut measured = 0;
        for icon in TileIcon::ALL {
            for vertical in [false, true] {
                let at = |a: usize, b: usize| {
                    if vertical {
                        ink(icon, a, b)
                    } else {
                        ink(icon, b, a)
                    }
                };
                let mut thick = Vec::new();
                for a in 0..size {
                    let mut b = 0;
                    while b < size {
                        if !at(a, b) {
                            b += 1;
                            continue;
                        }
                        let start = b;
                        while b < size && at(a, b) {
                            b += 1;
                        }
                        let run = b - start;
                        // The same run in the 11 lines that follow.
                        let straight = a + 12 <= size
                            && (a..a + 12).all(|line| {
                                (start == 0 || !at(line, start - 1))
                                    && (start..b).all(|p| at(line, p))
                                    && (b == size || !at(line, b))
                            });
                        if straight && run <= 8 {
                            thick.push(run);
                        }
                    }
                }
                measured += thick.len();
                assert!(
                    thick.iter().all(|&run| run == 5),
                    "{icon:?} ({}): straight strokes of {thick:?} px",
                    if vertical { "vertical" } else { "horizontal" }
                );
            }
        }
        // Not an empty check: the glyphs do have straight strokes.
        assert!(measured > 500, "{measured} straight strokes measured");
    }

    #[test]
    fn every_category_tile_has_a_glyph() {
        for route in [
            ScreenRoute::Library,
            ScreenRoute::AudiobookLibrary,
            ScreenRoute::ReadingStats,
            ScreenRoute::Upload,
            ScreenRoute::Files,
            ScreenRoute::Settings,
            ScreenRoute::Network,
            ScreenRoute::OtaUpdate,
            ScreenRoute::Audio,
            ScreenRoute::Clock,
            ScreenRoute::Display,
            ScreenRoute::Language,
            ScreenRoute::DeviceInfo,
            ScreenRoute::Setup,
        ] {
            assert!(route_icon(route).is_some(), "{route:?}");
        }
        assert_eq!(route_icon(ScreenRoute::Home), None);
    }

    #[test]
    fn a_tile_shows_its_glyph_pixel_for_pixel() {
        let mut frame = FrameBuffer::new_white();
        let top_left = Point::new(40, 100);
        let tile = Size::new(COMPACT_TILE_SIZE.width, COMPACT_TILE_SIZE.height);
        {
            let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
            draw_icon_tile(
                &mut display,
                top_left,
                tile,
                "",
                TileIcon::Clock,
                false,
                DisplayPreferences::default(),
            )
            .unwrap();
        }
        let left = top_left.x + tile.width as i32 / 2 - i32::from(TILE_ICON_SIZE) / 2;
        let top = top_left.y + TILE_ICON_TOP_INSET;
        for y in 0..usize::from(TILE_ICON_SIZE) {
            for x in 0..usize::from(TILE_ICON_SIZE) {
                let native = DisplayOrientation::Portrait
                    .map_logical_to_native(Point::new(left + x as i32, top + y as i32))
                    .unwrap();
                assert_eq!(
                    frame.is_black(native),
                    Some(ink(TileIcon::Clock, x, y)),
                    "({x}, {y})"
                );
            }
        }
    }
}
