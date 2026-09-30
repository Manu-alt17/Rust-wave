//! Home dashboard grid tiles: icon + title only, no item counts or list
//! chevrons. Selection is conveyed purely by the border weight, matching the
//! rest of the shell's list rows.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageDrawable},
    pixelcolor::BinaryColor,
    prelude::{Drawable, DrawTarget, OriginDimensions, Pixel, Point, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};
use embedded_iconoir::{
    icons::size48px::{
        actions::{InfoEmpty as CompactInfoEmpty, RefreshDouble as CompactRefreshDouble},
        activities::{BookStack as CompactBookStack, StatsReport as CompactStatsReport},
        audio::{Mic as CompactMic, SoundLow as CompactSoundLow},
        connectivity::Wifi as CompactWifi,
        docs::{Folder as CompactFolder, MultiplePages as CompactMultiplePages},
        editor::TextSize as CompactTextSize,
        other::{Clock as CompactClock, Import as CompactImport, Language as CompactLanguage},
        system::{Calculator as CompactCalculator, Settings as CompactSettingsIcon},
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
/// Native square size of the `size48px` glyph used on compact tiles. Kept
/// unchanged as the layout anchor -- the tile box and title position below
/// are measured from this, not from the larger drawn size -- so only the
/// glyph itself grows (see `COMPACT_ICON_DRAW_SIZE`).
const COMPACT_ICON_SIZE: i32 = 48;
/// Distance from the tile's top edge to the *native* 48px icon box's top
/// edge (the upscaled glyph drawn from it extends further upward -- see
/// `COMPACT_ICON_DRAW_SIZE` -- so its actual visual top sits 16px above
/// this). Set so the icon+title group's visual ink -- the upscaled icon's
/// top edge down to the title's descenders (g/p/y and friends), not just
/// its baseline -- sits centered as a whole within `COMPACT_TILE_SIZE`.
const COMPACT_ICON_TOP_INSET: i32 = 46;
const COMPACT_LABEL_BASELINE_INSET: i32 = COMPACT_ICON_TOP_INSET + COMPACT_ICON_SIZE + 22;

/// The `size48px` icon pack has no in-between preset size, so compact-tile
/// glyphs are drawn nearest-neighbor upscaled by this ratio (48 -> 64px, a
/// clean integer ratio close to a 30% increase) rather than switching to the
/// much larger `size96px` pack. Drawn centered on the same point the native
/// 48px glyph would have occupied, so the tile box and title stay put.
const COMPACT_ICON_SCALE_NUM: i32 = 4;
const COMPACT_ICON_SCALE_DEN: i32 = 3;
const COMPACT_ICON_DRAW_SIZE: i32 = COMPACT_ICON_SIZE * COMPACT_ICON_SCALE_NUM / COMPACT_ICON_SCALE_DEN;

/// Compact icon + title tile shared by the Home dashboard's grid (which also
/// carries a Continue Reading card and an Oggi/Streak row above it, hence the
/// smaller footprint), the Settings grid, and the Tools grid
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
        |display, icon_top_left| draw_route_icon_compact(display, entry.route, icon_top_left),
    )
}

/// Same card, icon and title layout as [`draw_home_tile_compact`], for menus
/// that are not router categories (e.g. Reader Options): any `size48px`
/// iconoir glyph, any label, any footprint at least as tall as
/// [`COMPACT_TILE_SIZE`] (icon and title stay centered horizontally and keep
/// their vertical insets from the top edge).
pub fn draw_icon_tile<I>(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile_size: Size,
    label: &str,
    icon: &I,
    selected: bool,
    preferences: DisplayPreferences,
) -> Result<(), Infallible>
where
    I: ImageDrawable<Color = BinaryColor>,
{
    draw_tile_with(
        display,
        top_left,
        tile_size,
        label,
        selected,
        preferences,
        |display, icon_top_left| draw_iconoir_icon_scaled(display, icon_top_left, icon),
    )
}

