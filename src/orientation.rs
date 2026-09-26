//! Logical display orientation for the native 800 × 480 e-paper framebuffer.
//!
//! The panel transport always remains 800 × 480. Product-facing screens draw
//! into an orientation-aware target so application code does not need to know
//! how logical coordinates map into the packed native panel buffer.

use core::convert::Infallible;

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, Pixel, Point},
    primitives::{PointsIter, Rectangle},
};

use crate::framebuffer::{FrameBuffer, HEIGHT, ROW_BYTES, WIDTH};

/// Supported logical UI orientations over the native 800 × 480 panel buffer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisplayOrientation {
    /// Native 800 × 480 landscape coordinates.
    Landscape,
    /// Upright 480 × 800 product layout with the USB connector at the bottom.
    #[default]
    Portrait,
    /// Native landscape coordinates rotated by 180 degrees.
    LandscapeInverted,
    /// Portrait coordinates rotated by 180 degrees.
    PortraitInverted,
}

impl DisplayOrientation {
    /// Return the logical drawing dimensions for this orientation.
    #[must_use]
    pub const fn logical_size(self) -> Size {
        match self {
            Self::Landscape | Self::LandscapeInverted => Size::new(WIDTH, HEIGHT),
            Self::Portrait | Self::PortraitInverted => Size::new(HEIGHT, WIDTH),
        }
    }

    /// Return a short human-readable orientation name for diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Landscape => "Landscape",
            Self::Portrait => "Portrait",
            Self::LandscapeInverted => "Landscape inverted",
            Self::PortraitInverted => "Portrait inverted",
        }
    }

    /// Transform a logical screen point into native panel coordinates.
    ///
    /// Points outside the logical drawing surface are discarded before they
    /// can reach the packed native framebuffer.
    #[must_use]
    pub fn map_logical_to_native(self, point: Point) -> Option<Point> {
        let size = self.logical_size();
        if point.x < 0
            || point.y < 0
            || point.x >= size.width as i32
            || point.y >= size.height as i32
        {
            return None;
        }

        let x = point.x;
        let y = point.y;
        Some(match self {
            Self::Landscape => Point::new(x, y),
            Self::Portrait => Point::new(y, HEIGHT as i32 - 1 - x),
            Self::LandscapeInverted => Point::new(WIDTH as i32 - 1 - x, HEIGHT as i32 - 1 - y),
            Self::PortraitInverted => Point::new(WIDTH as i32 - 1 - y, x),
        })
    }
}

/// Orientation-aware embedded-graphics target backed by a native framebuffer.
pub struct OrientedFrameBuffer<'a> {
    frame: &'a mut FrameBuffer,
    orientation: DisplayOrientation,
}

impl<'a> OrientedFrameBuffer<'a> {
    /// Wrap a native framebuffer with a logical display orientation.
    #[must_use]
    pub fn new(frame: &'a mut FrameBuffer, orientation: DisplayOrientation) -> Self {
        Self { frame, orientation }
    }

    /// Return the active logical orientation.
    #[must_use]
    pub const fn orientation(&self) -> DisplayOrientation {
        self.orientation
    }