fn draw_tile_with(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    tile_size: Size,
    label: &str,
    selected: bool,
    preferences: DisplayPreferences,
    draw_icon: impl FnOnce(&mut OrientedFrameBuffer<'_>, Point) -> Result<(), Infallible>,
) -> Result<(), Infallible> {
    draw_tile_frame(display, top_left, tile_size, selected)?;

    let center_x = top_left.x + tile_size.width as i32 / 2;
    // Anchored on the native glyph's bottom edge (`COMPACT_ICON_TOP_INSET +
    // COMPACT_ICON_SIZE`), not its center: the title's position is untouched,
    // so keeping the gap between icon and title at the original 22px means
    // the extra height the larger glyph needs can only come from growing
    // upward, not from eating into that gap.
    let icon_bottom_y = top_left.y + COMPACT_ICON_TOP_INSET + COMPACT_ICON_SIZE;
    let icon_top_left = Point::new(
        center_x - COMPACT_ICON_DRAW_SIZE / 2,
        icon_bottom_y - COMPACT_ICON_DRAW_SIZE,
    );
    draw_icon(display, icon_top_left)?;

    let heading = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
    let label_width = heading.text_width(label);
    Text::new(
        label,
        Point::new(
            center_x - label_width / 2,
            top_left.y + COMPACT_LABEL_BASELINE_INSET,
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

/// Dispatch to the category glyph, drawn as a `size48px` `embedded-iconoir`
/// outline glyph nearest-neighbor upscaled to [`COMPACT_ICON_DRAW_SIZE`] (see
/// [`draw_iconoir_icon_scaled`]): BookStack/StatsReport/Import/
/// Folder/Settings on the Home grid (Continue Reading draws its own cover art
/// instead of a glyph — see
/// `screens::category::draw_continue_reading_tile`; Bookmarks has no tile of
/// its own, reached instead by holding SELECT on a cover in the Library
/// grid), and one glyph per entry on the Tools and Settings grids.
fn draw_route_icon_compact(
    display: &mut OrientedFrameBuffer<'_>,
    route: ScreenRoute,
    top_left: Point,
) -> Result<(), Infallible> {
    match route {
        ScreenRoute::Library => {
            draw_iconoir_icon_scaled(display, top_left, &CompactBookStack::new(BinaryColor::On))
        }
        ScreenRoute::ReadingStats => {
            draw_iconoir_icon_scaled(display, top_left, &CompactStatsReport::new(BinaryColor::On))
        }
        ScreenRoute::WifiTransfer => {
            draw_iconoir_icon_scaled(display, top_left, &CompactImport::new(BinaryColor::On))
        }
        ScreenRoute::Tools => {
            draw_iconoir_icon_scaled(display, top_left, &CompactFolder::new(BinaryColor::On))
        }
        ScreenRoute::Settings => draw_iconoir_icon_scaled(
            display,
            top_left,
            &CompactSettingsIcon::new(BinaryColor::On),
        ),
        ScreenRoute::Audio => {
            draw_iconoir_icon_scaled(display, top_left, &CompactSoundLow::new(BinaryColor::On))
        }
        ScreenRoute::Clock => {
            draw_iconoir_icon_scaled(display, top_left, &CompactClock::new(BinaryColor::On))
        }
        ScreenRoute::Display => {
            draw_iconoir_icon_scaled(display, top_left, &CompactTextSize::new(BinaryColor::On))
        }
        ScreenRoute::Language => {
            draw_iconoir_icon_scaled(display, top_left, &CompactLanguage::new(BinaryColor::On))
        }
        ScreenRoute::DeviceInfo => {
            draw_iconoir_icon_scaled(display, top_left, &CompactInfoEmpty::new(BinaryColor::On))
        }
        ScreenRoute::Network => {
            draw_iconoir_icon_scaled(display, top_left, &CompactWifi::new(BinaryColor::On))
        }
        ScreenRoute::OtaUpdate => {
            draw_iconoir_icon_scaled(display, top_left, &CompactRefreshDouble::new(BinaryColor::On))
        }
        ScreenRoute::Files => {
            draw_iconoir_icon_scaled(display, top_left, &CompactMultiplePages::new(BinaryColor::On))
        }
        ScreenRoute::UnitConverter => {
            draw_iconoir_icon_scaled(display, top_left, &CompactCalculator::new(BinaryColor::On))
        }
        ScreenRoute::VoiceNotes => {
            draw_iconoir_icon_scaled(display, top_left, &CompactMic::new(BinaryColor::On))
        }
        _ => Ok(()),
    }
}

/// Draw an `embedded-iconoir` glyph at `top_left`.
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

/// Draw a `size48px` `embedded-iconoir` glyph at `top_left`, nearest-neighbor
/// upscaled from [`COMPACT_ICON_SIZE`] to [`COMPACT_ICON_DRAW_SIZE`]. Calls
/// `ImageDrawable::draw` directly instead of going through `Image`, since
/// `Image` would apply the screen-position offset before this code ever sees
/// the pixels -- here the glyph must still be scaled about its own local
/// origin first.
fn draw_iconoir_icon_scaled<I>(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    icon: &I,
) -> Result<(), Infallible>
where
    I: ImageDrawable<Color = BinaryColor>,
{
    let mut target = UpscalingTarget {
        inner: display,
        dest_top_left: top_left,
    };
    icon.draw(&mut target)
}

/// `DrawTarget` that expands every source pixel an `ImageDrawable` emits, in
/// its own native `COMPACT_ICON_SIZE`-square local coordinates, into the
/// block of destination pixels a standard integer-ratio nearest-neighbor
/// scale-up maps it to -- so the non-integer 4/3 ratio
/// ([`COMPACT_ICON_SCALE_NUM`]/[`COMPACT_ICON_SCALE_DEN`]) tiles cleanly with
/// no gaps or overlaps, the same result a bitmap image scaler would produce.
/// Only used for the compact Home-grid glyphs (see
/// [`draw_iconoir_icon_scaled`]) -- iconoir icons only ever emit foreground
/// (ink) pixels, so there is no background to fill.
struct UpscalingTarget<'a, 'b> {
    inner: &'a mut OrientedFrameBuffer<'b>,
    dest_top_left: Point,
}

impl DrawTarget for UpscalingTarget<'_, '_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let (x_start, x_end) = scaled_block_range(point.x);
            let (y_start, y_end) = scaled_block_range(point.y);
            for y in y_start..=y_end {
                for x in x_start..=x_end {
                    self.inner.draw_iter(core::iter::once(Pixel(
                        Point::new(self.dest_top_left.x + x, self.dest_top_left.y + y),
                        color,
                    )))?;
                }
            }
        }
        Ok(())
    }
}

impl OriginDimensions for UpscalingTarget<'_, '_> {
    fn size(&self) -> Size {
        Size::new(COMPACT_ICON_DRAW_SIZE as u32, COMPACT_ICON_DRAW_SIZE as u32)
    }
}

/// Destination pixel range (inclusive) that source pixel `index` (0-based,
/// along one axis of a [`COMPACT_ICON_SIZE`]-square source) expands into
/// when scaled up to [`COMPACT_ICON_DRAW_SIZE`], matching the standard
/// `floor(dst * native_size / scaled_size) == src` nearest-neighbor mapping
/// used by common image scalers.
fn scaled_block_range(index: i32) -> (i32, i32) {
    let native_size = COMPACT_ICON_SIZE;
    let scaled_size = COMPACT_ICON_DRAW_SIZE;
    let start = (index * scaled_size + native_size - 1) / native_size;
    let end = ((index + 1) * scaled_size + native_size - 1) / native_size - 1;
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::{scaled_block_range, COMPACT_ICON_DRAW_SIZE, COMPACT_ICON_SIZE};

    #[test]
    fn compact_icon_draw_size_is_the_expected_thirty_percent_class_bump() {
        assert_eq!(COMPACT_ICON_SIZE, 48);
        assert_eq!(COMPACT_ICON_DRAW_SIZE, 64);
    }

    #[test]
    fn scaled_block_ranges_tile_the_destination_with_no_gaps_or_overlaps() {
        let mut next_expected_start = 0;
        for index in 0..COMPACT_ICON_SIZE {
            let (start, end) = scaled_block_range(index);
            assert_eq!(start, next_expected_start, "gap/overlap before index {index}");
            assert!(end >= start, "empty block at index {index}");
            next_expected_start = end + 1;
        }
        assert_eq!(
            next_expected_start, COMPACT_ICON_DRAW_SIZE,
            "blocks must exactly cover the destination size"
        );
    }
}