    /// Fast-path blit of a packed 1bpp bitmap (row-major, MSB-first, bit `1`
    /// = ink) straight into the backing framebuffer, bypassing
    /// embedded-graphics' generic per-pixel `Image`/`Drawable` iteration
    /// chain. Only implements the `Portrait` coordinate transform — the only
    /// orientation the Library grid (its one caller) ever runs under, see
    /// [`crate::app::state::AppState`]'s route/orientation sync — so callers
    /// must check [`Self::orientation`] first and fall back to the generic
    /// `Image` draw for any other orientation; misuse is caught by a
    /// `debug_assert` rather than risking a release-build panic.
    ///
    /// Pixels already white need no write: the caller always draws onto a
    /// freshly `clear_white`d frame, so only ink bits are ever set.
    pub fn blit_packed_bitmap_portrait(
        &mut self,
        top_left: Point,
        width: u16,
        height: u16,
        bits: &[u8],
    ) {
        debug_assert_eq!(self.orientation, DisplayOrientation::Portrait);
        let width = i32::from(width);
        let height = i32::from(height);
        if width <= 0 || height <= 0 {
            return;
        }
        let src_row_bytes = (width as usize).div_ceil(8);
        debug_assert!(bits.len() >= src_row_bytes * height as usize);

        // Portrait maps logical (x, y) to native (y, HEIGHT - 1 - x). A
        // source row (fixed local y) therefore always lands at a single
        // native column, and a source column (local x) always lands at the
        // same native row regardless of which source row it came from — so
        // both the destination byte-column and the valid column range can
        // be hoisted out of the row loop entirely.
        let ny_base = HEIGHT as i32 - 1 - top_left.x;
        let col_start = (ny_base - (HEIGHT as i32 - 1)).max(0);
        let col_end = ny_base.min(width - 1);
        if col_end < col_start {
            return;
        }

        let frame_bytes = self.frame.bytes_mut();
        for row in 0..height {
            let nx = top_left.y + row;
            if nx < 0 || nx >= WIDTH as i32 {
                continue;
            }
            let byte_col = nx as usize / 8;
            let dest_mask = 0x80u8 >> (nx as usize % 8);
            let src_row_offset = row as usize * src_row_bytes;
            let mut byte_index = (ny_base - col_start) as usize * ROW_BYTES + byte_col;
            for col in col_start..=col_end {
                let src_byte = bits[src_row_offset + col as usize / 8];
                let src_mask = 0x80u8 >> (col as usize % 8);
                if src_byte & src_mask != 0 {
                    frame_bytes[byte_index] &= !dest_mask;
                }
                // Steps past native row 0 after a bitmap's last column when
                // it touches the logical right edge; that value is never
                // used, so wrap instead of tripping the overflow check.
                byte_index = byte_index.wrapping_sub(ROW_BYTES);
            }
        }
    }

    /// Ink-only blit of a packed 1bpp bitmap (same format as
    /// [`Self::blit_packed_bitmap_portrait`]) under any orientation: the
    /// Portrait fast path when it applies, otherwise one mapped write per
    /// ink bit. Like the Portrait path it never paints white, so callers
    /// drawing over existing content clear the target area first (see
    /// [`Self::draw_packed_bitmap_opaque`]).
    pub fn blit_packed_bitmap(&mut self, top_left: Point, width: u16, height: u16, bits: &[u8]) {
        if self.orientation == DisplayOrientation::Portrait {
            self.blit_packed_bitmap_portrait(top_left, width, height, bits);
            return;
        }
        let row_bytes = usize::from(width).div_ceil(8);
        for row in 0..usize::from(height) {
            let source = &bits[row * row_bytes..(row + 1) * row_bytes];
            for (byte_index, &byte) in source.iter().enumerate() {
                if byte == 0 {
                    continue;
                }
                for bit in 0..8 {
                    let col = byte_index * 8 + bit;
                    if col >= usize::from(width) || byte & (0x80 >> bit) == 0 {
                        continue;
                    }
                    let point = Point::new(top_left.x + col as i32, top_left.y + row as i32);
                    if let Some(native) = self.orientation.map_logical_to_native(point) {
                        self.frame.set_native_black_in_bounds(
                            native.x as usize,
                            native.y as usize,
                            true,
                        );
                    }
                }
            }
        }
    }

    /// Opaque draw of a packed 1bpp bitmap, pixel-identical to
    /// `Image::new(&ImageRaw::<BinaryColor>::new(bits, width), top_left)`
    /// (which also paints the bitmap's white pixels), but done as one fast
    /// rectangle clear plus an ink-only blit instead of a per-pixel draw.
    pub fn draw_packed_bitmap_opaque(
        &mut self,
        top_left: Point,
        width: u16,
        height: u16,
        bits: &[u8],
    ) {
        // Never read past `bits`: like `ImageRaw`, draw only the rows it holds.
        let row_bytes = usize::from(width).div_ceil(8);
        let height = match row_bytes {
            0 => 0,
            _ => height.min(u16::try_from(bits.len() / row_bytes).unwrap_or(u16::MAX)),
        };
        let area = Rectangle::new(top_left, Size::new(u32::from(width), u32::from(height)));
        self.fill_logical_rect(&area, false);
        self.blit_packed_bitmap(top_left, width, height, bits);
    }

    /// Fill a logical rectangle, clipped to the logical surface. Every
    /// orientation maps an axis-aligned logical rectangle onto an
    /// axis-aligned native one, which is then filled a byte at a time.
    fn fill_logical_rect(&mut self, area: &Rectangle, black: bool) {
        let size = self.orientation.logical_size();
        let area = area.intersection(&Rectangle::new(Point::zero(), size));
        if area.size.width == 0 || area.size.height == 0 {
            return;
        }
        let (lx0, ly0) = (area.top_left.x, area.top_left.y);
        let (lx1, ly1) = (lx0 + area.size.width as i32, ly0 + area.size.height as i32);
        let (w, h) = (WIDTH as i32, HEIGHT as i32);
        let (x0, y0, x1, y1) = match self.orientation {
            DisplayOrientation::Landscape => (lx0, ly0, lx1, ly1),
            DisplayOrientation::Portrait => (ly0, h - lx1, ly1, h - lx0),
            DisplayOrientation::LandscapeInverted => (w - lx1, h - ly1, w - lx0, h - ly0),
            DisplayOrientation::PortraitInverted => (w - ly1, lx0, w - ly0, lx1),
        };
        self.frame.fill_native_rect(x0, y0, x1, y1, black);
    }

    /// Whitens the pixels outside a `radius`-pixel quarter circle in each of
    /// `top_left..top_left+size`'s four corners, so an already-drawn
    /// rectangular image (e.g. a blitted cover thumbnail --
    /// [`Self::blit_packed_bitmap_portrait`]) reads as rounded without
    /// having to bake the rounding into the source bitmap itself. Unlike
    /// that ink-only fast blit, this must be able to paint pixels back to
    /// white, so it goes through [`FrameBuffer::set_native_black`] directly
    /// rather than the packed-bit OR/AND-only path -- still cheap, since the
    /// pixel count touched is bounded by `radius` regardless of the image
    /// size, nowhere near the cost of re-decoding and re-dithering the
    /// source image.
    pub fn mask_rounded_corners(&mut self, top_left: Point, size: Size, radius: i32) {
        let width = size.width as i32;
        let height = size.height as i32;
        if width <= 0 || height <= 0 {
            return;
        }
        let radius = radius.min(width / 2).min(height / 2);
        if radius <= 0 {
            return;
        }
        let radius_sq = i64::from(radius) * i64::from(radius);
        for y in 0..radius {
            for x in 0..radius {
                // Distance from the corner circle's center, `radius` pixels
                // in from the tip along both axes.
                let dx = i64::from(radius - x);
                let dy = i64::from(radius - y);
                if dx * dx + dy * dy <= radius_sq {
                    continue;
                }
                self.whiten_local_pixel(top_left, x, y);
                self.whiten_local_pixel(top_left, width - 1 - x, y);
                self.whiten_local_pixel(top_left, x, height - 1 - y);
                self.whiten_local_pixel(top_left, width - 1 - x, height - 1 - y);
            }
        }
    }

    /// Swap black and white across the whole panel. Orientation-independent,
    /// so it runs straight on the packed bytes.
    pub fn invert_all(&mut self) {
        self.frame.invert();
    }

    /// Swap black and white inside one logical rectangle, clipped to the
    /// logical surface. Per pixel, so meant for small areas such as a
    /// preview swatch; use [`Self::invert_all`] for a full screen.
    pub fn invert_logical_rect(&mut self, area: &Rectangle) {
        for point in area.points() {
            if let Some(native_point) = self.orientation.map_logical_to_native(point) {
                if let Some(black) = self.frame.is_black(native_point) {
                    self.frame.set_native_black(native_point, !black);
                }
            }
        }
    }

    fn whiten_local_pixel(&mut self, top_left: Point, local_x: i32, local_y: i32) {
        let point = Point::new(top_left.x + local_x, top_left.y + local_y);
        if let Some(native_point) = self.orientation.map_logical_to_native(point) {
            self.frame.set_native_black(native_point, false);
        }
    }
}

impl OriginDimensions for OrientedFrameBuffer<'_> {
    fn size(&self) -> Size {
        self.orientation.logical_size()
    }
}

impl DrawTarget for OrientedFrameBuffer<'_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        // Resolve the orientation once per call rather than once per pixel,
        // and bounds-check each pixel once (against the logical surface)
        // instead of twice. Same mapping as `map_logical_to_native`.
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        match self.orientation {
            DisplayOrientation::Landscape => draw_mapped(self.frame, pixels, w, h, |x, y| (x, y)),
            DisplayOrientation::Portrait => {
                draw_mapped(self.frame, pixels, h, w, |x, y| (y, h - 1 - x))
            }
            DisplayOrientation::LandscapeInverted => {
                draw_mapped(self.frame, pixels, w, h, |x, y| (w - 1 - x, h - 1 - y))
            }
            DisplayOrientation::PortraitInverted => {
                draw_mapped(self.frame, pixels, h, w, |x, y| (w - 1 - y, x))
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        self.fill_logical_rect(area, color == BinaryColor::On);
        Ok(())
    }
}

/// Write every pixel inside the `logical_width x logical_height` surface
/// through `map` (logical `(x, y)` to native `(x, y)`), discarding the rest.
#[inline(always)]
fn draw_mapped<I>(
    frame: &mut FrameBuffer,
    pixels: I,
    logical_width: usize,
    logical_height: usize,
    map: impl Fn(usize, usize) -> (usize, usize),
) where
    I: IntoIterator<Item = Pixel<BinaryColor>>,
{
    for Pixel(point, color) in pixels {
        // Negative coordinates wrap to huge values and fail the same check.
        let (x, y) = (point.x as usize, point.y as usize);
        if x < logical_width && y < logical_height {
            let (native_x, native_y) = map(x, y);
            frame.set_native_black_in_bounds(native_x, native_y, color == BinaryColor::On);
        }
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{
        pixelcolor::BinaryColor,
        prelude::{DrawTarget, Pixel, Point},
    };

    use super::{DisplayOrientation, OrientedFrameBuffer};
    use crate::framebuffer::FrameBuffer;

    #[test]
    fn blit_packed_bitmap_portrait_matches_generic_image_draw() {
        use embedded_graphics::{
            image::{Image, ImageRaw},
            prelude::Drawable,
        };

        // 8 x 3 bitmap, row-major MSB-first, exercising a non-trivial
        // pattern across full-byte rows.
        let width: u16 = 8;
        let height: u16 = 3;
        let bits: [u8; 3] = [0b1010_0101, 0b0000_1111, 0b1111_0000];
        let top_left = Point::new(40, 60);

        let mut frame_generic = FrameBuffer::new_white();
        {
            let mut display =
                OrientedFrameBuffer::new(&mut frame_generic, DisplayOrientation::Portrait);
            let raw = ImageRaw::<BinaryColor>::new(&bits, u32::from(width));
            Image::new(&raw, top_left).draw(&mut display).unwrap();
        }

        let mut frame_fast = FrameBuffer::new_white();
        {
            let mut display =
                OrientedFrameBuffer::new(&mut frame_fast, DisplayOrientation::Portrait);
            display.blit_packed_bitmap_portrait(top_left, width, height, &bits);
        }

        assert_eq!(frame_generic.as_bytes(), frame_fast.as_bytes());
    }

    #[test]
    fn mask_rounded_corners_whitens_the_tip_but_keeps_the_center_black() {
        use embedded_graphics::{
            geometry::Size,
            prelude::{Drawable, Primitive},
            primitives::{PrimitiveStyle, Rectangle},
        };

        let orientation = DisplayOrientation::Portrait;
        let top_left = Point::new(40, 60);
        let size = Size::new(40, 40);

        let mut frame = FrameBuffer::new_white();
        {
            let mut display = OrientedFrameBuffer::new(&mut frame, orientation);
            Rectangle::new(top_left, size)
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(&mut display)
                .unwrap();
            display.mask_rounded_corners(top_left, size, 12);
        }

        let corner_native = orientation.map_logical_to_native(top_left).unwrap();
        assert_eq!(
            frame.is_black(corner_native),
            Some(false),
            "corner tip should be masked back to white"
        );

        let center = Point::new(
            top_left.x + size.width as i32 / 2,
            top_left.y + size.height as i32 / 2,
        );
        let center_native = orientation.map_logical_to_native(center).unwrap();
        assert_eq!(
            frame.is_black(center_native),
            Some(true),
            "center should be untouched"
        );
    }

    #[test]
    fn portrait_is_the_product_default() {
        assert_eq!(DisplayOrientation::default(), DisplayOrientation::Portrait);
        assert_eq!(DisplayOrientation::Portrait.logical_size().width, 480);
        assert_eq!(DisplayOrientation::Portrait.logical_size().height, 800);
    }

    #[test]
    fn portrait_maps_logical_corners_to_native_buffer() {
        let orientation = DisplayOrientation::Portrait;
        assert_eq!(
            orientation.map_logical_to_native(Point::new(0, 0)),
            Some(Point::new(0, 479))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(479, 0)),
            Some(Point::new(0, 0))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(0, 799)),
            Some(Point::new(799, 479))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(479, 799)),
            Some(Point::new(799, 0))
        );
    }

    #[test]
    fn all_orientation_mappings_stay_inside_native_frame() {
        for orientation in [
            DisplayOrientation::Landscape,
            DisplayOrientation::Portrait,
            DisplayOrientation::LandscapeInverted,
            DisplayOrientation::PortraitInverted,
        ] {
            let size = orientation.logical_size();
            for point in [
                Point::new(0, 0),
                Point::new(size.width as i32 - 1, 0),
                Point::new(0, size.height as i32 - 1),
                Point::new(size.width as i32 - 1, size.height as i32 - 1),
            ] {
                let mapped = orientation.map_logical_to_native(point).unwrap();
                assert!((0..800).contains(&mapped.x));
                assert!((0..480).contains(&mapped.y));
            }
        }
    }

    #[test]
    fn out_of_bounds_logical_points_are_rejected() {
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(-1, 0)),
            None
        );
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(480, 0)),
            None
        );
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(0, 800)),
            None
        );
    }

    #[test]
    fn oriented_target_writes_to_native_framebuffer() {
        let mut frame = FrameBuffer::new_white();
        let mut oriented = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
        oriented
            .draw_iter([Pixel(Point::new(0, 0), BinaryColor::On)])
            .unwrap();
        assert_eq!(frame.is_black(Point::new(0, 479)), Some(true));
    }
}

#[cfg(test)]
mod render_perf_tests {
    use embedded_graphics::{
        geometry::Size,
        image::{Image, ImageRaw},
        pixelcolor::BinaryColor,
        prelude::{Drawable, Point, Primitive},
        primitives::{PrimitiveStyle, Rectangle},
    };

    use super::{DisplayOrientation, OrientedFrameBuffer};
    use crate::{
        app::{
            reader_typography::reader_body_style,
            typography::{Text, TextBounds},
        },
        framebuffer::FrameBuffer,
        reader::{BookFont, BookFontSize, ReadingTheme},
    };

    /// One full Reader text page, a handful of UI fills and one full-width
    /// image: the three drawing shapes every screen is made of.
    fn draw_sample_screen(frame: &mut FrameBuffer, orientation: DisplayOrientation) {
        frame.clear_white();
        let mut display = OrientedFrameBuffer::new(frame, orientation);
        let style = reader_body_style(BookFont::Serif, BookFontSize::Large, ReadingTheme::Classic);
        let bounds = TextBounds::new(24, 40, 456, 740);
        for line in 0..23 {
            Text::new(
                "The reader turned the page quickly, già perché",
                Point::new(24, 60 + line * 29),
                style,
            )
            .draw_clipped(&mut display, bounds)
            .unwrap();
        }
        for row in 0..8 {
            Rectangle::new(Point::new(0, row * 90), Size::new(480, 44))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(&mut display)
                .unwrap();
            Rectangle::new(Point::new(10, row * 90 + 50), Size::new(460, 36))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
                .draw(&mut display)
                .unwrap();
        }
        let bits = vec![0b1010_0110_u8; 432 / 8 * 600];
        let raw = ImageRaw::<BinaryColor>::new(&bits, 432);
        Image::new(&raw, Point::new(24, 100))
            .draw(&mut display)
            .unwrap();
    }

    /// The pre-optimization drawing path: per-pixel `map_logical_to_native`
    /// plus `set_native_black`, and embedded-graphics' default `fill_solid`
    /// (which falls back to `draw_iter`). Reference for pixel equality.
    struct PerPixelReference<'a> {
        frame: &'a mut FrameBuffer,
        orientation: DisplayOrientation,
    }

    impl embedded_graphics::prelude::OriginDimensions for PerPixelReference<'_> {
        fn size(&self) -> Size {
            self.orientation.logical_size()
        }
    }

    impl embedded_graphics::prelude::DrawTarget for PerPixelReference<'_> {
        type Color = BinaryColor;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
        {
            for embedded_graphics::Pixel(point, color) in pixels {
                if let Some(native) = self.orientation.map_logical_to_native(point) {
                    self.frame
                        .set_native_black(native, color == BinaryColor::On);
                }
            }
            Ok(())
        }
    }

    fn draw_shapes<D>(display: &mut D)
    where
        D: embedded_graphics::prelude::DrawTarget<Color = BinaryColor>,
        D::Error: core::fmt::Debug,
    {
        use embedded_graphics::primitives::{Circle, Line, RoundedRectangle};
        let style = reader_body_style(
            BookFont::Literata,
            BookFontSize::XLarge,
            ReadingTheme::Classic,
        );
        for line in 0..30 {
            Text::new(
                "Clipped text crosses every edge: àèìòù ÀÈ ~|{}",
                Point::new(-17 + line * 3, -5 + line * 29),
                style,
            )
            .draw_clipped(display, TextBounds::new(-40, 10, 900, 820))
            .unwrap();
        }
        // Odd offsets and widths hit every partial-byte case, and several
        // rectangles hang off each edge of the logical surface.
        for index in 0..40 {
            let x = -30 + index * 23 + index % 7;
            let y = -25 + index * 21;
            let size = Size::new((index as u32 * 13) % 97 + 1, (index as u32 * 7) % 61 + 1);
            let color = if index % 3 == 0 {
                BinaryColor::Off
            } else {
                BinaryColor::On
            };
            Rectangle::new(Point::new(x, y), size)
                .into_styled(PrimitiveStyle::with_fill(color))
                .draw(display)
                .unwrap();
            Rectangle::new(Point::new(y, x), size)
                .into_styled(PrimitiveStyle::with_stroke(
                    BinaryColor::On,
                    (index % 4) as u32 + 1,
                ))
                .draw(display)
                .unwrap();
        }
        RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(30, 400), Size::new(300, 120)),
            Size::new(18, 18),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)
        .unwrap();
        Circle::new(Point::new(-40, 600), 150)
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)
            .unwrap();
        Line::new(Point::new(-10, -10), Point::new(900, 700))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 3))
            .draw(display)
            .unwrap();
    }

    fn patterned_bitmap(width: u16, height: u16) -> Vec<u8> {
        let row_bytes = usize::from(width).div_ceil(8);
        (0..row_bytes * usize::from(height))
            .map(|index| (index as u32).wrapping_mul(2_654_435_761).to_le_bytes()[1])
            .collect()
    }

    #[test]
    fn fast_paths_match_the_per_pixel_reference_in_every_orientation() {
        let (width, height) = (203_u16, 157_u16);
        let bits = patterned_bitmap(width, height);
        for orientation in [
            DisplayOrientation::Landscape,
            DisplayOrientation::Portrait,
            DisplayOrientation::LandscapeInverted,
            DisplayOrientation::PortraitInverted,
        ] {
            // Drawn over existing ink, and hanging off an edge, so the
            // opaque bitmap's white pixels and clipping are both checked.
            for top_left in [
                Point::new(15, 33),
                Point::new(-50, 700),
                Point::new(400, -20),
            ] {
                let mut expected = FrameBuffer::new_white();
                {
                    let mut display = PerPixelReference {
                        frame: &mut expected,
                        orientation,
                    };
                    draw_shapes(&mut display);
                    let raw = ImageRaw::<BinaryColor>::new(&bits, u32::from(width));
                    Image::new(&raw, top_left).draw(&mut display).unwrap();
                }
                let mut actual = FrameBuffer::new_white();
                {
                    let mut display = OrientedFrameBuffer::new(&mut actual, orientation);
                    draw_shapes(&mut display);
                    display.draw_packed_bitmap_opaque(top_left, width, height, &bits);
                }
                assert!(
                    expected == actual,
                    "{orientation:?} at {top_left:?} differs from the per-pixel reference"
                );
            }
        }
    }

    #[test]
    fn opaque_bitmap_draw_never_reads_past_a_short_buffer() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Landscape);
        // Claims 50 rows but only holds 3: must draw 3, not panic.
        display.draw_packed_bitmap_opaque(Point::new(5, 5), 16, 50, &[0xFF; 6]);
        assert_eq!(frame.is_black(Point::new(5, 7)), Some(true));
        assert_eq!(frame.is_black(Point::new(5, 8)), Some(false));
    }

    #[test]
    fn invert_logical_rect_flips_only_inside_the_rect_in_portrait() {
        use embedded_graphics::{
            geometry::Size,
            pixelcolor::BinaryColor,
            prelude::{DrawTarget, Pixel},
            primitives::Rectangle,
        };

        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
        display
            .draw_iter([Pixel(Point::new(12, 22), BinaryColor::On)])
            .unwrap();
        display.invert_logical_rect(&Rectangle::new(Point::new(10, 20), Size::new(5, 5)));
        let orientation = DisplayOrientation::Portrait;
        let black = |frame: &FrameBuffer, x, y| {
            frame.is_black(orientation.map_logical_to_native(Point::new(x, y)).unwrap())
        };
        assert_eq!(black(&frame, 10, 20), Some(true));
        assert_eq!(black(&frame, 14, 24), Some(true));
        assert_eq!(black(&frame, 12, 22), Some(false));
        assert_eq!(black(&frame, 15, 20), Some(false));
        assert_eq!(black(&frame, 10, 25), Some(false));
    }

    #[test]
    fn invert_all_flips_every_pixel() {
        use embedded_graphics::{
            pixelcolor::BinaryColor,
            prelude::{DrawTarget, Pixel},
        };

        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Landscape);
        display
            .draw_iter([Pixel(Point::new(3, 3), BinaryColor::On)])
            .unwrap();
        display.invert_all();
        assert_eq!(frame.is_black(Point::new(3, 3)), Some(false));
        assert_eq!(frame.is_black(Point::new(0, 0)), Some(true));
        assert_eq!(frame.is_black(Point::new(799, 479)), Some(true));
    }

    #[test]
    #[ignore = "host timing benchmark; run with --release --ignored --nocapture"]
    fn bench_sample_screen_render() {
        let mut frame = FrameBuffer::new_white();
        for orientation in [DisplayOrientation::Portrait, DisplayOrientation::Landscape] {
            let rounds = 200;
            let started = std::time::Instant::now();
            for _ in 0..rounds {
                draw_sample_screen(&mut frame, orientation);
            }
            println!(
                "bench-render orientation={orientation:?} per-frame-us={}",
                started.elapsed().as_micros() / rounds
            );
        }
    }
}
